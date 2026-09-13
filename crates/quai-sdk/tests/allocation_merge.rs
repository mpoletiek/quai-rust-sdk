//! Live allocation recovery preserves IDs while sealing earlier burned history.
#![cfg(feature = "backup")]
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
use quai_sdk::payments::{PaymentDirection, PrivatePaymentCode};
use quai_sdk::primitives::{Hash32, get_bytes};
use quai_sdk::wallet::allocation::{
    AddressAllocationBook, AddressAllocationId, AddressAllocationStatus,
};
use quai_sdk::wallet::discovery::NetworkScope;
use quai_sdk::wallet::full_backup::{BackupOrigin, PortableWalletCapture, WalletBackup};
use quai_sdk::wallet::payment_allocation::{
    PaymentAllocationBook, PaymentAllocationId, PaymentAllocationStatus,
};
use quai_sdk::wallet::{AccountPublic, CoinType};
use quai_sdk::{U256, Zone};
fn fixture() -> serde_json::Value {
    serde_json::from_slice(include_bytes!(
        "fixtures/shared/test-infra/fixtures/allocation-merge.json"
    ))
    .unwrap()
}
fn bytes(value: &serde_json::Value) -> Vec<u8> {
    get_bytes(&format!("0x{}", value.as_str().unwrap())).unwrap()
}
fn scope(row: &serde_json::Value) -> NetworkScope {
    NetworkScope {
        chain_id: U256::from(15000),
        genesis: Hash32::from_bytes([1; 32]),
        zone: Zone::from_byte(row["zone"].as_u64().unwrap() as u8).unwrap(),
    }
}
fn id(n: u128) -> [u8; 16] {
    n.to_be_bytes()
}
fn account(row: &serde_json::Value) -> AccountPublic {
    AccountPublic::import(
        row["accountXpub"].as_str().unwrap(),
        if row["coin"] == 969 {
            CoinType::Qi
        } else {
            CoinType::Quai
        },
        row["account"].as_u64().unwrap() as u32,
    )
    .unwrap()
}
fn hd_origin() -> BackupOrigin {
    BackupOrigin::from_seed(&get_bytes(fixture()["hdSeed"].as_str().unwrap()).unwrap()).unwrap()
}
fn owner() -> PrivatePaymentCode {
    PrivatePaymentCode::from_seed(
        &get_bytes(fixture()["ownerSeed"].as_str().unwrap()).unwrap(),
        0,
    )
    .unwrap()
}
fn peer() -> quai_sdk::payments::PaymentCode {
    PrivatePaymentCode::from_seed(
        &get_bytes(fixture()["peerSeed"].as_str().unwrap()).unwrap(),
        0,
    )
    .unwrap()
    .public_code()
    .clone()
}
fn payment_origin() -> BackupOrigin {
    BackupOrigin::from_seed(&get_bytes(fixture()["ownerSeed"].as_str().unwrap()).unwrap()).unwrap()
}
fn direction(row: &serde_json::Value) -> PaymentDirection {
    if row["direction"] == "send" {
        PaymentDirection::Send
    } else {
        PaymentDirection::Receive
    }
}
fn hd_backup(row: &serde_json::Value) -> WalletBackup {
    let source = AddressAllocationBook::new(
        scope(row),
        account(row),
        row["nextReceive"].as_u64().unwrap() as u32,
        row["nextChange"].as_u64().unwrap() as u32,
    )
    .unwrap();
    WalletBackup::capture_portable(
        PortableWalletCapture {
            allocations: &[&source],
            ..Default::default()
        },
        vec![hd_origin()],
    )
    .unwrap()
}
fn payment_backup(row: &serde_json::Value) -> WalletBackup {
    let source = PaymentAllocationBook::new(
        scope(row),
        &owner(),
        peer(),
        direction(row),
        row["nextIndex"].as_u64().unwrap() as u32,
    )
    .unwrap();
    WalletBackup::capture_portable(
        PortableWalletCapture {
            payments: &[&source],
            ..Default::default()
        },
        vec![payment_origin()],
    )
    .unwrap()
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn independent_hd_merge_frames_preserve_ids_and_strict_legacy_rules() {
    for row in fixture()["addresses"].as_array().unwrap() {
        let mut b =
            AddressAllocationBook::from_state(&bytes(&row["old"]), scope(row), account(row))
                .unwrap();
        let old_count = b.allocations().count();
        assert_eq!(
            b.merge_backup(&hd_backup(row)).unwrap() as u64,
            row["abandoned"].as_u64().unwrap()
        );
        let encoded = b.export_state();
        assert_eq!(encoded, bytes(&row["merged"]));
        let mut restored =
            AddressAllocationBook::from_state(&encoded, scope(row), account(row)).unwrap();
        assert_eq!(restored.allocations().count(), old_count);
        if row["state"] != "empty" {
            assert!(
                restored
                    .reserve(AddressAllocationId(id(1)), false, 1)
                    .is_err()
            );
            let mut legacy = encoded.clone();
            legacy[7] = b'1';
            assert!(AddressAllocationBook::from_state(&legacy, scope(row), account(row)).is_err());
        }
        if row["state"] == "pending" {
            assert!(matches!(
                restored
                    .allocation(AddressAllocationId(id(1)))
                    .unwrap()
                    .status,
                AddressAllocationStatus::Abandoned
            ));
            let mut pending = encoded.clone();
            pending[148] = 0;
            assert!(AddressAllocationBook::from_state(&pending, scope(row), account(row)).is_err());
        }
        let next = restored.next_index(false);
        restored
            .reserve(AddressAllocationId(id(2)), false, 10)
            .unwrap();
        assert_eq!(
            restored
                .allocation(AddressAllocationId(id(2)))
                .unwrap()
                .range
                .start,
            next
        );
        let encoded = restored.export_state();
        assert_eq!(
            AddressAllocationBook::from_state(&encoded, scope(row), account(row))
                .unwrap()
                .export_state(),
            encoded
        );
        assert_eq!(restored.merge_backup(&hd_backup(row)).unwrap(), 1);
        assert_eq!(restored.next_index(false), next + 10);
        assert_eq!(restored.allocations().count(), old_count + 1);
    }
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn independent_payment_merge_frames_preserve_exposures_and_strict_legacy_rules() {
    let owner = owner();
    let peer = peer();
    for row in fixture()["payments"].as_array().unwrap() {
        let mut b = PaymentAllocationBook::from_state(
            &bytes(&row["old"]),
            scope(row),
            &owner,
            peer.clone(),
            direction(row),
        )
        .unwrap();
        let old_count = b.allocations().count();
        assert_eq!(
            b.merge_backup(&owner, &payment_backup(row)).unwrap() as u64,
            row["abandoned"].as_u64().unwrap()
        );
        let encoded = b.export_state();
        assert_eq!(encoded, bytes(&row["merged"]));
        let mut restored = PaymentAllocationBook::from_state(
            &encoded,
            scope(row),
            &owner,
            peer.clone(),
            direction(row),
        )
        .unwrap();
        assert_eq!(restored.allocations().count(), old_count);
        if row["state"] != "empty" {
            assert!(restored.reserve(PaymentAllocationId(id(1)), 1).is_err());
            let mut legacy = encoded.clone();
            legacy[7] = b'1';
            assert!(
                PaymentAllocationBook::from_state(
                    &legacy,
                    scope(row),
                    &owner,
                    peer.clone(),
                    direction(row)
                )
                .is_err()
            );
        }
        if row["state"] == "pending" {
            assert!(matches!(
                restored
                    .allocation(PaymentAllocationId(id(1)))
                    .unwrap()
                    .status,
                PaymentAllocationStatus::Abandoned
            ));
            let mut pending = encoded.clone();
            pending[139] = 0;
            assert!(
                PaymentAllocationBook::from_state(
                    &pending,
                    scope(row),
                    &owner,
                    peer.clone(),
                    direction(row)
                )
                .is_err()
            );
        }
        let next = restored.next_index();
        restored.reserve(PaymentAllocationId(id(2)), 10).unwrap();
        assert_eq!(
            restored
                .allocation(PaymentAllocationId(id(2)))
                .unwrap()
                .range
                .start,
            next
        );
        let encoded = restored.export_state();
        assert_eq!(
            PaymentAllocationBook::from_state(
                &encoded,
                scope(row),
                &owner,
                peer.clone(),
                direction(row)
            )
            .unwrap()
            .export_state(),
            encoded
        );
        assert_eq!(
            restored.merge_backup(&owner, &payment_backup(row)).unwrap(),
            1
        );
        assert_eq!(restored.next_index(), next + 10);
        assert_eq!(restored.allocations().count(), old_count + 1);
    }
}

#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn sealed_history_rejects_overlap_straddling_and_exhaustion_never_rewinds() {
    let f = fixture();
    let row = f["addresses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["state"] == "pending")
        .unwrap();
    let mut b =
        AddressAllocationBook::from_state(&bytes(&row["old"]), scope(row), account(row)).unwrap();
    b.merge_backup(&hd_backup(row)).unwrap();
    let mut straddling = b.export_state();
    straddling[144..148].copy_from_slice(&(b.next_index(false) + 1).to_be_bytes());
    assert!(AddressAllocationBook::from_state(&straddling, scope(row), account(row)).is_err());
    b.reserve(AddressAllocationId(id(2)), false, 10).unwrap();
    b.merge_backup(&hd_backup(row)).unwrap();
    let mut overlapping = b.export_state();
    overlapping[166..170].copy_from_slice(&0u32.to_be_bytes());
    assert!(AddressAllocationBook::from_state(&overlapping, scope(row), account(row)).is_err());
    let mut exhausted = row.clone();
    exhausted["nextReceive"] = (1u64 << 31).into();
    exhausted["nextChange"] = (1u64 << 31).into();
    b.merge_backup(&hd_backup(&exhausted)).unwrap();
    b.merge_backup(&hd_backup(row)).unwrap();
    assert_eq!(b.next_index(false), 1 << 31);
    assert_eq!(b.next_index(true), 1 << 31);
    assert!(b.reserve(AddressAllocationId(id(3)), false, 1).is_err());
    assert_eq!(
        AddressAllocationBook::from_state(&b.export_state(), scope(row), account(row))
            .unwrap()
            .export_state(),
        b.export_state()
    );
    let row = f["payments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["state"] == "pending")
        .unwrap();
    let owner = owner();
    let peer = peer();
    let mut b = PaymentAllocationBook::from_state(
        &bytes(&row["old"]),
        scope(row),
        &owner,
        peer.clone(),
        direction(row),
    )
    .unwrap();
    b.merge_backup(&owner, &payment_backup(row)).unwrap();
    let mut straddling = b.export_state();
    straddling[135..139].copy_from_slice(&(b.next_index() + 1).to_be_bytes());
    assert!(
        PaymentAllocationBook::from_state(
            &straddling,
            scope(row),
            &owner,
            peer.clone(),
            direction(row)
        )
        .is_err()
    );
    b.reserve(PaymentAllocationId(id(2)), 10).unwrap();
    b.merge_backup(&owner, &payment_backup(row)).unwrap();
    let mut overlapping = b.export_state();
    overlapping[156..160].copy_from_slice(&0u32.to_be_bytes());
    assert!(
        PaymentAllocationBook::from_state(
            &overlapping,
            scope(row),
            &owner,
            peer.clone(),
            direction(row)
        )
        .is_err()
    );
    let mut exhausted = row.clone();
    exhausted["nextIndex"] = (1u64 << 31).into();
    b.merge_backup(&owner, &payment_backup(&exhausted)).unwrap();
    b.merge_backup(&owner, &payment_backup(row)).unwrap();
    assert_eq!(b.next_index(), 1 << 31);
    assert!(b.reserve(PaymentAllocationId(id(3)), 1).is_err());
    assert_eq!(
        PaymentAllocationBook::from_state(
            &b.export_state(),
            scope(row),
            &owner,
            peer,
            direction(row)
        )
        .unwrap()
        .export_state(),
        b.export_state()
    );
}

#[cfg(all(target_arch = "wasm32", feature = "browser"))]
mod browser {
    use super::*;
    use quai_sdk::browser::BrowserError;
    use quai_sdk::browser_addresses::{BrowserAddressBook, BrowserAddressError};
    use quai_sdk::browser_payments::{BrowserPaymentBook, BrowserPaymentError};
    fn name() -> String {
        let mut b = [0; 8];
        quai_sdk::crypto::fill_random(&mut b).unwrap();
        format!("quai-allocation-merge-{:x}", u64::from_be_bytes(b))
    }
    async fn race<A: core::future::Future, B: core::future::Future>(
        a: A,
        b: B,
    ) -> (A::Output, B::Output) {
        use std::task::Poll;
        let (mut a, mut b) = (Box::pin(a), Box::pin(b));
        let (mut x, mut y) = (None, None);
        std::future::poll_fn(|cx| {
            if x.is_none()
                && let Poll::Ready(v) = a.as_mut().poll(cx)
            {
                x = Some(v);
            }
            if y.is_none()
                && let Poll::Ready(v) = b.as_mut().poll(cx)
            {
                y = Some(v);
            }
            if x.is_some() && y.is_some() {
                Poll::Ready((x.take().unwrap(), y.take().unwrap()))
            } else {
                Poll::Pending
            }
        })
        .await
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn live_hd_and_payment_merges_commit_independent_frames_and_reopen() {
        let f = fixture();
        let row = f["addresses"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["state"] == "pending")
            .unwrap();
        let database = name();
        let store = BrowserAddressBook::open(&database, scope(row), account(row))
            .await
            .unwrap();
        store.initialize(0, 0).await.unwrap();
        store
            .reserve(
                AddressAllocationId(id(1)),
                row["change"].as_bool().unwrap(),
                row["index"].as_u64().unwrap() as u32 + 1,
            )
            .await
            .unwrap();
        assert_eq!(store.merge_backup(&hd_backup(row)).await.unwrap(), 1);
        assert_eq!(
            store.snapshot().await.unwrap().book.export_state(),
            bytes(&row["merged"])
        );
        let reopened = BrowserAddressBook::open(&database, scope(row), account(row))
            .await
            .unwrap();
        assert_eq!(
            reopened.snapshot().await.unwrap().book.export_state(),
            bytes(&row["merged"])
        );
        assert!(
            reopened
                .initialize_from_backup(&hd_backup(row))
                .await
                .is_err()
        );
        let row = f["payments"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["state"] == "pending")
            .unwrap();
        let owner = owner();
        let store = BrowserPaymentBook::open(&database, scope(row), &owner, peer(), direction(row))
            .await
            .unwrap();
        store.initialize(&owner, 0).await.unwrap();
        store
            .reserve(
                &owner,
                PaymentAllocationId(id(1)),
                row["index"].as_u64().unwrap() as u32 + 1,
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .merge_backup(&owner, &payment_backup(row))
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            store.snapshot(&owner).await.unwrap().book.export_state(),
            bytes(&row["merged"])
        );
        let reopened =
            BrowserPaymentBook::open(&database, scope(row), &owner, peer(), direction(row))
                .await
                .unwrap();
        assert_eq!(
            reopened.snapshot(&owner).await.unwrap().book.export_state(),
            bytes(&row["merged"])
        );
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn allocation_and_restore_races_keep_every_committed_range_consumed() {
        let f = fixture();
        let row = &f["addresses"][0];
        let database = name();
        let store = BrowserAddressBook::open(&database, scope(row), account(row))
            .await
            .unwrap();
        store.initialize(0, 0).await.unwrap();
        store
            .reserve(AddressAllocationId(id(1)), false, 10)
            .await
            .unwrap();
        let backup = hd_backup(row);
        let (merged, reserved) = race(
            store.merge_backup(&backup),
            store.reserve(AddressAllocationId(id(2)), false, 10),
        )
        .await;
        assert_eq!(
            usize::from(merged.is_ok()) + usize::from(reserved.is_ok()),
            1
        );
        if merged.is_ok() {
            assert!(matches!(
                reserved,
                Err(BrowserAddressError::Browser(BrowserError::StorageConflict))
            ));
            store
                .reserve(AddressAllocationId(id(2)), false, 10)
                .await
                .unwrap();
        } else {
            assert!(matches!(
                merged,
                Err(BrowserAddressError::Browser(BrowserError::StorageConflict))
            ));
            store.merge_backup(&backup).await.unwrap();
        }
        let snapshot = store.snapshot().await.unwrap();
        assert_eq!(snapshot.book.allocations().count(), 2);
        assert!(snapshot.book.next_index(false) >= 100);
        assert!(
            store
                .reserve(AddressAllocationId(id(1)), false, 1)
                .await
                .is_err()
        );
        let row = &f["payments"][0];
        let owner = owner();
        let store = BrowserPaymentBook::open(&database, scope(row), &owner, peer(), direction(row))
            .await
            .unwrap();
        store.initialize(&owner, 0).await.unwrap();
        store
            .reserve(&owner, PaymentAllocationId(id(1)), 10)
            .await
            .unwrap();
        let backup = payment_backup(row);
        let (merged, reserved) = race(
            store.merge_backup(&owner, &backup),
            store.reserve(&owner, PaymentAllocationId(id(2)), 10),
        )
        .await;
        assert_eq!(
            usize::from(merged.is_ok()) + usize::from(reserved.is_ok()),
            1
        );
        if merged.is_ok() {
            assert!(matches!(
                reserved,
                Err(BrowserPaymentError::Browser(BrowserError::StorageConflict))
            ));
            store
                .reserve(&owner, PaymentAllocationId(id(2)), 10)
                .await
                .unwrap();
        } else {
            assert!(matches!(
                merged,
                Err(BrowserPaymentError::Browser(BrowserError::StorageConflict))
            ));
            store.merge_backup(&owner, &backup).await.unwrap();
        }
        let snapshot = store.snapshot(&owner).await.unwrap();
        assert_eq!(snapshot.book.allocations().count(), 2);
        assert!(snapshot.book.next_index() >= 100);
        assert!(
            store
                .reserve(&owner, PaymentAllocationId(id(1)), 1)
                .await
                .is_err()
        );
    }
}
