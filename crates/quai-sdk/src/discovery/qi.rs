//! Portable current-outpoint discovery; never advertises a historical or atomic snapshot.
use quai_consensus::{Denomination, OutPoint};
use quai_provider::{Provider, ProviderError};
use quai_rpc::{Transport, U256};
use quai_wallet::discovery::{CanonicalStatus, Checkpoint, IndexRange, NetworkScope, ScanStop};
use quai_wallet::{AccountPublic, CoinType, DerivedAddress, Search, WalletError};
use std::collections::BTreeSet;

/// Default consecutive empty matching addresses on each Qi branch.
pub const DEFAULT_QI_GAP: u32 = 50;
/// Public current-outpoint scan bounds, usable on native or wasm32.
#[derive(Clone, Debug)]
pub struct QiDiscoveryOptions {
    /// Receive raw BIP32 interval, including skipped zones/ledgers.
    pub receive: IndexRange,
    /// Independent change interval.
    pub change: IndexRange,
    /// Matching-address gap; None examines the full explicit ranges.
    pub gap_limit: Option<u32>,
    /// Maximum matching addresses across both branches, 1..=100,000.
    pub max_addresses: usize,
    /// Maximum total outputs retained across all addresses, 1..=100,000.
    pub max_outpoints: usize,
}
impl Default for QiDiscoveryOptions {
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
            max_outpoints: 100_000,
        }
    }
}
/// Compact current output observation. Unknown RPC extension fields are not retained.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CurrentQiOutput {
    /// Exact creating transaction and output index.
    pub outpoint: OutPoint,
    /// Validated fixed denomination; value is in Qits.
    pub denomination: Denomination,
    /// Node-reported absolute unlock height; no trim profile is inferred.
    pub unlock_height: U256,
}
/// One matched public HD address, including empty addresses used to measure the gap.
#[derive(Clone, Debug)]
pub struct CurrentQiAddress {
    /// Exact public key, coin/account/branch/index and zone.
    pub derived: DerivedAddress,
    /// Outputs returned by this address's latest-only RPC query.
    pub outputs: Vec<CurrentQiOutput>,
}
/// Bounded current-state discovery, without private keys or a storage backend.
#[derive(Clone, Debug)]
pub struct CurrentQiDiscovery {
    /// Explicit expected network identity.
    pub scope: NetworkScope,
    /// Head sampled before any address observation.
    pub checkpoint: Checkpoint,
    /// Matches only if the final latest and numbered headers still match the start.
    /// Even Matches does not make separate latest-only reads atomic.
    pub canonical: CanonicalStatus,
    /// Public matches and outputs, in receive then change order.
    pub addresses: Vec<CurrentQiAddress>,
    /// First unexamined raw child per branch.
    pub next_index: [u32; 2],
    /// Independent branch stop reasons. Cancellation leaves canonicality NotChecked.
    pub stopped: [ScanStop; 2],
}
/// Exact observed amounts; unlocked outputs may still be claimed, spent or trimmed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ObservedQiBalance {
    /// Sum over only this report's bounded address coverage.
    pub total: U256,
    /// Outputs whose reported unlock height exceeds the candidate height.
    pub locked: U256,
    /// Other observed outputs; deliberately not called spendable.
    pub unlocked: U256,
}
/// Portable Qi discovery errors preserve bounded provider/derivation diagnostics.
#[derive(Debug, thiserror::Error)]
pub enum QiDiscoveryError {
    /// Invalid coin, scope or explicit resource bounds.
    #[error("invalid Qi discovery request")]
    InvalidRequest,
    /// Cancellation before the initial network observations.
    #[error("Qi discovery cancelled before starting")]
    Cancelled,
    /// Reported chain/genesis differs from the requested identity.
    #[error("Qi discovery network identity mismatch")]
    IdentityMismatch,
    /// Total output budget exceeded, including across different addresses.
    #[error("Qi discovery output limit exceeded")]
    OutputLimit,
    /// Duplicate output across addresses or malformed denomination.
    #[error("inconsistent Qi discovery outputs")]
    InvalidOutputs,
    /// Head unavailable, changed, unchecked, or too new for the requested balance height.
    #[error("Qi discovery head is unavailable or changed")]
    ObservationChanged,
    /// Provider observation failed.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// Exact derivation failed.
    #[error(transparent)]
    Wallet(#[from] WalletError),
}
impl CurrentQiDiscovery {
    /// Sum exact denominations only after the scan's final head check succeeded.
    /// This neither merges local reservations nor proves full wallet coverage.
    pub fn balance_at(
        &self,
        candidate_height: U256,
    ) -> Result<ObservedQiBalance, QiDiscoveryError> {
        if self.canonical != CanonicalStatus::Matches || candidate_height < self.checkpoint.height {
            return Err(QiDiscoveryError::ObservationChanged);
        }
        let mut balance = ObservedQiBalance::default();
        for output in self.addresses.iter().flat_map(|a| &a.outputs) {
            let value = U256::from(output.denomination.value());
            balance.total = balance
                .total
                .checked_add(value)
                .ok_or(QiDiscoveryError::InvalidOutputs)?;
            let bucket = if output.unlock_height > candidate_height {
                &mut balance.locked
            } else {
                &mut balance.unlocked
            };
            *bucket = bucket
                .checked_add(value)
                .ok_or(QiDiscoveryError::InvalidOutputs)?;
        }
        Ok(balance)
    }
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
async fn head<T: Transport>(
    provider: &Provider<T>,
    scope: NetworkScope,
) -> Result<Checkpoint, QiDiscoveryError> {
    let header = provider
        .latest_header(scope.zone)
        .await?
        .ok_or(QiDiscoveryError::ObservationChanged)?;
    Ok(Checkpoint {
        hash: header.hash,
        height: U256::from(header.number),
    })
}
/// Scan current Qi receive/change outpoints with a matching-address gap (50 by default).
/// No indexer, signer or SQLite handle is required. Retain known addresses and use
/// explicit deep ranges when restoring: empty current outputs cannot reveal fully
/// spent history. Apply application claims/trim rules before using outputs to spend.
/// Cancellation returns public progress, never a completed current-state report.
pub async fn discover_qi<T: Transport>(
    provider: &Provider<T>,
    scope: NetworkScope,
    account: &AccountPublic,
    options: &QiDiscoveryOptions,
    mut cancelled: impl FnMut() -> bool,
) -> Result<CurrentQiDiscovery, QiDiscoveryError> {
    if account.coin_type() != CoinType::Qi
        || scope.chain_id == U256::ZERO
        || scope.genesis.bytes() == &[0; 32]
        || !(1..=100_000).contains(&options.max_addresses)
        || !(1..=100_000).contains(&options.max_outpoints)
        || options.gap_limit.is_some_and(|n| n == 0 || n > 10_000)
    {
        return Err(QiDiscoveryError::InvalidRequest);
    }
    for range in [options.receive, options.change] {
        if range.start > range.end || range.end > 1 << 31 || range.end - range.start > 1_000_000 {
            return Err(QiDiscoveryError::InvalidRequest);
        }
    }
    if cancelled() {
        return Err(QiDiscoveryError::Cancelled);
    }
    identity(provider, scope).await?;
    let checkpoint = head(provider, scope).await?;
    let mut report = CurrentQiDiscovery {
        scope,
        checkpoint,
        canonical: CanonicalStatus::NotChecked,
        addresses: vec![],
        next_index: [options.receive.start, options.change.start],
        stopped: [ScanStop::RangeEnd; 2],
    };
    let mut seen = BTreeSet::new();
    'branches: for (branch, range) in [options.receive, options.change].iter().enumerate() {
        let mut gap = 0;
        while report.next_index[branch] < range.end {
            if cancelled() {
                report.stopped[branch..].fill(ScanStop::Cancelled);
                return Ok(report);
            }
            if report.addresses.len() == options.max_addresses {
                report.stopped[branch..].fill(ScanStop::AddressLimit);
                break 'branches;
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
            if cancelled() {
                report.next_index[branch] = found.address.index;
                report.stopped[branch..].fill(ScanStop::Cancelled);
                return Ok(report);
            }
            let raw = provider
                .outpoints(
                    found
                        .address
                        .address
                        .try_into()
                        .map_err(|_| QiDiscoveryError::InvalidRequest)?,
                )
                .await?;
            if raw.len() > options.max_outpoints - seen.len() {
                return Err(QiDiscoveryError::OutputLimit);
            }
            let mut outputs = Vec::with_capacity(raw.len());
            for output in raw {
                let outpoint = OutPoint {
                    transaction_hash: output.outpoint.tx_hash,
                    index: output.outpoint.index,
                };
                if !seen.insert(outpoint) {
                    return Err(QiDiscoveryError::InvalidOutputs);
                }
                outputs.push(CurrentQiOutput {
                    outpoint,
                    denomination: Denomination::new(output.denomination)
                        .map_err(|_| QiDiscoveryError::InvalidOutputs)?,
                    unlock_height: output.lock,
                });
            }
            gap = if outputs.is_empty() { gap + 1 } else { 0 };
            report.next_index[branch] = found.next_index.unwrap_or(1 << 31);
            report.addresses.push(CurrentQiAddress {
                derived: found.address,
                outputs,
            });
            if options.gap_limit.is_some_and(|limit| gap >= limit) {
                report.stopped[branch] = ScanStop::GapLimit;
                break;
            }
        }
    }
    if cancelled() {
        report
            .stopped
            .iter_mut()
            .for_each(|s| *s = ScanStop::Cancelled);
        return Ok(report);
    }
    identity(provider, scope).await?;
    let after = head(provider, scope).await?;
    let height =
        u64::try_from(checkpoint.height).map_err(|_| QiDiscoveryError::ObservationChanged)?;
    let canonical = provider.header_at(scope.zone, height).await?;
    report.canonical =
        if checkpoint == after && canonical.is_some_and(|h| h.hash == checkpoint.hash) {
            CanonicalStatus::Matches
        } else {
            CanonicalStatus::Changed
        };
    Ok(report)
}
