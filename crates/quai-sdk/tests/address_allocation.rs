//! Bounded allocation journals, restart-safe ranges and actual browser CAS conflicts.
#![cfg(feature = "wallet")]
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
use quai_sdk::wallet::allocation::{
    AddressAllocationBook, AddressAllocationId, AddressAllocationStatus,
    MAX_ADDRESS_ALLOCATION_BYTES, MAX_ADDRESS_ALLOCATIONS,
};
use quai_sdk::wallet::discovery::NetworkScope;
use quai_sdk::wallet::metadata::StorageError;
use quai_sdk::wallet::{AccountPublic, CoinType, HdWallet, Search};
use quai_sdk::{U256, Zone};
fn scope() -> NetworkScope {
    NetworkScope {
        chain_id: U256::from(15000),
        genesis: quai_sdk::primitives::Hash32::from_bytes([1; 32]),
        zone: Zone::Cyprus1,
    }
}
fn account(coin: CoinType) -> AccountPublic {
    HdWallet::from_seed(&[7; 32], coin)
        .unwrap()
        .account_public(0)
        .unwrap()
}
fn id(n: u128) -> AddressAllocationId {
    AddressAllocationId(n.to_be_bytes())
}
fn found(book: &AddressAllocationBook, id: AddressAllocationId) -> u32 {
    let a = book.allocation(id).unwrap();
    book.account()
        .search(
            a.change,
            Search {
                zone: book.scope().zone,
                start_index: a.range.start,
                max_attempts: a.range.end - a.range.start,
            },
            || false,
        )
        .unwrap()
        .address
        .index
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn journal_retains_receive_change_ranges_completion_abandonment_and_ids() {
    for coin in [CoinType::Qi, CoinType::Quai] {
        let mut book = AddressAllocationBook::new(scope(), account(coin), 10, 20).unwrap();
        book.reserve(id(2), false, 10_000).unwrap();
        book.reserve(id(1), true, 10_000).unwrap();
        assert_eq!(
            (book.next_index(false), book.next_index(true)),
            (10_010, 10_020)
        );
        let index = found(&book, id(2));
        let address = book.complete(id(2), index).unwrap();
        assert_eq!(book.complete(id(2), index).unwrap(), address);
        assert_eq!(
            book.complete(id(2), index + 1),
            Err(StorageError::Transition)
        );
        assert_eq!(book.abandon(id(2)), Err(StorageError::Transition));
        book.abandon(id(1)).unwrap();
        assert_eq!(book.complete(id(1), 20), Err(StorageError::Transition));
        let bytes = book.export_state();
        let mut restored =
            AddressAllocationBook::from_state(&bytes, scope(), account(coin)).unwrap();
        assert_eq!(restored.export_state(), bytes);
        assert!(
            matches!(&restored.allocation(id(2)).unwrap().status,AddressAllocationStatus::Completed(a) if *a==address)
        );
        assert!(matches!(
            restored.allocation(id(1)).unwrap().status,
            AddressAllocationStatus::Abandoned
        ));
        assert_eq!(
            restored.reserve(id(1), true, 1),
            Err(StorageError::Conflict)
        );
        assert_eq!(
            restored.reserve(id(2), false, 1),
            Err(StorageError::Conflict)
        );
        restored.reserve(id(3), false, 100).unwrap();
        assert_eq!(restored.allocation(id(3)).unwrap().range.start, 10_010);
    }
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn allocation_codec_rejects_wrong_scope_account_order_overlap_gaps_and_bounds() {
    let public = account(CoinType::Qi);
    let mut book = AddressAllocationBook::new(scope(), public.clone(), 0, 0).unwrap();
    book.reserve(id(1), false, 100).unwrap();
    book.reserve(id(2), false, 100).unwrap();
    let bytes = book.export_state();
    for len in 0..bytes.len() {
        assert!(AddressAllocationBook::from_state(&bytes[..len], scope(), public.clone()).is_err());
    }
    let mut wrong_scope = scope();
    wrong_scope.chain_id += U256::from(1);
    assert!(AddressAllocationBook::from_state(&bytes, wrong_scope, public.clone()).is_err());
    assert!(AddressAllocationBook::from_state(&bytes, scope(), account(CoinType::Quai)).is_err());
    let mut cases = vec![];
    let mut b = bytes.clone();
    b.push(0);
    cases.push(b);
    for (offset, value) in [(0, b'X'), (139, 2), (148, 3)] {
        let mut b = bytes.clone();
        b[offset] = value;
        cases.push(b);
    }
    let mut b = bytes.clone();
    b[149..165].copy_from_slice(&bytes[123..139]);
    cases.push(b);
    let mut b = bytes.clone();
    b[166..170].copy_from_slice(&0u32.to_be_bytes());
    cases.push(b);
    let mut b = bytes.clone();
    b[166..170].copy_from_slice(&101u32.to_be_bytes());
    cases.push(b);
    let mut b = bytes.clone();
    b[113..117].copy_from_slice(&199u32.to_be_bytes());
    cases.push(b);
    let mut b = bytes.clone();
    b[105..109].copy_from_slice(&1u32.to_be_bytes());
    cases.push(b);
    let mut b = bytes.clone();
    b[121..123].copy_from_slice(&4097u16.to_be_bytes());
    cases.push(b);
    for b in cases {
        assert!(AddressAllocationBook::from_state(&b, scope(), public.clone()).is_err());
    }
    assert!(
        AddressAllocationBook::from_state(
            &vec![0; MAX_ADDRESS_ALLOCATION_BYTES + 1],
            scope(),
            public
        )
        .is_err()
    );
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn exhausted_ranges_and_journal_capacity_never_rewind_or_reuse_ids() {
    let public = account(CoinType::Qi);
    let mut book =
        AddressAllocationBook::new(scope(), public.clone(), (1 << 31) - 3, 1 << 31).unwrap();
    book.reserve(id(1), false, 100).unwrap();
    assert_eq!(book.next_index(false), 1 << 31);
    assert_eq!(book.allocation(id(1)).unwrap().range.end, 1 << 31);
    assert_eq!(
        book.reserve(id(2), false, 1),
        Err(StorageError::DerivationExhausted)
    );
    assert_eq!(
        book.reserve(id(2), true, 1),
        Err(StorageError::DerivationExhausted)
    );
    let mut book = AddressAllocationBook::new(scope(), public.clone(), 0, 0).unwrap();
    for n in 0..MAX_ADDRESS_ALLOCATIONS {
        book.reserve(id(n as u128), false, 1).unwrap();
    }
    let bytes = book.export_state();
    assert!(bytes.len() <= MAX_ADDRESS_ALLOCATION_BYTES);
    assert_eq!(
        AddressAllocationBook::from_state(&bytes, scope(), public.clone())
            .unwrap()
            .export_state(),
        bytes
    );
    assert_eq!(book.reserve(id(5000), false, 1), Err(StorageError::Invalid));
    book.abandon(id(0)).unwrap();
    assert_eq!(book.reserve(id(0), false, 1), Err(StorageError::Conflict));
    assert_eq!(book.reserve(id(5000), false, 1), Err(StorageError::Invalid));
    assert_eq!(book.next_index(false), MAX_ADDRESS_ALLOCATIONS as u32);
    assert!(AddressAllocationBook::new(scope(), public, (1 << 31) + 1, 0).is_err());
}
#[cfg(feature = "backup")]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn authenticated_backup_floors_include_skipped_change_ranges_and_prior_addresses() {
    let row: serde_json::Value = serde_json::from_slice(include_bytes!(
        "fixtures/shared/crates/quai-wallet/tests/full-backup-v5-portable.json"
    ))
    .unwrap();
    let bytes =
        quai_sdk::primitives::get_bytes(&format!("0x{}", row["envelope"].as_str().unwrap()))
            .unwrap();
    let backup = quai_sdk::wallet::full_backup::EncryptedWalletBackup::from_bytes(&bytes)
        .unwrap()
        .decrypt(row["password"].as_str().unwrap().as_bytes())
        .unwrap();
    let public = backup.origins()[0].account_public(CoinType::Qi, 0).unwrap();
    let mut book = AddressAllocationBook::from_backup(&backup, scope(), public).unwrap();
    assert_eq!(book.next_index(true), 10_000);
    assert!(book.next_index(false) > 0);
    book.reserve(id(10), true, 100).unwrap();
    assert_eq!(book.allocation(id(10)).unwrap().range.start, 10_000);
    assert!(AddressAllocationBook::from_backup(&backup, scope(), account(CoinType::Qi)).is_err());
    let new_account = backup.origins()[0].account_public(CoinType::Qi, 1).unwrap();
    let book = AddressAllocationBook::from_backup(&backup, scope(), new_account).unwrap();
    assert_eq!(book.next_index(false), 0);
    assert_eq!(book.next_index(true), 0);
    #[cfg(all(target_arch = "wasm32", feature = "browser"))]
    {
        let public = backup.origins()[0].account_public(CoinType::Qi, 0).unwrap();
        let store = quai_sdk::browser_addresses::BrowserAddressBook::open(
            &browser::name(),
            scope(),
            public,
        )
        .await
        .unwrap();
        store.initialize_from_backup(&backup).await.unwrap();
        assert_eq!(
            store.snapshot().await.unwrap().book.next_index(true),
            10_000
        );
        assert!(store.snapshot().await.unwrap().book.next_index(false) > 0);
        assert!(store.initialize_from_backup(&backup).await.is_err());
    }
}
#[cfg(all(target_arch = "wasm32", feature = "browser"))]
mod browser {
    use super::*;
    use quai_sdk::browser::{BrowserError, BrowserSnapshotStore, BrowserStorageScope};
    use quai_sdk::browser_addresses::{BrowserAddressBook, BrowserAddressError};
    use quai_sdk::wallet::metadata::KeyOrigin;
    pub(super) fn name() -> String {
        let mut bytes = [0; 8];
        quai_sdk::crypto::fill_random(&mut bytes).unwrap();
        format!(
            "quai-sdk-allocation-{}",
            bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
        )
    }
    async fn raw(name: &str, public: &AccountPublic) -> BrowserSnapshotStore {
        BrowserSnapshotStore::open(
            name,
            BrowserStorageScope {
                chain_id: scope().chain_id,
                genesis: scope().genesis,
                zone: scope().zone,
                wallet: AddressAllocationBook::account_identity(public),
            },
            MAX_ADDRESS_ALLOCATION_BYTES,
        )
        .await
        .unwrap()
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn cancelled_search_restarts_from_committed_range_and_returns_only_committed_addresses() {
        let name = name();
        let public = account(CoinType::Qi);
        let book = BrowserAddressBook::open(&name, scope(), public.clone())
            .await
            .unwrap();
        assert!(matches!(
            book.snapshot().await,
            Err(BrowserAddressError::Uninitialized)
        ));
        book.initialize(100, 200).await.unwrap();
        assert!(matches!(
            book.allocate(id(1), false, 10_000, || true).await,
            Err(BrowserAddressError::Search(_))
        ));
        assert_eq!(
            book.snapshot().await.unwrap().book.next_index(false),
            10_100
        );
        drop(book);
        let book = BrowserAddressBook::open(&name, scope(), public)
            .await
            .unwrap();
        let address = book.resume(id(1), || false).await.unwrap();
        let snapshot = book.snapshot().await.unwrap();
        assert!(
            matches!(&snapshot.book.allocation(id(1)).unwrap().status,AddressAllocationStatus::Completed(a) if *a==address)
        );
        assert_eq!(book.resume(id(1), || true).await.unwrap(), address);
        assert!(matches!(
            book.allocate(id(1), false, 100, || false).await,
            Err(BrowserAddressError::State(StorageError::Conflict))
        ));
        book.reserve(id(2), true, 100).await.unwrap();
        book.abandon(id(2)).await.unwrap();
        assert_eq!(book.snapshot().await.unwrap().book.next_index(true), 300);
        assert!(book.resume(id(2), || false).await.is_err());
        assert!(book.abandon(id(1)).await.is_err());
        if let KeyOrigin::Bip44 { index, .. } = address.origin() {
            assert!(index >= 100);
            assert!(book.complete(id(1), index + 1).await.is_err());
        } else {
            panic!("expected HD origin");
        }
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn competing_connections_cannot_reserve_the_same_range_or_reinitialize() {
        use std::{future::Future, task::Poll};
        let name = name();
        let public = account(CoinType::Quai);
        let a = BrowserAddressBook::open(&name, scope(), public.clone())
            .await
            .unwrap();
        a.initialize(0, 0).await.unwrap();
        let b = BrowserAddressBook::open(&name, scope(), public)
            .await
            .unwrap();
        let mut first = Box::pin(a.reserve(id(1), false, 10_000));
        let mut second = Box::pin(b.reserve(id(2), false, 10_000));
        let mut one = None;
        let mut two = None;
        let (one, two) = std::future::poll_fn(|cx| {
            if one.is_none()
                && let Poll::Ready(value) = first.as_mut().poll(cx)
            {
                one = Some(value);
            }
            if two.is_none()
                && let Poll::Ready(value) = second.as_mut().poll(cx)
            {
                two = Some(value);
            }
            if one.is_some() && two.is_some() {
                Poll::Ready((one.take().unwrap(), two.take().unwrap()))
            } else {
                Poll::Pending
            }
        })
        .await;
        assert_eq!(usize::from(one.is_ok()) + usize::from(two.is_ok()), 1);
        let loser = if one.is_ok() {
            assert!(matches!(
                two,
                Err(BrowserAddressError::Browser(BrowserError::StorageConflict))
            ));
            id(2)
        } else {
            assert!(matches!(
                one,
                Err(BrowserAddressError::Browser(BrowserError::StorageConflict))
            ));
            id(1)
        };
        a.reserve(loser, false, 10_000).await.unwrap();
        let first = a.resume(id(1), || false).await.unwrap();
        let second = b.resume(id(2), || false).await.unwrap();
        assert_ne!(first.address(), second.address());
        let snapshot = a.snapshot().await.unwrap();
        assert_eq!(snapshot.book.next_index(false), 20_000);
        assert_eq!(snapshot.book.allocations().count(), 2);
        assert!(matches!(
            b.initialize(0, 0).await,
            Err(BrowserAddressError::Browser(BrowserError::StorageConflict))
        ));
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn tombstones_and_corrupt_journals_never_reset_cursors() {
        let name = name();
        let public = account(CoinType::Qi);
        let book = BrowserAddressBook::open(&name, scope(), public.clone())
            .await
            .unwrap();
        book.initialize(500, 600).await.unwrap();
        let raw = raw(&name, &public).await;
        let snapshot = raw.read().await.unwrap().unwrap();
        let mut bytes = snapshot.bytes.unwrap();
        bytes[73] ^= 1;
        let revision = raw
            .compare_exchange(Some(snapshot.revision), Some(&bytes))
            .await
            .unwrap();
        assert!(matches!(
            book.snapshot().await,
            Err(BrowserAddressError::State(StorageError::Invalid))
        ));
        assert!(book.initialize(0, 0).await.is_err());
        raw.compare_exchange(Some(revision), None).await.unwrap();
        assert!(matches!(
            book.snapshot().await,
            Err(BrowserAddressError::Uninitialized)
        ));
        assert!(matches!(
            book.initialize(0, 0).await,
            Err(BrowserAddressError::Browser(BrowserError::StorageConflict))
        ));
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn cancellation_after_write_dispatch_keeps_range_and_completed_address_recoverable() {
        use std::{future::Future, pin::Pin, task::Poll};
        async fn poll_once<F: Future>(mut future: Pin<&mut F>) -> Poll<F::Output> {
            std::future::poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await
        }
        let name = name();
        let public = account(CoinType::Qi);
        let book = BrowserAddressBook::open(&name, scope(), public.clone())
            .await
            .unwrap();
        book.initialize(0, 0).await.unwrap();
        let raw = raw(&name, &public).await;
        let mut reserve = Box::pin(book.reserve(id(1), false, 10_000));
        assert!(poll_once(reserve.as_mut()).await.is_pending());
        raw.read().await.unwrap();
        assert!(poll_once(reserve.as_mut()).await.is_pending());
        raw.read().await.unwrap();
        drop(reserve);
        let snapshot = book.snapshot().await.unwrap();
        assert_eq!(snapshot.book.next_index(false), 10_000);
        let index = found(&snapshot.book, id(1));
        let mut complete = Box::pin(book.complete(id(1), index));
        assert!(poll_once(complete.as_mut()).await.is_pending());
        raw.read().await.unwrap();
        assert!(poll_once(complete.as_mut()).await.is_pending());
        raw.read().await.unwrap();
        drop(complete);
        let snapshot = book.snapshot().await.unwrap();
        assert!(matches!(
            snapshot.book.allocation(id(1)).unwrap().status,
            AddressAllocationStatus::Completed(_)
        ));
        let restored = book.resume(id(1), || true).await.unwrap();
        assert!(matches!(restored.origin(),KeyOrigin::Bip44{index:i,..} if i==index));
    }
}

#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn independent_node_journal_encoding_matches_pinned_hd_account_and_address_pairs() {
    let fixture: serde_json::Value = serde_json::from_slice(include_bytes!(
        "fixtures/shared/test-infra/fixtures/address-allocation.json"
    ))
    .unwrap();
    let mut states = 0;
    for row in fixture["vectors"].as_array().unwrap() {
        let coin = if row["coin"] == 969 {
            CoinType::Qi
        } else {
            CoinType::Quai
        };
        let public = AccountPublic::import(
            row["accountXpub"].as_str().unwrap(),
            coin,
            row["account"].as_u64().unwrap() as u32,
        )
        .unwrap();
        let mut scope = scope();
        scope.zone = Zone::from_byte(row["zone"].as_u64().unwrap() as u8).unwrap();
        let index = row["index"].as_u64().unwrap() as u32;
        for (name, expected) in row["states"].as_object().unwrap() {
            let mut book = AddressAllocationBook::new(scope, public.clone(), 0, 0).unwrap();
            if name != "empty" {
                book.reserve(id(1), row["change"].as_bool().unwrap(), index + 1)
                    .unwrap();
            }
            if name == "completed" {
                assert_eq!(
                    book.complete(id(1), index).unwrap().address().to_string(),
                    row["address"].as_str().unwrap()
                );
            }
            if name == "abandoned" {
                book.abandon(id(1)).unwrap();
            }
            let expected =
                quai_sdk::primitives::get_bytes(&format!("0x{}", expected.as_str().unwrap()))
                    .unwrap();
            assert_eq!(book.export_state(), expected);
            assert_eq!(
                AddressAllocationBook::from_state(&expected, scope, public.clone())
                    .unwrap()
                    .export_state(),
                expected
            );
            states += 1;
        }
    }
    assert_eq!(states, 24);
}
