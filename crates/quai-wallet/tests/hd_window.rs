//! A search window must return exactly what an index-by-index oracle finds.
//!
//! The oracle derives every child through `bip32` with `derive_address`, which
//! shares nothing with the scan path, so agreement checks both the window's
//! bookkeeping and the fast derivation. Scanners persist `next_index`
//! monotonically, so an off-by-one here would skip an address permanently.
use quai_primitives::Zone;
use quai_wallet::{AccountPublic, CoinType, HdWallet, Search, WalletError, WindowStop};

fn account() -> AccountPublic {
    HdWallet::from_seed(&[0x2b; 64], CoinType::Qi)
        .unwrap()
        .account_public(0)
        .unwrap()
}

/// Matching indexes in `start..end`, by exhaustive derivation.
fn oracle(account: &AccountPublic, change: bool, start: u32, end: u32) -> Vec<u32> {
    (start..end)
        .filter(|&index| {
            account
                .derive_address(change, index)
                .is_ok_and(|a| a.zone == Zone::Cyprus1)
        })
        .collect()
}

fn search(start: u32, max_attempts: u32) -> Search {
    Search {
        zone: Zone::Cyprus1,
        start_index: start,
        max_attempts,
    }
}

#[test]
fn a_filled_window_matches_the_oracle_and_resumes_after_its_last_address() {
    let account = account();
    for change in [false, true] {
        for start in [0u32, 1, 777] {
            let window = account
                .search_window(change, search(start, 20_000), 4, || false)
                .unwrap();
            assert_eq!(window.stop, WindowStop::Filled);
            let found: Vec<u32> = window.addresses.iter().map(|a| a.index).collect();
            let last = *found.last().unwrap();
            assert_eq!(found, oracle(&account, change, start, last + 1));
            assert_eq!(found.len(), 4);
            assert_eq!(window.attempts, last + 1 - start);
            assert_eq!(window.next_index, Some(last + 1));
            assert!(window.addresses.iter().all(|a| a.change == change));
        }
    }
}

#[test]
fn exhaustion_and_cancellation_report_the_first_unexamined_index() {
    let account = account();
    // Exhausted by max_attempts: every match below the bound, none beyond.
    let window = account
        .search_window(false, search(10, 3_000), 64, || false)
        .unwrap();
    assert_eq!(window.stop, WindowStop::Exhausted);
    assert_eq!(window.attempts, 3_000);
    assert_eq!(window.next_index, Some(3_010));
    let found: Vec<u32> = window.addresses.iter().map(|a| a.index).collect();
    assert_eq!(found, oracle(&account, false, 10, 3_010));

    // Exhausted by the nonhardened index space, not wrapped past it.
    let top = (1u32 << 31) - 5;
    let window = account
        .search_window(false, search(top, 1_000), 64, || false)
        .unwrap();
    assert_eq!(window.stop, WindowStop::Exhausted);
    assert_eq!(window.attempts, 5);
    assert_eq!(window.next_index, None);

    // Cancelled before the 1,501st candidate: exactly 1,500 examined.
    let mut calls = 0;
    let window = account
        .search_window(false, search(0, 20_000), 64, || {
            calls += 1;
            calls > 1_500
        })
        .unwrap();
    assert_eq!(window.stop, WindowStop::Cancelled);
    assert_eq!(window.attempts, 1_500);
    assert_eq!(window.next_index, Some(1_500));
    let found: Vec<u32> = window.addresses.iter().map(|a| a.index).collect();
    assert_eq!(found, oracle(&account, false, 0, 1_500));
}

#[test]
fn single_search_keeps_its_error_contract() {
    let account = account();
    assert!(matches!(
        account.search(false, search(0, 1), || true),
        Err(WalletError::Cancelled {
            attempts: 0,
            next_index: 0
        })
    ));
    assert!(matches!(
        account.search_window(false, search(0, 1), 0, || false),
        Err(WalletError::InvalidSearchLimit)
    ));
    let first = oracle(&account, false, 0, 5_000)[0];
    assert!(matches!(
        account.search(false, search(0, first), || false),
        Err(WalletError::SearchExhausted { attempts, next_index: Some(next) })
            if attempts == first && next == first
    ));
}

/// Minimal executor: the yielding search wakes itself, so polling in a loop
/// completes it, and counting polls shows that it did yield.
fn block_on<F: std::future::Future>(future: F) -> (F::Output, usize) {
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    let mut future = std::pin::pin!(future);
    let mut polls = 0;
    loop {
        polls += 1;
        if let std::task::Poll::Ready(value) = future.as_mut().poll(&mut context) {
            return (value, polls);
        }
    }
}

#[test]
fn the_yielding_search_equals_one_call_and_yields_between_slices() {
    use quai_wallet::Grinding;
    let account = account();
    for (start, max_attempts, count) in [(0u32, 20_000u32, 4usize), (10, 3_000, 64), (777, 1, 1)] {
        let once = account
            .search_window(false, search(start, max_attempts), count, || false)
            .unwrap();
        let (sliced, polls) = block_on(account.search_window_async(
            false,
            search(start, max_attempts),
            count,
            Grinding::Sequential,
            || false,
        ));
        assert_eq!(sliced.unwrap(), once, "start {start}");
        if once.attempts > 512 {
            assert!(polls > 1, "a long window must yield");
        }
    }
    // Cancellation lands on the same candidate, across a slice boundary.
    let cancel_after = |limit: u32| {
        let mut calls = 0;
        move || {
            calls += 1;
            calls > limit
        }
    };
    let once = account
        .search_window(false, search(0, 20_000), 64, cancel_after(1_500))
        .unwrap();
    let (sliced, _) = block_on(account.search_window_async(
        false,
        search(0, 20_000),
        64,
        Grinding::Sequential,
        cancel_after(1_500),
    ));
    assert_eq!(sliced.unwrap(), once);
    assert_eq!(once.stop, WindowStop::Cancelled);
}

#[cfg(all(feature = "rayon", not(target_arch = "wasm32")))]
#[test]
fn the_parallel_window_equals_the_sequential_window() {
    use quai_wallet::Grinding;
    let account = account();
    for change in [false, true] {
        for (start, max_attempts, count) in [
            (0u32, 60_000u32, 50usize),
            (5, 3_000, 64),
            (2_047, 60_000, 7),
        ] {
            let sequential = account
                .search_window(change, search(start, max_attempts), count, || false)
                .unwrap();
            let parallel = account
                .search_window_parallel(change, search(start, max_attempts), count, || false)
                .unwrap();
            assert_eq!(parallel, sequential, "start {start}");
            let (sliced, _) = block_on(account.search_window_async(
                change,
                search(start, max_attempts),
                count,
                Grinding::Parallel,
                || false,
            ));
            assert_eq!(sliced.unwrap(), sequential);
        }
    }
    let top = (1u32 << 31) - 5;
    assert_eq!(
        account
            .search_window_parallel(false, search(top, 1_000), 64, || false)
            .unwrap(),
        account
            .search_window(false, search(top, 1_000), 64, || false)
            .unwrap()
    );
}
