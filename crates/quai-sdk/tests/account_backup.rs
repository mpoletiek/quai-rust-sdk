//! Portable account-only backup capture and conservative live custody recovery.
#![cfg(feature = "backup")]
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
use quai_sdk::consensus::QuaiTransaction;
use quai_sdk::crypto::{PublicKey, SecretKey};
use quai_sdk::primitives::Hash32;
use quai_sdk::wallet::account_custody::{AccountOperationBook, ReservationId, ReservationState};
use quai_sdk::wallet::discovery::{Checkpoint, NetworkScope};
use quai_sdk::wallet::full_backup::{BackupOrigin, WalletBackup};
use quai_sdk::wallet::metadata::PublicAddress;
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
fn tx(nonce: u64, fee: u64) -> QuaiTransaction {
    QuaiTransaction {
        chain_id: scope().chain_id,
        nonce,
        to: None,
        value: U256::ZERO,
        gas_limit: 21000,
        gas_price: U256::from(fee),
        data: vec![],
        access_list: vec![],
    }
}
fn book() -> AccountOperationBook {
    AccountOperationBook::new(scope(), key().public_key(), 0).unwrap()
}
fn capture(b: &AccountOperationBook) -> WalletBackup {
    WalletBackup::capture_account_custody(
        b,
        PublicAddress::imported(&key().public_key()).unwrap(),
        vec![BackupOrigin::from_private_key(&key())],
    )
    .unwrap()
}
fn signed(b: &mut AccountOperationBook, n: u128, nonce: u64) -> Hash32 {
    b.reserve_nonce(id(n), nonce).unwrap();
    let t = tx(nonce, 0).sign(&key()).unwrap();
    b.commit_signed(id(n), &t).unwrap();
    t.hash().unwrap()
}
fn block() -> Checkpoint {
    Checkpoint {
        hash: Hash32::from_bytes([2; 32]),
        height: U256::from(100),
    }
}

#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn merge_unions_custody_and_candidates_without_rewinding_live_nonce_floor() {
    let mut source = book();
    let root = signed(&mut source, 1, 0);
    let mut live =
        AccountOperationBook::from_backup(&capture(&source), scope(), key().public_key()).unwrap();
    source
        .commit_replacement(id(1), root, &tx(0, 1).sign(&key()).unwrap())
        .unwrap();
    source.reserve_nonce(id(3), 10).unwrap();
    live.commit_replacement(id(1), root, &tx(0, 2).sign(&key()).unwrap())
        .unwrap();
    let second = signed(&mut live, 2, 1);
    live.observe_inclusion(id(1), root, block()).unwrap();
    live.observe_inclusion(id(2), second, block()).unwrap();
    let report = live.merge_backup(&capture(&source)).unwrap();
    assert_eq!(report.operations_added, 1);
    assert_eq!(report.candidates_added, 1);
    assert_eq!(report.invalidated_inclusions, 2);
    assert_eq!(report.next_nonce, 11);
    assert_eq!(live.operations().count(), 3);
    assert_eq!(live.operation(id(1)).unwrap().replacements.len(), 2);
    assert_eq!(
        live.operation(id(1)).unwrap().state,
        ReservationState::Submitted
    );
    assert_eq!(
        live.operation(id(2)).unwrap().state,
        ReservationState::Submitted
    );
    let before = live.export_state().unwrap();
    let report = live.merge_backup(&capture(&source)).unwrap();
    assert_eq!(report.operations_added, 0);
    assert_eq!(report.candidates_added, 0);
    assert_eq!(report.invalidated_inclusions, 0);
    assert_eq!(live.export_state().unwrap(), before);
    assert_eq!(live.reserve_nonce(id(4), 0).unwrap(), 11);
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn stale_unsigned_backup_cannot_release_or_reopen_live_operations() {
    let mut source = book();
    source.reserve_nonce(id(1), 0).unwrap();
    let reserved = capture(&source);
    source.release_unsigned(id(1)).unwrap();
    let released = capture(&source);
    let mut live = book();
    live.reserve_nonce(id(1), 0).unwrap();
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
    source.reopen_unsigned(id(1)).unwrap();
    source
        .commit_signed(id(1), &tx(0, 0).sign(&key()).unwrap())
        .unwrap();
    live.merge_backup(&capture(&source)).unwrap();
    assert_eq!(
        live.operation(id(1)).unwrap().state,
        ReservationState::Signed
    );
    live.merge_backup(&released).unwrap();
    assert_eq!(
        live.operation(id(1)).unwrap().state,
        ReservationState::Signed
    );
    assert!(live.release_unsigned(id(1)).is_err());
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn conflicting_roots_ids_nonce_claims_and_union_bounds_reject_atomically() {
    let mut live = book();
    signed(&mut live, 1, 0);
    let before = live.export_state().unwrap();
    for case in 0..3 {
        let mut source = book();
        match case {
            0 => {
                source.reserve_nonce(id(1), 1).unwrap();
            }
            1 => {
                source.reserve_nonce(id(2), 0).unwrap();
            }
            _ => {
                source.reserve_nonce(id(1), 0).unwrap();
                let mut different = tx(0, 0);
                different.value = U256::from(1);
                source
                    .commit_signed(id(1), &different.sign(&key()).unwrap())
                    .unwrap();
            }
        }
        assert!(live.merge_backup(&capture(&source)).is_err());
        assert_eq!(live.export_state().unwrap(), before);
    }
    let mut full = book();
    for n in 0..256 {
        full.reserve_nonce(id(n + 1), n as u64).unwrap();
    }
    let before = full.export_state().unwrap();
    let mut extra = book();
    extra.reserve_nonce(id(500), 500).unwrap();
    assert!(full.merge_backup(&capture(&extra)).is_err());
    assert_eq!(full.export_state().unwrap(), before);
    let mut source = book();
    let root = signed(&mut source, 1, 0);
    for fee in 1..=32 {
        source
            .commit_replacement(id(1), root, &tx(0, fee).sign(&key()).unwrap())
            .unwrap();
    }
    live.commit_replacement(id(1), root, &tx(0, 33).sign(&key()).unwrap())
        .unwrap();
    let before = live.export_state().unwrap();
    assert!(live.merge_backup(&capture(&source)).is_err());
    assert_eq!(live.export_state().unwrap(), before);
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn capture_proves_private_ownership_and_preserves_explicit_hd_metadata() {
    let b = book();
    let metadata = PublicAddress::imported(&key().public_key()).unwrap();
    assert!(WalletBackup::capture_account_custody(&b, metadata.clone(), vec![]).is_err());
    assert!(
        WalletBackup::capture_account_custody(
            &b,
            metadata,
            vec![BackupOrigin::from_seed(&[7; 32]).unwrap()]
        )
        .is_err()
    );
    let origin = BackupOrigin::from_seed(&[7; 32]).unwrap();
    let public = origin
        .account_public(quai_sdk::wallet::CoinType::Quai, 0)
        .unwrap();
    let found = public
        .search(
            false,
            quai_sdk::wallet::Search {
                zone: Zone::Cyprus1,
                start_index: 0,
                max_attempts: 10_000,
            },
            || false,
        )
        .unwrap();
    let metadata = PublicAddress::derive(&public, false, found.address.index).unwrap();
    let owner = PublicKey::from_sec1_bytes(metadata.public_key()).unwrap();
    let b = AccountOperationBook::new(scope(), owner, 40).unwrap();
    let backup = WalletBackup::capture_account_custody(&b, metadata.clone(), vec![origin]).unwrap();
    assert_eq!(
        backup.scope_state(scope()).unwrap().addresses(),
        std::slice::from_ref(&metadata)
    );
    assert_eq!(
        backup
            .scope_state(scope())
            .unwrap()
            .derivation_cursors()
            .count(),
        0
    );
    assert_eq!(
        AccountOperationBook::from_backup(&backup, scope(), owner)
            .unwrap()
            .next_nonce(),
        40
    );
    assert!(
        WalletBackup::capture_account_custody(
            &book(),
            metadata,
            vec![BackupOrigin::from_private_key(&key())]
        )
        .is_err()
    );
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn portable_capture_encrypts_and_restores_candidates_without_stale_inclusion() {
    let mut b = book();
    let root = signed(&mut b, 1, 0);
    b.commit_replacement(id(1), root, &tx(0, 1).sign(&key()).unwrap())
        .unwrap();
    b.observe_inclusion(id(1), root, block()).unwrap();
    let backup = capture(&b);
    let envelope = backup
        .encrypt(
            b"public toy backup password",
            quai_sdk::wallet::BackupKdf::default(),
        )
        .unwrap();
    let decoded = envelope.decrypt(b"public toy backup password").unwrap();
    let restored =
        AccountOperationBook::from_backup(&decoded, scope(), key().public_key()).unwrap();
    assert_eq!(
        restored.operation(id(1)).unwrap().state,
        ReservationState::Submitted
    );
    assert!(restored.operation(id(1)).unwrap().inclusion.is_none());
    assert_eq!(
        restored.operation(id(1)).unwrap().payload,
        b.operation(id(1)).unwrap().payload
    );
    assert_eq!(
        restored.operation(id(1)).unwrap().replacements,
        b.operation(id(1)).unwrap().replacements
    );
    #[cfg(all(not(target_arch = "wasm32"), feature = "sqlite"))]
    {
        let mut random = [0; 16];
        quai_sdk::crypto::fill_random(&mut random).unwrap();
        let directory = std::env::temp_dir().join(format!(
            "quai-account-backup-{:x}",
            u128::from_be_bytes(random)
        ));
        std::fs::create_dir(&directory).unwrap();
        let mut store =
            quai_sdk::wallet::storage::SqliteStore::open(directory.join("wallet.sqlite"), scope())
                .unwrap();
        let report = decoded.restore(&mut store).unwrap();
        assert_eq!(report.retained_signed_operations, 1);
        assert_eq!(
            store.reserved_nonce(id(1)).unwrap(),
            Some((restored.address(), 0))
        );
        assert_eq!(
            store.signed_payload(id(1)).unwrap(),
            restored.operation(id(1)).unwrap().payload
        );
        assert_eq!(
            store.replacement_candidates(id(1)).unwrap(),
            restored.operation(id(1)).unwrap().replacements
        );
        assert!(store.release_unsigned(id(1)).is_err());
        assert_eq!(
            store.reserve_nonce(id(2), restored.address(), 0).unwrap(),
            1
        );
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn hash_only_custody_can_gain_matching_bytes_without_releasing_the_claim() {
    let f: serde_json::Value = serde_json::from_slice(include_bytes!(
        "fixtures/shared/test-infra/fixtures/account-custody.json"
    ))
    .unwrap();
    let bytes = quai_sdk::primitives::get_bytes(&format!(
        "0x{}",
        f["states"]["hashOnly"].as_str().unwrap()
    ))
    .unwrap();
    let mut live = AccountOperationBook::from_state(&bytes, scope(), key().public_key()).unwrap();
    let hash_only = capture(&live);
    let mut source = book();
    signed(&mut source, 1, 0);
    live.merge_backup(&capture(&source)).unwrap();
    assert!(live.operation(id(1)).unwrap().payload.is_some());
    assert!(live.release_unsigned(id(1)).is_err());
    let before = live.export_state().unwrap();
    live.merge_backup(&hash_only).unwrap();
    assert_eq!(live.export_state().unwrap(), before);
}

#[cfg(all(target_arch = "wasm32", feature = "browser"))]
mod browser {
    use super::*;
    use quai_sdk::browser_accounts::{BrowserAccountBook, BrowserAccountError};
    fn name() -> String {
        let mut b = [0; 8];
        quai_sdk::crypto::fill_random(&mut b).unwrap();
        format!("quai-account-backup-{:x}", u64::from_be_bytes(b))
    }
    async fn open(name: &str) -> BrowserAccountBook {
        BrowserAccountBook::open(name, scope(), key().public_key())
            .await
            .unwrap()
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn existing_journal_merges_backup_and_rejects_stale_observation_revision() {
        let name = name();
        let b = open(&name).await;
        b.initialize(0).await.unwrap();
        b.reserve_nonce(id(1), 0).await.unwrap();
        let root = tx(0, 0).sign(&key()).unwrap();
        b.commit_signed(id(1), &root).await.unwrap();
        let rev = b.snapshot().await.unwrap().revision;
        let mut source = book();
        signed(&mut source, 1, 0);
        source
            .commit_replacement(id(1), root.hash().unwrap(), &tx(0, 1).sign(&key()).unwrap())
            .unwrap();
        source.reserve_nonce(id(2), 50).unwrap();
        let report = b.merge_backup(&capture(&source)).await.unwrap();
        assert_eq!(report.next_nonce, 51);
        assert_eq!(report.operations_added, 1);
        assert!(
            b.observe_inclusion(rev, id(1), root.hash().unwrap(), block())
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
                .replacements
                .len(),
            1
        );
        assert_eq!(b.reserve_nonce(id(3), 0).await.unwrap(), 51);
        let snapshot = b.snapshot().await.unwrap();
        let exported = capture(&snapshot.book);
        let copy = open(&self::name()).await;
        copy.initialize_from_backup(&exported).await.unwrap();
        assert_eq!(
            copy.snapshot().await.unwrap().book.export_state().unwrap(),
            snapshot.book.export_state().unwrap()
        );
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn concurrent_restore_and_allocation_conflict_without_losing_either_claim() {
        use std::{future::Future, task::Poll};
        let name = name();
        let a = open(&name).await;
        a.initialize(0).await.unwrap();
        let b = open(&name).await;
        let mut source = book();
        source.reserve_nonce(id(99), 50).unwrap();
        let backup = capture(&source);
        let mut merging = Box::pin(a.merge_backup(&backup));
        let mut allocating = Box::pin(b.reserve_nonce(id(1), 0));
        let mut merge = None;
        let mut allocation = None;
        let (merge, allocation) = std::future::poll_fn(|cx| {
            if merge.is_none()
                && let Poll::Ready(v) = merging.as_mut().poll(cx)
            {
                merge = Some(v);
            }
            if allocation.is_none()
                && let Poll::Ready(v) = allocating.as_mut().poll(cx)
            {
                allocation = Some(v);
            }
            if merge.is_some() && allocation.is_some() {
                Poll::Ready((merge.take().unwrap(), allocation.take().unwrap()))
            } else {
                Poll::Pending
            }
        })
        .await;
        match (merge, allocation) {
            (
                Ok(_),
                Err(BrowserAccountError::Browser(quai_sdk::browser::BrowserError::StorageConflict)),
            ) => {
                assert_eq!(b.reserve_nonce(id(1), 0).await.unwrap(), 51);
            }
            (
                Err(BrowserAccountError::Browser(quai_sdk::browser::BrowserError::StorageConflict)),
                Ok(0),
            ) => {
                a.merge_backup(&backup).await.unwrap();
            }
            _ => panic!("exactly one writer must win"),
        }
        let snapshot = a.snapshot().await.unwrap();
        assert_eq!(snapshot.book.operations().count(), 2);
        assert!(snapshot.book.operation(id(99)).is_some());
        assert!(snapshot.book.operation(id(1)).is_some());
    }
}

/// Seeded, deterministic, dependency-free pseudo-random source.
struct Lcg(u64);
impl Lcg {
    fn below(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % n
    }
}

/// Random custody activity on one side: new reservations in the side's own ID
/// and nonce ranges, then signing, fee replacements, submission, release and
/// inclusion on any operation. Invalid transitions are simply refused.
fn diverge(book: &mut AccountOperationBook, side: u128, rng: &mut Lcg) {
    for step in 0..rng.below(12) as u128 {
        let ids: Vec<_> = book.operations().map(|op| (op.id, op.nonce)).collect();
        // Only called when `ids` is nonempty.
        let pick = |rng: &mut Lcg| ids[rng.below(ids.len() as u64) as usize];
        match rng.below(6) {
            0 | 1 => {
                let _ = book.reserve_nonce(id(side + step), side as u64 / 100 + step as u64);
            }
            2 if !ids.is_empty() => {
                let (op, nonce) = pick(rng);
                let _ = book.commit_signed(op, &tx(nonce, 0).sign(&key()).unwrap());
            }
            3 if !ids.is_empty() => {
                let (op, nonce) = pick(rng);
                let root = tx(nonce, 0).sign(&key()).unwrap().hash().unwrap();
                let fee = side as u64 / 1000 * 10 + 1 + rng.below(5);
                let _ = book.commit_replacement(op, root, &tx(nonce, fee).sign(&key()).unwrap());
            }
            4 if !ids.is_empty() => {
                let (op, _) = pick(rng);
                let _ = book.mark_submitted(op);
            }
            5 if !ids.is_empty() => {
                let (op, nonce) = pick(rng);
                let _ = book.release_unsigned(op);
                let root = tx(nonce, 0).sign(&key()).unwrap().hash().unwrap();
                let _ = book.observe_inclusion(op, root, block());
            }
            _ => {}
        }
    }
}

fn merged(live: &AccountOperationBook, backup: &AccountOperationBook) -> AccountOperationBook {
    let mut out = live.clone();
    out.merge_backup(&capture(backup)).unwrap();
    out
}

#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn merge_keeps_all_custody_is_idempotent_and_order_independent_for_signed_evidence() {
    use std::collections::BTreeSet;
    let signed_rank = |state: ReservationState| {
        matches!(
            state,
            ReservationState::Signed | ReservationState::Submitted | ReservationState::Confirmed
        )
    };
    let mut rng = Lcg(0x9e37_79b9_7f4a_7c15);
    // Guard against a vacuous pass: the random activity must reach these.
    let (mut submitted, mut replaced, mut signed_ops) = (0, 0, 0);
    for round in 0..40 {
        let mut base = book();
        for n in 0..rng.below(3) as u128 {
            signed(&mut base, 100 + n, n as u64);
        }
        let from = |b: &AccountOperationBook| {
            AccountOperationBook::from_backup(&capture(b), scope(), key().public_key()).unwrap()
        };
        let (mut a, mut b) = (from(&base), from(&base));
        diverge(&mut a, 1_000, &mut rng);
        diverge(&mut b, 5_000, &mut rng);

        let ab = merged(&a, &b);
        let ba = merged(&b, &a);
        assert_eq!(
            ab.next_nonce(),
            a.next_nonce().max(b.next_nonce()),
            "round {round}"
        );
        for (side, other) in [(&a, &b), (&b, &a)] {
            for op in side.operations() {
                let m = ab.operation(op.id).expect("no operation is dropped");
                assert_eq!(m.nonce, op.nonce);
                assert!(
                    m.inclusion.is_none(),
                    "inclusions are re-observed after merge"
                );
                let other_op = other.operation(op.id);
                let transaction = op.transaction.or(other_op.and_then(|o| o.transaction));
                assert_eq!(m.transaction, transaction, "round {round}");
                assert!(op.payload.is_none() || m.payload == op.payload);
                let edges: BTreeSet<_> = m.replacements.iter().map(|e| format!("{e:?}")).collect();
                for edge in &op.replacements {
                    assert!(
                        edges.contains(&format!("{edge:?}")),
                        "round {round}: edge dropped"
                    );
                }
                // Signed evidence on either side is never lost or downgraded.
                if transaction.is_some() {
                    assert!(signed_rank(m.state), "round {round}: {:?}", m.state);
                    assert_ne!(m.state, ReservationState::Confirmed);
                }
                if matches!(
                    op.state,
                    ReservationState::Submitted | ReservationState::Confirmed
                ) {
                    assert_eq!(m.state, ReservationState::Submitted, "round {round}");
                }
            }
        }
        // Order-independent wherever the outcome is decided by signed evidence
        // or the operation exists on one side; an unsigned state shared by
        // both deliberately follows the live book.
        for op in ab.operations() {
            let shared_unsigned = a.operation(op.id).is_some()
                && b.operation(op.id).is_some()
                && op.transaction.is_none();
            if !shared_unsigned {
                let other = ba.operation(op.id).unwrap();
                assert_eq!(
                    (op.nonce, op.state, op.transaction, &op.payload),
                    (other.nonce, other.state, other.transaction, &other.payload),
                    "round {round}"
                );
                let set = |o: &quai_sdk::wallet::account_custody::AccountOperation| {
                    o.replacements
                        .iter()
                        .map(|e| format!("{e:?}"))
                        .collect::<BTreeSet<_>>()
                };
                assert_eq!(set(op), set(other), "round {round}");
            }
        }
        assert_eq!(ab.operations().count(), ba.operations().count());
        for op in ab.operations() {
            submitted += usize::from(op.state == ReservationState::Submitted);
            replaced += op.replacements.len();
            signed_ops += usize::from(op.transaction.is_some());
        }
        // Idempotent: merging the same backup again changes nothing.
        let again = merged(&ab, &b);
        assert_eq!(
            again.export_state().unwrap(),
            ab.export_state().unwrap(),
            "round {round}"
        );
    }
    assert!(
        submitted > 0 && replaced > 0 && signed_ops > 0,
        "{submitted} {replaced} {signed_ops}"
    );
}
