//! Refresh an explicitly populated public Qi usage view without touching custody.
use super::QiDiscoveryError;
use super::qi::identity;
use quai_consensus::{Denomination, OutPoint};
use quai_primitives::QiAddress;
use quai_provider::{MAX_OUTPOINT_ADDRESSES, Provider};
use quai_rpc::Transport;
use quai_wallet::discovery::{Checkpoint, NetworkScope};
use quai_wallet::qi_addresses::{QiAddressBook, QiUsageObservation};
use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
};

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
    let checkpoint = crate::network::checkpoint(&head);
    // The book is a known, fixed set: every address is queried and there is no
    // gap rule that could stop early, so the reads can be batched without
    // changing which addresses are observed. `records` is keyed by address, so
    // the set is unique by construction and `outpoints_many`'s duplicate
    // rejection cannot fire. Accounting below still walks the book in order, so
    // `seen`, `max_outpoints` and `check_use` behave exactly as before.
    let addresses = book
        .addresses()
        .map(|record| {
            QiAddress::try_from(record.public().address())
                .map_err(|_| QiDiscoveryError::InvalidRequest)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut observed = BTreeMap::new();
    for page in addresses.chunks(MAX_OUTPOINT_ADDRESSES) {
        if cancelled() {
            return Err(QiDiscoveryError::Cancelled);
        }
        observed.extend(provider.outpoints_many(page).await?);
    }

    let mut seen = BTreeSet::new();
    let mut staged = Vec::new();
    for address in addresses {
        if cancelled() {
            return Err(QiDiscoveryError::Cancelled);
        }
        // A missing row is a failure, never an empty result: defaulting it would
        // silently record an address as unused.
        let outputs = observed
            .remove(&address)
            .ok_or(QiDiscoveryError::IncompleteObservation)?;
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
