//! Public toy transaction custody; no network or real funds.
#![cfg(feature = "backup")]
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
use quai_sdk::consensus::{QuaiTransaction, SignedQuaiTransaction};
use quai_sdk::crypto::{PublicKey, SecretKey};
use quai_sdk::primitives::{Hash32, get_bytes};
use quai_sdk::wallet::account_custody::{
    AccountOperationBook, MAX_ACCOUNT_CUSTODY_BYTES, MAX_ACCOUNT_OPERATIONS, ReservationId,
    ReservationState,
};
use quai_sdk::wallet::discovery::{Checkpoint, NetworkScope};
use quai_sdk::wallet::metadata::StorageError;
use quai_sdk::{U256, Zone};
fn scope() -> NetworkScope {
    NetworkScope {
        chain_id: U256::from(9),
        genesis: Hash32::from_bytes([1; 32]),
        zone: Zone::Cyprus1,
    }
}
fn key() -> SecretKey {
    let mut k = [0; 32];
    k[30] = 3;
    k[31] = 0x25;
    SecretKey::from_bytes(&k).unwrap()
}
fn id(n: u128) -> ReservationId {
    ReservationId(n.to_be_bytes())
}
fn tx(nonce: u64) -> QuaiTransaction {
    QuaiTransaction {
        chain_id: scope().chain_id,
        nonce,
        to: None,
        value: U256::ZERO,
        gas_limit: 21000,
        gas_price: U256::ZERO,
        data: vec![],
        access_list: vec![],
    }
}
fn book() -> AccountOperationBook {
    AccountOperationBook::new(scope(), key().public_key(), 0).unwrap()
}
fn block() -> Checkpoint {
    Checkpoint {
        hash: Hash32::from_bytes([2; 32]),
        height: U256::from(100),
    }
}
fn fixture() -> serde_json::Value {
    serde_json::from_slice(include_bytes!(
        "fixtures/shared/test-infra/fixtures/account-custody.json"
    ))
    .unwrap()
}

#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn pinned_signatures_and_independent_journal_encoding_cover_every_state() {
    let f = fixture();
    assert_eq!(
        key().public_key(),
        PublicKey::from_sec1_bytes(&get_bytes(f["publicKey"].as_str().unwrap()).unwrap()).unwrap()
    );
    for (name, encoded) in f["states"].as_object().unwrap() {
        let bytes = get_bytes(&format!("0x{}", encoded.as_str().unwrap())).unwrap();
        let restored =
            AccountOperationBook::from_state(&bytes, scope(), key().public_key()).unwrap();
        assert_eq!(restored.export_state().unwrap(), bytes);
        let mut expected = book();
        if name != "empty" {
            expected.reserve_nonce(id(1), 0).unwrap();
        }
        if name == "released" {
            expected.release_unsigned(id(1)).unwrap();
        }
        if ["signed", "submitted", "confirmed", "replacement"].contains(&name.as_str()) {
            expected
                .commit_signed(id(1), &tx(0).sign(&key()).unwrap())
                .unwrap();
        }
        if ["submitted", "replacement"].contains(&name.as_str()) {
            expected.mark_submitted(id(1)).unwrap();
        }
        if name == "confirmed" {
            expected
                .observe_inclusion(id(1), tx(0).sign(&key()).unwrap().hash().unwrap(), block())
                .unwrap();
        }
        if name == "replacement" {
            let signed = SignedQuaiTransaction::decode(
                &get_bytes(f["replacement"].as_str().unwrap()).unwrap(),
            )
            .unwrap();
            expected
                .commit_replacement(id(1), tx(0).sign(&key()).unwrap().hash().unwrap(), &signed)
                .unwrap();
        }
        if name != "hashOnly" {
            assert_eq!(expected.export_state().unwrap(), bytes, "{name}");
        }
    }
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn nonce_gaps_signed_candidates_and_reorgs_never_release_custody() {
    let mut b = book();
    assert_eq!(b.reserve_nonce(id(1), 7).unwrap(), 7);
    b.release_unsigned(id(1)).unwrap();
    assert_eq!(b.reserve_nonce(id(2), 0).unwrap(), 8);
    assert_eq!(b.reserve_nonce(id(1), 0), Err(StorageError::Conflict));
    b.reopen_unsigned(id(1)).unwrap();
    let root = tx(7).sign(&key()).unwrap();
    b.commit_signed(id(1), &root).unwrap();
    b.commit_signed(id(1), &root).unwrap();
    assert!(b.release_unsigned(id(1)).is_err());
    assert!(b.reopen_unsigned(id(1)).is_err());
    let mut replacement = tx(7);
    replacement.gas_price = U256::from(1);
    let signed = replacement.sign(&key()).unwrap();
    b.commit_replacement(id(1), root.hash().unwrap(), &signed)
        .unwrap();
    b.commit_replacement(id(1), root.hash().unwrap(), &signed)
        .unwrap();
    assert_eq!(b.operation(id(1)).unwrap().replacements.len(), 1);
    b.mark_submitted(id(1)).unwrap();
    b.observe_inclusion(id(1), signed.hash().unwrap(), block())
        .unwrap();
    assert_eq!(
        b.operation(id(1)).unwrap().transaction,
        Some(root.hash().unwrap())
    );
    assert!(
        b.invalidate_inclusion(id(1), (root.hash().unwrap(), block()))
            .is_err()
    );
    b.invalidate_inclusion(id(1), (signed.hash().unwrap(), block()))
        .unwrap();
    assert_eq!(
        b.operation(id(1)).unwrap().state,
        ReservationState::Submitted
    );
    assert_eq!(
        b.operation(id(1)).unwrap().payload,
        Some(root.signed_bytes().unwrap())
    );
    assert!(b.release_unsigned(id(1)).is_err());
    assert_eq!(b.next_nonce(), 9);
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn invalid_signatures_context_nonce_and_replacement_changes_are_atomic() {
    let mut b = book();
    b.reserve_nonce(id(1), 0).unwrap();
    let before = b.export_state().unwrap();
    for mut wrong in [tx(1), tx(0)] {
        if wrong.nonce == 0 {
            wrong.chain_id = U256::from(15000);
        }
        assert!(
            b.commit_signed(id(1), &wrong.sign(&key()).unwrap())
                .is_err()
        );
        assert_eq!(b.export_state().unwrap(), before);
    }
    let root = tx(0).sign(&key()).unwrap();
    b.commit_signed(id(1), &root).unwrap();
    let before = b.export_state().unwrap();
    for field in 0..5 {
        let mut wrong = tx(0);
        wrong.gas_price = U256::from(1);
        match field {
            0 => wrong.value = U256::from(1),
            1 => wrong.data = vec![1],
            2 => wrong.gas_limit += 1,
            3 => wrong.nonce += 1,
            _ => wrong.gas_price = U256::ZERO,
        };
        assert!(
            b.commit_replacement(id(1), root.hash().unwrap(), &wrong.sign(&key()).unwrap())
                .is_err()
        );
        assert_eq!(b.export_state().unwrap(), before);
    }
    assert!(b.observe_inclusion(id(1), Hash32::ZERO, block()).is_err());
    assert_eq!(b.export_state().unwrap(), before);
    for n in 1..=32 {
        let mut t = tx(0);
        t.gas_price = U256::from(n);
        b.commit_replacement(id(1), root.hash().unwrap(), &t.sign(&key()).unwrap())
            .unwrap();
    }
    let full = b.export_state().unwrap();
    let mut t = tx(0);
    t.gas_price = U256::from(33);
    assert!(
        b.commit_replacement(id(1), root.hash().unwrap(), &t.sign(&key()).unwrap())
            .is_err()
    );
    assert_eq!(b.export_state().unwrap(), full);
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn strict_codec_and_capacity_reject_without_resetting_nonce_or_ids() {
    let mut b = book();
    b.reserve_nonce(id(1), 0).unwrap();
    let bytes = b.export_state().unwrap();
    let mut two = b.clone();
    two.reserve_nonce(id(2), 0).unwrap();
    let mut duplicate_nonce = two.export_state().unwrap();
    duplicate_nonce[164..172].copy_from_slice(&0u64.to_be_bytes());
    assert!(
        AccountOperationBook::from_state(&duplicate_nonce, scope(), key().public_key()).is_err()
    );
    let mut signed = get_bytes(&format!(
        "0x{}",
        fixture()["states"]["signed"].as_str().unwrap()
    ))
    .unwrap();
    let last_signature_byte = signed.len() - 2;
    signed[last_signature_byte] ^= 1;
    assert!(AccountOperationBook::from_state(&signed, scope(), key().public_key()).is_err());
    for n in 0..bytes.len() {
        assert!(
            AccountOperationBook::from_state(&bytes[..n], scope(), key().public_key()).is_err()
        );
    }
    for offset in [
        0,
        8,
        40,
        72,
        73,
        114,
        115,
        116 + 24,
        116 + 25,
        116 + 26,
        116 + 27,
    ] {
        let mut bad = bytes.clone();
        bad[offset] = 255;
        assert!(
            AccountOperationBook::from_state(&bad, scope(), key().public_key()).is_err(),
            "{offset}"
        );
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(AccountOperationBook::from_state(&trailing, scope(), key().public_key()).is_err());
    let mut wrong_scope = scope();
    wrong_scope.genesis = Hash32::from_bytes([3; 32]);
    assert!(AccountOperationBook::from_state(&bytes, wrong_scope, key().public_key()).is_err());
    for n in 2..=MAX_ACCOUNT_OPERATIONS {
        b.reserve_nonce(id(n as u128), 0).unwrap();
        b.release_unsigned(id(n as u128)).unwrap();
    }
    let full = b.export_state().unwrap();
    assert!(b.reserve_nonce(id(300), 0).is_err());
    assert_eq!(b.export_state().unwrap(), full);
    let mut exhausted = AccountOperationBook::new(scope(), key().public_key(), u64::MAX).unwrap();
    assert_eq!(
        exhausted.reserve_nonce(id(1), 0),
        Err(StorageError::Overflow)
    );
    assert!(
        AccountOperationBook::from_state(
            &vec![0; MAX_ACCOUNT_CUSTODY_BYTES + 1],
            scope(),
            key().public_key()
        )
        .is_err()
    );
}
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn aggregate_payload_bound_rejects_atomically_without_losing_existing_claims() {
    let mut b = book();
    for n in 0..16 {
        b.reserve_nonce(id(n + 1), 0).unwrap();
        let mut large = tx(n as u64);
        large.data = vec![0xab; 1_000_000];
        b.commit_signed(id(n + 1), &large.sign(&key()).unwrap())
            .unwrap();
    }
    b.reserve_nonce(id(17), 0).unwrap();
    let before = b.export_state().unwrap();
    assert!(before.len() < MAX_ACCOUNT_CUSTODY_BYTES);
    let mut large = tx(16);
    large.data = vec![0xab; 1_000_000];
    assert!(
        b.commit_signed(id(17), &large.sign(&key()).unwrap())
            .is_err()
    );
    assert_eq!(b.export_state().unwrap(), before);
    assert_eq!(
        b.operation(id(17)).unwrap().state,
        ReservationState::Reserved
    );
    assert_eq!(b.next_nonce(), 17);
    assert_eq!(
        AccountOperationBook::from_state(&before, scope(), key().public_key())
            .unwrap()
            .export_state()
            .unwrap(),
        before
    );
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn authenticated_native_backup_preserves_exact_nonce_and_candidate_family() {
    let f: serde_json::Value = serde_json::from_slice(include_bytes!(
        "fixtures/shared/crates/quai-wallet/tests/full-backup-v4-portable.json"
    ))
    .unwrap();
    let bytes = get_bytes(&format!("0x{}", f["envelope"].as_str().unwrap())).unwrap();
    let backup = quai_sdk::wallet::full_backup::EncryptedWalletBackup::from_bytes(&bytes)
        .unwrap()
        .decrypt(f["password"].as_str().unwrap().as_bytes())
        .unwrap();
    let scope = backup.scopes()[0];
    let state = backup.scope_state(scope).unwrap();
    let address = state
        .addresses()
        .iter()
        .find(|a| quai_sdk::QuaiAddress::try_from(a.address()).is_ok())
        .unwrap();
    let owner = PublicKey::from_sec1_bytes(address.public_key()).unwrap();
    let mut b = AccountOperationBook::from_backup(&backup, scope, owner).unwrap();
    let operation = b.operations().next().unwrap();
    assert_eq!(operation.nonce, 7);
    assert_eq!(operation.replacements.len(), 2);
    assert!(operation.payload.is_some());
    assert_eq!(b.next_nonce(), 8);
    assert_eq!(b.reserve_nonce(id(1), 0).unwrap(), 8);
    assert!(AccountOperationBook::from_backup(&backup, scope, key().public_key()).is_err());
    #[cfg(all(target_arch = "wasm32", feature = "browser"))]
    {
        let store =
            quai_sdk::browser_accounts::BrowserAccountBook::open(&browser::name(), scope, owner)
                .await
                .unwrap();
        store.initialize_from_backup(&backup).await.unwrap();
        assert_eq!(store.snapshot().await.unwrap().book.operations().count(), 1);
        assert!(store.initialize_from_backup(&backup).await.is_err());
    }
}

#[cfg(all(target_arch = "wasm32", feature = "browser"))]
mod browser {
    use super::*;
    use quai_sdk::browser::{BrowserError, BrowserSnapshotStore, BrowserStorageScope};
    use quai_sdk::browser_accounts::{BrowserAccountBook, BrowserAccountError};
    use quai_sdk::signer::LocalSigner;
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn signer_cannot_substitute_a_different_approved_transaction() {
        use quai_sdk::signer::{Signer, SignerError};
        struct Altered(LocalSigner);
        impl Signer for Altered {
            fn address(&self) -> quai_sdk::Address {
                self.0.address()
            }
            fn chain_id(&self) -> U256 {
                self.0.chain_id()
            }
            fn sign_quai(&self, t: &QuaiTransaction) -> Result<SignedQuaiTransaction, SignerError> {
                let mut different = t.clone();
                different.value += U256::from(1);
                self.0.sign_quai(&different)
            }
            fn sign_qi_single(
                &self,
                t: &quai_sdk::consensus::QiTransaction,
            ) -> Result<quai_sdk::consensus::SignedQiTransaction, SignerError> {
                self.0.sign_qi_single(t)
            }
            fn sign_message(
                &self,
                m: &[u8],
            ) -> Result<quai_sdk::crypto::RecoverableSignature, SignerError> {
                self.0.sign_message(m)
            }
        }
        let b = open(&name()).await;
        b.initialize(0).await.unwrap();
        b.reserve_nonce(id(1), 0).await.unwrap();
        let before = b.snapshot().await.unwrap();
        let signer = Altered(LocalSigner::new(key(), scope().chain_id).unwrap());
        assert!(matches!(
            b.sign(id(1), &tx(0), &signer).await,
            Err(BrowserAccountError::Signer(SignerError::InvalidTransaction))
        ));
        let after = b.snapshot().await.unwrap();
        assert_eq!(before.revision, after.revision);
        assert_eq!(
            before.book.export_state().unwrap(),
            after.book.export_state().unwrap()
        );
    }
    pub(super) fn name() -> String {
        let mut b = [0; 8];
        quai_sdk::crypto::fill_random(&mut b).unwrap();
        format!("quai-account-custody-{:x}", u64::from_be_bytes(b))
    }
    async fn raw(name: &str) -> BrowserSnapshotStore {
        BrowserSnapshotStore::open(
            name,
            BrowserStorageScope {
                chain_id: scope().chain_id,
                genesis: scope().genesis,
                zone: scope().zone,
                wallet: AccountOperationBook::account_identity(key().public_key()),
            },
            MAX_ACCOUNT_CUSTODY_BYTES,
        )
        .await
        .unwrap()
    }
    async fn open(name: &str) -> BrowserAccountBook {
        BrowserAccountBook::open(name, scope(), key().public_key())
            .await
            .unwrap()
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn signing_restart_and_revision_fences_preserve_exact_custody() {
        let name = name();
        let b = open(&name).await;
        b.initialize(0).await.unwrap();
        b.reserve_nonce(id(1), 0).await.unwrap();
        let signer = LocalSigner::new(key(), scope().chain_id).unwrap();
        let signed = b.sign(id(1), &tx(0), &signer).await.unwrap();
        let old = b.snapshot().await.unwrap();
        assert_eq!(
            old.book.operation(id(1)).unwrap().payload,
            Some(signed.signed_bytes().unwrap())
        );
        assert!(b.sign(id(1), &tx(0), &signer).await.is_err());
        b.mark_submitted(id(1)).await.unwrap();
        assert!(matches!(
            b.observe_inclusion(old.revision, id(1), signed.hash().unwrap(), block())
                .await,
            Err(BrowserAccountError::State(StorageError::StaleSnapshot))
        ));
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
        let s = b.snapshot().await.unwrap();
        assert_eq!(
            s.book.operation(id(1)).unwrap().state,
            ReservationState::Submitted
        );
        assert!(b.release_unsigned(id(1)).await.is_err());
        assert_eq!(b.reserve_nonce(id(2), 0).await.unwrap(), 1);
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn competing_connections_cannot_allocate_the_same_nonce() {
        use std::{future::Future, task::Poll};
        let name = name();
        let a = open(&name).await;
        a.initialize(12).await.unwrap();
        let b = open(&name).await;
        let mut first = Box::pin(a.reserve_nonce(id(1), 0));
        let mut second = Box::pin(b.reserve_nonce(id(2), 0));
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
        let (loser, error) = match one {
            Ok(_) => (id(2), two.unwrap_err()),
            Err(error) => (id(1), error),
        };
        assert!(matches!(
            error,
            BrowserAccountError::Browser(BrowserError::StorageConflict)
        ));
        assert_eq!(a.reserve_nonce(loser, 0).await.unwrap(), 13);
        let s = b.snapshot().await.unwrap();
        assert_eq!(s.book.operations().count(), 2);
        assert_eq!(s.book.next_nonce(), 14);
        assert!(b.initialize(0).await.is_err());
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn dropped_writes_keep_nonce_and_signature_recoverable() {
        use std::{future::Future, pin::Pin, task::Poll};
        async fn once<F: Future>(mut f: Pin<&mut F>) -> Poll<F::Output> {
            std::future::poll_fn(|cx| Poll::Ready(f.as_mut().poll(cx))).await
        }
        let name = name();
        let b = open(&name).await;
        b.initialize(0).await.unwrap();
        let raw = raw(&name).await;
        let mut reserve = Box::pin(b.reserve_nonce(id(1), 0));
        assert!(once(reserve.as_mut()).await.is_pending());
        raw.read().await.unwrap();
        assert!(once(reserve.as_mut()).await.is_pending());
        raw.read().await.unwrap();
        drop(reserve);
        assert_eq!(b.snapshot().await.unwrap().book.next_nonce(), 1);
        let signer = LocalSigner::new(key(), scope().chain_id).unwrap();
        let transaction = tx(0);
        let mut signing = Box::pin(b.sign(id(1), &transaction, &signer));
        assert!(once(signing.as_mut()).await.is_pending());
        raw.read().await.unwrap();
        assert!(once(signing.as_mut()).await.is_pending());
        raw.read().await.unwrap();
        drop(signing);
        let s = b.snapshot().await.unwrap();
        let op = s.book.operation(id(1)).unwrap();
        assert_eq!(op.state, ReservationState::Signed);
        assert_eq!(
            op.payload,
            Some(transaction.sign(&key()).unwrap().signed_bytes().unwrap())
        );
        assert!(b.release_unsigned(id(1)).await.is_err());
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn corrupt_or_tombstoned_custody_never_becomes_a_fresh_wallet() {
        let name = name();
        let b = open(&name).await;
        b.initialize(50).await.unwrap();
        let raw = raw(&name).await;
        let s = raw.read().await.unwrap().unwrap();
        let mut bytes = s.bytes.unwrap();
        bytes[73] ^= 1;
        let rev = raw
            .compare_exchange(Some(s.revision), Some(&bytes))
            .await
            .unwrap();
        assert!(b.snapshot().await.is_err());
        assert!(b.initialize(0).await.is_err());
        raw.compare_exchange(Some(rev), None).await.unwrap();
        assert!(matches!(
            b.snapshot().await,
            Err(BrowserAccountError::Uninitialized)
        ));
        assert!(b.initialize(0).await.is_err());
    }
}
