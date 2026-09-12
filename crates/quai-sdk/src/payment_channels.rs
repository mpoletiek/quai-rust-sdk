//! Payment-code destination allocation and current-state receive discovery.
use crate::qi::{QiError, QiIntent};
use crate::qi_discovery::{DEFAULT_QI_GAP, refresh_qi};
use quai_payments::{
    PaymentCode, PaymentDirection, PaymentError, PaymentSearch, PrivatePaymentCode,
};
use quai_provider::Provider;
use quai_rpc::{Transport, U256};
use quai_wallet::discovery::{IndexRange, ScanStop};
use quai_wallet::storage::SqliteStore;

/// Allocate a destination per possible denomination output before preparing a
/// payment. Send ranges remain burned on partial failure. The peer code must
/// already be registered and exchanged out of band; no notification is inferred.
pub fn payment_intent(
    store: &mut SqliteStore,
    owner: &PrivatePaymentCode,
    peer: &PaymentCode,
    amount: U256,
    outputs: usize,
    attempts_per_address: u32,
    mut cancelled: impl FnMut() -> bool,
) -> Result<QiIntent, QiError> {
    if amount == U256::ZERO
        || !(1..=1024).contains(&outputs)
        || attempts_per_address == 0
        || outputs.saturating_mul(attempts_per_address as usize) > 100_000
    {
        return Err(QiError::InvalidPolicy);
    }
    let mut destinations = Vec::with_capacity(outputs);
    for _ in 0..outputs {
        if cancelled() {
            return Err(QiError::Cancelled);
        }
        destinations.push(
            store
                .allocate_payment_address(
                    owner,
                    peer,
                    PaymentDirection::Send,
                    attempts_per_address,
                    &mut cancelled,
                )?
                .found
                .address,
        );
    }
    Ok(QiIntent {
        amount,
        destinations,
    })
}

/// Receive scan bounds; raw child indexes include zone and ledger skips.
#[derive(Clone, Debug)]
pub struct PaymentScanOptions {
    /// Explicit interval; None gap scans the entire bounded interval.
    pub range: IndexRange,
    /// Consecutive matching addresses without current outpoints.
    pub gap_limit: Option<u32>,
    /// Maximum matching addresses per page, at most 1024.
    pub max_addresses: usize,
}
impl Default for PaymentScanOptions {
    fn default() -> Self {
        Self {
            range: IndexRange {
                start: 0,
                end: 1_000_000,
            },
            gap_limit: Some(DEFAULT_QI_GAP),
            max_addresses: 1024,
        }
    }
}
/// A completed page of locally validated receive indexes. Scanning does not
/// reserve new receive addresses or alter existing send allocation cursors.
#[derive(Clone, Debug)]
pub struct PaymentScanReport {
    /// Matching receive children, including the empty gap.
    pub indexes: Vec<u32>,
    /// First unexamined raw child for an explicit continuation page.
    pub next_index: u32,
    /// Coverage stop reason; a gap is not complete historical recovery.
    pub stopped: ScanStop,
}

/// Scan a registered peer's receive derivation, persist ownership-checked
/// exposures, then refresh all stored Qi addresses. Restore channel metadata
/// and use deep ranges when recovering past fully spent gaps or burned ranges.
/// Cancellation before import returns an error and preserves the existing view.
pub async fn scan_payment_channel<T: Transport>(
    provider: &Provider<T>,
    store: &mut SqliteStore,
    owner: &PrivatePaymentCode,
    peer: &PaymentCode,
    options: &PaymentScanOptions,
    mut cancelled: impl FnMut() -> bool,
) -> Result<PaymentScanReport, QiError> {
    if !(1..=1024).contains(&options.max_addresses)
        || options.gap_limit.is_some_and(|n| n == 0 || n > 1024)
        || options.range.start > options.range.end
        || options.range.end > 1 << 31
        || options.range.end - options.range.start > 1_000_000
    {
        return Err(QiError::InvalidPolicy);
    }
    let scope = store.scope();
    if store.payment_channel(owner, peer)?.is_none() {
        return Err(QiError::IdentityMismatch);
    }
    if cancelled() {
        return Err(QiError::Cancelled);
    }
    if provider.chain_id(scope.zone.into()).await? != scope.chain_id
        || provider.genesis_hash(scope.zone).await? != scope.genesis
    {
        return Err(QiError::IdentityMismatch);
    }
    let mut report = PaymentScanReport {
        indexes: Vec::new(),
        next_index: options.range.start,
        stopped: ScanStop::RangeEnd,
    };
    let mut gap = 0;
    while report.next_index < options.range.end {
        if cancelled() {
            return Err(QiError::Cancelled);
        }
        if report.indexes.len() == options.max_addresses {
            report.stopped = ScanStop::AddressLimit;
            break;
        }
        let found = match owner.search(
            peer,
            PaymentDirection::Receive,
            PaymentSearch {
                zone: scope.zone,
                start_index: report.next_index,
                max_attempts: (options.range.end - report.next_index)
                    .min(quai_payments::MAX_SEARCH_ATTEMPTS),
            },
            &mut cancelled,
        ) {
            Ok(found) => found,
            Err(PaymentError::SearchExhausted { next_index, .. }) => {
                report.next_index = next_index.unwrap_or(1 << 31);
                continue;
            }
            Err(PaymentError::SearchCancelled { .. }) => return Err(QiError::Cancelled),
            Err(_) => return Err(QiError::IdentityMismatch),
        };
        gap = if provider.outpoints(found.address).await?.is_empty() {
            gap + 1
        } else {
            0
        };
        report.indexes.push(found.index);
        report.next_index = found.next_index.unwrap_or(1 << 31);
        if options.gap_limit.is_some_and(|limit| gap >= limit) {
            report.stopped = ScanStop::GapLimit;
            break;
        }
    }
    if cancelled() {
        return Err(QiError::Cancelled);
    }
    store.import_payment_receive_indexes(owner, peer, &report.indexes)?;
    refresh_qi(provider, store, 100_000, cancelled).await?;
    Ok(report)
}
