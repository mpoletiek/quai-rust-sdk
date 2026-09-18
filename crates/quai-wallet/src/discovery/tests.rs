use super::*;
use crate::HdWallet;
use quai_consensus::{Denomination, OutPoint};
use quai_primitives::QiAddress;
use std::{
    sync::OnceLock,
    task::{Context, Poll, Waker},
};
pub(crate) fn ready<F: Future>(future: F) -> F::Output {
    let mut context = Context::from_waker(Waker::noop());
    let mut future = std::pin::pin!(future);
    match future.as_mut().poll(&mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("test source should be immediately ready"),
    }
}
fn scope() -> NetworkScope {
    NetworkScope {
        chain_id: U256::from(15000),
        genesis: Hash32::from_bytes([1; 32]),
        zone: Zone::Cyprus1,
    }
}
fn checkpoint() -> Checkpoint {
    Checkpoint {
        hash: Hash32::from_bytes([5; 32]),
        height: U256::from(5),
    }
}
fn account(coin: CoinType) -> AccountPublic {
    HdWallet::from_seed(&[0; 16], coin)
        .unwrap()
        .account_public(0)
        .unwrap()
}
fn matches() -> &'static [Vec<DerivedAddress>; 2] {
    static MATCHES: OnceLock<[Vec<DerivedAddress>; 2]> = OnceLock::new();
    MATCHES.get_or_init(|| {
        [false, true].map(|change| {
            let account = account(CoinType::Qi);
            let mut next = 0;
            (0..8)
                .map(|_| {
                    let found = account
                        .search(
                            change,
                            Search {
                                zone: scope().zone,
                                start_index: next,
                                max_attempts: 10000,
                            },
                            || false,
                        )
                        .unwrap();
                    next = found.address.index + 1;
                    found.address
                })
                .collect()
        })
    })
}
struct Source {
    history: HistoryCapability,
    missing_history: bool,
    changed: bool,
    mismatch: bool,
}
impl Source {
    fn new(history: HistoryCapability) -> Self {
        Self {
            history,
            missing_history: false,
            changed: false,
            mismatch: false,
        }
    }
}
impl ObservationSource for Source {
    fn history_capability(&self, _: NetworkScope, _: CoinType) -> HistoryCapability {
        self.history
    }
    async fn tip(&self, scope: NetworkScope) -> Result<ScopedCheckpoint, DiscoveryError> {
        Ok(ScopedCheckpoint {
            scope,
            checkpoint: checkpoint(),
        })
    }
    async fn observe(
        &self,
        scope: NetworkScope,
        address: &DerivedAddress,
        checkpoint: Checkpoint,
    ) -> Result<AddressObservation, DiscoveryError> {
        let position = matches()[usize::from(address.change)]
            .iter()
            .position(|value| value.address == address.address);
        let funded = position == Some(5);
        let coins = if funded && address.coin == CoinType::Qi {
            let mut hash = [0; 32];
            hash[3] = 0x80;
            hash[31] = if address.change { 2 } else { 1 };
            vec![CandidateCoin {
                outpoint: OutPoint {
                    transaction_hash: Hash32::from_bytes(hash),
                    index: 0,
                },
                address: QiAddress::try_from(address.address).unwrap(),
                denomination: Denomination::new(2).unwrap(),
                unlock_height: U256::from(10),
                expires_at: Some(U256::from(20)),
                reserved: true,
            }]
        } else {
            vec![]
        };
        Ok(AddressObservation {
            scope,
            checkpoint: if self.mismatch {
                Checkpoint {
                    height: U256::ZERO,
                    ..checkpoint
                }
            } else {
                checkpoint
            },
            address: address.address,
            ever_used: if self.history == HistoryCapability::HistoricalEverUsed
                && !self.missing_history
            {
                Some(position.is_some_and(|n| n <= 5))
            } else {
                None
            },
            account_balance: (address.coin == CoinType::Quai).then_some(U256::ZERO),
            account_nonce: (address.coin == CoinType::Quai).then_some(0),
            coins,
        })
    }
    async fn canonical(
        &self,
        scope: NetworkScope,
        _: U256,
    ) -> Result<Option<ScopedCheckpoint>, DiscoveryError> {
        Ok(if self.changed {
            None
        } else {
            Some(ScopedCheckpoint {
                scope,
                checkpoint: checkpoint(),
            })
        })
    }
}
fn request() -> DiscoveryRequest {
    DiscoveryRequest {
        scope: scope(),
        receive: IndexRange {
            start: 0,
            end: matches()[0][7].index + 1,
        },
        change: IndexRange {
            start: 0,
            end: matches()[1][7].index + 1,
        },
        gap_limit: Some(2),
        require_history: false,
        max_addresses: 100,
        max_coins: 100,
    }
}
#[test]
fn current_only_gap_cannot_recover_fully_spent_run_but_explicit_ranges_find_later_funds() {
    let source = Source::new(HistoryCapability::CurrentStateOnly);
    let account = account(CoinType::Qi);
    let mut request = request();
    let report = ready(discover(&source, &account, &request, || false)).unwrap();
    assert_eq!(report.history, HistoryCapability::CurrentStateOnly);
    assert_eq!(report.trust, ObservationTrust::SourceClaim);
    assert!(
        report
            .coverage
            .iter()
            .all(|c| c.stop == ScanStop::GapLimit && c.observed == 2)
    );
    assert!(
        report
            .addresses
            .iter()
            .all(|a| a.observation.coins.is_empty())
    );
    request.gap_limit = None;
    let report = ready(discover(&source, &account, &request, || false)).unwrap();
    assert!(report.coverage.iter().all(|c| c.stop == ScanStop::RangeEnd));
    assert_eq!(
        report
            .addresses
            .iter()
            .flat_map(|a| &a.observation.coins)
            .count(),
        2
    );
    assert!(
        report
            .addresses
            .iter()
            .flat_map(|a| &a.observation.coins)
            .all(|c| !c.reserved)
    );
    for coverage in &report.coverage {
        let indices: Vec<_> = report
            .addresses
            .iter()
            .filter(|a| a.derived.change == coverage.change)
            .map(|a| a.derived.index)
            .collect();
        let mut examined: BTreeSet<u32> = indices.iter().copied().collect();
        for range in &coverage.skipped {
            for index in range.start..range.end {
                assert!(examined.insert(index));
            }
        }
        assert_eq!(
            examined,
            (coverage.requested.start..coverage.next_index).collect()
        );
    }
}
#[test]
fn historical_ever_used_continues_across_fully_spent_addresses() {
    let source = Source::new(HistoryCapability::HistoricalEverUsed);
    let account = account(CoinType::Qi);
    let request = request();
    let report = ready(discover(&source, &account, &request, || false)).unwrap();
    assert!(report.coverage.iter().all(|c| c.observed == 8));
    assert_eq!(
        report
            .addresses
            .iter()
            .flat_map(|a| &a.observation.coins)
            .count(),
        2
    );
    assert_eq!(report.canonical, CanonicalStatus::Matches);
}
#[test]
fn unavailable_history_inconsistent_checkpoints_and_cancellation_are_explicit() {
    let account = account(CoinType::Qi);
    let mut request = request();
    request.require_history = true;
    assert!(matches!(
        ready(discover(
            &Source::new(HistoryCapability::CurrentStateOnly),
            &account,
            &request,
            || false
        )),
        Err(DiscoveryError::HistoryUnavailable)
    ));
    let mut source = Source::new(HistoryCapability::HistoricalEverUsed);
    source.missing_history = true;
    assert!(matches!(
        ready(discover(&source, &account, &request, || false)),
        Err(DiscoveryError::HistoryUnavailable)
    ));
    source.missing_history = false;
    source.mismatch = true;
    assert!(matches!(
        ready(discover(&source, &account, &request, || false)),
        Err(DiscoveryError::InvalidObservation)
    ));
    source.mismatch = false;
    source.changed = true;
    assert_eq!(
        ready(discover(&source, &account, &request, || false))
            .unwrap()
            .canonical,
        CanonicalStatus::Changed
    );
    let mut calls = 0;
    let report = ready(discover(&source, &account, &request, || {
        calls += 1;
        calls > 10
    }))
    .unwrap();
    assert!(
        report
            .coverage
            .iter()
            .all(|c| c.stop == ScanStop::Cancelled)
    );
    assert_eq!(report.canonical, CanonicalStatus::NotChecked);
}
#[test]
fn watch_only_quai_both_branches_and_spendability_flags() {
    let source = Source::new(HistoryCapability::CurrentStateOnly);
    let account = account(CoinType::Quai);
    let account = AccountPublic::import(&account.export(), CoinType::Quai, 0).unwrap();
    let mut request = request();
    request.receive.end = 10000;
    request.change.end = 10000;
    let report = ready(discover(&source, &account, &request, || false)).unwrap();
    assert!(report.coverage.iter().all(|c| c.observed == 2));
    let mut hash = [0; 32];
    hash[3] = 0x80;
    let coin = CandidateCoin {
        outpoint: OutPoint {
            transaction_hash: Hash32::from_bytes(hash),
            index: 0,
        },
        address: QiAddress::try_from(matches()[0][0].address).unwrap(),
        denomination: Denomination::new(0).unwrap(),
        unlock_height: U256::from(10),
        expires_at: Some(U256::from(20)),
        reserved: false,
    };
    assert!(classify_coin(&coin, U256::from(9), false).locked);
    assert!(classify_coin(&coin, U256::from(10), false).spendable);
    assert!(classify_coin(&coin, U256::from(20), false).expired);
    assert!(!classify_coin(&coin, U256::from(10), true).spendable);
}

/// Records each `observe_many` window, delegating every address to `Source`.
struct Recording {
    inner: Source,
    windows: std::sync::Mutex<Vec<Vec<DerivedAddress>>>,
}
impl ObservationSource for Recording {
    fn history_capability(&self, scope: NetworkScope, coin: CoinType) -> HistoryCapability {
        self.inner.history_capability(scope, coin)
    }
    async fn tip(&self, scope: NetworkScope) -> Result<ScopedCheckpoint, DiscoveryError> {
        self.inner.tip(scope).await
    }
    async fn observe(
        &self,
        _: NetworkScope,
        _: &DerivedAddress,
        _: Checkpoint,
    ) -> Result<AddressObservation, DiscoveryError> {
        panic!("the scanner reads through observe_many")
    }
    async fn observe_many(
        &self,
        scope: NetworkScope,
        addresses: &[DerivedAddress],
        checkpoint: Checkpoint,
    ) -> Result<Vec<AddressObservation>, DiscoveryError> {
        self.windows.lock().unwrap().push(addresses.to_vec());
        let mut observed = vec![];
        for address in addresses {
            observed.push(self.inner.observe(scope, address, checkpoint).await?);
        }
        Ok(observed)
    }
    async fn canonical(
        &self,
        scope: NetworkScope,
        height: U256,
    ) -> Result<Option<ScopedCheckpoint>, DiscoveryError> {
        self.inner.canonical(scope, height).await
    }
}

#[test]
fn windows_observe_exactly_the_reported_addresses_and_batch_deep_scans() {
    let account = account(CoinType::Qi);
    for (history, gap_limit) in [
        (HistoryCapability::CurrentStateOnly, Some(1)),
        (HistoryCapability::CurrentStateOnly, Some(2)),
        (HistoryCapability::HistoricalEverUsed, Some(2)),
        (HistoryCapability::HistoricalEverUsed, None),
    ] {
        let source = Recording {
            inner: Source::new(history),
            windows: Default::default(),
        };
        let request = DiscoveryRequest {
            gap_limit,
            ..request()
        };
        let report = ready(discover(&source, &account, &request, || false)).unwrap();
        let windows = source.windows.into_inner().unwrap();
        let observed: Vec<_> = windows.iter().flatten().cloned().collect();
        let reported: Vec<_> = report.addresses.iter().map(|a| a.derived.clone()).collect();
        // Same addresses in the same order: nothing read past the gap stop.
        assert_eq!(observed, reported, "{history:?} gap {gap_limit:?}");
        if gap_limit.is_none() {
            // A deep scan is unbounded by the gap, so each branch is one window.
            assert_eq!(windows.len(), 2);
            assert!(windows.iter().all(|w| w.len() == 8));
        }
    }
}

#[test]
fn a_short_batch_response_is_rejected() {
    struct Short(Source);
    impl ObservationSource for Short {
        fn history_capability(&self, scope: NetworkScope, coin: CoinType) -> HistoryCapability {
            self.0.history_capability(scope, coin)
        }
        async fn tip(&self, scope: NetworkScope) -> Result<ScopedCheckpoint, DiscoveryError> {
            self.0.tip(scope).await
        }
        async fn observe(
            &self,
            scope: NetworkScope,
            address: &DerivedAddress,
            checkpoint: Checkpoint,
        ) -> Result<AddressObservation, DiscoveryError> {
            self.0.observe(scope, address, checkpoint).await
        }
        async fn observe_many(
            &self,
            scope: NetworkScope,
            addresses: &[DerivedAddress],
            checkpoint: Checkpoint,
        ) -> Result<Vec<AddressObservation>, DiscoveryError> {
            // Drops the last address: counting it as unused could stop a
            // restore early, so the scanner must refuse the whole response.
            let mut observed = vec![];
            for address in &addresses[..addresses.len() - 1] {
                observed.push(self.0.observe(scope, address, checkpoint).await?);
            }
            Ok(observed)
        }
        async fn canonical(
            &self,
            scope: NetworkScope,
            height: U256,
        ) -> Result<Option<ScopedCheckpoint>, DiscoveryError> {
            self.0.canonical(scope, height).await
        }
    }
    let source = Short(Source::new(HistoryCapability::CurrentStateOnly));
    assert!(matches!(
        ready(discover(
            &source,
            &account(CoinType::Qi),
            &request(),
            || false
        )),
        Err(DiscoveryError::InvalidObservation)
    ));
}

#[test]
fn cancellation_inside_a_window_never_advances_past_an_unrecorded_address() {
    // A deep scan makes the receive branch one window spanning its range, so
    // every cancelled() call can be placed exactly: one before the tip, one at
    // the loop top, one per derived candidate, one before the read, then one
    // after each record.
    let account = account(CoinType::Qi);
    let request = DiscoveryRequest {
        gap_limit: None,
        ..request()
    };
    let receive = &matches()[0];
    let end = request.receive.end;

    // Cancelled while deriving, after three addresses were already found: they
    // were never observed, so nothing is recorded and the cursor stays put.
    let during = 2 + receive[2].index + 1 + 2;
    let source = Recording {
        inner: Source::new(HistoryCapability::CurrentStateOnly),
        windows: Default::default(),
    };
    let mut calls = 0;
    let report = ready(discover(&source, &account, &request, || {
        calls += 1;
        calls > during
    }))
    .unwrap();
    assert!(
        source.windows.into_inner().unwrap().is_empty(),
        "nothing read"
    );
    assert!(report.addresses.is_empty());
    assert_eq!(report.coverage[0].next_index, request.receive.start);
    assert!(report.coverage[0].skipped.is_empty());
    assert_eq!(report.coverage[0].stop, ScanStop::Cancelled);

    // Cancelled after the fourth record of a read window: the cursor is just
    // past the last recorded address and coverage is exactly what was seen.
    let before_records = 2 + end + 1;
    let mut calls = 0;
    let report = ready(discover(
        &Source::new(HistoryCapability::CurrentStateOnly),
        &account,
        &request,
        || {
            calls += 1;
            calls > before_records + 3
        },
    ))
    .unwrap();
    let coverage = &report.coverage[0];
    assert_eq!(report.addresses.len(), 4);
    assert_eq!(coverage.stop, ScanStop::Cancelled);
    assert_eq!(coverage.next_index, receive[3].index + 1);
    let mut examined: BTreeSet<u32> = report.addresses.iter().map(|a| a.derived.index).collect();
    for range in &coverage.skipped {
        for index in range.start..range.end {
            assert!(examined.insert(index), "skipped overlaps observed");
        }
    }
    assert_eq!(
        examined,
        (coverage.requested.start..coverage.next_index).collect()
    );
}
