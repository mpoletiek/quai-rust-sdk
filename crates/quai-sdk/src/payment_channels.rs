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
/// payment. Only examined candidates are consumed, before returning each address.
/// Already returned destinations remain consumed on partial failure. The peer code must
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
                .allocate_payment_address_compact(
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
    validate_scan_options(options)?;
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

fn validate_scan_options(options: &PaymentScanOptions) -> Result<(), QiError> {
    if !(1..=1024).contains(&options.max_addresses)
        || options.gap_limit.is_some_and(|n| n == 0 || n > 1024)
        || options.range.start > options.range.end
        || options.range.end > 1 << 31
        || options.range.end - options.range.start > 1_000_000
    {
        return Err(QiError::InvalidPolicy);
    }
    Ok(())
}

/// Continue beyond this zone's highest persisted receive exposure. This is an
/// explicit extension of discovery, not a historical coverage claim: imported
/// metadata can be sparse. Existing addresses are refreshed along with new ones.
/// Useful after a gap-limited page or a refresh failure after metadata import.
/// For a complete bounded rescan, use `scan_payment_channel` with an explicit
/// range and no gap limit instead. Neither method rewinds send allocation cursors.
pub async fn continue_payment_channel<T: Transport>(
    provider: &Provider<T>,
    store: &mut SqliteStore,
    owner: &PrivatePaymentCode,
    peer: &PaymentCode,
    options: &PaymentScanOptions,
    cancelled: impl FnMut() -> bool,
) -> Result<PaymentScanReport, QiError> {
    validate_scan_options(options)?;
    let mut next = options.clone();
    let last = store
        .payment_addresses(owner, peer, PaymentDirection::Receive)?
        .into_iter()
        .filter(|record| record.zone == store.scope().zone)
        .map(|record| record.index)
        .max();
    if let Some(last) = last {
        next.range.start = next.range.start.max(last + 1).min(next.range.end);
    }
    scan_payment_channel(provider, store, owner, peer, &next, cancelled).await
}

/// One announced channel scanned during mailbox discovery.
#[cfg(feature = "abi")]
#[derive(Clone, Debug)]
pub struct MailboxChannelScan {
    /// Validated sender code taken from the mailbox.
    pub sender: PaymentCode,
    /// Whether this call registered the channel; existing channels are rescanned.
    pub newly_registered: bool,
    /// Receive scan result for this channel.
    pub report: PaymentScanReport,
}

/// Bounded mailbox discovery result. Absence of funds is not proof of none.
#[cfg(feature = "abi")]
#[derive(Clone, Debug, Default)]
pub struct MailboxDiscoveryReport {
    /// Announced channels registered (if needed) and scanned, in announcement order.
    pub scanned: Vec<MailboxChannelScan>,
    /// Valid announcements beyond `max_channels`; call again to process them.
    pub deferred: Vec<PaymentCode>,
    /// Announced entries that are not valid payment codes.
    pub invalid: Vec<String>,
    /// Repeated valid announcements that were collapsed.
    pub duplicates: usize,
}

/// Read Pelagus-compatible mailbox announcements for `owner`, register up to
/// `max_channels` announced senders (1..=64) and scan each with `options`.
/// Announcements are unauthenticated: anyone can make this call register a
/// code, so the channel count is bounded and registration persists metadata.
/// Already-registered channels count toward the bound and are rescanned.
/// Send cursors never change; nothing is notified or broadcast.
#[cfg(feature = "abi")]
#[allow(clippy::too_many_arguments)]
pub async fn discover_mailbox_channels<T: Transport>(
    provider: &Provider<T>,
    store: &mut SqliteStore,
    owner: &PrivatePaymentCode,
    mailbox: &crate::payment_mailbox::PaymentMailbox<'_, T>,
    caller: quai_primitives::QuaiAddress,
    max_channels: usize,
    options: &PaymentScanOptions,
    mut cancelled: impl FnMut() -> bool,
) -> Result<MailboxDiscoveryReport, QiError> {
    if !(1..=64).contains(&max_channels) {
        return Err(QiError::InvalidPolicy);
    }
    validate_scan_options(options)?;
    let announced = mailbox
        .notifications(caller, owner.public_code(), quai_provider::BlockTag::Latest)
        .await
        .map_err(|error| match error {
            crate::contracts::ContractError::Provider(error) => QiError::Provider(error),
            _ => QiError::IdentityMismatch,
        })?;
    let mut report = MailboxDiscoveryReport {
        invalid: announced.invalid,
        duplicates: announced.duplicates,
        ..Default::default()
    };
    let mut senders = announced.senders.into_iter();
    for sender in senders.by_ref().take(max_channels) {
        if cancelled() {
            return Err(QiError::Cancelled);
        }
        if sender == *owner.public_code() {
            report.invalid.push(sender.to_base58());
            continue;
        }
        let newly_registered = store.payment_channel(owner, &sender)?.is_none();
        if newly_registered {
            store.import_payment_channel(
                owner,
                &quai_payments::PaymentChannel::new(owner, sender.clone()),
                None,
            )?;
        }
        let scan =
            scan_payment_channel(provider, store, owner, &sender, options, &mut cancelled).await?;
        report.scanned.push(MailboxChannelScan {
            sender,
            newly_registered,
            report: scan,
        });
    }
    report.deferred = senders.collect();
    Ok(report)
}
