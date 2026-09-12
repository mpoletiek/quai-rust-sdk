//! Current-state Qi gap scanning and wallet refresh, matching normal quais.js use.
//! These latest-only node observations are not historical or atomic snapshots.
use crate::qi::QiError;
use quai_consensus::{Denomination, OutPoint};
use quai_primitives::{QiAddress, Zone};
use quai_provider::Provider;
use quai_rpc::{Transport, U256};
use quai_wallet::discovery::{Checkpoint, IndexRange, NetworkScope, ScanStop};
use quai_wallet::storage::{PublicAddress, Snapshot, SqliteStore};
use quai_wallet::{AccountPublic, CandidateCoin, CoinType, Search, WalletError};
use std::collections::BTreeSet;

/// Consecutive matching Qi addresses without current outpoints before gap stop.
pub const DEFAULT_QI_GAP: u32 = 50;

/// Mutually exclusive current-snapshot balance buckets in native Qits.
/// Durable claims take precedence over lock/expiry categories.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
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
pub struct QiScanOptions {
    /// Receive raw BIP32 child interval, including zone/ledger skips.
    pub receive: IndexRange,
    /// Change raw BIP32 child interval.
    pub change: IndexRange,
    /// Gap counts matching addresses, not raw children or individual UTXOs.
    pub gap_limit: Option<u32>,
    /// Global matching-address query limit, at most 100,000.
    pub max_addresses: usize,
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
        }
    }
}
/// Coverage of a current-state scan. Empty addresses may have fully spent history.
#[derive(Clone, Debug)]
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
    if provider.chain_id(scope.zone.into()).await? != scope.chain_id
        || provider.genesis_hash(scope.zone).await? != scope.genesis
    {
        return Err(QiError::IdentityMismatch);
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
    mut cancelled: impl FnMut() -> bool,
) -> Result<QiScanReport, QiError> {
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
        let mut gap = 0;
        while report.next_index[branch] < range.end {
            if cancelled() {
                report.stopped[branch..].fill(ScanStop::Cancelled);
                return Ok(report);
            }
            if report.addresses.len() == options.max_addresses {
                report.stopped[branch..].fill(ScanStop::AddressLimit);
                return Ok(report);
            }
            let start = report.next_index[branch];
            let found = match account.search(
                branch == 1,
                Search {
                    zone: scope.zone,
                    start_index: start,
                    max_attempts: range.end - start,
                },
                &mut cancelled,
            ) {
                Ok(found) => found,
                Err(WalletError::SearchExhausted { .. }) => {
                    report.next_index[branch] = range.end;
                    break;
                }
                Err(WalletError::Cancelled { next_index, .. }) => {
                    report.next_index[branch] = next_index;
                    report.stopped[branch..].fill(ScanStop::Cancelled);
                    return Ok(report);
                }
                Err(error) => return Err(error.into()),
            };
            let metadata = PublicAddress::derive(account, branch == 1, found.address.index)?;
            let address =
                QiAddress::try_from(metadata.address()).map_err(|_| QiError::IdentityMismatch)?;
            let outputs = provider.outpoints(address).await?;
            gap = if outputs.is_empty() { gap + 1 } else { 0 };
            report.next_index[branch] = found.next_index.unwrap_or(1 << 31);
            report.addresses.push(metadata);
            if options.gap_limit.is_some_and(|limit| gap >= limit) {
                report.stopped[branch] = ScanStop::GapLimit;
                break;
            }
        }
    }
    Ok(report)
}

/// Read all persisted Qi addresses, including imported/channel/change addresses,
/// then atomically replace their current coin view while preserving reservations.
/// Header samples must agree, but this is still a trusted latest-state observation,
/// not an atomic RPC snapshot, historical recovery, or spendability proof.
/// Node-side validation remains authoritative if an output is spent or trimmed later.
pub async fn refresh_qi<T: Transport>(
    provider: &Provider<T>,
    store: &mut SqliteStore,
    max_addresses: usize,
    mut cancelled: impl FnMut() -> bool,
) -> Result<Checkpoint, QiError> {
    if !(1..=100_000).contains(&max_addresses) {
        return Err(QiError::InvalidPolicy);
    }
    let scope = store.scope();
    if cancelled() {
        return Err(QiError::Cancelled);
    }
    identity(provider, scope).await?;
    let snapshot = store.snapshot()?;
    let mut generation = snapshot.generation;
    if let Some(old) = snapshot.checkpoint {
        let height = u64::try_from(old.height).map_err(|_| QiError::StaleSnapshot)?;
        let canonical = provider
            .header_at(scope.zone, height)
            .await?
            .map(|h| Checkpoint {
                hash: h.hash,
                height: U256::from(h.number),
            });
        if canonical != Some(old) {
            store.reconcile_checkpoint(generation, canonical)?;
            generation = store.snapshot()?.generation;
        }
    }
    let before = tip(provider, scope.zone).await?;
    let addresses = store.addresses()?;
    if addresses.len() > max_addresses {
        return Err(QiError::InvalidPolicy);
    }
    let mut coins = Vec::new();
    let mut seen = BTreeSet::new();
    for metadata in addresses {
        let Ok(address) = QiAddress::try_from(metadata.address()) else {
            continue;
        };
        if cancelled() {
            return Err(QiError::Cancelled);
        }
        for output in provider.outpoints(address).await? {
            let outpoint = OutPoint {
                transaction_hash: output.outpoint.tx_hash,
                index: output.outpoint.index,
            };
            if coins.len() == 100_000 || !seen.insert(outpoint) {
                return Err(QiError::IdentityMismatch);
            }
            coins.push(CandidateCoin {
                outpoint,
                address,
                denomination: Denomination::new(output.denomination)?,
                unlock_height: output.lock,
                expires_at: None,
                reserved: false,
            });
        }
    }
    if cancelled() {
        return Err(QiError::Cancelled);
    }
    let after = tip(provider, scope.zone).await?;
    if before != after {
        return Err(QiError::StaleSnapshot);
    }
    store.replace_snapshot(&Snapshot {
        scope,
        generation,
        checkpoint: Some(after),
        coins,
    })?;
    Ok(after)
}

async fn tip<T: Transport>(provider: &Provider<T>, zone: Zone) -> Result<Checkpoint, QiError> {
    let header = provider
        .latest_header(zone)
        .await?
        .ok_or(QiError::StaleSnapshot)?;
    Ok(Checkpoint {
        hash: header.hash,
        height: U256::from(header.number),
    })
}

/// Gap-scan a Qi account, persist discovered metadata, then refresh all known
/// origins. Change pools must already be allocated. Interrupted refresh leaves
/// the checkpoint invalid, so a session cannot use the incomplete refresh.
pub async fn scan_and_refresh_qi<T: Transport>(
    provider: &Provider<T>,
    store: &mut SqliteStore,
    account: &AccountPublic,
    options: &QiScanOptions,
    mut cancelled: impl FnMut() -> bool,
) -> Result<QiScanReport, QiError> {
    let report = scan_qi(provider, store.scope(), account, options, &mut cancelled).await?;
    if report.stopped.contains(&ScanStop::Cancelled) {
        return Err(QiError::Cancelled);
    }
    let generation = store.snapshot()?.generation;
    store.import_metadata(generation, &report.addresses)?;
    refresh_qi(provider, store, 100_000, cancelled).await?;
    Ok(report)
}
