//! Portable current-outpoint discovery; never advertises a historical or atomic snapshot.
use quai_consensus::{Denomination, OutPoint};
use quai_primitives::QiAddress;
use quai_provider::{Provider, ProviderError};
use quai_rpc::{Transport, U256};
use quai_wallet::discovery::{
    CanonicalStatus, Checkpoint, GapCounter, IndexRange, NetworkScope, ScanStop,
};
use quai_wallet::{AccountPublic, CoinType, DerivedAddress, Search, WalletError, WindowStop};
use std::collections::BTreeSet;
use std::future::Future;

/// Default consecutive empty matching addresses on each Qi branch.
pub const DEFAULT_QI_GAP: u32 = 50;
/// Public current-outpoint scan bounds, usable on native or wasm32.
#[derive(Clone, Debug)]
#[non_exhaustive]
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
impl QiDiscoveryOptions {
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
    /// Replace `max_outpoints`.
    pub const fn with_max_outpoints(mut self, max_outpoints: usize) -> Self {
        self.max_outpoints = max_outpoints;
        self
    }
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
    /// Caller-supplied use hint for an address with no current outputs.
    /// This affects gap counting and does not certify historical coverage.
    pub use_hint: bool,
}
/// Bounded current-state discovery, without private keys or a storage backend.
#[derive(Clone, Debug)]
#[non_exhaustive]
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
#[non_exhaustive]
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
#[non_exhaustive]
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
    /// An optional caller-owned address-use query failed.
    #[error("Qi address-use check failed")]
    UseCheckFailed,
    /// A batched read omitted an address it was asked for.
    #[error("Qi discovery observation omitted an address")]
    IncompleteObservation,
    /// Provider observation failed.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// Exact derivation failed.
    #[error(transparent)]
    Wallet(#[from] WalletError),
}
impl QiDiscoveryError {
    /// How to react to this failure; see [`quai_primitives::ErrorClass`].
    pub fn class(&self) -> quai_primitives::ErrorClass {
        use quai_primitives::ErrorClass;
        match self {
            Self::IdentityMismatch => ErrorClass::NetworkMismatch,
            Self::Provider(error) => error.class(),
            Self::ObservationChanged => ErrorClass::Stale,
            Self::Cancelled => ErrorClass::Cancelled,
            Self::UseCheckFailed | Self::IncompleteObservation => ErrorClass::Transient,
            _ => ErrorClass::Invalid,
        }
    }
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
pub(super) async fn identity<T: Transport>(
    provider: &Provider<T>,
    scope: NetworkScope,
) -> Result<(), QiDiscoveryError> {
    if !crate::network::on_network(provider, scope, scope.zone).await? {
        return Err(QiDiscoveryError::IdentityMismatch);
    }
    Ok(())
}
async fn head<T: Transport>(
    provider: &Provider<T>,
    scope: NetworkScope,
) -> Result<Checkpoint, QiDiscoveryError> {
    crate::network::latest_checkpoint(provider, scope.zone)
        .await?
        .ok_or(QiDiscoveryError::ObservationChanged)
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
    cancelled: impl FnMut() -> bool,
) -> Result<CurrentQiDiscovery, QiDiscoveryError> {
    discover_qi_with_use_checker(provider, scope, account, options, cancelled, |_, _| {
        std::future::ready(Ok(false))
    })
    .await
}

/// Current Qi discovery with an optional, explicitly scoped address-use hint.
/// Called only when an address has no current outputs; true resets the gap but
/// never fabricates a UTXO or an atomic/historical coverage claim. Errors propagate
/// instead of treating an unavailable history service as unused. The caller must
/// bound its callback's I/O, time and memory, including in browser workers.
pub async fn discover_qi_with_use_checker<T, F, Fut>(
    provider: &Provider<T>,
    scope: NetworkScope,
    account: &AccountPublic,
    options: &QiDiscoveryOptions,
    mut cancelled: impl FnMut() -> bool,
    mut check_use: F,
) -> Result<CurrentQiDiscovery, QiDiscoveryError>
where
    T: Transport,
    F: FnMut(NetworkScope, QiAddress) -> Fut,
    Fut: Future<Output = Result<bool, QiDiscoveryError>>,
{
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
        let mut gap = GapCounter::new(options.gap_limit);
        while report.next_index[branch] < range.end {
            if cancelled() {
                report.stopped[branch..].fill(ScanStop::Cancelled);
                return Ok(report);
            }
            if report.addresses.len() == options.max_addresses {
                report.stopped[branch..].fill(ScanStop::AddressLimit);
                break 'branches;
            }
            // Read a window the gap rule guarantees this branch examines, so
            // nothing past the gap stop is disclosed. See
            // `GapCounter::guaranteed_remaining`, including for aborted windows.
            let start = report.next_index[branch];
            let window = account.search_window(
                branch == 1,
                Search {
                    zone: scope.zone,
                    start_index: start,
                    max_attempts: range.end - start,
                },
                gap.window(options.max_addresses - report.addresses.len()),
                &mut cancelled,
            )?;
            if window.stop == WindowStop::Cancelled || cancelled() {
                // Nothing in this window was observed; resume past any
                // mismatches only when no address was found before them.
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
            let addresses = window
                .addresses
                .iter()
                .map(|derived| {
                    QiAddress::try_from(derived.address)
                        .map_err(|_| QiDiscoveryError::InvalidRequest)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let mut observed = provider.outpoints_many(&addresses).await?;
            // Consume in derivation-index order, never by iterating the
            // address-keyed map, and advance only after recording each address.
            for (derived, address) in window.addresses.into_iter().zip(addresses) {
                // A missing row is a failure, never an empty result: counting
                // an unread address toward the gap could stop a restore early.
                let raw = observed
                    .remove(&address)
                    .ok_or(QiDiscoveryError::IncompleteObservation)?;
                if raw.len() > options.max_outpoints - seen.len() {
                    return Err(QiDiscoveryError::OutputLimit);
                }
                let mut outputs = Vec::with_capacity(raw.len());
                for output in raw {
                    let outpoint = OutPoint {
                        transaction_hash: output.outpoint.tx_hash,
                        index: output.outpoint.index,
                    };
                    // An outpoint from another zone, or the zero hash, could never
                    // be spent here and would fail every later selection.
                    let hash = outpoint.transaction_hash.bytes();
                    if hash[2] != scope.zone.byte() || *hash == [0; 32] || !seen.insert(outpoint) {
                        return Err(QiDiscoveryError::InvalidOutputs);
                    }
                    outputs.push(CurrentQiOutput {
                        outpoint,
                        denomination: Denomination::new(output.denomination)
                            .map_err(|_| QiDiscoveryError::InvalidOutputs)?,
                        unlock_height: output.lock,
                    });
                }
                let use_hint = outputs.is_empty() && check_use(scope, address).await?;
                let reached_gap_limit = gap.observe(!outputs.is_empty() || use_hint);
                report.next_index[branch] = derived.index + 1;
                report.addresses.push(CurrentQiAddress {
                    derived,
                    outputs,
                    use_hint,
                });
                if reached_gap_limit {
                    report.stopped[branch] = ScanStop::GapLimit;
                    continue 'branches;
                }
                if cancelled() {
                    report.stopped[branch..].fill(ScanStop::Cancelled);
                    return Ok(report);
                }
            }
            if window.stop == WindowStop::Exhausted {
                report.next_index[branch] = range.end;
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
