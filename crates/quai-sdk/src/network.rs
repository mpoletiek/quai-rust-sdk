//! Network identity and checkpoint reads shared by every scoped workflow.
use quai_primitives::Zone;
use quai_provider::{BlockTag, Provider, ProviderError, ZoneHeader};
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

/// [`on_network`] and header reads in one round trip where the transport
/// batches. `None` when `zone` is not on the scope's network; that is decided
/// before any header is parsed. All reads are address-free, so joining them
/// discloses nothing before the network is confirmed.
pub(crate) async fn headers_on_network<T: Transport>(
    provider: &Provider<T>,
    scope: NetworkScope,
    zone: Zone,
    blocks: &[BlockTag],
) -> Result<Option<Vec<Option<ZoneHeader>>>, ProviderError> {
    if provider.expected_chain_id() != scope.chain_id {
        return Ok(None);
    }
    provider
        .headers_on_network(zone, scope.genesis, blocks)
        .await
}
