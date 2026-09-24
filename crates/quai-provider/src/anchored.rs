//! Two-round reads anchored to one rechecked block, shared by code
//! observations and state proofs.
//!
//! Round one reads the genesis and the selected header, and names no address,
//! so a wrong network learns nothing about what the caller asked for. Round
//! two carries the caller's reads, pinned to that header's block hash, with the
//! header and genesis rechecked in the same batch.
use crate::{BlockTag, Provider, ProviderError, ZoneHeader, types};
use quai_primitives::{Hash32, Zone};
use quai_rpc::{Transport, U256};
use serde_json::{Value, json};

/// The block round two reads at, and the network it was read on.
pub(crate) struct Anchor {
    pub(crate) zone: Zone,
    pub(crate) genesis: Hash32,
    pub(crate) header: ZoneHeader,
}
impl Anchor {
    /// The `quai_*` block parameter naming this anchor's block by hash.
    pub(crate) fn block_param(&self) -> Value {
        json!({ "blockHash": self.header.hash.to_string() })
    }
}

impl<T: Transport> Provider<T> {
    /// Round one: `Latest` or a positive mined height, and the genesis. A
    /// genesis other than `trusted` returns `GenesisMismatch` before any
    /// address is sent. `what` names the operation in an invalid-block error.
    pub(crate) async fn anchor(
        &self,
        zone: Zone,
        block: BlockTag,
        trusted: Option<Hash32>,
        what: &'static str,
    ) -> Result<Anchor, ProviderError> {
        match block {
            BlockTag::Latest => (),
            BlockTag::Number(n) if n > U256::ZERO && n <= U256::from(i64::MAX as u64) => (),
            _ => return Err(ProviderError::InvalidRequest(what)),
        }
        let mut values = self
            .header_values(zone, &[BlockTag::Number(U256::ZERO), block])
            .await
            .into_iter();
        let genesis = types::genesis_hash(next(&mut values)?)?;
        if trusted.is_some_and(|trusted| trusted != genesis) {
            return Err(ProviderError::GenesisMismatch);
        }
        let header = Self::parse_zone_header(next(&mut values)?, zone, block)?
            .ok_or(ProviderError::ObservationChanged)?;
        Ok(Anchor {
            zone,
            genesis,
            header,
        })
    }

    /// Round two: `calls`, then a recheck that the anchor's header is still
    /// canonical at its height and the genesis unchanged, in one batch where
    /// the transport batches, otherwise one read each in that order.
    ///
    /// Returns one result per call, in order. A failed recheck outranks a
    /// failed call, because a block reorganized away can also make its state
    /// unreadable.
    pub(crate) async fn read_at_anchor(
        &self,
        anchor: &Anchor,
        mut calls: Vec<(&str, Value)>,
    ) -> Result<Vec<Result<Value, ProviderError>>, ProviderError> {
        let count = calls.len();
        let recheck = BlockTag::Number(U256::from(anchor.header.number));
        calls.push(("quai_getHeaderByNumber", json!([recheck.rpc_value()?])));
        calls.push(("quai_getHeaderByNumber", json!(["0x0"])));
        let endpoint = self.routing.endpoint(anchor.zone.into())?;
        let mut values: Vec<Result<Value, ProviderError>> =
            match self.guarded_batch(endpoint, calls.clone()).await {
                Some(batch) => batch?
                    .into_iter()
                    .map(|value| value.map_err(ProviderError::from))
                    .collect(),
                // Nothing was sent: read each in order, stopping at a failure.
                None => {
                    let mut values = Vec::with_capacity(calls.len());
                    for (method, params) in calls {
                        values.push(Ok(self.read(anchor.zone.into(), method, params).await?));
                    }
                    values
                }
            };
        if values.len() != count + 2 {
            return Err(ProviderError::InvalidResult("batch response count"));
        }
        let rechecked_genesis = values.pop().expect("checked count")?;
        let rechecked = values.pop().expect("checked count")?;
        if Self::parse_zone_header(rechecked, anchor.zone, recheck)?
            .is_none_or(|h| h.hash != anchor.header.hash)
            || types::genesis_hash(rechecked_genesis)? != anchor.genesis
        {
            return Err(ProviderError::ObservationChanged);
        }
        Ok(values)
    }
}

/// The next of a fixed number of responses; a short list is a malformed batch.
fn next(
    values: &mut impl Iterator<Item = Result<Value, ProviderError>>,
) -> Result<Value, ProviderError> {
    values
        .next()
        .unwrap_or(Err(ProviderError::InvalidResult("batch response count")))
}
