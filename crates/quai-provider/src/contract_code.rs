//! Runtime-code observations bound to rechecked canonical block and genesis identities.
use crate::{BlockReference, BlockTag, DeploymentCode, Provider, ProviderError, ZoneHeader, types};
use quai_primitives::{Hash32, QuaiAddress, Zone};
use quai_rpc::{Transport, U256};
use serde_json::{Value, json};

/// Most addresses one [`Provider::observe_contract_codes`] call accepts: its
/// second round carries a guard, every code read and two rechecks in one batch.
pub const MAX_CONTRACT_CODE_TARGETS: usize = quai_rpc::MAX_BATCH_CALLS - 3;

/// Source-reported runtime code at one exact canonical block, including empty code.
/// This does not prove ABI semantics, proxy implementation, future code or finality.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ContractCodeObservation {
    /// Configured chain, checked by the guard that leads each read or batch.
    pub chain_id: U256,
    /// Source genesis identity, rechecked across the observation.
    pub genesis: Hash32,
    /// Exact account whose runtime bytes were requested.
    pub address: QuaiAddress,
    /// Block whose code was read by hash, then rechecked as canonical at its height.
    pub block: BlockReference,
    /// Bounded runtime bytes and Keccak-256, with optional expected-hash comparison.
    pub code: DeploymentCode,
}
impl<T: Transport> Provider<T> {
    /// Sample code at `Latest` or an explicit positive height, then recheck the
    /// canonical header and genesis. Pending/genesis code queries are excluded.
    /// Empty code is a valid observation; a contract binding must reject it when
    /// it requires deployed code. No missing/error result is silently retried.
    ///
    /// Where the transport batches this is two round trips: the genesis and the
    /// header, then the code read by that block's hash with both rechecks.
    /// Otherwise it is five reads in that order. This does not know the trusted
    /// genesis, so the address is sent before the network is confirmed; use
    /// [`Self::observe_contract_codes`] when the genesis is known.
    pub async fn observe_contract_code(
        &self,
        address: QuaiAddress,
        block: BlockTag,
        expected_runtime: Option<Hash32>,
    ) -> Result<ContractCodeObservation, ProviderError> {
        let mut observations = self
            .observe_codes(address.zone(), &[(address, expected_runtime)], block, None)
            .await?;
        Ok(observations.remove(0))
    }

    /// Observe several contracts of one zone at the same block, in the order
    /// given, each with an optional expected runtime Keccak-256.
    ///
    /// The first round reads only the genesis and the header, and a genesis
    /// other than the trusted `genesis` returns `GenesisMismatch` before any
    /// address is sent. The second round reads every address's code by that
    /// block's hash and rechecks the header and genesis, in one batch where the
    /// transport batches. A changed anchor returns `ObservationChanged`.
    ///
    /// Accepts 1 to [`MAX_CONTRACT_CODE_TARGETS`] addresses, all in one zone.
    /// Empty code is a valid observation, as in [`Self::observe_contract_code`].
    ///
    /// Use this when the contracts must be observed at the same block, or to
    /// send fewer requests. It is not always faster than concurrent single
    /// calls: every runtime arrives in one response, and on a high-latency link
    /// a large response takes extra round trips to deliver. Against a public
    /// gateway at 250 ms, five contracts of 10–15 KB each took about 1.0 s this
    /// way and 0.75 s as five concurrent single calls.
    pub async fn observe_contract_codes(
        &self,
        genesis: Hash32,
        targets: &[(QuaiAddress, Option<Hash32>)],
        block: BlockTag,
    ) -> Result<Vec<ContractCodeObservation>, ProviderError> {
        if genesis == Hash32::ZERO {
            return Err(ProviderError::InvalidRequest("zero trusted genesis"));
        }
        let Some((first, _)) = targets.first() else {
            return Err(ProviderError::InvalidRequest("no contract code targets"));
        };
        if targets.len() > MAX_CONTRACT_CODE_TARGETS {
            return Err(ProviderError::InvalidRequest(
                "too many contract code targets",
            ));
        }
        let zone = first.zone();
        if targets.iter().any(|(address, _)| address.zone() != zone) {
            return Err(ProviderError::InvalidRequest(
                "contract code targets span zones",
            ));
        }
        self.observe_codes(zone, targets, block, Some(genesis))
            .await
    }

    async fn observe_codes(
        &self,
        zone: Zone,
        targets: &[(QuaiAddress, Option<Hash32>)],
        block: BlockTag,
        trusted: Option<Hash32>,
    ) -> Result<Vec<ContractCodeObservation>, ProviderError> {
        match block {
            BlockTag::Latest => (),
            BlockTag::Number(n) if n > U256::ZERO && n <= U256::from(i64::MAX as u64) => (),
            _ => {
                return Err(ProviderError::InvalidRequest(
                    "code observation requires a positive mined height or latest",
                ));
            }
        }
        // Round one is address-free, so a wrong network learns no address.
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
        let codes = self.read_codes_at(zone, targets, &header, genesis).await?;
        Ok(targets
            .iter()
            .zip(codes)
            .map(|(&(address, expected_runtime), bytes)| {
                let hash = Hash32::from_bytes(quai_crypto::keccak256(bytes.bytes()));
                ContractCodeObservation {
                    chain_id: self.expected_chain_id,
                    genesis,
                    address,
                    block: BlockReference {
                        number: header.number,
                        hash: header.hash,
                    },
                    code: DeploymentCode {
                        bytes,
                        hash,
                        matches_expected: expected_runtime.map(|expected| expected == hash),
                    },
                }
            })
            .collect())
    }

    /// Read each target's code at `header`'s hash, then recheck that the header
    /// is still canonical at its height and the genesis is unchanged.
    ///
    /// Reading by hash pins the code to that exact block even if the chain
    /// reorganizes during the read; the header recheck then says whether that
    /// block is still canonical. A failed recheck outranks a failed code read,
    /// because a block reorganized away can also make its code unreadable.
    async fn read_codes_at(
        &self,
        zone: Zone,
        targets: &[(QuaiAddress, Option<Hash32>)],
        header: &ZoneHeader,
        genesis: Hash32,
    ) -> Result<Vec<crate::RpcData>, ProviderError> {
        let at = json!({ "blockHash": header.hash.to_string() });
        let recheck = BlockTag::Number(U256::from(header.number));
        let mut calls = targets
            .iter()
            .map(|(address, _)| ("quai_getCode", json!([address.to_string(), at])))
            .collect::<Vec<_>>();
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
        if values.len() != targets.len() + 2 {
            return Err(ProviderError::InvalidResult("batch response count"));
        }
        let rechecked_genesis = values.pop().expect("checked count")?;
        let rechecked = values.pop().expect("checked count")?;
        if Self::parse_zone_header(rechecked, zone, recheck)?.is_none_or(|h| h.hash != header.hash)
            || types::genesis_hash(rechecked_genesis)? != genesis
        {
            return Err(ProviderError::ObservationChanged);
        }
        values
            .into_iter()
            .map(|value| types::data(value?))
            .collect()
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
