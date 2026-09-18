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
            _ => c[0].outpoint.transaction_hash = Hash32::ZERO,
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
            &QiDiscoveryOptions::default()
                .with_gap_limit(Some(2))
                .with_max_addresses(16)
                .with_max_outpoints(8),
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
        for public in &owners {
            assert_eq!(
                restored.book.address(public.address().try_into().unwrap()),
                Some(public)
            );
        }
        use quai_sdk::wallet::full_backup::{BackupOrigin, WalletBackup};
        let backup = WalletBackup::capture_qi_custody(
            &restored.book,
            vec![
                BackupOrigin::from_seed(&[7; 32]).unwrap(),
                BackupOrigin::from_private_key(&ring.resolve(&owners[1]).unwrap()),
                BackupOrigin::from_private_key(&ring.resolve(&owners[2]).unwrap()),
            ],
        )
        .unwrap();
        let copy = QiOperationBook::from_backup(&backup, scope(), identity()).unwrap();
        assert_eq!(copy.claimed_outpoints(), restored.book.claimed_outpoints());
        assert_eq!(
            copy.addresses().collect::<Vec<_>>(),
            restored.book.addresses().collect::<Vec<_>>()
        );
        assert_eq!(
            restored.book.operation(id(1)).unwrap().payload,
            Some(signed.signed_bytes().unwrap())
        );
    }
}

mod backup_tests {
    use super::*;
    use quai_sdk::wallet::full_backup::{BackupOrigin, WalletBackup};
    fn origins() -> Vec<BackupOrigin> {
        let keys: BTreeMap<_, _> = vectors()
            .iter()
            .flat_map(keys)
            .map(|k| (k.public_key().address(), k))
            .collect();
        keys.values().map(BackupOrigin::from_private_key).collect()
    }
    fn capture(b: &QiOperationBook) -> WalletBackup {
        WalletBackup::capture_qi_custody(b, origins()).unwrap()
    }
    fn tx(row: usize, offset: u16) -> QiTransaction {
        let mut tx = root(&vectors()[row]).transaction().clone();
        for input in &mut tx.inputs {
            input.previous_output.index += offset;
        }
        tx.outputs[0].denomination = Denomination::new(3).unwrap();
        tx
    }
    fn signed(b: &mut QiOperationBook, n: u128, row: usize, offset: u16) -> SignedQiOperation {
        let tx = tx(row, offset);
        reserve(b, n, &tx);
        let signed = sign(&tx, &keys(&vectors()[row]));
        b.commit_signed(id(n), &signed).unwrap();
        signed
    }
    fn lower(signed: &SignedQiOperation, row: usize, denomination: u8) -> SignedQiOperation {
        let mut tx = signed.transaction().clone();
        tx.outputs[0].denomination = Denomination::new(denomination).unwrap();
        sign(&tx, &keys(&vectors()[row]))
    }
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    fn qi_backup_union_keeps_branches_disjoint_ids_and_invalidates_observations() {
        let mut source = book();
        let root = signed(&mut source, 1, 0, 0);
        let mut live = source.clone();
        source
            .commit_replacement(id(1), root.hash().unwrap(), &lower(&root, 0, 2))
            .unwrap();
        live.commit_replacement(id(1), root.hash().unwrap(), &lower(&root, 0, 1))
            .unwrap();
        let second = signed(&mut live, 2, 0, 100);
        signed(&mut source, 3, 0, 200);
        live.observe_inclusion(id(1), root.hash().unwrap(), block())
            .unwrap();
        live.observe_inclusion(id(2), second.hash().unwrap(), block())
            .unwrap();
        let report = live.merge_backup(&capture(&source)).unwrap();
        assert_eq!(report.operations_added, 1);
        assert_eq!(report.candidates_added, 1);
        assert_eq!(report.invalidated_inclusions, 2);
        assert_eq!(report.invalidated_checkpoints, 2);
        assert_eq!(live.operations().count(), 3);
        assert_eq!(live.operation(id(1)).unwrap().replacements.len(), 2);
        assert_eq!(
            live.operation(id(2)).unwrap().state,
            ReservationState::Submitted
        );
        let before = live.export_state().unwrap();
        assert_eq!(
            live.merge_backup(&capture(&source)).unwrap(),
            Default::default()
        );
        assert_eq!(live.export_state().unwrap(), before);
    }
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    fn qi_unsigned_backups_never_release_or_reopen_and_signed_evidence_retains_claims() {
        let mut source = book();
        reserve(&mut source, 1, &tx(0, 0));
        let reserved = capture(&source);
        let mut live = source.clone();
        source.release_unsigned(id(1)).unwrap();
        let released = capture(&source);
        live.merge_backup(&released).unwrap();
        assert_eq!(
            live.operation(id(1)).unwrap().state,
            ReservationState::Reserved
        );
        live.release_unsigned(id(1)).unwrap();
        live.merge_backup(&reserved).unwrap();
        assert_eq!(
            live.operation(id(1)).unwrap().state,
            ReservationState::Released
        );
        assert!(live.operation(id(1)).unwrap().claims.is_empty());
        let mut original = QiOperationBook::from_backup(&reserved, scope(), identity()).unwrap();
        original
            .commit_signed(id(1), &sign(&tx(0, 0), &keys(&vectors()[0])))
            .unwrap();
        live.merge_backup(&capture(&original)).unwrap();
        live.merge_backup(&released).unwrap();
        assert_eq!(
            live.operation(id(1)).unwrap().state,
            ReservationState::Signed
        );
        assert_eq!(live.claimed_outpoints(), original.claimed_outpoints());
        assert!(live.release_unsigned(id(1)).is_err());
    }
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    fn qi_merge_conflicting_inputs_roots_ids_and_capacity_preserves_live_bytes() {
        let mut live = book();
        signed(&mut live, 1, 0, 0);
        let before = live.export_state().unwrap();
        for case in 0..3 {
            let mut source = book();
            match case {
                0 => {
                    signed(&mut source, 1, 0, 100);
                }
                1 => {
                    signed(&mut source, 2, 0, 0);
                }
                _ => {
                    let mut different = tx(0, 0);
                    different.outputs[0].denomination = Denomination::new(2).unwrap();
                    reserve(&mut source, 1, &different);
                    source
                        .commit_signed(id(1), &sign(&different, &keys(&vectors()[0])))
                        .unwrap();
                }
            }
            assert!(live.merge_backup(&capture(&source)).is_err());
            assert_eq!(live.export_state().unwrap(), before);
        }
        let mut released = book();
        reserve(&mut released, 1, &tx(0, 0));
        released.release_unsigned(id(1)).unwrap();
        reserve(&mut released, 2, &tx(0, 0));
        let before = released.export_state().unwrap();
        assert!(released.merge_backup(&capture(&live)).is_err()); // Previously released ID cannot steal a reassigned point.
        assert_eq!(released.export_state().unwrap(), before);
        let mut full = book();
        for n in 1..=MAX_QI_OPERATIONS as u128 {
            reserve(&mut full, n, &tx(0, 0));
            full.release_unsigned(id(n)).unwrap();
        }
        let before = full.export_state().unwrap();
        let mut source = book();
        signed(&mut source, 257, 0, 100);
        assert!(full.merge_backup(&capture(&source)).is_err());
        assert_eq!(full.export_state().unwrap(), before);
    }
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    fn qi_hash_only_custody_gains_matching_canonical_bytes() {
        for row in vectors() {
            let encoded = get_bytes(&format!(
                "0x{}",
                row["states"]["hashOnly"].as_str().unwrap()
            ))
            .unwrap();
            let mut live = QiOperationBook::from_state(&encoded, scope(), identity()).unwrap();
            let hash_only = capture(&live);
            let mut source = book();
            let root = root(&row);
            reserve(&mut source, 1, root.transaction());
            source.commit_signed(id(1), &root).unwrap();
            live.merge_backup(&capture(&source)).unwrap();
            assert_eq!(
                live.operation(id(1)).unwrap().payload,
                Some(root.signed_bytes().unwrap())
            );
            let before = live.export_state().unwrap();
            live.merge_backup(&hash_only).unwrap();
            assert_eq!(live.export_state().unwrap(), before);
            assert!(live.release_unsigned(id(1)).is_err());
        }
    }
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    fn qi_capture_encrypts_all_forms_and_restores_native_custody_without_observations() {
        let mut b = book();
        for row in 0..4 {
            let root = signed(&mut b, row as u128 + 1, row, row as u16 * 100);
            b.commit_replacement(
                id(row as u128 + 1),
                root.hash().unwrap(),
                &lower(&root, row, 2),
            )
            .unwrap();
            b.observe_inclusion(id(row as u128 + 1), root.hash().unwrap(), block())
                .unwrap();
        }
        let backup = capture(&b);
        let encoded = backup
            .encrypt(
                b"public toy backup password",
                quai_sdk::wallet::BackupKdf::default(),
            )
            .unwrap();
        assert_eq!(encoded.as_bytes()[8], 5);
        let decoded = encoded.decrypt(b"public toy backup password").unwrap();
        let restored = QiOperationBook::from_backup(&decoded, scope(), identity()).unwrap();
        assert_eq!(restored.claimed_outpoints(), b.claimed_outpoints());
        for op in restored.operations() {
            assert_eq!(op.state, ReservationState::Submitted);
            assert!(op.inclusion.is_none() && op.reservation_checkpoint.is_none());
            assert_eq!(op.payload, b.operation(op.id).unwrap().payload);
            assert_eq!(op.replacements, b.operation(op.id).unwrap().replacements);
        }
        #[cfg(all(not(target_arch = "wasm32"), feature = "sqlite"))]
        {
            let mut random = [0; 16];
            quai_sdk::crypto::fill_random(&mut random).unwrap();
            let directory = std::env::temp_dir()
                .join(format!("quai-qi-backup-{:x}", u128::from_be_bytes(random)));
            std::fs::create_dir(&directory).unwrap();
            let mut store = quai_sdk::wallet::storage::SqliteStore::open(
                directory.join("wallet.sqlite"),
                scope(),
            )
            .unwrap();
            assert_eq!(
                decoded
                    .restore(&mut store)
                    .unwrap()
                    .retained_signed_operations,
                4
            );
            for op in restored.operations() {
                let expected: std::collections::BTreeSet<_> =
                    op.claims.iter().map(|c| c.outpoint).collect();
                assert_eq!(
                    store
                        .reserved_outpoints(op.id)
                        .unwrap()
                        .into_iter()
                        .collect::<std::collections::BTreeSet<_>>(),
                    expected
                );
                assert_eq!(store.signed_payload(op.id).unwrap(), op.payload);
                assert_eq!(
                    store.replacement_candidates(op.id).unwrap(),
                    op.replacements
                );
                assert!(
                    store
                        .reservation(op.id)
                        .unwrap()
                        .unwrap()
                        .inclusion
                        .is_none()
                );
                assert!(store.release_unsigned(op.id).is_err());
            }
            drop(store);
            std::fs::remove_dir_all(directory).unwrap();
        }
    }
    #[cfg(all(target_arch = "wasm32", feature = "browser"))]
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn browser_qi_backup_merge_race_keeps_claims_and_fences_old_observations() {
        use quai_sdk::browser::BrowserError;
        use quai_sdk::browser_qi::{BrowserQiBook, BrowserQiError};
        use std::{future::Future, task::Poll};
        let database = super::browser::name();
        let a = BrowserQiBook::open(&database, scope(), identity())
            .await
            .unwrap();
        let b = BrowserQiBook::open(&database, scope(), identity())
            .await
            .unwrap();
        a.initialize().await.unwrap();
        let rev = a.snapshot().await.unwrap().revision;
        let mut one = book();
        let root = signed(&mut one, 1, 0, 0);
        let mut two = book();
        signed(&mut two, 2, 0, 100);
        let first = capture(&one);
        let second = capture(&two);
        let mut left = Box::pin(a.merge_backup(&first));
        let mut right = Box::pin(b.merge_backup(&second));
        let (mut x, mut y) = (None, None);
        let (x, y) = std::future::poll_fn(|cx| {
            if x.is_none()
                && let Poll::Ready(v) = left.as_mut().poll(cx)
            {
                x = Some(v);
            }
            if y.is_none()
                && let Poll::Ready(v) = right.as_mut().poll(cx)
            {
                y = Some(v);
            }
            if x.is_some() && y.is_some() {
                Poll::Ready((x.take().unwrap(), y.take().unwrap()))
            } else {
                Poll::Pending
            }
        })
        .await;
        assert_eq!(usize::from(x.is_ok()) + usize::from(y.is_ok()), 1);
        assert_eq!(a.snapshot().await.unwrap().book.operations().count(), 1);
        let loser = if x.is_ok() {
            assert!(matches!(
                y,
                Err(BrowserQiError::Browser(BrowserError::StorageConflict))
            ));
            &second
        } else {
            assert!(matches!(
                x,
                Err(BrowserQiError::Browser(BrowserError::StorageConflict))
            ));
            &first
        };
        assert_eq!(a.merge_backup(loser).await.unwrap().operations_added, 1);
        assert!(
            a.observe_inclusion(rev, id(1), root.hash().unwrap(), block())
                .await
                .is_err()
        );
        let s = b.snapshot().await.unwrap();
        assert_eq!(s.book.operations().count(), 2);
        assert_eq!(s.book.claimed_outpoints().len(), 2);
        let revision = s.revision;
        let mut collision = book();
        signed(&mut collision, 3, 0, 0);
        assert!(b.merge_backup(&capture(&collision)).await.is_err());
        assert_eq!(b.snapshot().await.unwrap().revision, revision);
        let backup = capture(&s.book);
        let restored = BrowserQiBook::open(&super::browser::name(), scope(), identity())
            .await
            .unwrap();
        restored.initialize_from_backup(&backup).await.unwrap();
        assert_eq!(
            restored
                .snapshot()
                .await
                .unwrap()
                .book
                .export_state()
                .unwrap(),
            s.book.export_state().unwrap()
        );
    }
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    fn qi_capture_proves_hd_ancestry_and_rejects_conflicting_public_origins() {
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
        let public = PublicAddress::derive(&account, false, found.address.index).unwrap();
        let mut tx = tx(0, 0);
        tx.inputs[0].public_key =
            quai_sdk::crypto::PublicKey::from_sec1_bytes(public.public_key()).unwrap();
        let (coins, imported) = source(&tx);
        let mut hd = book();
        hd.reserve(
            id(1),
            checkpoint(),
            U256::from(10),
            &coins,
            std::slice::from_ref(&public),
        )
        .unwrap();
        assert!(
            WalletBackup::capture_qi_custody(&hd, vec![BackupOrigin::from_seed(&[8; 32]).unwrap()])
                .is_err()
        );
        let key = quai_sdk::wallet::qi_keys::QiKeyResolver::resolve(&wallet, &public).unwrap();
        assert!(
            WalletBackup::capture_qi_custody(&hd, vec![BackupOrigin::from_private_key(&key)])
                .is_err()
        );
        let backup =
            WalletBackup::capture_qi_custody(&hd, vec![BackupOrigin::from_seed(&[7; 32]).unwrap()])
                .unwrap();
        assert_eq!(backup.scope_state(scope()).unwrap().addresses(), &[public]);
        let mut standalone = book();
        standalone
            .reserve(id(1), checkpoint(), U256::from(10), &coins, &imported)
            .unwrap();
        let standalone_backup = WalletBackup::capture_qi_custody(
            &standalone,
            vec![BackupOrigin::from_private_key(&key)],
        )
        .unwrap();
        let before = hd.export_state().unwrap();
        assert!(hd.merge_backup(&standalone_backup).is_err());
        assert_eq!(hd.export_state().unwrap(), before);
    }
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    fn qi_candidate_union_limit_rejects_without_dropping_either_branch() {
        let mut live = book();
        let root = signed(&mut live, 1, 0, 0);
        let mut source = live.clone();
        for n in 1..=33u8 {
            let mut tx = root.transaction().clone();
            tx.outputs[0].denomination = Denomination::new(0).unwrap();
            let mut address = [0; 20];
            address[1] = 0x80;
            address[19] = n;
            tx.outputs[0].address = QiAddress::try_from(address).unwrap().address();
            let replacement = sign(&tx, &keys(&vectors()[0]));
            let target = if n <= 16 { &mut live } else { &mut source };
            target
                .commit_replacement(id(1), root.hash().unwrap(), &replacement)
                .unwrap();
        }
        let before = live.export_state().unwrap();
        assert!(live.merge_backup(&capture(&source)).is_err());
        assert_eq!(live.export_state().unwrap(), before);
        assert_eq!(source.operation(id(1)).unwrap().replacements.len(), 17);
    }
}

#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn refund_outpoints_retain_quai_hash_bits_in_portable_custody() {
    let row = &vectors()[0];
    let mut tx = root(row).transaction().clone();
    for input in &mut tx.inputs {
        let mut hash = *input.previous_output.transaction_hash.bytes();
        hash[3] &= 0x7f;
        input.previous_output.transaction_hash = Hash32::from_bytes(hash);
    }
    let signed = sign(&tx, &keys(row));
    let mut b = book();
    reserve(&mut b, 1, &tx);
    b.commit_signed(id(1), &signed).unwrap();
    let bytes = b.export_state().unwrap();
    let reopened = QiOperationBook::from_state(&bytes, scope(), identity()).unwrap();
    assert_eq!(reopened.export_state().unwrap(), bytes);
    assert_eq!(
        reopened.operation(id(1)).unwrap().claims.len(),
        tx.inputs.len()
    );
    tx.inputs[0].previous_output.transaction_hash = Hash32::ZERO;
    assert!(tx.signing_digest().is_err());
    let (coins, owners) = source(&tx);
    assert!(
        book()
            .reserve(id(2), checkpoint(), U256::from(10), &coins, &owners)
            .is_err()
    );
}
