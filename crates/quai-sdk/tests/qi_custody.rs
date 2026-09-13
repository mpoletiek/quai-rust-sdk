//! Public toy Qi custody, fixed-denomination observations and actual browser races.
#![cfg(feature = "backup")]
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
use quai_sdk::consensus::{
    Denomination, QiConversionTransaction, QiTransaction, QiWrappingTransaction, SignedQiOperation,
};
use quai_sdk::crypto::SecretKey;
use quai_sdk::primitives::{Hash32, get_bytes};
use quai_sdk::wallet::CandidateCoin;
use quai_sdk::wallet::discovery::{Checkpoint, NetworkScope};
use quai_sdk::wallet::metadata::{PublicAddress, StorageError};
use quai_sdk::wallet::qi_custody::{
    MAX_QI_CUSTODY_ADDRESSES, MAX_QI_CUSTODY_BYTES, MAX_QI_OPERATIONS, QiOperationBook,
    ReservationId, ReservationState,
};
use quai_sdk::{QiAddress, U256, Zone};
use std::collections::BTreeMap;
fn scope() -> NetworkScope {
    NetworkScope {
        chain_id: U256::from(15000),
        genesis: Hash32::from_bytes([1; 32]),
        zone: Zone::Cyprus1,
    }
}
fn identity() -> Hash32 {
    Hash32::from_bytes([7; 32])
}
fn checkpoint() -> Checkpoint {
    Checkpoint {
        hash: Hash32::from_bytes([2; 32]),
        height: U256::from(10),
    }
}
fn block() -> Checkpoint {
    Checkpoint {
        hash: Hash32::from_bytes([3; 32]),
        height: U256::from(11),
    }
}
fn id(n: u128) -> ReservationId {
    ReservationId(n.to_be_bytes())
}
fn book() -> QiOperationBook {
    QiOperationBook::new(scope(), identity()).unwrap()
}
fn vectors() -> Vec<serde_json::Value> {
    serde_json::from_slice::<serde_json::Value>(include_bytes!(
        "fixtures/shared/test-infra/fixtures/qi-custody.json"
    ))
    .unwrap()["vectors"]
        .as_array()
        .unwrap()
        .clone()
}
fn root(row: &serde_json::Value) -> SignedQiOperation {
    SignedQiOperation::decode(&get_bytes(row["signed"].as_str().unwrap()).unwrap()).unwrap()
}
fn keys(row: &serde_json::Value) -> Vec<SecretKey> {
    row["publicTestSecrets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            SecretKey::from_bytes(&get_bytes(s.as_str().unwrap()).unwrap().try_into().unwrap())
                .unwrap()
        })
        .collect()
}
fn source(tx: &QiTransaction) -> (Vec<CandidateCoin>, Vec<PublicAddress>) {
    let owners: BTreeMap<_, _> = tx
        .inputs
        .iter()
        .map(|i| {
            (
                i.public_key.address(),
                PublicAddress::imported(&i.public_key).unwrap(),
            )
        })
        .collect();
    let coins = tx
        .inputs
        .iter()
        .map(|i| CandidateCoin {
            outpoint: i.previous_output,
            address: QiAddress::try_from(i.public_key.address()).unwrap(),
            denomination: Denomination::new(14).unwrap(),
            unlock_height: U256::ZERO,
            expires_at: None,
            reserved: false,
        })
        .collect();
    (coins, owners.into_values().collect())
}
fn reserve(b: &mut QiOperationBook, n: u128, tx: &QiTransaction) {
    let (c, o) = source(tx);
    b.reserve(id(n), checkpoint(), U256::from(10), &c, &o)
        .unwrap();
}
fn sign(tx: &QiTransaction, keys: &[SecretKey]) -> SignedQiOperation {
    let keys: Vec<_> = keys.iter().collect();
    match tx.data.len() {
        0 => SignedQiOperation::Transfer(tx.sign_local(&keys).unwrap()),
        20 => SignedQiOperation::Wrapping(
            QiWrappingTransaction::from_transaction(tx.clone())
                .unwrap()
                .sign_local(&keys)
                .unwrap(),
        ),
        22 => SignedQiOperation::Conversion(
            QiConversionTransaction::from_transaction(tx.clone())
                .unwrap()
                .sign_local(&keys)
                .unwrap(),
        ),
        _ => panic!("unexpected type"),
    }
}

#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn independent_node_encodings_preserve_all_qi_operation_types_and_states() {
    for row in vectors() {
        let signed = root(&row);
        for (name, encoded) in row["states"].as_object().unwrap() {
            let bytes = get_bytes(&format!("0x{}", encoded.as_str().unwrap())).unwrap();
            let restored = QiOperationBook::from_state(&bytes, scope(), identity()).unwrap();
            assert_eq!(restored.export_state().unwrap(), bytes);
            let mut expected = book();
            if name != "empty" {
                reserve(&mut expected, 1, signed.transaction());
            }
            if name == "released" {
                expected.release_unsigned(id(1)).unwrap();
            }
            if ["signed", "submitted", "confirmed"].contains(&name.as_str()) {
                expected.commit_signed(id(1), &signed).unwrap();
            }
            if name == "submitted" {
                expected.mark_submitted(id(1)).unwrap();
            }
            if name == "confirmed" {
                expected
                    .observe_inclusion(id(1), signed.hash().unwrap(), block())
                    .unwrap();
            }
            if name != "hashOnly" {
                assert_eq!(
                    expected.export_state().unwrap(),
                    bytes,
                    "{} {name}",
                    row["id"]
                );
            }
        }
    }
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn source_lock_expiry_bounds_duplicate_claims_and_release_are_checked_atomically() {
    let tx = root(&vectors()[0]);
    let (coins, owners) = source(tx.transaction());
    let mut b = book();
    let before = b.export_state().unwrap();
    for case in 0..7 {
        let mut c = coins.clone();
        match case {
            0 => c[0].unlock_height = U256::from(11),
            1 => c[0].expires_at = Some(U256::from(10)),
            2 => {
                c[0].unlock_height = U256::from(5);
                c[0].expires_at = Some(U256::from(5));
            }
            3 => c[0].reserved = true,
            4 => c.push(c[0].clone()),
            5 => {
                let mut h = *c[0].outpoint.transaction_hash.bytes();
                h[2] = 0x10;
                c[0].outpoint.transaction_hash = Hash32::from_bytes(h);
            }
            _ => {
                let mut h = *c[0].outpoint.transaction_hash.bytes();
                h[3] = 0;
                c[0].outpoint.transaction_hash = Hash32::from_bytes(h);
            }
        }
        assert!(
            b.reserve(id(1), checkpoint(), U256::from(10), &c, &owners)
                .is_err()
        );
        assert_eq!(b.export_state().unwrap(), before);
    }
    assert!(
        b.reserve(id(1), checkpoint(), U256::from(9), &coins, &owners)
            .is_err()
    );
    assert!(
        b.reserve(id(1), checkpoint(), U256::from(10), &coins, &[])
            .is_err()
    );
    b.reserve(id(1), checkpoint(), U256::from(10), &coins, &owners)
        .unwrap();
    assert!(b.claimed(coins[0].outpoint));
    let held = b.export_state().unwrap();
    assert_eq!(
        b.reserve(id(2), checkpoint(), U256::from(10), &coins, &owners),
        Err(StorageError::Conflict)
    );
    assert_eq!(b.export_state().unwrap(), held);
    b.release_unsigned(id(1)).unwrap();
    assert!(!b.claimed(coins[0].outpoint));
    assert_eq!(b.addresses().count(), owners.len());
    assert!(
        b.reserve(id(1), checkpoint(), U256::from(10), &coins, &owners)
            .is_err()
    );
    b.reserve(id(2), checkpoint(), U256::from(10), &coins, &[])
        .unwrap();
    assert!(b.claimed(coins[0].outpoint));
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn signed_candidates_and_reorgs_retain_all_original_input_claims() {
    for row in vectors() {
        let mut tx = root(&row).transaction().clone();
        if tx.data.is_empty() {
            tx.outputs[0].denomination = Denomination::new(1).unwrap();
        }
        let keys = keys(&row);
        let signed = sign(&tx, &keys);
        let mut b = book();
        reserve(&mut b, 1, &tx);
        b.commit_signed(id(1), &signed).unwrap();
        b.commit_signed(id(1), &signed).unwrap();
        assert!(b.release_unsigned(id(1)).is_err());
        let mut lower = tx.clone();
        lower.outputs[0].denomination =
            Denomination::new(lower.outputs[0].denomination.index() - 1).unwrap();
        let replacement = sign(&lower, &keys);
        b.commit_replacement(id(1), signed.hash().unwrap(), &replacement)
            .unwrap();
        b.commit_replacement(id(1), signed.hash().unwrap(), &replacement)
            .unwrap();
        assert_eq!(b.operation(id(1)).unwrap().replacements.len(), 1);
        b.mark_submitted(id(1)).unwrap();
        b.observe_inclusion(id(1), replacement.hash().unwrap(), block())
            .unwrap();
        assert!(b.release_unsigned(id(1)).is_err());
        let wrong = (signed.hash().unwrap(), block());
        assert!(b.invalidate_inclusion(id(1), wrong).is_err());
        b.invalidate_inclusion(id(1), (replacement.hash().unwrap(), block()))
            .unwrap();
        assert_eq!(
            b.operation(id(1)).unwrap().state,
            ReservationState::Submitted
        );
        assert_eq!(
            b.operation(id(1)).unwrap().payload,
            Some(signed.signed_bytes().unwrap())
        );
        assert_eq!(b.claimed_outpoints().len(), tx.inputs.len());
        let before = b.export_state().unwrap();
        let mut bad = lower.clone();
        bad.inputs[0].previous_output.index += 10;
        assert!(
            b.commit_replacement(id(1), signed.hash().unwrap(), &sign(&bad, &keys))
                .is_err()
        );
        assert_eq!(b.export_state().unwrap(), before);
        assert!(b.commit_signed(id(1), &replacement).is_err());
    }
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn strict_qi_codec_scope_identity_and_count_bounds_never_reset_custody() {
    let row = &vectors()[0];
    let bytes = get_bytes(&format!(
        "0x{}",
        row["states"]["reserved"].as_str().unwrap()
    ))
    .unwrap();
    for n in 0..bytes.len() {
        assert!(QiOperationBook::from_state(&bytes[..n], scope(), identity()).is_err());
    }
    for offset in [0, 8, 40, 72, 73, 105] {
        let mut bad = bytes.clone();
        bad[offset] = 255;
        assert!(QiOperationBook::from_state(&bad, scope(), identity()).is_err());
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(QiOperationBook::from_state(&trailing, scope(), identity()).is_err());
    assert!(QiOperationBook::from_state(&bytes, scope(), Hash32::from_bytes([8; 32])).is_err());
    let mut signed =
        get_bytes(&format!("0x{}", row["states"]["signed"].as_str().unwrap())).unwrap();
    let index = signed.len() - 2;
    signed[index] ^= 1;
    assert!(QiOperationBook::from_state(&signed, scope(), identity()).is_err());
    let root = root(row);
    let (c, o) = source(root.transaction());
    let mut b = book();
    for n in 1..=MAX_QI_OPERATIONS {
        b.reserve(id(n as u128), checkpoint(), U256::from(10), &c, &o)
            .unwrap();
        b.release_unsigned(id(n as u128)).unwrap();
    }
    let full = b.export_state().unwrap();
    assert!(
        b.reserve(id(300), checkpoint(), U256::from(10), &c, &o)
            .is_err()
    );
    assert_eq!(b.export_state().unwrap(), full);
    assert!(
        book()
            .reserve(
                id(1),
                checkpoint(),
                U256::from(10),
                &c,
                &vec![o[0].clone(); MAX_QI_CUSTODY_ADDRESSES + 1]
            )
            .is_err()
    );
    assert!(
        book()
            .reserve(
                id(1),
                checkpoint(),
                U256::from(10),
                &vec![c[0].clone(); 4097],
                &o
            )
            .is_err()
    );
    assert!(
        QiOperationBook::from_state(&vec![0; MAX_QI_CUSTODY_BYTES + 1], scope(), identity())
            .is_err()
    );
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn authenticated_native_qi_backup_preserves_origins_claims_and_candidates() {
    let f: serde_json::Value = serde_json::from_slice(include_bytes!(
        "fixtures/shared/crates/quai-wallet/tests/full-backup-v5-portable.json"
    ))
    .unwrap();
    let bytes = get_bytes(&format!("0x{}", f["envelope"].as_str().unwrap())).unwrap();
    let backup = quai_sdk::wallet::full_backup::EncryptedWalletBackup::from_bytes(&bytes)
        .unwrap()
        .decrypt(f["password"].as_str().unwrap().as_bytes())
        .unwrap();
    let b = QiOperationBook::from_backup(&backup, scope(), identity()).unwrap();
    let source = backup.scope_state(scope()).unwrap();
    assert!(b.addresses().count() > 0);
    let records: Vec<_> = source.operations().filter(|o| o.nonce.is_none()).collect();
    assert_eq!(b.operations().count(), records.len());
    assert!(!records.is_empty());
    for r in records {
        let op = b.operation(r.reservation.id).unwrap();
        assert_eq!(op.payload.as_deref(), r.signed_payload);
        assert_eq!(op.replacements, r.replacements);
        assert_eq!(op.claims.len(), r.qi_claims.len());
        assert!(op.reservation_checkpoint.is_none());
        assert!(op.inclusion.is_none());
    }
    #[cfg(all(target_arch = "wasm32", feature = "browser"))]
    {
        let store =
            quai_sdk::browser_qi::BrowserQiBook::open(&browser::name(), scope(), identity())
                .await
                .unwrap();
        store.initialize_from_backup(&backup).await.unwrap();
        assert_eq!(
            store.snapshot().await.unwrap().book.export_state().unwrap(),
            b.export_state().unwrap()
        );
        assert!(store.initialize_from_backup(&backup).await.is_err());
    }
}

#[cfg(all(target_arch = "wasm32", feature = "browser"))]
mod browser {
    use super::*;
    use quai_sdk::browser::{BrowserError, BrowserSnapshotStore, BrowserStorageScope};
    use quai_sdk::browser_qi::{BrowserQiBook, BrowserQiError};
    use quai_sdk::consensus::OutPoint;
    use quai_sdk::wallet::qi_keys::{QiKeyResolver, QiKeyring};
    pub(super) fn name() -> String {
        let mut b = [0; 8];
        quai_sdk::crypto::fill_random(&mut b).unwrap();
        format!("quai-qi-custody-{:x}", u64::from_be_bytes(b))
    }
    async fn open(name: &str) -> BrowserQiBook {
        BrowserQiBook::open(name, scope(), identity())
            .await
            .unwrap()
    }
    async fn raw(name: &str) -> BrowserSnapshotStore {
        BrowserSnapshotStore::open(
            name,
            BrowserStorageScope {
                chain_id: scope().chain_id,
                genesis: scope().genesis,
                zone: scope().zone,
                wallet: QiOperationBook::storage_identity(identity()),
            },
            MAX_QI_CUSTODY_BYTES,
        )
        .await
        .unwrap()
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn actual_worker_signs_and_reopens_all_three_qi_forms() {
        for row in vectors() {
            let name = name();
            let b = open(&name).await;
            b.initialize().await.unwrap();
            let tx = root(&row).transaction().clone();
            let (c, o) = source(&tx);
            let rev = b.snapshot().await.unwrap().revision;
            b.reserve(rev, id(1), checkpoint(), U256::from(10), &c, &o)
                .await
                .unwrap();
            let mut keyring = QiKeyring::new(None).unwrap();
            for k in keys(&row) {
                keyring.import(k).unwrap();
            }
            let signed = b.sign(id(1), &tx, &keyring).await.unwrap();
            assert_eq!(signed.transaction(), &tx);
            let s = b.snapshot().await.unwrap();
            assert_eq!(
                s.book.operation(id(1)).unwrap().payload,
                Some(signed.signed_bytes().unwrap())
            );
            assert!(b.release_unsigned(id(1)).await.is_err());
            assert!(b.sign(id(1), &tx, &keyring).await.is_err());
            b.mark_submitted(id(1)).await.unwrap();
            let rev = b.snapshot().await.unwrap().revision;
            b.observe_inclusion(rev, id(1), signed.hash().unwrap(), block())
                .await
                .unwrap();
            let rev = b.snapshot().await.unwrap().revision;
            b.invalidate_inclusion(rev, id(1), (signed.hash().unwrap(), block()))
                .await
                .unwrap();
            assert!(
                b.observe_inclusion(rev, id(1), signed.hash().unwrap(), block())
                    .await
                    .is_err()
            );
            drop(b);
            let b = open(&name).await;
            assert_eq!(
                b.snapshot()
                    .await
                    .unwrap()
                    .book
                    .operation(id(1))
                    .unwrap()
                    .state,
                ReservationState::Submitted
            );
        }
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn two_tabs_cannot_claim_the_same_outpoint_from_one_discovery_revision() {
        use std::{future::Future, task::Poll};
        let name = name();
        let a = open(&name).await;
        a.initialize().await.unwrap();
        let b = open(&name).await;
        let row = &vectors()[0];
        let (c, o) = source(root(row).transaction());
        let rev = a.snapshot().await.unwrap().revision;
        let mut first = Box::pin(a.reserve(rev, id(1), checkpoint(), U256::from(10), &c, &o));
        let mut second = Box::pin(b.reserve(rev, id(2), checkpoint(), U256::from(10), &c, &o));
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
        let loser = match one {
            Ok(_) => {
                assert!(matches!(
                    two,
                    Err(BrowserQiError::Browser(BrowserError::StorageConflict))
                ));
                id(2)
            }
            Err(e) => {
                assert!(matches!(
                    e,
                    BrowserQiError::Browser(BrowserError::StorageConflict)
                ));
                id(1)
            }
        };
        let fresh = a.snapshot().await.unwrap();
        assert_eq!(fresh.book.operations().count(), 1);
        assert!(matches!(
            a.reserve(fresh.revision, loser, checkpoint(), U256::from(10), &c, &o)
                .await,
            Err(BrowserQiError::State(StorageError::Conflict))
        ));
        assert!(b.initialize().await.is_err());
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn cancellation_after_claim_and_sign_dispatch_keeps_exact_custody() {
        use std::{future::Future, pin::Pin, task::Poll};
        async fn once<F: Future>(mut f: Pin<&mut F>) -> Poll<F::Output> {
            std::future::poll_fn(|cx| Poll::Ready(f.as_mut().poll(cx))).await
        }
        let name = name();
        let b = open(&name).await;
        b.initialize().await.unwrap();
        let raw = raw(&name).await;
        let row = &vectors()[0];
        let tx = root(row).transaction().clone();
        let (c, o) = source(&tx);
        let rev = b.snapshot().await.unwrap().revision;
        let mut reserve = Box::pin(b.reserve(rev, id(1), checkpoint(), U256::from(10), &c, &o));
        assert!(once(reserve.as_mut()).await.is_pending());
        raw.read().await.unwrap();
        assert!(once(reserve.as_mut()).await.is_pending());
        raw.read().await.unwrap();
        drop(reserve);
        assert!(b.snapshot().await.unwrap().book.claimed(c[0].outpoint));
        let mut keyring = QiKeyring::new(None).unwrap();
        for k in keys(row) {
            keyring.import(k).unwrap();
        }
        let mut signing = Box::pin(b.sign(id(1), &tx, &keyring));
        assert!(once(signing.as_mut()).await.is_pending());
        raw.read().await.unwrap();
        assert!(once(signing.as_mut()).await.is_pending());
        raw.read().await.unwrap();
        drop(signing);
        let s = b.snapshot().await.unwrap();
        let op = s.book.operation(id(1)).unwrap();
        assert_eq!(op.state, ReservationState::Signed);
        assert_eq!(
            SignedQiOperation::decode(op.payload.as_ref().unwrap())
                .unwrap()
                .transaction(),
            &tx
        );
        assert!(b.release_unsigned(id(1)).await.is_err());
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn corrupt_tombstoned_or_wrong_key_custody_cannot_be_reset_or_signed() {
        struct Wrong;
        impl QiKeyResolver for Wrong {
            fn resolve(&self, _: &PublicAddress) -> Result<SecretKey, StorageError> {
                let mut k = [0; 32];
                k[31] = 1;
                Ok(SecretKey::from_bytes(&k).unwrap())
            }
        }
        let name = name();
        let b = open(&name).await;
        b.initialize().await.unwrap();
        let tx = root(&vectors()[0]).transaction().clone();
        let (c, o) = source(&tx);
        let rev = b.snapshot().await.unwrap().revision;
        b.reserve(rev, id(1), checkpoint(), U256::from(10), &c, &o)
            .await
            .unwrap();
        let before = b.snapshot().await.unwrap();
        assert!(b.sign(id(1), &tx, &Wrong).await.is_err());
        assert_eq!(b.snapshot().await.unwrap().revision, before.revision);
        let raw = raw(&name).await;
        let s = raw.read().await.unwrap().unwrap();
        let mut bytes = s.bytes.unwrap();
        bytes[73] ^= 1;
        let rev = raw
            .compare_exchange(Some(s.revision), Some(&bytes))
            .await
            .unwrap();
        assert!(b.snapshot().await.is_err());
        assert!(b.initialize().await.is_err());
        raw.compare_exchange(Some(rev), None).await.unwrap();
        assert!(matches!(
            b.snapshot().await,
            Err(BrowserQiError::Uninitialized)
        ));
        assert!(b.initialize().await.is_err());
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn fetch_discovery_checks_locks_origins_and_revision_before_claiming() {
        use quai_sdk::browser::{BrowserConfig, BrowserFetchTransport};
        use quai_sdk::discovery::{QiDiscoveryOptions, discover_qi};
        use quai_sdk::wallet::{CoinType, HdWallet};
        use quai_sdk::{Provider, Routing};
        let scope = NetworkScope {
            genesis: "0x663a73416275109a01aad3a4c29ea9e310aded63c5eea491243b7312ad8cd16b"
                .parse()
                .unwrap(),
            ..scope()
        };
        let provider = Provider::new(
            BrowserFetchTransport::new(BrowserConfig::default()).unwrap(),
            Routing::direct(
                &format!(
                    "{}/qi",
                    option_env!("QUAI_BROWSER_FIXTURE_URL").unwrap_or("http://127.0.0.1:18080")
                ),
                scope.zone.into(),
            )
            .unwrap(),
            scope.chain_id,
        );
        let wallet = HdWallet::from_seed(&[1; 16], CoinType::Qi).unwrap();
        let account = wallet.account_public(0).unwrap();
        let b = BrowserQiBook::open(&name(), scope, identity())
            .await
            .unwrap();
        b.initialize().await.unwrap();
        let rev = b.snapshot().await.unwrap().revision;
        let mut report = discover_qi(
            &provider,
            scope,
            &account,
            &QiDiscoveryOptions {
                gap_limit: Some(2),
                max_addresses: 16,
                max_outpoints: 8,
                ..Default::default()
            },
            || false,
        )
        .await
        .unwrap();
        let selected = [report.addresses[0].outputs[0].outpoint];
        assert!(
            b.reserve_discovery(rev, id(1), &report, &account, U256::from(100), &selected)
                .await
                .is_err()
        );
        report.addresses[0].derived.account += 1;
        assert!(
            b.reserve_discovery(rev, id(1), &report, &account, U256::from(101), &selected)
                .await
                .is_err()
        );
        report.addresses[0].derived.account -= 1;
        assert!(
            b.reserve_discovery(
                rev,
                id(1),
                &report,
                &account,
                U256::from(101),
                &[selected[0]; 2]
            )
            .await
            .is_err()
        );
        assert_eq!(b.snapshot().await.unwrap().revision, rev);
        b.reserve_discovery(rev, id(1), &report, &account, U256::from(101), &selected)
            .await
            .unwrap();
        let s = b.snapshot().await.unwrap();
        assert!(s.book.claimed(selected[0]));
        let public =
            PublicAddress::derive(&account, false, report.addresses[0].derived.index).unwrap();
        assert_eq!(
            s.book.address(public.address().try_into().unwrap()),
            Some(&public)
        );
        assert!(matches!(
            b.reserve_discovery(rev, id(2), &report, &account, U256::from(101), &selected)
                .await,
            Err(BrowserQiError::State(StorageError::StaleSnapshot))
        ));
        let tx = QiTransaction {
            chain_id: scope.chain_id,
            inputs: vec![quai_sdk::consensus::QiInput {
                previous_output: selected[0],
                public_key: quai_sdk::crypto::PublicKey::from_sec1_bytes(public.public_key())
                    .unwrap(),
            }],
            outputs: vec![quai_sdk::consensus::QiOutput {
                address: "0x0080000000000000000000000000000000000001"
                    .parse()
                    .unwrap(),
                denomination: Denomination::new(0).unwrap(),
            }],
            data: vec![],
        };
        assert_eq!(
            b.sign(id(1), &tx, &wallet).await.unwrap().transaction(),
            &tx
        );
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn mixed_hd_imported_and_payment_keys_preserve_origins_and_sign_in_order() {
        use quai_sdk::consensus::QiInput;
        use quai_sdk::crypto::PublicKey;
        use quai_sdk::payments::{PaymentDirection, PaymentSearch, PrivatePaymentCode};
        use quai_sdk::wallet::{CoinType, HdWallet, Search};
        let wallet = HdWallet::from_seed(&[7; 32], CoinType::Qi).unwrap();
        let account = wallet.account_public(0).unwrap();
        let found = account
            .search(
                false,
                Search {
                    zone: Zone::Cyprus1,
                    start_index: 0,
                    max_attempts: 100_000,
                },
                || false,
            )
            .unwrap();
        let hd = PublicAddress::derive(&account, false, found.address.index).unwrap();
        let mut ring = QiKeyring::new(Some(&wallet)).unwrap();
        let mut scalar = [0; 32];
        scalar[31] = 130;
        let imported = ring
            .import(SecretKey::from_bytes(&scalar).unwrap())
            .unwrap();
        let owner = PrivatePaymentCode::from_seed(&[1; 32], 0).unwrap();
        let peer = PrivatePaymentCode::from_seed(&[2; 32], 0).unwrap();
        let payment = owner
            .search(
                peer.public_code(),
                PaymentDirection::Receive,
                PaymentSearch {
                    zone: Zone::Cyprus1,
                    start_index: 0,
                    max_attempts: 100_000,
                },
                || false,
            )
            .unwrap();
        let payment = ring
            .import_payment_receive(&owner, peer.public_code(), payment.index)
            .unwrap();
        let owners = [hd, imported, payment];
        let mut tx = root(&vectors()[0]).transaction().clone();
        tx.inputs = owners
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mut hash = [0; 32];
                hash[3] = 0x80;
                hash[31] = i as u8 + 1;
                QiInput {
                    previous_output: OutPoint {
                        transaction_hash: Hash32::from_bytes(hash),
                        index: 0,
                    },
                    public_key: PublicKey::from_sec1_bytes(p.public_key()).unwrap(),
                }
            })
            .collect();
        let (coins, _) = source(&tx);
        let database = name();
        let b = open(&database).await;
        b.initialize().await.unwrap();
        b.reserve(
            b.snapshot().await.unwrap().revision,
            id(1),
            checkpoint(),
            U256::from(10),
            &coins,
            &owners,
        )
        .await
        .unwrap();
        let signed = b.sign(id(1), &tx, &ring).await.unwrap();
        assert_eq!(
            SignedQiOperation::decode(&signed.signed_bytes().unwrap())
                .unwrap()
                .transaction(),
            &tx
        );
        let restored = open(&database).await.snapshot().await.unwrap();
        for public in owners {
            assert_eq!(
                restored.book.address(public.address().try_into().unwrap()),
                Some(&public)
            );
        }
        assert_eq!(
            restored.book.operation(id(1)).unwrap().payload,
            Some(signed.signed_bytes().unwrap())
        );
    }
}
