//! Payment exposure durability, ownership and independent public fixture checks.
#![cfg(all(feature = "wallet", feature = "payments"))]
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
use quai_sdk::payments::{
    PaymentChannel, PaymentCode, PaymentDirection, PaymentSearch, PrivatePaymentCode,
};
use quai_sdk::wallet::discovery::NetworkScope;
use quai_sdk::wallet::metadata::StorageError;
use quai_sdk::wallet::payment_allocation::*;
use quai_sdk::{U256, Zone};
fn scope() -> NetworkScope {
    NetworkScope {
        chain_id: U256::from(15000),
        genesis: quai_sdk::primitives::Hash32::from_bytes([1; 32]),
        zone: Zone::Cyprus1,
    }
}
fn owner() -> PrivatePaymentCode {
    PrivatePaymentCode::from_seed(&[1; 16], 0).unwrap()
}
fn peer() -> PaymentCode {
    PrivatePaymentCode::from_seed(&[2; 16], 0)
        .unwrap()
        .public_code()
        .clone()
}
fn id(n: u128) -> PaymentAllocationId {
    PaymentAllocationId(n.to_be_bytes())
}
fn found(book: &PaymentAllocationBook, owner: &PrivatePaymentCode, id: PaymentAllocationId) -> u32 {
    let a = book.allocation(id).unwrap();
    owner
        .search(
            book.peer(),
            book.direction(),
            PaymentSearch {
                zone: book.scope().zone,
                start_index: a.range.start,
                max_attempts: a.range.end - a.range.start,
            },
            || false,
        )
        .unwrap()
        .index
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn independent_node_states_match_pinned_payment_derivations_and_receive_ownership() {
    let fixture: serde_json::Value = serde_json::from_slice(include_bytes!(
        "fixtures/shared/test-infra/fixtures/payment-allocation.json"
    ))
    .unwrap();
    let owner = owner();
    let peer = peer();
    assert_eq!(owner.public_code().to_base58(), fixture["ownerCode"]);
    assert_eq!(peer.to_base58(), fixture["peerCode"]);
    let mut states = 0;
    for row in fixture["vectors"].as_array().unwrap() {
        let direction = if row["direction"] == "send" {
            PaymentDirection::Send
        } else {
            PaymentDirection::Receive
        };
        let mut scope = scope();
        scope.zone = Zone::from_byte(row["zone"].as_u64().unwrap() as u8).unwrap();
        let index = row["index"].as_u64().unwrap() as u32;
        for (name, expected) in row["states"].as_object().unwrap() {
            let mut book =
                PaymentAllocationBook::new(scope, &owner, peer.clone(), direction, 0).unwrap();
            if name != "empty" {
                book.reserve(id(1), index + 1).unwrap();
            }
            if name == "completed" {
                let record = book.complete(&owner, id(1), index).unwrap();
                assert_eq!(record.address.to_string(), row["address"]);
                assert_eq!(
                    format!(
                        "0x{}",
                        record
                            .public_key
                            .to_compressed()
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<String>()
                    ),
                    row["publicKey"]
                );
                if direction == PaymentDirection::Receive {
                    assert_eq!(
                        owner.receive_key(&peer, index).unwrap().public_key(),
                        record.public_key
                    );
                } else {
                    let receiver = PrivatePaymentCode::from_seed(&[2; 16], 0).unwrap();
                    assert_eq!(
                        receiver
                            .receive_key(owner.public_code(), index)
                            .unwrap()
                            .public_key(),
                        record.public_key
                    );
                }
            }
            if name == "abandoned" {
                book.abandon(id(1)).unwrap();
            }
            let expected =
                quai_sdk::primitives::get_bytes(&format!("0x{}", expected.as_str().unwrap()))
                    .unwrap();
            assert_eq!(book.export_state(), expected);
            assert_eq!(
                PaymentAllocationBook::from_state(
                    &expected,
                    scope,
                    &owner,
                    peer.clone(),
                    direction
                )
                .unwrap()
                .export_state(),
                expected
            );
            states += 1;
        }
    }
    assert_eq!(states, 24);
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn journal_retains_burned_ranges_and_verifies_owner_on_every_completion() {
    let owner = owner();
    let wrong = PrivatePaymentCode::from_seed(&[3; 16], 0).unwrap();
    for direction in [PaymentDirection::Send, PaymentDirection::Receive] {
        let mut book = PaymentAllocationBook::new(scope(), &owner, peer(), direction, 100).unwrap();
        book.reserve(id(2), 10_000).unwrap();
        book.reserve(id(1), 200).unwrap();
        let index = found(&book, &owner, id(2));
        assert!(book.complete(&wrong, id(2), index).is_err());
        let record = book.complete(&owner, id(2), index).unwrap();
        assert_eq!(book.complete(&owner, id(2), index).unwrap(), record);
        assert!(book.complete(&wrong, id(2), index).is_err());
        assert_eq!(
            book.complete(&owner, id(2), index + 1),
            Err(StorageError::Transition)
        );
        assert_eq!(book.abandon(id(2)), Err(StorageError::Transition));
        book.abandon(id(1)).unwrap();
        assert!(book.complete(&owner, id(1), 10_100).is_err());
        let bytes = book.export_state();
        let mut book =
            PaymentAllocationBook::from_state(&bytes, scope(), &owner, peer(), direction).unwrap();
        assert_eq!(book.next_index(), 10_300);
        for used in [id(1), id(2)] {
            assert_eq!(book.reserve(used, 1), Err(StorageError::Conflict));
        }
        book.reserve(id(3), 1).unwrap();
        assert_eq!(book.next_index(), 10_301);
    }
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn import_rejects_foreign_context_truncation_overlap_cursor_rewind_and_capacity_overflow() {
    let owner = owner();
    let direction = PaymentDirection::Receive;
    let mut book = PaymentAllocationBook::new(scope(), &owner, peer(), direction, 5).unwrap();
    book.reserve(id(1), 100).unwrap();
    book.reserve(id(2), 200).unwrap();
    let bytes = book.export_state();
    for len in 0..bytes.len() {
        assert!(
            PaymentAllocationBook::from_state(&bytes[..len], scope(), &owner, peer(), direction)
                .is_err()
        );
    }
    let mut cases = vec![];
    for offset in [0, 8, 40, 72, 73, 105, 109, 113, 139] {
        let mut b = bytes.clone();
        b[offset] ^= 0xff;
        cases.push(b);
    }
    let mut b = bytes.clone();
    b.push(0);
    cases.push(b);
    let mut b = bytes.clone();
    b[140..156].copy_from_slice(&bytes[115..131]);
    cases.push(b);
    for start in [5u32, 106] {
        let mut b = bytes.clone();
        b[156..160].copy_from_slice(&start.to_be_bytes());
        cases.push(b);
    }
    for b in cases {
        assert!(PaymentAllocationBook::from_state(&b, scope(), &owner, peer(), direction).is_err());
    }
    assert!(
        PaymentAllocationBook::from_state(&bytes, scope(), &owner, peer(), PaymentDirection::Send)
            .is_err()
    );
    let wrong = PrivatePaymentCode::from_seed(&[3; 16], 0).unwrap();
    assert!(PaymentAllocationBook::from_state(&bytes, scope(), &wrong, peer(), direction).is_err());
    assert!(
        PaymentAllocationBook::from_state(
            &bytes,
            scope(),
            &owner,
            wrong.public_code().clone(),
            direction
        )
        .is_err()
    );
    let mut full = PaymentAllocationBook::new(scope(), &owner, peer(), direction, 0).unwrap();
    for i in 0..MAX_PAYMENT_ALLOCATIONS {
        full.reserve(id(i as u128), 1).unwrap();
    }
    let bytes = full.export_state();
    assert!(bytes.len() <= MAX_PAYMENT_ALLOCATION_BYTES);
    assert_eq!(
        PaymentAllocationBook::from_state(&bytes, scope(), &owner, peer(), direction)
            .unwrap()
            .allocations()
            .count(),
        MAX_PAYMENT_ALLOCATIONS
    );
    full.abandon(id(0)).unwrap();
    assert!(
        full.reserve(id(MAX_PAYMENT_ALLOCATIONS as u128), 1)
            .is_err()
    );
    assert!(
        PaymentAllocationBook::from_state(
            &vec![0; MAX_PAYMENT_ALLOCATION_BYTES + 1],
            scope(),
            &owner,
            peer(),
            direction
        )
        .is_err()
    );
    let mut end =
        PaymentAllocationBook::new(scope(), &owner, peer(), direction, (1 << 31) - 1).unwrap();
    end.reserve(id(0), 100).unwrap();
    assert_eq!(end.next_index(), 1 << 31);
    assert_eq!(
        end.reserve(id(1), 1),
        Err(StorageError::DerivationExhausted)
    );
    assert!(PaymentAllocationBook::new(scope(), &owner, peer(), direction, (1 << 31) + 1).is_err());
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn channel_import_preserves_direction_zone_and_exhausted_cursors() {
    let owner = owner();
    let mut channel = PaymentChannel::new(&owner, peer());
    channel
        .advance_cursor(&owner, PaymentDirection::Send, Zone::Cyprus1, Some(50_000))
        .unwrap();
    channel
        .advance_cursor(&owner, PaymentDirection::Receive, Zone::Cyprus1, None)
        .unwrap();
    let send =
        PaymentAllocationBook::from_channel(scope(), &owner, &channel, PaymentDirection::Send)
            .unwrap();
    let receive =
        PaymentAllocationBook::from_channel(scope(), &owner, &channel, PaymentDirection::Receive)
            .unwrap();
    assert_eq!(send.next_index(), 50_000);
    assert_eq!(receive.next_index(), 1 << 31);
    assert_ne!(send.identity(), receive.identity());
    let mut other = scope();
    other.zone = Zone::Hydra3;
    assert_eq!(
        PaymentAllocationBook::from_channel(other, &owner, &channel, PaymentDirection::Send)
            .unwrap()
            .next_index(),
        0
    );
    let wrong = PrivatePaymentCode::from_seed(&[2; 16], 0).unwrap();
    assert!(
        PaymentAllocationBook::from_channel(scope(), &wrong, &channel, PaymentDirection::Send)
            .is_err()
    );
}

#[cfg(feature = "backup")]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn authenticated_backup_initialization_retains_payment_exposure_floor() {
    use quai_sdk::wallet::full_backup::EncryptedWalletBackup;
    let fixture: serde_json::Value = serde_json::from_slice(include_bytes!(
        "fixtures/shared/crates/quai-wallet/tests/full-backup-v3-portable.json"
    ))
    .unwrap();
    let envelope =
        quai_sdk::primitives::get_bytes(&format!("0x{}", fixture["envelope"].as_str().unwrap()))
            .unwrap();
    let backup = EncryptedWalletBackup::from_bytes(&envelope)
        .unwrap()
        .decrypt(fixture["password"].as_str().unwrap().as_bytes())
        .unwrap();
    let channel = backup.payment_channels().next().unwrap();
    let owner = backup.origins()[0].payment_code(channel.account()).unwrap();
    let peer = PaymentCode::from_bytes(channel.peer_code_bytes()).unwrap();
    let exposure = backup.payment_exposures().next().unwrap();
    let direction = exposure.record.direction;
    let mut scope = scope();
    scope.zone = exposure.record.zone;
    let book = PaymentAllocationBook::from_backup(&backup, scope, &owner, peer.clone(), direction)
        .unwrap();
    assert!(book.next_index() >= exposure.record.burned.end);
    assert!(book.next_index() > exposure.record.index);
    let mut wrong = scope;
    wrong.chain_id += U256::from(1);
    assert!(
        PaymentAllocationBook::from_backup(&backup, wrong, &owner, peer.clone(), direction)
            .is_err()
    );
    #[cfg(all(target_arch = "wasm32", feature = "browser"))]
    {
        let db = quai_sdk::browser_payments::BrowserPaymentBook::open(
            &browser::name(),
            scope,
            &owner,
            peer,
            direction,
        )
        .await
        .unwrap();
        db.initialize_from_backup(&owner, &backup).await.unwrap();
        assert_eq!(
            db.snapshot(&owner).await.unwrap().book.next_index(),
            book.next_index()
        );
        assert!(db.initialize_from_backup(&owner, &backup).await.is_err());
    }
}

#[cfg(all(target_arch = "wasm32", feature = "browser"))]
mod browser {
    use super::*;
    use quai_sdk::browser::{BrowserError, BrowserSnapshotStore, BrowserStorageScope};
    use quai_sdk::browser_payments::{BrowserPaymentBook, BrowserPaymentError};
    pub(super) fn name() -> String {
        let mut bytes = [0; 8];
        quai_sdk::crypto::fill_random(&mut bytes).unwrap();
        format!(
            "quai-sdk-payment-{}",
            bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
        )
    }
    async fn open(
        name: &str,
        owner: &PrivatePaymentCode,
        direction: PaymentDirection,
    ) -> BrowserPaymentBook {
        BrowserPaymentBook::open(name, scope(), owner, peer(), direction)
            .await
            .unwrap()
    }
    async fn raw(
        name: &str,
        owner: &PrivatePaymentCode,
        direction: PaymentDirection,
    ) -> BrowserSnapshotStore {
        BrowserSnapshotStore::open(
            name,
            BrowserStorageScope {
                chain_id: scope().chain_id,
                genesis: scope().genesis,
                zone: scope().zone,
                wallet: PaymentAllocationBook::channel_identity(
                    owner.public_code(),
                    owner.account(),
                    &peer(),
                    direction,
                ),
            },
            MAX_PAYMENT_ALLOCATION_BYTES,
        )
        .await
        .unwrap()
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn cancellation_and_restart_keep_ranges_and_only_return_committed_destinations() {
        let owner = owner();
        let name = name();
        let direction = PaymentDirection::Receive;
        let db = open(&name, &owner, direction).await;
        assert!(matches!(
            db.snapshot(&owner).await,
            Err(BrowserPaymentError::Uninitialized)
        ));
        db.initialize(&owner, 100).await.unwrap();
        assert!(matches!(
            db.allocate(&owner, id(1), 10_000, || true).await,
            Err(BrowserPaymentError::Search(_))
        ));
        assert_eq!(db.snapshot(&owner).await.unwrap().book.next_index(), 10_100);
        drop(db);
        let db = open(&name, &owner, direction).await;
        let record = db.resume(&owner, id(1), || false).await.unwrap();
        assert_eq!(
            owner
                .receive_key(&peer(), record.index)
                .unwrap()
                .public_key(),
            record.public_key
        );
        let snapshot = db.snapshot(&owner).await.unwrap();
        assert!(
            matches!(&snapshot.book.allocation(id(1)).unwrap().status,PaymentAllocationStatus::Completed(r) if *r==record)
        );
        assert_eq!(db.resume(&owner, id(1), || true).await.unwrap(), record);
        assert!(db.allocate(&owner, id(1), 1, || false).await.is_err());
        assert!(db.complete(&owner, id(1), record.index + 1).await.is_err());
        assert!(db.abandon(&owner, id(1)).await.is_err());
        db.reserve(&owner, id(2), 200).await.unwrap();
        db.abandon(&owner, id(2)).await.unwrap();
        assert!(db.resume(&owner, id(2), || false).await.is_err());
        assert_eq!(db.snapshot(&owner).await.unwrap().book.next_index(), 10_300);
        let wrong = PrivatePaymentCode::from_seed(&[3; 16], 0).unwrap();
        assert!(db.snapshot(&wrong).await.is_err());
        assert!(db.resume(&wrong, id(1), || false).await.is_err());
        let send = open(&name, &owner, PaymentDirection::Send).await;
        assert!(matches!(
            send.snapshot(&owner).await,
            Err(BrowserPaymentError::Uninitialized)
        ));
        send.initialize(&owner, 0).await.unwrap();
        let sent = send
            .allocate(&owner, id(1), 10_000, || false)
            .await
            .unwrap();
        let receiver = PrivatePaymentCode::from_seed(&[2; 16], 0).unwrap();
        assert_eq!(
            receiver
                .receive_key(owner.public_code(), sent.index)
                .unwrap()
                .public_key(),
            sent.public_key
        );
        assert_eq!(db.snapshot(&owner).await.unwrap().book.next_index(), 10_300);
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn competing_connections_commit_distinct_ranges_and_reject_reinitialization() {
        use std::{future::Future, task::Poll};
        let owner = owner();
        let name = name();
        let direction = PaymentDirection::Send;
        let a = open(&name, &owner, direction).await;
        a.initialize(&owner, 0).await.unwrap();
        let b = open(&name, &owner, direction).await;
        let mut first = Box::pin(a.reserve(&owner, id(1), 10_000));
        let mut second = Box::pin(b.reserve(&owner, id(2), 10_000));
        let mut one = None;
        let mut two = None;
        let (one, two) = std::future::poll_fn(|cx| {
            if one.is_none()
                && let Poll::Ready(v) = first.as_mut().poll(cx)
            {
                one = Some(v);
            }
            if two.is_none()
                && let Poll::Ready(v) = second.as_mut().poll(cx)
            {
                two = Some(v);
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
                Err(BrowserPaymentError::Browser(BrowserError::StorageConflict))
            ));
            id(2)
        } else {
            assert!(matches!(
                one,
                Err(BrowserPaymentError::Browser(BrowserError::StorageConflict))
            ));
            id(1)
        };
        a.reserve(&owner, loser, 10_000).await.unwrap();
        let one = a.resume(&owner, id(1), || false).await.unwrap();
        let two = b.resume(&owner, id(2), || false).await.unwrap();
        assert_ne!(one.address, two.address);
        assert!(one.burned.end <= two.burned.start || two.burned.end <= one.burned.start);
        assert_eq!(a.snapshot(&owner).await.unwrap().book.next_index(), 20_000);
        assert!(b.initialize(&owner, 0).await.is_err());
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn corrupt_state_and_tombstones_cannot_reset_or_expose_destinations() {
        let owner = owner();
        let name = name();
        let direction = PaymentDirection::Receive;
        let db = open(&name, &owner, direction).await;
        db.initialize(&owner, 500).await.unwrap();
        let raw = raw(&name, &owner, direction).await;
        let snapshot = raw.read().await.unwrap().unwrap();
        let mut bytes = snapshot.bytes.unwrap();
        bytes[73] ^= 1;
        let revision = raw
            .compare_exchange(Some(snapshot.revision), Some(&bytes))
            .await
            .unwrap();
        assert!(matches!(
            db.snapshot(&owner).await,
            Err(BrowserPaymentError::State(StorageError::Invalid))
        ));
        assert!(db.initialize(&owner, 0).await.is_err());
        raw.compare_exchange(Some(revision), None).await.unwrap();
        assert!(matches!(
            db.snapshot(&owner).await,
            Err(BrowserPaymentError::Uninitialized)
        ));
        assert!(db.initialize(&owner, 0).await.is_err());
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn dropped_write_futures_preserve_committed_ranges_and_exposures() {
        use std::{future::Future, pin::Pin, task::Poll};
        async fn poll_once<F: Future>(mut f: Pin<&mut F>) -> Poll<F::Output> {
            std::future::poll_fn(|cx| Poll::Ready(f.as_mut().poll(cx))).await
        }
        let owner = owner();
        let name = name();
        let direction = PaymentDirection::Receive;
        let db = open(&name, &owner, direction).await;
        db.initialize(&owner, 0).await.unwrap();
        let raw = raw(&name, &owner, direction).await;
        let mut reserve = Box::pin(db.reserve(&owner, id(1), 10_000));
        assert!(poll_once(reserve.as_mut()).await.is_pending());
        raw.read().await.unwrap();
        assert!(poll_once(reserve.as_mut()).await.is_pending());
        raw.read().await.unwrap();
        drop(reserve);
        let snapshot = db.snapshot(&owner).await.unwrap();
        assert_eq!(snapshot.book.next_index(), 10_000);
        let index = found(&snapshot.book, &owner, id(1));
        let mut complete = Box::pin(db.complete(&owner, id(1), index));
        assert!(poll_once(complete.as_mut()).await.is_pending());
        raw.read().await.unwrap();
        assert!(poll_once(complete.as_mut()).await.is_pending());
        raw.read().await.unwrap();
        drop(complete);
        assert!(matches!(
            db.snapshot(&owner)
                .await
                .unwrap()
                .book
                .allocation(id(1))
                .unwrap()
                .status,
            PaymentAllocationStatus::Completed(_)
        ));
        assert_eq!(
            db.resume(&owner, id(1), || true).await.unwrap().index,
            index
        );
    }
}
