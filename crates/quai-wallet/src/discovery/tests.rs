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
