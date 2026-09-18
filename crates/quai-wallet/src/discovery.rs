//! Bounded, history-aware watch-only discovery; observations are not chain proofs.
use crate::{AccountPublic, CandidateCoin, CoinType, DerivedAddress, Search, WindowStop};
use quai_consensus::OutPoint;
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
    /// Observe several public addresses at the exact checkpoint, one result per
    /// address in input order.
    ///
    /// [`discover`] calls this with a window the gap rule guarantees it will
    /// examine whatever the answers, so a source may read them together without
    /// disclosing any address a completed one-at-a-time scan would not have.
    /// The default observes them one at a time through [`Self::observe`], and
    /// calls it for every address before awaiting any: a source whose `observe`
    /// sends a request before its future is polled should override this.
    #[cfg(not(target_arch = "wasm32"))]
    fn observe_many(
        &self,
        scope: NetworkScope,
        addresses: &[DerivedAddress],
        checkpoint: Checkpoint,
    ) -> impl Future<Output = Result<Vec<AddressObservation>, DiscoveryError>> + Send {
        // Futures are created up front so the returned future holds only them,
        // not `&self`, and is Send without requiring the source to be Sync.
        // They run one at a time, in order, provided `observe` does its work
        // when polled rather than when called, as an `async fn` does.
        let pending: Vec<_> = addresses
            .iter()
            .map(|address| self.observe(scope, address, checkpoint))
            .collect();
        async move {
            let mut observed = Vec::with_capacity(pending.len());
            for observation in pending {
                observed.push(observation.await?);
            }
            Ok(observed)
        }
    }
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
    /// Browser batch observation with no Send requirement; see the native form.
    #[cfg(target_arch = "wasm32")]
    fn observe_many(
        &self,
        scope: NetworkScope,
        addresses: &[DerivedAddress],
        checkpoint: Checkpoint,
    ) -> impl Future<Output = Result<Vec<AddressObservation>, DiscoveryError>> {
        async move {
            let mut observed = Vec::with_capacity(addresses.len());
            for address in addresses {
                observed.push(self.observe(scope, address, checkpoint).await?);
            }
            Ok(observed)
        }
    }
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
/// Most addresses a scanner derives ahead and reads in one round trip.
///
/// Not a safety bound: [`GapCounter::window`] is already bounded by the gap
/// guarantee. It caps how much derivation runs before the first read. Each
/// usable address costs hundreds of candidate derivations, so a very wide
/// window would front-load seconds of CPU before any network work began.
pub const MAX_SCAN_WINDOW: usize = 64;

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
    /// A batch of at most this size therefore queries only addresses a
    /// sequential scan that runs to completion would also query: nothing past
    /// the gap stop is disclosed. Results are consumed in order, so no
    /// observation past a failing address enters a cross-address budget. A
    /// scan aborted partway through a window (an error, a budget, or
    /// cancellation after the read) has still disclosed the rest of that
    /// window, at most `MAX_SCAN_WINDOW - 1` addresses a retry would query too.
    pub const fn guaranteed_remaining(&self) -> Option<u32> {
        match self.limit {
            // saturating_sub is defensive; observe stops the branch on equality.
            Some(limit) => Some(limit.saturating_sub(self.consecutive_unused)),
            None => None,
        }
    }
    /// Addresses a branch derives and reads together next.
    ///
    /// The gap guarantee, further capped by the scan's remaining address budget
    /// and [`MAX_SCAN_WINDOW`]. At least one, so a live branch always advances;
    /// callers check the address budget before asking.
    pub fn window(&self, address_budget: usize) -> usize {
        self.guaranteed_remaining()
            .map_or(usize::MAX, |n| usize::try_from(n).unwrap_or(usize::MAX))
            .min(address_budget)
            .clamp(1, MAX_SCAN_WINDOW)
    }
}

/// Exact examined interval and compact, lossless skipped raw-index intervals.
#[derive(Clone, Debug)]
#[non_exhaustive]
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
#[non_exhaustive]
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
/// Addresses are derived and observed in gap-bounded windows (see
/// [`ObservationSource::observe_many`]). Cancellation is checked before each window,
/// inside HD grinding and after each recorded address; in-flight source futures
/// require their own bounded timeout/cancellation support.
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
        'branch: while coverage.next_index < coverage.requested.end {
            if was_cancelled || cancelled() {
                was_cancelled = true;
                break;
            }
            if report.addresses.len() >= request.max_addresses {
                coverage.stop = ScanStop::AddressLimit;
                break;
            }
            // Derive a window the branch is guaranteed to examine and observe
            // it together. The gap counter cannot stop the branch inside its
            // own guarantee whatever the source answers, so nothing past the
            // gap stop is disclosed. See `GapCounter::guaranteed_remaining`
            // for what an aborted window discloses.
            let start = coverage.next_index;
            let window = account
                .search_window(
                    coverage.change,
                    Search {
                        zone: request.scope.zone,
                        start_index: start,
                        max_attempts: coverage.requested.end - start,
                    },
                    gap.window(request.max_addresses - report.addresses.len()),
                    &mut cancelled,
                )
                .map_err(|_| DiscoveryError::Derivation)?;
            let derived_to = window.next_index.unwrap_or(1 << 31);
            if window.addresses.is_empty() {
                // Every candidate examined was a zone or ledger mismatch.
                skip(coverage, derived_to);
                was_cancelled = window.stop == WindowStop::Cancelled;
                break;
            }
            // Addresses found before a cancellation are discarded unobserved,
            // so the cursor stays at the window start.
            if window.stop == WindowStop::Cancelled || cancelled() {
                was_cancelled = true;
                break;
            }
            let observed = source
                .observe_many(request.scope, &window.addresses, tip.checkpoint)
                .await?;
            if observed.len() != window.addresses.len() {
                return Err(DiscoveryError::InvalidObservation);
            }
            // Consume in derivation-index order. The cursor advances only after
            // each observation is recorded, so an error or cancellation never
            // leaves it past an unrecorded address.
            for (derived, mut observation) in window.addresses.into_iter().zip(observed) {
                let used = check_observation(
                    &mut observation,
                    &derived,
                    request,
                    tip.checkpoint,
                    account.coin_type(),
                    history,
                    &mut seen,
                    &mut total_coins,
                )?;
                let reached_gap_limit = gap.observe(used);
                skip(coverage, derived.index);
                coverage.next_index = derived.index + 1;
                coverage.observed += 1;
                report.addresses.push(ObservedAddress {
                    derived,
                    observation,
                });
                if cancelled() {
                    was_cancelled = true;
                    break 'branch;
                }
                if reached_gap_limit {
                    coverage.stop = ScanStop::GapLimit;
                    break 'branch;
                }
            }
            if window.stop == WindowStop::Exhausted {
                skip(coverage, derived_to);
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

/// Record raw children from the cursor up to `to` as skipped and advance to it.
fn skip(coverage: &mut BranchCoverage, to: u32) {
    if to > coverage.next_index {
        coverage.skipped.push(IndexRange {
            start: coverage.next_index,
            end: to,
        });
        coverage.next_index = to;
    }
}

/// Check one source response against its request and return whether the
/// address counts as used. Coins enter the report's budget and duplicate set.
#[allow(clippy::too_many_arguments)]
fn check_observation(
    observation: &mut AddressObservation,
    derived: &DerivedAddress,
    request: &DiscoveryRequest,
    checkpoint: Checkpoint,
    coin: CoinType,
    history: HistoryCapability,
    seen: &mut BTreeSet<OutPoint>,
    total_coins: &mut usize,
) -> Result<bool, DiscoveryError> {
    if observation.scope != request.scope
        || observation.checkpoint != checkpoint
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
    if coin == CoinType::Quai
        && (!observation.coins.is_empty()
            || observation.account_balance.is_none()
            || observation.account_nonce.is_none())
        || coin == CoinType::Qi
            && (observation.account_balance.is_some() || observation.account_nonce.is_some())
    {
        return Err(DiscoveryError::InvalidObservation);
    }
    *total_coins = total_coins
        .checked_add(observation.coins.len())
        .ok_or(DiscoveryError::InvalidObservation)?;
    if *total_coins > request.max_coins {
        return Err(DiscoveryError::InvalidObservation);
    }
    for candidate in &mut observation.coins {
        let hash = candidate.outpoint.transaction_hash.bytes();
        if candidate.address.address() != derived.address
            || hash[2] != request.scope.zone.byte()
            || *hash == [0; 32]
            || !seen.insert(candidate.outpoint)
            || candidate
                .expires_at
                .is_some_and(|height| height <= candidate.unlock_height)
        {
            return Err(DiscoveryError::InvalidObservation);
        }
        candidate.reserved = false;
    }
    Ok(if history == HistoryCapability::HistoricalEverUsed {
        observation.ever_used == Some(true)
    } else {
        observation.has_current_activity()
    })
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
