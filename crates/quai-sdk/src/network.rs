//! Network identity and checkpoint reads shared by every scoped workflow.
use quai_primitives::Zone;
use quai_provider::{Provider, ProviderError, ZoneHeader};
use quai_rpc::{Transport, U256};
use quai_wallet::discovery::{Checkpoint, NetworkScope};

/// Whether `zone` is on the scope's network: its chain ID and genesis hash.
///
/// Callers map `false` to their own error: a mismatch means different things at
/// the start of an operation and at its end.
///
/// The chain ID needs no read of its own. Every provider read, including the
/// genesis read here, already fails with `ChainMismatch` unless the node
/// reports the provider's expected chain ID, so comparing that expectation with
/// the scope is equivalent to reading it again, and costs no round trip.
pub(crate) async fn on_network<T: Transport>(
    provider: &Provider<T>,
    scope: NetworkScope,
    zone: Zone,
) -> Result<bool, ProviderError> {
    Ok(provider.expected_chain_id() == scope.chain_id
        && provider.genesis_hash(zone).await? == scope.genesis)
}

/// The checkpoint a header identifies.
pub(crate) fn checkpoint(header: &ZoneHeader) -> Checkpoint {
    Checkpoint {
        hash: header.hash,
        height: U256::from(header.number),
    }
}

/// The latest header's checkpoint. None means the node reported no header.
pub(crate) async fn latest_checkpoint<T: Transport>(
    provider: &Provider<T>,
    zone: Zone,
) -> Result<Option<Checkpoint>, ProviderError> {
    Ok(provider.latest_header(zone).await?.as_ref().map(checkpoint))
}
