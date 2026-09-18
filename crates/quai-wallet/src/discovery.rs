//! Bounded, history-aware watch-only discovery; observations are not chain proofs.
use crate::{AccountPublic, CandidateCoin, CoinType, DerivedAddress, Search, WalletError};
use quai_consensus::U256;
use quai_primitives::{Address, Hash32, Zone};
use std::{collections::BTreeSet, future::Future};
use thiserror::Error;

/// Exact network identity: matching chain IDs alone do not identify a network.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkScope {
    /// Unsigned 256-bit chain ID.
    pub chain_id: U256,
    /// Trusted configured genesis hash.
    pub genesis: Hash32,
    /// Zone whose state is stored.
    pub zone: Zone,
}
impl NetworkScope {
    pub(crate) fn key(self) -> [u8; 65] {
        let mut key = [0; 65];
        key[..32].copy_from_slice(&self.chain_id.to_be_bytes::<32>());
        key[32..64].copy_from_slice(self.genesis.bytes());
        key[64] = self.zone.byte();
        key
    }
}
/// Caller-verified block observation; storage does not verify canonicality/finality.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Checkpoint {
    /// Observed block hash.
    pub hash: Hash32,
    /// Observed block height.
    pub height: U256,
}
/// What the source claims to know for every observed address at its checkpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryCapability {
    /// Current balances/outpoints cannot identify fully spent, previously used addresses.
    CurrentStateOnly,
    /// Source supplies historical ever-used information, including fully spent addresses.
    HistoricalEverUsed,
}
/// Explicit trust boundary; none of these observations constitute verified chain proofs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservationTrust {
    /// Relies on the configured source's truthfulness, history index and canonical view.
    SourceClaim,
}
/// Exact identity attached to a node checkpoint response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScopedCheckpoint {
    /// Configured chain, genesis and zone.
    pub scope: NetworkScope,
    /// Source-reported block.
    pub checkpoint: Checkpoint,
}
/// Public source response; all addresses/UTXOs are checked before entering a report.
#[derive(Clone, Debug)]
pub struct AddressObservation {
    /// Response identity must equal the requested network scope.
    pub scope: NetworkScope,
    /// Response must refer to the exact requested checkpoint.
    pub checkpoint: Checkpoint,
    /// Response must refer to the exact requested address.
    pub address: Address,
    /// Required for HistoricalEverUsed; None means history is unavailable.
    pub ever_used: Option<bool>,
    /// Required for Quai, absent for Qi; never proof of an unused address.
    pub account_balance: Option<U256>,
    /// Required for Quai, absent for Qi, at the same checkpoint.
    pub account_nonce: Option<u64>,
    /// Qi current unspent outputs. Provider reserved flags are ignored.
    pub coins: Vec<CandidateCoin>,
}
impl AddressObservation {
    /// Whether current state demonstrates activity; false does not imply never used.
    pub fn has_current_activity(&self) -> bool {
        !self.coins.is_empty()
            || self.account_balance.is_some_and(|v| v != U256::ZERO)
            || self.account_nonce.is_some_and(|v| v != 0)
    }
}
/// Source or data failures contain no secret material or arbitrary remote text.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
#[non_exhaustive]
pub enum DiscoveryError {
    /// Invalid range, bound or zero genesis identity.
    #[error("invalid discovery bounds or identity")]
    InvalidRequest,
    /// Cancelled before selecting a scan checkpoint; no observation was requested.
    #[error("wallet discovery cancelled before starting")]
    Cancelled,
    /// Source does not supply the requested historical coverage.
    #[error("historical address usage is unavailable")]
    HistoryUnavailable,
    /// Observation source request failed.
    #[error("wallet observation source unavailable")]
    SourceUnavailable,
    /// Source returned inconsistent scope, checkpoint, address or coin data.
    #[error("inconsistent wallet observation")]
    InvalidObservation,
    /// Key derivation failed, rather than merely selecting another ledger/zone.
    #[error("wallet discovery key derivation failed")]
    Derivation,
}
/// Async transport-independent observation contract. Implementations must bound I/O,
/// response sizes and timeouts. All response state must be read at the requested block;
/// do not fabricate historical observations from unpinned latest-only RPC methods.
pub trait ObservationSource {
    /// Source-advertised historical coverage for this exact scope and coin type.
    fn history_capability(&self, scope: NetworkScope, coin: CoinType) -> HistoryCapability;
    /// Select a source-observed scan checkpoint (native futures are Send).
    #[cfg(not(target_arch = "wasm32"))]
    fn tip(
        &self,
        scope: NetworkScope,
    ) -> impl Future<Output = Result<ScopedCheckpoint, DiscoveryError>> + Send;
    /// Observe one public address at the exact checkpoint.
    #[cfg(not(target_arch = "wasm32"))]
    fn observe(
        &self,
        scope: NetworkScope,
        address: &DerivedAddress,
        checkpoint: Checkpoint,
    ) -> impl Future<Output = Result<AddressObservation, DiscoveryError>> + Send;
    /// Re-read canonical hash at this height. None means absent/noncanonical/unknown;
    /// the report then cannot be committed as a consistent snapshot.
    #[cfg(not(target_arch = "wasm32"))]
    fn canonical(
        &self,
        scope: NetworkScope,
        height: U256,
    ) -> impl Future<Output = Result<Option<ScopedCheckpoint>, DiscoveryError>> + Send;
    /// Browser checkpoint request may retain thread-local, non-Send transport state.
    #[cfg(target_arch = "wasm32")]
    fn tip(
        &self,
        scope: NetworkScope,
    ) -> impl Future<Output = Result<ScopedCheckpoint, DiscoveryError>>;
    /// Browser observation request with no Send requirement.
    #[cfg(target_arch = "wasm32")]
    fn observe(
        &self,
        scope: NetworkScope,
        address: &DerivedAddress,
        checkpoint: Checkpoint,
    ) -> impl Future<Output = Result<AddressObservation, DiscoveryError>>;
    /// Browser canonical-view request with no Send requirement.
    #[cfg(target_arch = "wasm32")]
    fn canonical(
        &self,
        scope: NetworkScope,
        height: U256,
    ) -> impl Future<Output = Result<Option<ScopedCheckpoint>, DiscoveryError>>;
}
/// Half-open raw BIP32 index range. Zone/ledger mismatches still consume indexes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndexRange {
    /// First raw child index, inclusive.
    pub start: u32,
    /// Last raw child index, exclusive; may equal 2^31.
    pub end: u32,
}
/// Receive and change are scanned independently, always with explicit raw bounds.
#[derive(Clone, Debug)]
pub struct DiscoveryRequest {
    /// Trusted network identity.
    pub scope: NetworkScope,
    /// External branch index interval.
    pub receive: IndexRange,
    /// Internal branch index interval.
    pub change: IndexRange,
    /// Optional consecutive-empty matching-address heuristic. None scans each full range.
    /// Even historical gaps cannot prove no used addresses exist beyond the bounds.
    pub gap_limit: Option<u32>,
    /// Reject current-state-only sources before scanning when true.
    pub require_history: bool,
    /// Maximum observed matching addresses across both branches, 1 through 100,000.
    pub max_addresses: usize,
    /// Maximum total returned current UTXOs, 1 through 100,000.
    pub max_coins: usize,
}
/// Why a branch stopped; all are bounded coverage, never a proof of complete recovery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanStop {
    /// Examined the complete explicit raw index interval.
    RangeEnd,
    /// Consecutive source-reported unused/empty matching addresses reached the gap heuristic.
    GapLimit,
    /// Global matching-address bound reached.
    AddressLimit,
    /// Cancellation observed; partial report must not be committed as a snapshot.
    Cancelled,
}
/// The BIP44-style consecutive-unused-address rule, as one shared object.
///
/// Every scanner in this workspace applies the same recurrence: the counter
/// resets on a used address and increments on an unused one, and the branch
/// stops once it reaches the limit. It lived as three hand-written copies, in
/// the storage-backed Qi scanner, the portable Qi scanner and the generic
/// source scanner. Three copies of a rule that decides whether a restored
/// wallet finds all of its funds is a divergence hazard: one scanner stopping
/// at 49 where another stops at 50 is a silent difference in what is
/// recoverable.
///
/// [`Self::guaranteed_remaining`] is the same rule read forwards, and is what
/// makes batched scanning safe: the branch provably cannot stop within that
/// many further addresses, whatever the node answers, so they can be fetched
/// together without querying an address the sequential scan would not have.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GapCounter {
    consecutive_unused: u32,
    limit: Option<u32>,
}
impl GapCounter {
    /// Start a branch. `None` disables the rule, as an explicit deep scan does.
    pub const fn new(limit: Option<u32>) -> Self {
        Self {
            consecutive_unused: 0,
            limit,
        }
    }
    /// Record one observed address in index order.
    ///
    /// Returns true when the branch must stop at this address, which the caller
    /// reports as [`ScanStop::GapLimit`]. The address is still part of the
    /// report: the rule stops *after* it, exactly as the sequential scanners do.
    pub const fn observe(&mut self, used: bool) -> bool {
        self.consecutive_unused = if used {
            0
        } else {
            self.consecutive_unused.saturating_add(1)
        };
        match self.limit {
            Some(limit) => self.consecutive_unused >= limit,
            None => false,
        }
    }
    /// Consecutive unused addresses observed since the last used one.
    pub const fn consecutive_unused(&self) -> u32 {
        self.consecutive_unused
    }
    /// How many further addresses this branch is guaranteed to examine.
    ///
    /// The counter can only reach the limit by incrementing once per address,
    /// so the branch cannot stop before that many more are observed, no matter
    /// what the node reports for any of them. `None` means unbounded, which is
    /// what an explicit deep scan with no gap limit requests.
    ///
    /// A batch of at most this size therefore queries only addresses the
    /// sequential scan would also have queried: no speculation, no extra
    /// disclosure to the node, and no observation that could contaminate a
    /// cross-address budget.
    pub const fn guaranteed_remaining(&self) -> Option<u32> {
        match self.limit {
            // saturating_sub is defensive; observe stops the branch on equality.
            Some(limit) => Some(limit.saturating_sub(self.consecutive_unused)),
            None => None,
        }
    }
}

/// Exact examined interval and compact, lossless skipped raw-index intervals.
#[derive(Clone, Debug)]
pub struct BranchCoverage {
    /// Change branch when true, receive otherwise.
    pub change: bool,
    /// Caller-authorized raw interval.
    pub requested: IndexRange,
    /// First unexamined raw index, including continuation after skipped candidates.
    pub next_index: u32,
    /// Ledger/zone mismatches, represented as half-open intervals.
    pub skipped: Vec<IndexRange>,
    /// Number of matching addresses actually observed.
    pub observed: usize,
    /// Reason scanning stopped.
    pub stop: ScanStop,
}
/// A matching HD address and its checkpoint-tagged public observation.
#[derive(Clone, Debug)]
pub struct ObservedAddress {
    /// Actual child index and public-key origin, not a count of matching addresses.
    pub derived: DerivedAddress,
    /// Source observation; fully spent historical addresses can have no current coins.
    pub observation: AddressObservation,
}
/// Final canonical-view recheck, not a consensus finality proof.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CanonicalStatus {
    /// Source still reports the scan checkpoint's hash at its height.
    Matches,
    /// Source changed/removed that hash; discard/invalidate corresponding snapshots.
    Changed,
    /// Cancellation prevented the final check.
    NotChecked,
}
/// Bounded discovery outcome. There is intentionally no `complete recovery` flag.
#[derive(Clone, Debug)]
pub struct DiscoveryReport {
    /// Exact scanned network scope.
    pub scope: NetworkScope,
    /// Account xpub only, enabling public origin validation when committing metadata.
    pub account_xpub: String,
    /// BIP44 coin type.
    pub coin: CoinType,
    /// Declared hardened account index.
    pub account: u32,
    /// Shared checkpoint for all responses.
    pub checkpoint: Checkpoint,
    /// Source's declared history capability.
    pub history: HistoryCapability,
    /// Explicit source-trust boundary.
    pub trust: ObservationTrust,
    /// Canonical-view recheck status.
    pub canonical: CanonicalStatus,
    /// Receive then change coverage.
    pub coverage: [BranchCoverage; 2],
    /// Matching addresses observed within bounds, including empty and fully spent ones.
    pub addresses: Vec<ObservedAddress>,
}
/// Independent spendability flags: a reserved output may also be locked/expired.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoinClassification {
    /// A durable local operation still holds this outpoint.
    pub reserved: bool,
    /// Candidate spend height precedes the node/profile unlock height.
    pub locked: bool,
    /// Candidate height is at/after the profile's exclusive expiry bound.
    pub expired: bool,
    /// Unreserved, unlocked and unexpired at the supplied candidate height.
    pub spendable: bool,
}
/// Classify using locally recovered reservation state, not an RPC reserved flag.
pub fn classify_coin(
    coin: &CandidateCoin,
    candidate_height: U256,
    reserved: bool,
) -> CoinClassification {
    let locked = candidate_height < coin.unlock_height;
    let expired = coin
        .expires_at
        .is_some_and(|height| candidate_height >= height);
    CoinClassification {
        reserved,
        locked,
        expired,
        spendable: !reserved && !locked && !expired,
    }
}

/// Scan one watch-only account's receive and change branches. Use explicit ranges
/// without gap stopping when recovering wallets that exceeded a gap or lack history.
/// Cancellation is checked around every awaited observation and inside HD grinding;
/// in-flight source futures require their own bounded timeout/cancellation support.
pub async fn discover<S: ObservationSource>(
    source: &S,
    account: &AccountPublic,
    request: &DiscoveryRequest,
    mut cancelled: impl FnMut() -> bool,
) -> Result<DiscoveryReport, DiscoveryError> {
    for range in [request.receive, request.change] {
        if range.start > range.end || range.end > 1 << 31 || range.end - range.start > 1_000_000 {
            return Err(DiscoveryError::InvalidRequest);
        }
    }
    if request.scope.genesis.bytes() == &[0; 32]
        || !(1..=100_000).contains(&request.max_addresses)
        || !(1..=100_000).contains(&request.max_coins)
        || request.gap_limit.is_some_and(|v| v == 0 || v > 10_000)
    {
        return Err(DiscoveryError::InvalidRequest);
    }
    if cancelled() {
        return Err(DiscoveryError::Cancelled);
    }
    let history = source.history_capability(request.scope, account.coin_type());
    if request.require_history && history != HistoryCapability::HistoricalEverUsed {
        return Err(DiscoveryError::HistoryUnavailable);
    }
    let tip = source.tip(request.scope).await?;
    if tip.scope != request.scope {
        return Err(DiscoveryError::InvalidObservation);
    }
    let mut report = DiscoveryReport {
        scope: request.scope,
        account_xpub: account.export(),
        coin: account.coin_type(),
        account: account.account_index(),
        checkpoint: tip.checkpoint,
        history,
        trust: ObservationTrust::SourceClaim,
        canonical: CanonicalStatus::NotChecked,
        coverage: [(false, request.receive), (true, request.change)].map(|(change, range)| {
            BranchCoverage {
                change,
                requested: range,
                next_index: range.start,
                skipped: vec![],
                observed: 0,
                stop: ScanStop::RangeEnd,
            }
        }),
        addresses: vec![],
    };
    let mut total_coins = 0usize;
    let mut seen = BTreeSet::new();
    let mut was_cancelled = false;
    for coverage in &mut report.coverage {
        let mut gap = GapCounter::new(request.gap_limit);
        while coverage.next_index < coverage.requested.end {
            if was_cancelled || cancelled() {
                was_cancelled = true;
                coverage.stop = ScanStop::Cancelled;
                break;
            }
            if report.addresses.len() >= request.max_addresses {
                coverage.stop = ScanStop::AddressLimit;
                break;
            }
            let start = coverage.next_index;
            let found = account.search(
                coverage.change,
                Search {
                    zone: request.scope.zone,
                    start_index: start,
                    max_attempts: coverage.requested.end - start,
                },
                &mut cancelled,
            );
            let derived = match found {
                Ok(found) => {
                    if found.address.index > start {
                        coverage.skipped.push(IndexRange {
                            start,
                            end: found.address.index,
                        });
                    }
                    found.address
                }
                Err(WalletError::SearchExhausted { attempts, .. }) => {
                    coverage.next_index = start + attempts;
                    if attempts > 0 {
                        coverage.skipped.push(IndexRange {
                            start,
                            end: coverage.next_index,
                        });
                    }
                    break;
                }
                Err(WalletError::Cancelled { attempts, .. }) => {
                    coverage.next_index = start + attempts;
                    if attempts > 0 {
                        coverage.skipped.push(IndexRange {
                            start,
                            end: coverage.next_index,
                        });
                    }
                    coverage.stop = ScanStop::Cancelled;
                    was_cancelled = true;
                    break;
                }
                Err(_) => return Err(DiscoveryError::Derivation),
            };
            // Do not advance past a matching address until its observation is recorded.
            coverage.next_index = derived.index;
            if cancelled() {
                was_cancelled = true;
                coverage.stop = ScanStop::Cancelled;
                break;
            }
            let mut observation = source
                .observe(request.scope, &derived, tip.checkpoint)
                .await?;
            if observation.scope != request.scope
                || observation.checkpoint != tip.checkpoint
                || observation.address != derived.address
            {
                return Err(DiscoveryError::InvalidObservation);
            }
            if history == HistoryCapability::HistoricalEverUsed && observation.ever_used.is_none() {
                return Err(DiscoveryError::HistoryUnavailable);
            }
            if observation.ever_used == Some(false) && observation.has_current_activity() {
                return Err(DiscoveryError::InvalidObservation);
            }
            if account.coin_type() == CoinType::Quai
                && (!observation.coins.is_empty()
                    || observation.account_balance.is_none()
                    || observation.account_nonce.is_none())
                || account.coin_type() == CoinType::Qi
                    && (observation.account_balance.is_some()
                        || observation.account_nonce.is_some())
            {
                return Err(DiscoveryError::InvalidObservation);
            }
            total_coins = total_coins
                .checked_add(observation.coins.len())
                .ok_or(DiscoveryError::InvalidObservation)?;
            if total_coins > request.max_coins {
                return Err(DiscoveryError::InvalidObservation);
            }
            for coin in &mut observation.coins {
                let hash = coin.outpoint.transaction_hash.bytes();
                if coin.address.address() != derived.address
                    || hash[2] != request.scope.zone.byte()
                    || *hash == [0; 32]
                    || !seen.insert(coin.outpoint)
                    || coin
                        .expires_at
                        .is_some_and(|height| height <= coin.unlock_height)
                {
                    return Err(DiscoveryError::InvalidObservation);
                }
                coin.reserved = false;
            }
            let used = if history == HistoryCapability::HistoricalEverUsed {
                observation.ever_used == Some(true)
            } else {
                observation.has_current_activity()
            };
            let reached_gap_limit = gap.observe(used);
            coverage.next_index = derived.index + 1;
            coverage.observed += 1;
            report.addresses.push(ObservedAddress {
                derived,
                observation,
            });
            if cancelled() {
                was_cancelled = true;
                coverage.stop = ScanStop::Cancelled;
                break;
            }
            if reached_gap_limit {
                coverage.stop = ScanStop::GapLimit;
                break;
            }
        }
        if was_cancelled {
            coverage.stop = ScanStop::Cancelled;
        }
    }
    if !was_cancelled {
        let canonical = source
            .canonical(request.scope, tip.checkpoint.height)
            .await?;
        report.canonical = match canonical {
            Some(value)
                if value.scope != request.scope
                    || value.checkpoint.height != tip.checkpoint.height =>
            {
                return Err(DiscoveryError::InvalidObservation);
            }
            Some(value) if value.checkpoint == tip.checkpoint => CanonicalStatus::Matches,
            _ => CanonicalStatus::Changed,
        };
    }
    Ok(report)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod gap_counter_tests {
    use super::GapCounter;

    #[test]
    fn the_counter_resets_on_use_and_stops_after_the_limit() {
        let mut gap = GapCounter::new(Some(3));
        // Unused addresses accumulate; the branch stops on the third.
        assert!(!gap.observe(false));
        assert!(!gap.observe(false));
        assert!(
            gap.observe(false),
            "the limit stops the branch at the third"
        );

        // A used address resets the run, so the limit is not reached early.
        let mut gap = GapCounter::new(Some(3));
        assert!(!gap.observe(false));
        assert!(!gap.observe(false));
        assert!(!gap.observe(true), "a used address resets the run");
        assert_eq!(gap.consecutive_unused(), 0);
        assert!(!gap.observe(false));
        assert!(!gap.observe(false));
        assert!(gap.observe(false));
    }

    #[test]
    fn no_limit_never_stops_the_branch() {
        let mut gap = GapCounter::new(None);
        for _ in 0..10_000 {
            assert!(!gap.observe(false));
        }
        assert_eq!(gap.guaranteed_remaining(), None, "unbounded by request");
    }

    #[test]
    fn guaranteed_remaining_never_promises_more_than_the_branch_will_examine() {
        // This is the property batched scanning depends on: a batch of at most
        // `guaranteed_remaining` queries only addresses the sequential scan
        // would also have queried, whatever the node answers for any of them.
        for limit in 1..=64u32 {
            for used_at in [None, Some(0), Some(1), Some(limit / 2)] {
                let mut gap = GapCounter::new(Some(limit));
                let mut examined = 0u32;
                loop {
                    let promised = gap.guaranteed_remaining().unwrap();
                    assert!(promised > 0, "a live branch always has room to examine");
                    // Worst case for the promise: every one of them is unused.
                    let mut probe = gap;
                    for step in 0..promised {
                        let stops = probe.observe(false);
                        assert_eq!(
                            stops,
                            step + 1 == promised,
                            "limit {limit}: the branch stopped early inside its own guarantee"
                        );
                    }
                    let used = used_at == Some(examined);
                    let stopped = gap.observe(used);
                    examined += 1;
                    if stopped {
                        break;
                    }
                    assert!(
                        examined < limit * 4,
                        "limit {limit}: branch did not terminate"
                    );
                }
            }
        }
    }
}
