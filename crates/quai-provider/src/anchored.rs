//! Two-round reads anchored to one rechecked block, shared by code
//! observations and state proofs.
//!
//! Round one reads the genesis and the selected header, and names no address,
//! so a wrong network learns nothing about what the caller asked for. Round
//! two carries the caller's reads, pinned to that header's block hash, with the
//! header and genesis rechecked in the same batch.
use crate::header_hash::{VerifiedHeader, verify_header_hash};
use crate::{BlockReference, BlockTag, Provider, ProviderError, ZoneHeader, types};
use quai_primitives::{Hash32, Zone};
use quai_rpc::{Transport, U256};
use serde_json::{Value, json};

/// The block round two reads at, and the network it was read on.
pub(crate) struct Anchor {
    pub(crate) zone: Zone,
    pub(crate) genesis: Hash32,
    pub(crate) header: ZoneHeader,
    /// The header's recomputed hashes, when round one was asked to verify them.
    pub(crate) verified: Option<VerifiedHeader>,
}
impl Anchor {
    pub(crate) fn reference(&self) -> BlockReference {
        BlockReference {
            number: self.header.number,
            hash: self.header.hash,
        }
    }
}

/// The `quai_*` block parameter naming a block by hash. `requireCanonical`
/// has the node refuse a hash it no longer holds canonical at its height.
pub(crate) fn block_param(block: BlockReference) -> Value {
    json!({ "blockHash": block.hash.to_string(), "requireCanonical": true })
}

impl<T: Transport> Provider<T> {
    /// Round one: `Latest` or a positive mined height, and the genesis. A
    /// genesis other than `trusted` returns `GenesisMismatch` before any
    /// address is sent. `what` names the operation in an invalid-block error.
    /// With `verify`, the header's hashes are recomputed from its fields.
    pub(crate) async fn anchor(
        &self,
        zone: Zone,
        block: BlockTag,
        trusted: Option<Hash32>,
        what: &'static str,
        verify: bool,
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
        let value = next(&mut values)?;
        let verified = if verify && !value.is_null() {
            Some(verify_header_hash(&value)?)
        } else {
            None
        };
        let header = Self::parse_zone_header(value, zone, block)?
            .ok_or(ProviderError::ObservationChanged)?;
        Ok(Anchor {
            zone,
            genesis,
            header,
            verified,
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
        zone: Zone,
        genesis: Hash32,
        block: BlockReference,
        mut calls: Vec<(&str, Value)>,
    ) -> Result<Vec<Result<Value, ProviderError>>, ProviderError> {
        let count = calls.len();
        let recheck = BlockTag::Number(U256::from(block.number));
        calls.push(("quai_getHeaderByNumber", json!([recheck.rpc_value()?])));
        calls.push(("quai_getHeaderByNumber", json!(["0x0"])));
        let endpoint = self.routing.endpoint(zone.into())?;
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
                        values.push(Ok(self.read(zone.into(), method, params).await?));
                    }
                    values
                }
            };
        if values.len() != count + 2 {
            return Err(ProviderError::InvalidResult("batch response count"));
        }
        let rechecked_genesis = values.pop().expect("checked count")?;
        let rechecked = values.pop().expect("checked count")?;
        if Self::parse_zone_header(rechecked, zone, recheck)?.is_none_or(|h| h.hash != block.hash)
            || types::genesis_hash(rechecked_genesis)? != genesis
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
