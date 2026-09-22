//! Current-state Qi gap scanning and wallet refresh, matching normal quais.js use.
//! These latest-only node observations are not historical or atomic snapshots.
use crate::qi::QiError;
use quai_consensus::{Denomination, OutPoint};
use quai_primitives::QiAddress;
use quai_provider::{BlockTag, MAX_OUTPOINT_ADDRESSES, Provider};
use quai_rpc::{Transport, U256};
use quai_wallet::discovery::{Checkpoint, GapCounter, IndexRange, NetworkScope, ScanStop};
use quai_wallet::storage::{PublicAddress, Snapshot, SqliteStore, StorageError};
use quai_wallet::{AccountPublic, CandidateCoin, CoinType, Grinding, Search, WindowStop};
use std::collections::BTreeSet;
use std::future::Future;

pub use crate::discovery::DEFAULT_QI_GAP;

/// Mutually exclusive current-snapshot balance buckets in native Qits.
/// Durable claims take precedence over lock/expiry categories.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct QiBalance {
    /// All source-reported currently held outputs, including reserved/locked.
    pub total: U256,
    /// Unreserved, unlocked, unexpired outputs at the supplied candidate height.
    pub spendable: U256,
    /// Outputs held by a durable operation regardless of lock status.
    pub reserved: U256,
    /// Unreserved outputs whose observed lock exceeds the candidate height.
    pub locked: U256,
    /// Unreserved outputs outside an explicitly supplied expiry profile.
    pub expired: U256,
}

/// Aggregate every stored Qi origin with exact fixed denominations. A valid
/// snapshot is required; this does not refresh, prove history or include pending
/// incoming transactions. Use the next candidate block height for spend planning.
pub fn qi_balance(store: &mut SqliteStore, candidate_height: U256) -> Result<QiBalance, QiError> {
    let snapshot = store.snapshot()?;
    let checkpoint = snapshot.checkpoint.ok_or(QiError::MissingSnapshot)?;
    if candidate_height < checkpoint.height {
        return Err(QiError::StaleSnapshot);
    }
    let mut balance = QiBalance::default();
    for coin in snapshot.coins {
        let value = U256::from(coin.denomination.value());
        balance.total = balance
            .total
            .checked_add(value)
            .ok_or(QiError::InvalidPolicy)?;
        let bucket = if coin.reserved {
            &mut balance.reserved
        } else if coin.expires_at.is_some_and(|end| candidate_height >= end) {
            &mut balance.expired
        } else if coin.unlock_height > candidate_height {
            &mut balance.locked
        } else {
            &mut balance.spendable
        };
        *bucket = bucket.checked_add(value).ok_or(QiError::InvalidPolicy)?;
    }
    Ok(balance)
}

/// Bounded receive/change scan options. `None` gap performs an explicit deep scan.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct QiScanOptions {
    /// Receive raw BIP32 child interval, including zone/ledger skips.
    pub receive: IndexRange,
    /// Change raw BIP32 child interval.
    pub change: IndexRange,
    /// Gap counts matching addresses, not raw children or individual UTXOs.
    pub gap_limit: Option<u32>,
    /// Global matching-address query limit, at most 100,000.
    pub max_addresses: usize,
    /// How candidate addresses are derived. `Grinding::Parallel`, available
    /// with the `rayon` feature, uses the whole rayon pool.
    pub grinding: Grinding,
}
impl QiScanOptions {
    /// Replace `grinding`.
    pub const fn with_grinding(mut self, grinding: Grinding) -> Self {
        self.grinding = grinding;
        self
    }
    /// Replace `receive`.
    pub const fn with_receive(mut self, receive: IndexRange) -> Self {
        self.receive = receive;
        self
    }
    /// Replace `change`.
    pub const fn with_change(mut self, change: IndexRange) -> Self {
        self.change = change;
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
impl Default for QiScanOptions {
    fn default() -> Self {
        Self {
            receive: IndexRange {
                start: 0,
                end: 1_000_000,
            },
            change: IndexRange {
                start: 0,
                end: 1_000_000,
            },
            gap_limit: Some(DEFAULT_QI_GAP),
            max_addresses: 10_000,
            grinding: Grinding::Sequential,
        }
    }
}
/// Coverage of a current-state scan. Empty addresses may have fully spent history.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct QiScanReport {
    /// Derived public receive/change metadata, including the observed gap.
    pub addresses: Vec<PublicAddress>,
    /// First unexamined raw child for receive and change respectively.
    pub next_index: [u32; 2],
    /// Stop reasons for receive and change respectively.
    pub stopped: [ScanStop; 2],
}

async fn identity<T: Transport>(
    provider: &Provider<T>,
    scope: NetworkScope,
) -> Result<(), QiError> {
    if !crate::network::on_network(provider, scope, scope.zone).await? {
        return Err(QiError::NetworkMismatch);
    }
    Ok(())
}

/// Discover receive/change addresses by current outpoints, without secret keys.
/// No storage changes occur. A cancelled scan returns continuation metadata.
/// A successful gap stop does not establish complete historical wallet recovery.
pub async fn scan_qi<T: Transport>(
    provider: &Provider<T>,
    scope: NetworkScope,
    account: &AccountPublic,
    options: &QiScanOptions,
    cancelled: impl FnMut() -> bool,
) -> Result<QiScanReport, QiError> {
    scan_qi_with_use_checker(provider, scope, account, options, cancelled, |_, _| {
        std::future::ready(Ok(false))
    })
    .await
}

/// Scan with a caller-owned use hint for addresses without current outputs.
/// A true hint resets the gap; errors abort without importing metadata. Hints
/// never prove historical coverage. Bound callback I/O and retain known addresses.
pub async fn scan_qi_with_use_checker<T, F, Fut>(
    provider: &Provider<T>,
    scope: NetworkScope,
    account: &AccountPublic,
    options: &QiScanOptions,
    mut cancelled: impl FnMut() -> bool,
    mut check_use: F,
) -> Result<QiScanReport, QiError>
where
    T: Transport,
    F: FnMut(NetworkScope, QiAddress) -> Fut,
    Fut: Future<Output = Result<bool, QiError>>,
{
    if account.coin_type() != CoinType::Qi
        || !(1..=100_000).contains(&options.max_addresses)
        || options.gap_limit.is_some_and(|n| n == 0 || n > 10_000)
    {
        return Err(QiError::InvalidPolicy);
    }
    for range in [options.receive, options.change] {
        if range.start > range.end || range.end > 1 << 31 || range.end - range.start > 1_000_000 {
            return Err(QiError::InvalidPolicy);
        }
    }
    if cancelled() {
        return Err(QiError::Cancelled);
    }
    identity(provider, scope).await?;
    let mut report = QiScanReport {
        addresses: Vec::new(),
        next_index: [options.receive.start, options.change.start],
        stopped: [ScanStop::RangeEnd; 2],
    };
    for (branch, range) in [options.receive, options.change].iter().enumerate() {
        let mut gap = GapCounter::new(options.gap_limit);
        while report.next_index[branch] < range.end {
            if cancelled() {
                report.stopped[branch..].fill(ScanStop::Cancelled);
                return Ok(report);
            }
            if report.addresses.len() == options.max_addresses {
                report.stopped[branch..].fill(ScanStop::AddressLimit);
                return Ok(report);
            }
            // Derive a window of addresses the branch is guaranteed to examine,
            // then read them together. The bound is the gap counter's own rule
            // read forwards: the branch cannot stop within `guaranteed_remaining`
            // further addresses whatever the node answers, so none of these is
            // speculative. A completed scan queries exactly the sequential
            // scan's set, which is what makes this a batching change rather
            // than a policy change. An aborted window is covered at
            // `GapCounter::guaranteed_remaining`.
            let start = report.next_index[branch];
            let window = account
                .search_window_async(
                    branch == 1,
                    Search {
                        zone: scope.zone,
                        start_index: start,
                        max_attempts: range.end - start,
                    },
                    gap.window(options.max_addresses - report.addresses.len()),
                    options.grinding,
                    &mut cancelled,
                )
                .await?;
            if window.stop == WindowStop::Cancelled || cancelled() {
                // Nothing in this window was observed, so the resume point is
                // where the uncancelled scan would continue.
                if window.addresses.is_empty() {
                    report.next_index[branch] = window.next_index.unwrap_or(range.end);
                }
                report.stopped[branch..].fill(ScanStop::Cancelled);
                return Ok(report);
            }
            if window.addresses.is_empty() {
                report.next_index[branch] = range.end;
                break;
            }
            let derived = window.addresses;
            let addresses = derived
                .iter()
                .map(|found| {
                    QiAddress::try_from(found.address).map_err(|_| QiError::IdentityMismatch)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let observed = provider.outpoints_many(&addresses).await?;

            // Validate strictly in derivation-index order. The map is keyed by
            // address, so iterating it would apply the gap rule in address-byte
            // order and could stop the branch at the wrong point. The first
            // failure is reported after every address before it is recorded.
            let mut rows = Vec::with_capacity(derived.len());
            let mut failure = None;
            for found in &derived {
                let row = PublicAddress::derive(account, branch == 1, found.index)
                    .map_err(QiError::from)
                    .and_then(|metadata| {
                        let address = QiAddress::try_from(metadata.address())
                            .map_err(|_| QiError::IdentityMismatch)?;
                        // A missing row is a failure, never an empty result:
                        // defaulting it would count an unread address toward
                        // the gap and could stop a restore early.
                        let empty = observed
                            .get(&address)
                            .ok_or(QiError::IncompleteObservation)?
                            .is_empty();
                        Ok((found.index, metadata, address, empty))
                    });
                match row {
                    Ok(row) => rows.push(row),
                    Err(error) => {
                        failure = Some(error);
                        break;
                    }
                }
            }
            let needed = rows
                .iter()
                .map(|(_, _, address, empty)| (*address, *empty))
                .collect();
            let mut hints =
                std::pin::pin!(crate::network::use_hints(scope, needed, &mut check_use));
            let mut stop = false;
            for (index, metadata, _, empty) in rows {
                let use_hint = futures_util::StreamExt::next(&mut hints)
                    .await
                    .ok_or(QiError::IncompleteObservation)??;
                let reached_gap_limit = gap.observe(!empty || use_hint);
                // Advance only after the observation is recorded, so a failure
                // or cancellation never leaves the cursor past an unread
                // address. The stored cursor is monotonic and cannot be rewound.
                report.next_index[branch] = index + 1;
                report.addresses.push(metadata);
                if reached_gap_limit {
                    report.stopped[branch] = ScanStop::GapLimit;
                    stop = true;
                    break;
                }
                if cancelled() {
                    report.stopped[branch..].fill(ScanStop::Cancelled);
                    return Ok(report);
                }
            }
            if stop {
                break;
            }
            if let Some(error) = failure {
                return Err(error);
            }
            if window.stop == WindowStop::Exhausted {
                report.next_index[branch] = range.end;
                break;
            }
        }
    }
    Ok(report)
}

/// Options for [`refresh_qi_with`].
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct RefreshOptions {
    /// Stored-address limit, at most 100,000.
    pub max_addresses: usize,
    /// Refreshes tried before failing, 1 to 16. An attempt is lost to a new
    /// block during the reads or to another writer's commit, and the next one
    /// redoes every read.
    pub attempts: usize,
}
impl RefreshOptions {
    /// Replace `max_addresses`.
    pub const fn with_max_addresses(mut self, max_addresses: usize) -> Self {
        self.max_addresses = max_addresses;
        self
    }
    /// Replace `attempts`.
    pub const fn with_attempts(mut self, attempts: usize) -> Self {
        self.attempts = attempts;
        self
    }
}
impl Default for RefreshOptions {
    /// 100,000 addresses and three attempts, as `refresh_qi` uses.
    fn default() -> Self {
        Self {
            max_addresses: 100_000,
            attempts: 3,
        }
    }
}

/// Read all persisted Qi addresses, including imported/channel/change addresses,
/// then atomically replace their current coin view while preserving reservations.
/// The snapshot is labelled with the latest block seen before the reads and
/// written only if the latest block is unchanged after them. When nothing but
/// the label changed, only the label is written and the generation is kept.
///
/// Another handle on the same store may commit first. Its snapshot is returned
/// when it is labelled at or after this refresh's block; otherwise the refresh
/// starts again, so reads are never committed under a generation they did not
/// start from. After three lost attempts it fails with `QiError::StaleSnapshot`
/// (class `Stale`). This is still a trusted latest-state observation, not an
/// atomic RPC snapshot, historical recovery, or spendability proof. Node-side
/// validation remains authoritative if an output is spent or trimmed later.
pub async fn refresh_qi<T: Transport>(
    provider: &Provider<T>,
    store: &mut SqliteStore,
    max_addresses: usize,
    cancelled: impl FnMut() -> bool,
) -> Result<Checkpoint, QiError> {
    let options = RefreshOptions::default().with_max_addresses(max_addresses);
    refresh_qi_with(provider, store, options, cancelled).await
}

/// [`refresh_qi`] with an explicit attempt budget.
pub async fn refresh_qi_with<T: Transport>(
    provider: &Provider<T>,
    store: &mut SqliteStore,
    options: RefreshOptions,
    mut cancelled: impl FnMut() -> bool,
) -> Result<Checkpoint, QiError> {
    if !(1..=100_000).contains(&options.max_addresses) || !(1..=16).contains(&options.attempts) {
        return Err(QiError::InvalidPolicy);
    }
    // The tip must not move across the reads: then every output read is on
    // the chain through the label, and a later reorg of any block it came from
    // is caught by the label's canonicality check before a spend. Labelling
    // with an earlier block let an output from an orphaned block be selected,
    // leaving the spend's other inputs claimed. Reads are batched, so a
    // refresh usually fits inside one block; a new block retries the reads.
    for _ in 0..options.attempts {
        if let Some(checkpoint) =
            refresh_once(provider, store, options.max_addresses, &mut cancelled).await?
        {
            return Ok(checkpoint);
        }
    }
    Err(QiError::StaleSnapshot)
}

/// One refresh. `None` means a new block arrived during the reads, or another
/// writer committed an older snapshot first, and nothing was written.
async fn refresh_once<T: Transport>(
    provider: &Provider<T>,
    store: &mut SqliteStore,
    max_addresses: usize,
    cancelled: &mut impl FnMut() -> bool,
) -> Result<Option<Checkpoint>, QiError> {
    let scope = store.scope();
    if cancelled() {
        return Err(QiError::Cancelled);
    }
    let snapshot = store.snapshot()?;
    let mut generation = snapshot.generation;
    // The network check, the tip and the stored checkpoint's canonical header
    // are independent, address-free reads, so they travel together.
    let mut blocks = vec![BlockTag::Latest];
    if let Some(old) = snapshot.checkpoint {
        u64::try_from(old.height).map_err(|_| QiError::StaleSnapshot)?;
        blocks.push(BlockTag::Number(old.height));
    }
    let headers = crate::network::headers_on_network(provider, scope, scope.zone, &blocks)
        .await?
        .ok_or(QiError::NetworkMismatch)?;
    if let Some(old) = snapshot.checkpoint {
        let canonical = headers[1].as_ref().map(crate::network::checkpoint);
        if canonical != Some(old) {
            match store.reconcile_checkpoint(generation, canonical) {
                Err(StorageError::StaleSnapshot) => return Ok(None),
                result => result?,
            };
            generation = store.snapshot()?.generation;
        }
    }
    let before = headers[0]
        .as_ref()
        .map(crate::network::checkpoint)
        .ok_or(QiError::StaleSnapshot)?;
    let addresses = store.addresses()?;
    if addresses.len() > max_addresses {
        return Err(QiError::InvalidPolicy);
    }
    let mut coins = Vec::new();
    let mut seen = BTreeSet::new();
    let addresses: Vec<_> = addresses
        .into_iter()
        .filter_map(|metadata| QiAddress::try_from(metadata.address()).ok())
        .collect();
    // `outpoints_many` pages and adapts its own batch size, so each call gets
    // its full argument bound; paging smaller here would restart that
    // adaptation on every page. Cancellation is checked between calls.
    for page in addresses.chunks(MAX_OUTPOINT_ADDRESSES) {
        if cancelled() {
            return Err(QiError::Cancelled);
        }
        let mut observed = provider.outpoints_many(page).await?;
        for &address in page {
            // A missing row is a failure, never an empty result: it would
            // replace the address's coins with none.
            let outputs = observed
                .remove(&address)
                .ok_or(QiError::IncompleteObservation)?;
            for output in outputs {
                let outpoint = OutPoint {
                    transaction_hash: output.outpoint.tx_hash,
                    index: output.outpoint.index,
                };
                if coins.len() == 100_000 || !seen.insert(outpoint) {
                    return Err(QiError::IdentityMismatch);
                }
                coins.push(
                    CandidateCoin::new(outpoint, address, Denomination::new(output.denomination)?)
                        .with_unlock_height(output.lock),
                );
            }
        }
    }
    if cancelled() {
        return Err(QiError::Cancelled);
    }
    let after = provider
        .headers(scope.zone, &[BlockTag::Latest])
        .await?
        .pop()
        .flatten()
        .as_ref()
        .map(crate::network::checkpoint);
    if after != Some(before) {
        return Ok(None);
    }
    match store.replace_snapshot(&Snapshot::new(scope, generation, Some(before), coins)) {
        Ok(_) => Ok(Some(before)),
        // Another writer committed after our snapshot read. Its snapshot
        // stands in for ours only if it is at least as new; our reads are never
        // committed under its generation, which an import may have moved.
        Err(StorageError::StaleSnapshot) => Ok(store
            .snapshot()?
            .checkpoint
            .filter(|stored| stored.height > before.height || *stored == before)),
        Err(error) => Err(error.into()),
    }
}

/// Gap-scan a Qi account, persist discovered metadata, then refresh all known
/// origins. Change pools must already be allocated. Interrupted refresh leaves
/// the checkpoint invalid, so a session cannot use the incomplete refresh.
pub async fn scan_and_refresh_qi<T: Transport>(
    provider: &Provider<T>,
    store: &mut SqliteStore,
    account: &AccountPublic,
    options: &QiScanOptions,
    cancelled: impl FnMut() -> bool,
) -> Result<QiScanReport, QiError> {
    scan_and_refresh_qi_with_use_checker(provider, store, account, options, cancelled, |_, _| {
        std::future::ready(Ok(false))
    })
    .await
}

/// Gap-scan with optional use hints, persist public metadata, then refresh all
/// known origins. A failed checker leaves storage untouched. Refresh cancellation
/// after metadata import leaves the checkpoint invalid, preserving claim safety.
pub async fn scan_and_refresh_qi_with_use_checker<T, F, Fut>(
    provider: &Provider<T>,
    store: &mut SqliteStore,
    account: &AccountPublic,
    options: &QiScanOptions,
    mut cancelled: impl FnMut() -> bool,
    check_use: F,
) -> Result<QiScanReport, QiError>
where
    T: Transport,
    F: FnMut(NetworkScope, QiAddress) -> Fut,
    Fut: Future<Output = Result<bool, QiError>>,
{
    let report = scan_qi_with_use_checker(
        provider,
        store.scope(),
        account,
        options,
        &mut cancelled,
        check_use,
    )
    .await?;
    if report.stopped.contains(&ScanStop::Cancelled) {
        return Err(QiError::Cancelled);
    }
    let generation = store.snapshot()?.generation;
    store.import_metadata(generation, &report.addresses)?;
    refresh_qi(provider, store, 100_000, cancelled).await?;
    Ok(report)
}
