//! Refresh an explicitly populated public Qi usage view without touching custody.
use super::QiDiscoveryError;
use quai_consensus::{Denomination, OutPoint};
use quai_primitives::QiAddress;
use quai_provider::Provider;
use quai_rpc::{Transport, U256};
use quai_wallet::discovery::{Checkpoint, NetworkScope};
use quai_wallet::qi_addresses::{QiAddressBook, QiUsageObservation};
use std::{collections::BTreeSet, future::Future};

/// Refresh every registered HD/imported/channel receive address in one scoped
/// usage book. Head/genesis are checked before and after latest-only reads. The
/// bounded batch commits only after all checks; errors/cancellation leave it intact.
/// No scan, allocation, coin cache, claim mutation, send or history guarantee.
pub async fn refresh_qi_address_book<T: Transport>(
    provider: &Provider<T>,
    book: &mut QiAddressBook,
    max_outpoints: usize,
    cancelled: impl FnMut() -> bool,
) -> Result<Checkpoint, QiDiscoveryError> {
    refresh_qi_address_book_with_use_checker(provider, book, max_outpoints, cancelled, |_, _| {
        std::future::ready(Ok(false))
    })
    .await
}
/// Refresh with an explicit use hint, invoked only for currently empty addresses.
/// Errors propagate. No Send bound: callbacks may own worker-local state. The
/// caller bounds callback work/time and can apply its own deadline to this future.
/// Prior positive use and attempted use survive empty reads until explicit cache
/// invalidation. A matching final head still does not make latest-only RPC atomic.
pub async fn refresh_qi_address_book_with_use_checker<T, F, Fut>(
    provider: &Provider<T>,
    book: &mut QiAddressBook,
    max_outpoints: usize,
    mut cancelled: impl FnMut() -> bool,
    mut check_use: F,
) -> Result<Checkpoint, QiDiscoveryError>
where
    T: Transport,
    F: FnMut(NetworkScope, QiAddress) -> Fut,
    Fut: Future<Output = Result<bool, QiDiscoveryError>>,
{
    if !(1..=100_000).contains(&max_outpoints) {
        return Err(QiDiscoveryError::InvalidRequest);
    }
    if cancelled() {
        return Err(QiDiscoveryError::Cancelled);
    }
    let scope = book.scope();
    identity(provider, scope).await?;
    let head = provider
        .latest_header(scope.zone)
        .await?
        .ok_or(QiDiscoveryError::ObservationChanged)?;
    let checkpoint = Checkpoint {
        hash: head.hash,
        height: U256::from(head.number),
    };
    let mut seen = BTreeSet::new();
    let mut staged = Vec::new();
    for record in book.addresses() {
        if cancelled() {
            return Err(QiDiscoveryError::Cancelled);
        }
        let address = QiAddress::try_from(record.public().address())
            .map_err(|_| QiDiscoveryError::InvalidRequest)?;
        let outputs = provider.outpoints(address).await?;
        if seen.len().saturating_add(outputs.len()) > max_outpoints {
            return Err(QiDiscoveryError::OutputLimit);
        }
        for output in &outputs {
            Denomination::new(output.denomination).map_err(|_| QiDiscoveryError::InvalidOutputs)?;
            if !seen.insert(OutPoint {
                transaction_hash: output.outpoint.tx_hash,
                index: output.outpoint.index,
            }) {
                return Err(QiDiscoveryError::InvalidOutputs);
            }
        }
        let used = if outputs.is_empty() {
            check_use(scope, address).await?
        } else {
            true
        };
        staged.push(QiUsageObservation { address, used });
    }
    if cancelled() {
        return Err(QiDiscoveryError::Cancelled);
    }
    identity(provider, scope).await?;
    let after = provider
        .latest_header(scope.zone)
        .await?
        .ok_or(QiDiscoveryError::ObservationChanged)?;
    let canonical = provider.header_at(scope.zone, head.number).await?;
    if head.hash != after.hash
        || head.number != after.number
        || canonical.is_none_or(|h| h.hash != head.hash)
    {
        return Err(QiDiscoveryError::ObservationChanged);
    }
    if cancelled() {
        return Err(QiDiscoveryError::Cancelled);
    }
    book.record_observations(scope, checkpoint, &staged)
        .map_err(|_| QiDiscoveryError::ObservationChanged)?;
    Ok(checkpoint)
}
async fn identity<T: Transport>(
    provider: &Provider<T>,
    scope: NetworkScope,
) -> Result<(), QiDiscoveryError> {
    if provider.chain_id(scope.zone.into()).await? != scope.chain_id
        || provider.genesis_hash(scope.zone).await? != scope.genesis
    {
        return Err(QiDiscoveryError::IdentityMismatch);
    }
    Ok(())
}
