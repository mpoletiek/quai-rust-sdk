//! The parallel grind must return results identical to the sequential one.
//!
//! "Valid" is not enough. `next_index` is persisted monotonically, so a search
//! that resumed from a different match than the sequential search would could
//! skip an address permanently, and a wallet restored on a machine with a
//! different core count would derive a different address set. Every field is
//! compared, not just the address.
#![cfg(all(feature = "rayon", not(target_arch = "wasm32")))]

use quai_primitives::Zone;
use quai_wallet::{CoinType, HdWallet, Search, WalletError};

const SEED: [u8; 64] = [0x2b; 64];

fn account(coin: CoinType) -> quai_wallet::AccountPublic {
    HdWallet::from_seed(&SEED, coin)
        .expect("fixture seed builds a wallet")
        .account_public(0)
        .expect("account zero derives")
}

#[test]
fn parallel_and_sequential_search_agree_on_every_field() {
    for coin in [CoinType::Qi, CoinType::Quai] {
        let account = account(coin);
        for change in [false, true] {
            for zone in [Zone::Cyprus1, Zone::Paxos2, Zone::Hydra3] {
                // Several start indexes, including ones landing mid-chunk, so
                // the chunk boundary is not accidentally aligned with a match.
                for start in [0u32, 1, 7, 2_047, 2_048, 2_049, 5_000] {
                    let request = Search {
                        zone,
                        start_index: start,
                        max_attempts: 60_000,
                    };
                    let sequential = account.search(change, request, || false);
                    let parallel = account.search_parallel(change, request, || false);
                    match (sequential, parallel) {
                        (Ok(a), Ok(b)) => {
                            assert_eq!(a.address, b.address, "address differs at start {start}");
                            assert_eq!(
                                a.attempts, b.attempts,
                                "attempts differ at start {start}: {a:?} vs {b:?}"
                            );
                            assert_eq!(
                                a.next_index, b.next_index,
                                "next_index differs at start {start}"
                            );
                            assert_eq!(a.address.zone, zone);
                            assert_eq!(a.address.index, b.address.index);
                        }
                        (Err(a), Err(b)) => assert_eq!(
                            std::mem::discriminant(&a),
                            std::mem::discriminant(&b),
                            "error kind differs at start {start}"
                        ),
                        (a, b) => panic!("outcomes differ at start {start}: {a:?} vs {b:?}"),
                    }
                }
            }
        }
    }
}

#[test]
fn a_match_beyond_the_attempt_budget_is_not_returned_by_either() {
    // A budget too small to contain a match must exhaust identically, and the
    // resume index must point at the first unexamined candidate on both paths.
    let account = account(CoinType::Qi);
    for attempts in [1u32, 2, 17, 100] {
        let request = Search {
            zone: Zone::Hydra3,
            start_index: 0,
            max_attempts: attempts,
        };
        let sequential = account.search(false, request, || false);
        let parallel = account.search_parallel(false, request, || false);
        match (&sequential, &parallel) {
            (
                Err(WalletError::SearchExhausted {
                    attempts: a,
                    next_index: ai,
                }),
                Err(WalletError::SearchExhausted {
                    attempts: b,
                    next_index: bi,
                }),
            ) => {
                assert_eq!(a, b, "exhaust attempts differ at budget {attempts}");
                assert_eq!(ai, bi, "exhaust resume index differs at budget {attempts}");
            }
            // A budget that happens to contain a match must match on both.
            (Ok(a), Ok(b)) => {
                assert_eq!(a.address, b.address);
                assert_eq!(a.attempts, b.attempts);
                assert_eq!(a.next_index, b.next_index);
            }
            _ => panic!("outcomes differ at budget {attempts}: {sequential:?} vs {parallel:?}"),
        }
    }
}

#[test]
fn cancellation_reports_a_resume_point_that_skips_nothing() {
    // Cancellation is coarser in the parallel path, checked once per chunk. The
    // property that must hold regardless is that the reported resume index
    // never points past a candidate that was not examined.
    let account = account(CoinType::Qi);
    let mut calls = 0u32;
    let result = account.search_parallel(
        false,
        Search {
            zone: Zone::Hydra3,
            start_index: 100,
            max_attempts: 1_000_000,
        },
        || {
            calls += 1;
            calls > 1
        },
    );
    match result {
        Err(WalletError::Cancelled {
            attempts,
            next_index,
        }) => {
            assert_eq!(
                next_index,
                100 + attempts,
                "resume index must be the start of the unexamined remainder"
            );
            // Resuming from there must still find the same address the
            // uncancelled search would.
            let full = account
                .search(
                    false,
                    Search {
                        zone: Zone::Hydra3,
                        start_index: 100,
                        max_attempts: 1_000_000,
                    },
                    || false,
                )
                .unwrap();
            let resumed = account
                .search_parallel(
                    false,
                    Search {
                        zone: Zone::Hydra3,
                        start_index: next_index,
                        max_attempts: 1_000_000,
                    },
                    || false,
                )
                .unwrap();
            assert_eq!(
                full.address, resumed.address,
                "resuming after cancellation skipped the match"
            );
        }
        // Cancelling before the first chunk completes is also valid.
        Ok(found) => assert_eq!(found.address.zone, Zone::Hydra3),
        other => panic!("unexpected outcome {other:?}"),
    }
}

#[test]
fn the_same_search_is_reproducible_across_repeated_parallel_runs() {
    // Determinism under a real thread pool: the reduction takes the lowest
    // index, so scheduling order must not change the answer.
    let account = account(CoinType::Qi);
    let request = Search {
        zone: Zone::Cyprus1,
        start_index: 0,
        max_attempts: 60_000,
    };
    let first = account.search_parallel(false, request, || false).unwrap();
    for _ in 0..16 {
        let again = account.search_parallel(false, request, || false).unwrap();
        assert_eq!(first.address, again.address);
        assert_eq!(first.attempts, again.attempts);
        assert_eq!(first.next_index, again.next_index);
    }
}
