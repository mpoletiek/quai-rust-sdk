//! Runtime-code observations bound to rechecked canonical block and genesis identities.
use crate::{BlockReference, BlockTag, DeploymentCode, Provider, ProviderError};
use quai_primitives::{Hash32, QuaiAddress};
use quai_rpc::{Transport, U256};

/// Source-reported runtime code at one exact canonical block, including empty code.
/// This does not prove ABI semantics, proxy implementation, future code or finality.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ContractCodeObservation {
    /// Configured chain, checked against the endpoint before each individual read.
    pub chain_id: U256,
    /// Source genesis identity, rechecked across the observation.
    pub genesis: Hash32,
    /// Exact account whose runtime bytes were requested.
    pub address: QuaiAddress,
    /// Canonical block identity rechecked after reading code by this height.
    pub block: BlockReference,
    /// Bounded runtime bytes and Keccak-256, with optional expected-hash comparison.
    pub code: DeploymentCode,
}
impl<T: Transport> Provider<T> {
    /// Sample code at `Latest` or an explicit positive height, then recheck the
    /// canonical header and genesis. Pending/genesis code queries are excluded.
    /// Empty code is a valid observation; a contract binding must reject it when
    /// it requires deployed code. No missing/error result is silently retried.
    pub async fn observe_contract_code(
        &self,
        address: QuaiAddress,
        block: BlockTag,
        expected_runtime: Option<Hash32>,
    ) -> Result<ContractCodeObservation, ProviderError> {
        match block {
            BlockTag::Latest => (),
            BlockTag::Number(n) if n > U256::ZERO && n <= U256::from(i64::MAX as u64) => (),
            _ => {
                return Err(ProviderError::InvalidRequest(
                    "code observation requires a positive mined height or latest",
                ));
            }
        }
        let zone = address.zone();
        let genesis = self.genesis_hash(zone).await?;
        let header = match block {
            BlockTag::Latest => self.latest_header(zone).await?,
            BlockTag::Number(n) => self.header_at(zone, n.to::<u64>()).await?,
            BlockTag::Pending => unreachable!(),
        }
        .ok_or(ProviderError::ObservationChanged)?;
        let bytes = self
            .code(address, BlockTag::Number(U256::from(header.number)))
            .await?;
        let hash = Hash32::from_bytes(quai_crypto::keccak256(bytes.bytes()));
        if self
            .header_at(zone, header.number)
            .await?
            .is_none_or(|h| h.hash != header.hash)
            || self.genesis_hash(zone).await? != genesis
        {
            return Err(ProviderError::ObservationChanged);
        }
        Ok(ContractCodeObservation {
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
        })
    }
}
