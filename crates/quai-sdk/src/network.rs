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

/// Use-checker calls in flight at once during a scan.
///
/// A checker usually asks a history service, so awaiting one address at a time
/// made a restore wait on 100 sequential calls for an empty wallet's two gaps.
/// Kept small: the callback's owner bounds its own I/O.
pub(crate) const USE_CHECK_CONCURRENCY: usize = 4;

/// Each row's use hint, in input order: the checker's answer for a row that
/// needs one, `false` otherwise. At most [`USE_CHECK_CONCURRENCY`] calls run at
/// once. Consume in order and stop at the first error, so the error reported
/// is the first one in order; dropping the stream cancels calls in flight.
pub(crate) fn use_hints<'a, E, F, Fut>(
    scope: NetworkScope,
    rows: Vec<(quai_primitives::QiAddress, bool)>,
    check_use: &'a mut F,
) -> impl futures_util::Stream<Item = Result<bool, E>> + 'a
where
    E: 'a,
    F: FnMut(NetworkScope, quai_primitives::QiAddress) -> Fut,
    Fut: std::future::Future<Output = Result<bool, E>> + 'a,
{
    use futures_util::StreamExt;
    futures_util::stream::iter(rows)
        .map(move |(address, needed)| {
            let pending = needed.then(|| check_use(scope, address));
            async move {
                match pending {
                    Some(check) => check.await,
                    None => Ok(false),
                }
            }
        })
        .buffered(USE_CHECK_CONCURRENCY)
}
