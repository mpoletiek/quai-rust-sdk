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
/// persisting anything. Also returns the value of the current outputs found.
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
) -> Result<(PaymentScanReport, U256), QiError> {
    let mut report = PaymentScanReport {
        indexes: Vec::new(),
        next_index: options.range.start,
        stopped: ScanStop::RangeEnd,
    };
    let mut gap = GapCounter::new(options.gap_limit);
    let mut found_value = U256::ZERO;
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
                let outputs = observed
                    .remove(&result.address)
                    .ok_or(QiError::IncompleteObservation)?;
                let used = !outputs.is_empty();
                for output in outputs {
                    found_value = found_value
                        .checked_add(U256::from(
                            quai_consensus::Denomination::new(output.denomination)?.value(),
                        ))
                        .ok_or(QiError::InvalidPolicy)?;
                }
                let reached_gap_limit = gap.observe(used);
                report.indexes.push(result.index);
                report.next_index = result.next_index.unwrap_or(1 << 31);
                if reached_gap_limit {
                    report.stopped = ScanStop::GapLimit;
                    return Ok((report, found_value));
                }
            }
        }
        report.next_index = cursor;
    }
    Ok((report, found_value))
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

/// Matching addresses a new announced sender's probe reads.
///
/// A sender pays its first receive address first, so a real channel with
/// unspent payments shows them within a few addresses. A probe costs about
/// 5 x 512 BIP47 candidates instead of the full gap's 25,600, and reads at
/// most twice this many addresses however the sender funds them.
#[cfg(feature = "abi")]
pub const MAILBOX_PROBE_GAP: u32 = 5;

/// What mailbox discovery may persist for a sender it has not registered.
#[cfg(feature = "abi")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum MailboxRegistration {
    /// Persist nothing for an unregistered sender; report its probe. The
    /// default, because announcements are unauthenticated: the application
    /// decides, for example by asking the user, whether to register one.
    #[default]
    ReportOnly,
    /// Register any sender whose probe finds unspent outputs, then scan it
    /// fully. Anyone can fund a probe with dust, so this suits a restore or an
    /// application that accepts that cost, not unattended background sync.
    RegisterFunded,
}

/// Where mailbox discovery reads announcements.
#[cfg(feature = "abi")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum MailboxSource {
    /// `getNotifications`: every announcement to the receiver in one call.
    /// Simple, but the response grows with the list, and anyone can grow the
    /// list, so past the response limit this read fails for good.
    #[default]
    Contract,
    /// `NotificationSent` logs in the inclusive block range, read in bounded
    /// requests (see `PaymentMailbox::notifications_in_blocks`). Spam slows
    /// it but cannot disable it. Page through one range with `start`, then
    /// move on to the next range from `to + 1`, keeping `to` at a block the
    /// wallet treats as settled.
    Logs {
        /// First block.
        from: u64,
        /// Last block, inclusive.
        to: u64,
    },
}

/// One page of mailbox discovery.
#[cfg(feature = "abi")]
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct MailboxDiscovery {
    /// First announcement index within `source`; continue with the report's
    /// `next_start`.
    pub start: usize,
    /// Distinct valid senders to scan in this page, 1 through 64.
    pub max_channels: usize,
    /// Scan bounds. With `gap_limit: None` an unregistered sender is scanned
    /// over the whole range instead of probed, as a restore needs when early
    /// payments may be spent; give an explicit range, since each BIP47
    /// candidate costs about 600 us.
    pub options: PaymentScanOptions,
    /// What to persist for unregistered senders.
    pub registration: MailboxRegistration,
    /// Where announcements are read.
    pub source: MailboxSource,
}
#[cfg(feature = "abi")]
impl MailboxDiscovery {
    /// A report-only page with default scan bounds.
    pub fn new(start: usize, max_channels: usize) -> Self {
        Self {
            start,
            max_channels,
            options: PaymentScanOptions::default(),
            registration: MailboxRegistration::ReportOnly,
            source: MailboxSource::Contract,
        }
    }
    /// Replace `start`.
    pub fn with_start(mut self, start: usize) -> Self {
        self.start = start;
        self
    }
    /// Replace `max_channels`.
    pub fn with_max_channels(mut self, max_channels: usize) -> Self {
        self.max_channels = max_channels;
        self
    }
    /// Replace `options`.
    pub fn with_options(mut self, options: PaymentScanOptions) -> Self {
        self.options = options;
        self
    }
    /// Replace `registration`.
    pub fn with_registration(mut self, registration: MailboxRegistration) -> Self {
        self.registration = registration;
        self
    }
    /// Replace `source`.
    pub fn with_source(mut self, source: MailboxSource) -> Self {
        self.source = source;
        self
    }
}

/// How discovery left an announced sender's channel.
#[cfg(feature = "abi")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelRegistration {
    /// Already registered; rescanned in full.
    Existing,
    /// Registered by this call and scanned in full.
    Registered,
    /// Not registered and nothing persisted; the report is its probe. It is
    /// probed again on every later pass that covers its index.
    Unregistered,
    /// Its probe found funds, but the store's channel limit is reached.
    Refused,
}

/// One announced sender scanned during mailbox discovery.
#[cfg(feature = "abi")]
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct MailboxChannelScan {
    /// Validated sender code taken from the mailbox.
    pub sender: PaymentCode,
    /// What happened to its channel.
    pub registration: ChannelRegistration,
    /// Receive scan or probe result.
    pub report: PaymentScanReport,
    /// Value of the unspent outputs the scan or probe found, in Qits. A
    /// threshold on this separates a real sender from probe dust.
    pub found: U256,
}

/// Bounded mailbox discovery result. Absence of funds is not proof of none.
#[cfg(feature = "abi")]
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct MailboxDiscoveryReport {
    /// Announced senders scanned, in announcement order.
    pub scanned: Vec<MailboxChannelScan>,
    /// Valid announcements after this page, not scanned.
    pub deferred: Vec<PaymentCode>,
    /// `start` for the next page when `deferred` is nonempty.
    pub next_start: Option<usize>,
    /// Announced entries that are not valid payment codes.
    pub invalid: Vec<String>,
    /// Repeated valid announcements that were collapsed.
    pub duplicates: usize,
}

/// Read Pelagus-compatible mailbox announcements for `owner` and scan one page
/// of distinct valid senders. Continue with `next_start` until it is `None`;
/// the mailbox only appends, so indexes are stable.
///
/// Registered channels are rescanned in full. An unregistered sender is
/// probed over [`MAILBOX_PROBE_GAP`] addresses (or scanned over the whole
/// range when `gap_limit` is `None`) and, under the default
/// [`MailboxRegistration::ReportOnly`], nothing is persisted for it: anyone can
/// announce a code for one zero-value transaction. Stored addresses are
/// refreshed once per page, and after a mid-page failure if anything was
/// already imported. Send cursors never change; nothing is broadcast.
///
/// With the default [`MailboxSource::Contract`], the mailbox returns every
/// announcement in one call, so a list larger than the transport's response
/// limit (about 5,400 announcements at the default 2 MiB) fails until that
/// limit is raised. [`MailboxSource::Logs`] reads bounded block ranges instead.
#[cfg(feature = "abi")]
pub async fn discover_mailbox_channels<T: Transport>(
    provider: &Provider<T>,
    store: &mut SqliteStore,
    owner: &PrivatePaymentCode,
    mailbox: &crate::payment_mailbox::PaymentMailbox<'_, T>,
    caller: quai_primitives::QuaiAddress,
    request: &MailboxDiscovery,
    mut cancelled: impl FnMut() -> bool,
) -> Result<MailboxDiscoveryReport, QiError> {
    if !(1..=64).contains(&request.max_channels) {
        return Err(QiError::InvalidPolicy);
    }
    validate_scan_options(&request.options)?;
    let announced = match request.source {
        MailboxSource::Contract => {
            mailbox
                .notifications(caller, owner.public_code(), quai_provider::BlockTag::Latest)
                .await
        }
        MailboxSource::Logs { from, to } if from <= to => {
            mailbox
                .notifications_in_blocks(owner.public_code(), from, to)
                .await
        }
        MailboxSource::Logs { .. } => return Err(QiError::InvalidPolicy),
    }
    .map_err(|error| match error {
        crate::contracts::ContractError::Provider(error) => QiError::Provider(error),
        _ => QiError::MailboxUnreadable,
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
    // Bound the probe's size as well as its gap: a sender knows the shared
    // secret, so it can fund every few addresses and keep the gap open. With
    // no gap limit the caller asked for full scans, so the probe is one.
    let probe = match request.options.gap_limit {
        Some(gap) => PaymentScanOptions {
            gap_limit: Some(gap.min(MAILBOX_PROBE_GAP)),
            max_addresses: request
                .options
                .max_addresses
                .min(2 * MAILBOX_PROBE_GAP as usize),
            ..request.options.clone()
        },
        None => request.options.clone(),
    };
    let end = request.start.saturating_add(request.max_channels);
    let mut senders = announced.senders.into_iter().skip(request.start);
    let mut imported = false;
    let mut failure = None;
    for sender in senders.by_ref().take(request.max_channels) {
        if sender == *owner.public_code() {
            report.invalid.push(sender.to_base58());
            continue;
        }
        match scan_announced(
            provider,
            store,
            owner,
            sender,
            request,
            &probe,
            &mut imported,
            &mut cancelled,
        )
        .await
        {
            Ok(scan) => report.scanned.push(scan),
            Err(error) => {
                failure = Some(error);
                break;
            }
        }
    }
    // Imports cleared the coin snapshot; rebuild it once, even after a
    // failure, so a mid-page error does not leave the wallet without one.
    if imported && !matches!(failure, Some(QiError::Cancelled)) {
        let refreshed = refresh_qi(provider, store, 100_000, &mut cancelled).await;
        if failure.is_none() {
            refreshed?;
        }
    }
    if let Some(error) = failure {
        return Err(error);
    }
    report.deferred = senders.collect();
    report.next_start = (!report.deferred.is_empty()).then_some(end);
    Ok(report)
}

/// Scan one announced sender under the page's registration policy.
#[cfg(feature = "abi")]
#[allow(clippy::too_many_arguments)]
async fn scan_announced<T: Transport>(
    provider: &Provider<T>,
    store: &mut SqliteStore,
    owner: &PrivatePaymentCode,
    sender: PaymentCode,
    request: &MailboxDiscovery,
    probe: &PaymentScanOptions,
    imported: &mut bool,
    cancelled: &mut impl FnMut() -> bool,
) -> Result<MailboxChannelScan, QiError> {
    if cancelled() {
        return Err(QiError::Cancelled);
    }
    let scope = store.scope();
    let (registration, probed) = if store.payment_channel(owner, &sender)?.is_some() {
        (ChannelRegistration::Existing, None)
    } else {
        let (probed, found) =
            scan_channel(provider, scope, owner, &sender, probe, cancelled).await?;
        if found == U256::ZERO || request.registration == MailboxRegistration::ReportOnly {
            return Ok(MailboxChannelScan {
                sender,
                registration: ChannelRegistration::Unregistered,
                report: probed,
                found,
            });
        }
        match store.import_payment_channel(
            owner,
            &quai_payments::PaymentChannel::new(owner, sender.clone()),
            None,
        ) {
            // The store's channel limit: skip this sender rather than fail
            // every later page at the same index.
            Err(quai_wallet::storage::StorageError::Invalid) => {
                return Ok(MailboxChannelScan {
                    sender,
                    registration: ChannelRegistration::Refused,
                    report: probed,
                    found,
                });
            }
            Err(error) => return Err(error.into()),
            // Registering cleared the coin snapshot, so it needs rebuilding
            // even if the scan below fails.
            Ok(_) => *imported = true,
        }
        // A probe as wide as the full scan already is the full scan.
        let full = probe.gap_limit == request.options.gap_limit
            && probe.max_addresses == request.options.max_addresses;
        let reuse = full.then_some((probed, found));
        (ChannelRegistration::Registered, reuse)
    };
    let (scan, found) = match probed {
        Some(done) => done,
        None => scan_channel(provider, scope, owner, &sender, &request.options, cancelled).await?,
    };
    if cancelled() {
        return Err(QiError::Cancelled);
    }
    store.import_payment_receive_indexes(owner, &sender, &scan.indexes)?;
    *imported = true;
    Ok(MailboxChannelScan {
        sender,
        registration,
        report: scan,
        found,
    })
}
