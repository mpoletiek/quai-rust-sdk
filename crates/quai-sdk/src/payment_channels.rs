//! Payment-code destination allocation and current-state receive discovery.
use crate::qi::{QiError, QiIntent};
use crate::qi_discovery::{DEFAULT_QI_GAP, refresh_qi};
use quai_payments::{
    PaymentCode, PaymentDirection, PaymentError, PaymentSearch, PrivatePaymentCode,
};
use quai_provider::Provider;
use quai_rpc::{Transport, U256};
use quai_wallet::discovery::{GapCounter, IndexRange, NetworkScope, ScanStop};
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
#[non_exhaustive]
pub struct PaymentScanOptions {
    /// Explicit interval; None gap scans the entire bounded interval.
    pub range: IndexRange,
    /// Consecutive matching addresses without current outpoints.
    pub gap_limit: Option<u32>,
    /// Maximum matching addresses per page, at most 1024.
    pub max_addresses: usize,
}
impl PaymentScanOptions {
    /// Replace `range`.
    pub const fn with_range(mut self, range: IndexRange) -> Self {
        self.range = range;
        self
    }
    /// Replace `gap_limit`.
    pub const fn with_gap_limit(mut self, gap_limit: Option<u32>) -> Self {
        self.gap_limit = gap_limit;
        self
    }
    /// Replace `max_addresses`.
    pub const fn with_max_addresses(mut self, max_addresses: usize) -> Self {
        self.max_addresses = max_addresses;
        self
    }
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
#[non_exhaustive]
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
    if !crate::network::on_network(provider, scope, scope.zone).await? {
        return Err(QiError::NetworkMismatch);
    }
    let (report, _) = scan_channel(provider, scope, owner, peer, options, &mut cancelled).await?;
    if cancelled() {
        return Err(QiError::Cancelled);
    }
    store.import_payment_receive_indexes(owner, peer, &report.indexes)?;
    refresh_qi(provider, store, 100_000, cancelled).await?;
    Ok(report)
}

/// Derive and read one channel's receive addresses under the gap rule, without
/// persisting anything. Also returns whether any address holds outputs.
///
/// Reads run in gap-bounded windows through `outpoints_many`, consumed in
/// derivation order, exactly as the HD scanners do: a completed scan queries
/// only what a one-at-a-time scan would, and a missing row is an error.
async fn scan_channel<T: Transport>(
    provider: &Provider<T>,
    scope: NetworkScope,
    owner: &PrivatePaymentCode,
    peer: &PaymentCode,
    options: &PaymentScanOptions,
    cancelled: &mut impl FnMut() -> bool,
) -> Result<(PaymentScanReport, bool), QiError> {
    let mut report = PaymentScanReport {
        indexes: Vec::new(),
        next_index: options.range.start,
        stopped: ScanStop::RangeEnd,
    };
    let mut gap = GapCounter::new(options.gap_limit);
    let mut funded = false;
    while report.next_index < options.range.end {
        if cancelled() {
            return Err(QiError::Cancelled);
        }
        if report.indexes.len() == options.max_addresses {
            report.stopped = ScanStop::AddressLimit;
            break;
        }
        let window = gap.window(options.max_addresses - report.indexes.len());
        let mut found = Vec::with_capacity(window);
        let mut cursor = report.next_index;
        while found.len() < window && cursor < options.range.end {
            match owner.search(
                peer,
                PaymentDirection::Receive,
                PaymentSearch {
                    zone: scope.zone,
                    start_index: cursor,
                    max_attempts: (options.range.end - cursor)
                        .min(quai_payments::MAX_SEARCH_ATTEMPTS),
                },
                &mut *cancelled,
            ) {
                Ok(result) => {
                    cursor = result.next_index.unwrap_or(1 << 31);
                    found.push(result);
                }
                Err(PaymentError::SearchExhausted { next_index, .. }) => {
                    cursor = next_index.unwrap_or(1 << 31);
                }
                Err(PaymentError::SearchCancelled { .. }) => return Err(QiError::Cancelled),
                Err(_) => return Err(QiError::IdentityMismatch),
            }
        }
        if !found.is_empty() {
            let addresses: Vec<_> = found.iter().map(|result| result.address).collect();
            let mut observed = provider.outpoints_many(&addresses).await?;
            for result in &found {
                let used = !observed
                    .remove(&result.address)
                    .ok_or(QiError::IncompleteObservation)?
                    .is_empty();
                funded |= used;
                let reached_gap_limit = gap.observe(used);
                report.indexes.push(result.index);
                report.next_index = result.next_index.unwrap_or(1 << 31);
                if reached_gap_limit {
                    report.stopped = ScanStop::GapLimit;
                    return Ok((report, funded));
                }
            }
        }
        report.next_index = cursor;
    }
    Ok((report, funded))
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

/// Matching addresses a new announced sender's probe reads before registering.
///
/// A sender pays its first receive address first, so a real channel is funded
/// within a few addresses unless its early payments were already spent, which
/// needs the channel registered. A probe costs about 5 x 512 BIP47 candidates
/// instead of the full gap's 25,600.
#[cfg(feature = "abi")]
pub const MAILBOX_PROBE_GAP: u32 = 5;

/// One announced channel scanned during mailbox discovery.
#[cfg(feature = "abi")]
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct MailboxChannelScan {
    /// Validated sender code taken from the mailbox.
    pub sender: PaymentCode,
    /// Whether this call registered the channel; existing channels are rescanned.
    pub newly_registered: bool,
    /// Whether the channel is registered after this call. False for an
    /// unregistered sender whose probe found nothing: it was not persisted.
    pub registered: bool,
    /// Receive scan result: the full scan for a registered channel, otherwise
    /// the probe.
    pub report: PaymentScanReport,
}

/// Bounded mailbox discovery result. Absence of funds is not proof of none.
#[cfg(feature = "abi")]
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct MailboxDiscoveryReport {
    /// Announced channels registered (if needed) and scanned, in announcement order.
    pub scanned: Vec<MailboxChannelScan>,
    /// Valid announcements after this page, not registered or scanned.
    pub deferred: Vec<PaymentCode>,
    /// `start` for the next page when `deferred` is nonempty.
    pub next_start: Option<usize>,
    /// Announced entries that are not valid payment codes.
    pub invalid: Vec<String>,
    /// Repeated valid announcements that were collapsed.
    pub duplicates: usize,
}

/// Read Pelagus-compatible mailbox announcements for `owner`, then scan one page
/// of up to `max_channels` (1..=64) distinct valid senders beginning at index
/// `start`. Continue with `next_start` until it is `None`; the mailbox only
/// appends, so indexes are stable.
///
/// Announcements are unauthenticated and cost the announcer one zero-value
/// transaction, so an unregistered sender is only probed first, over
/// [`MAILBOX_PROBE_GAP`] addresses. Only a funded probe registers the channel
/// and runs the full scan with `options`; an empty one persists nothing, so
/// spam cannot grow the wallet's stored addresses or every later refresh.
/// Already-registered channels are rescanned in full. Stored addresses are
/// refreshed once per page. Send cursors never change; nothing is notified or
/// broadcast. To recover a sender whose early payments were spent before the
/// channel was registered, register it and call `scan_payment_channel`.
#[cfg(feature = "abi")]
#[allow(clippy::too_many_arguments)]
pub async fn discover_mailbox_channels<T: Transport>(
    provider: &Provider<T>,
    store: &mut SqliteStore,
    owner: &PrivatePaymentCode,
    mailbox: &crate::payment_mailbox::PaymentMailbox<'_, T>,
    caller: quai_primitives::QuaiAddress,
    start: usize,
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
    let scope = store.scope();
    if !crate::network::on_network(provider, scope, scope.zone).await? {
        return Err(QiError::NetworkMismatch);
    }
    let probe = PaymentScanOptions {
        gap_limit: Some(
            options
                .gap_limit
                .map_or(MAILBOX_PROBE_GAP, |gap| gap.min(MAILBOX_PROBE_GAP)),
        ),
        ..options.clone()
    };
    let end = start.saturating_add(max_channels);
    let mut senders = announced.senders.into_iter().skip(start);
    let mut imported = false;
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
            let (probed, funded) =
                scan_channel(provider, scope, owner, &sender, &probe, &mut cancelled).await?;
            if !funded {
                report.scanned.push(MailboxChannelScan {
                    sender,
                    newly_registered: false,
                    registered: false,
                    report: probed,
                });
                continue;
            }
            store.import_payment_channel(
                owner,
                &quai_payments::PaymentChannel::new(owner, sender.clone()),
                None,
            )?;
        }
        let (scan, _) =
            scan_channel(provider, scope, owner, &sender, options, &mut cancelled).await?;
        if cancelled() {
            return Err(QiError::Cancelled);
        }
        store.import_payment_receive_indexes(owner, &sender, &scan.indexes)?;
        imported = true;
        report.scanned.push(MailboxChannelScan {
            sender,
            newly_registered,
            registered: true,
            report: scan,
        });
    }
    if imported {
        refresh_qi(provider, store, 100_000, &mut cancelled).await?;
    }
    report.deferred = senders.collect();
    report.next_start = (!report.deferred.is_empty()).then_some(end);
    Ok(report)
}
