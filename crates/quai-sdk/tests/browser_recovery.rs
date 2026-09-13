//! Portable family observations and actual IndexedDB recovery/submission races.
#![cfg(feature = "backup")]
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
use quai_sdk::candidate_observation::{
    CandidateObservation, FamilyObservationError, observe_signed_candidates,
};
use quai_sdk::consensus::{QuaiTransaction, SignedQiOperation, SignedQuaiTransaction};
use quai_sdk::crypto::SecretKey;
use quai_sdk::primitives::{Hash32, get_bytes};
use quai_sdk::rpc::{RpcError, Transport};
use quai_sdk::wallet::discovery::NetworkScope;
use quai_sdk::{Endpoint, Provider, Routing, U256, Zone};
use serde_json::{Value, json};
use std::collections::BTreeMap;
#[cfg(target_arch = "wasm32")]
type Shared = std::rc::Rc<std::cell::RefCell<State>>;
#[cfg(not(target_arch = "wasm32"))]
type Shared = std::sync::Arc<std::sync::Mutex<State>>;
struct State {
    chain: U256,
    mode: u8,
    receipts: BTreeMap<String, Value>,
    calls: usize,
    sends: Vec<String>,
    stall: bool,
}
impl Default for State {
    fn default() -> Self {
        Self {
            chain: U256::from(9),
            mode: 0,
            receipts: BTreeMap::new(),
            calls: 0,
            sends: vec![],
            stall: false,
        }
    }
}
#[derive(Clone, Default)]
struct Mock(Shared);
impl Mock {
    fn state(&self) -> impl std::ops::DerefMut<Target = State> {
        #[cfg(target_arch = "wasm32")]
        {
            self.0.borrow_mut()
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.0.lock().unwrap()
        }
    }
    fn provider(&self) -> Provider<Self> {
        Provider::new(
            self.clone(),
            Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
            self.state().chain,
        )
    }
}
fn hash(n: u8) -> Hash32 {
    Hash32::from_bytes([n; 32])
}
fn scope() -> NetworkScope {
    NetworkScope {
        chain_id: U256::from(9),
        genesis: hash(1),
        zone: Zone::Cyprus1,
    }
}
fn key() -> SecretKey {
    let mut b = [0; 32];
    b[30..].copy_from_slice(&805u16.to_be_bytes());
    SecretKey::from_bytes(&b).unwrap()
}
fn signed(price: u64) -> SignedQuaiTransaction {
    QuaiTransaction {
        chain_id: scope().chain_id,
        nonce: 0,
        to: Some(key().public_key().address()),
        value: U256::from(7),
        gas_limit: 21000,
        gas_price: U256::from(price),
        data: vec![],
        access_list: vec![],
    }
    .sign(&key())
    .unwrap()
}
fn receipt(hash: Hash32, kind: u8) -> Value {
    json!({"transactionHash":hash.to_string(),"type":format!("0x{kind:x}"),"from":if kind==0 {Some(key().public_key().address().to_string())} else {None},"to":if kind==0 {Some(key().public_key().address().to_string())} else {None},"blockHash":crate::hash(2).to_string(),"blockNumber":"0x10","transactionIndex":"0x0","status":"0x0","gasUsed":"0x5208","cumulativeGasUsed":"0x5208","effectiveGasPrice":"0x1","logsBloom":format!("0x{}","00".repeat(10240)),"logs":[]})
}
fn qi_vectors() -> Vec<Value> {
    serde_json::from_slice::<Value>(include_bytes!(
        "fixtures/shared/test-infra/fixtures/qi-custody.json"
    ))
    .unwrap()["vectors"]
        .as_array()
        .unwrap()
        .clone()
}
impl Transport for Mock {
    async fn request(&self, _: &Endpoint, method: &str, params: Value) -> Result<Value, RpcError> {
        {
            self.state().calls += 1;
        }
        if method == "quai_getTransactionReceipt" {
            std::future::poll_fn(|_| {
                if self.state().stall {
                    std::task::Poll::Pending
                } else {
                    std::task::Poll::Ready(())
                }
            })
            .await;
        }
        let mut s = self.state();
        Ok(match method {
            "quai_chainId" => json!(format!("{:#x}", s.chain)),
            "quai_getHeaderByNumber" => {
                let (n, h) = match params[0].as_str().unwrap() {
                    "0x0" => ("0x0", hash(if s.mode == 3 { 9 } else { 1 })),
                    "0x10" => ("0x10", hash(if s.mode == 1 { 9 } else { 2 })),
                    "latest" => ("0x11", hash(3)),
                    "0x11" => ("0x11", hash(if s.mode == 2 { 9 } else { 3 })),
                    _ => panic!("unexpected height"),
                };
                json!({"gasLimit":"0x100000","stateLimit":"0x100000","woHeader":{"hash":h.to_string(),"number":n,"location":if n=="0x0" {"0x"} else {"0x0000"},"parentHash":if n=="0x0" {Hash32::ZERO.to_string()} else {hash(1).to_string()},"primeTerminusNumber":"0x10"}})
            }
            "quai_getTransactionReceipt" => s
                .receipts
                .get(params[0].as_str().unwrap())
                .cloned()
                .unwrap_or(Value::Null),
            "quai_getTransactionByHash" => Value::Null,
            "quai_sendRawTransaction" => {
                let wire = params[0].as_str().unwrap().to_owned();
                s.sends.push(wire.clone());
                if s.mode == 4 {
                    return Err(RpcError::Timeout);
                }
                let bytes = get_bytes(&wire).unwrap();
                let hash = if let Ok(tx) = SignedQuaiTransaction::decode(&bytes) {
                    tx.hash().unwrap()
                } else {
                    SignedQiOperation::decode(&bytes).unwrap().hash().unwrap()
                };
                json!(hash.to_string())
            }
            _ => panic!("unexpected RPC {method}"),
        })
    }
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn portable_observation_binds_receipts_and_rejects_duplicate_or_competing_candidates() {
    let root = signed(2);
    let replacement = signed(3);
    let payloads = vec![
        root.signed_bytes().unwrap(),
        replacement.signed_bytes().unwrap(),
    ];
    let m = Mock::default();
    let rh = replacement.hash().unwrap();
    m.state().receipts.insert(rh.to_string(), receipt(rh, 0));
    let view = observe_signed_candidates(&m.provider(), scope(), &payloads)
        .await
        .unwrap();
    assert_eq!(view.canonical, Some(rh));
    assert!(matches!(
        view.candidates[1].1,
        CandidateObservation::Included {
            confirmations: 2,
            outcome: quai_sdk::provider::ReceiptOutcome::Failed,
            ..
        }
    ));
    assert!(matches!(
        view.candidates[0].1,
        CandidateObservation::NotObserved
    ));
    m.state().receipts.insert(
        root.hash().unwrap().to_string(),
        receipt(root.hash().unwrap(), 0),
    );
    assert!(matches!(
        observe_signed_candidates(&m.provider(), scope(), &payloads).await,
        Err(FamilyObservationError::Changed)
    ));
    m.state().receipts.remove(&root.hash().unwrap().to_string());
    for mode in [1, 2, 3] {
        m.state().mode = mode;
        let result = observe_signed_candidates(&m.provider(), scope(), &payloads).await;
        if mode == 1 {
            assert!(matches!(
                result.unwrap().candidates[1].1,
                CandidateObservation::Noncanonical
            ));
        } else {
            assert!(result.is_err());
        }
    }
    let fresh = Mock::default();
    assert!(
        observe_signed_candidates(
            &fresh.provider(),
            scope(),
            &[payloads[0].clone(), payloads[0].clone()]
        )
        .await
        .is_err()
    );
    assert_eq!(fresh.state().calls, 0);
    m.state().mode = 0;
    for bad in [receipt(rh, 2), {
        let mut v = receipt(rh, 0);
        v["from"] = json!("0x0000000000000000000000000000000000000001");
        v
    }] {
        m.state().receipts.insert(rh.to_string(), bad);
        assert!(matches!(
            observe_signed_candidates(&m.provider(), scope(), &payloads).await,
            Err(FamilyObservationError::Invalid)
        ));
    }
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn portable_observation_accepts_each_qi_wire_form_and_rejects_wrong_receipt_ledger() {
    for row in qi_vectors() {
        let bytes = get_bytes(row["signed"].as_str().unwrap()).unwrap();
        let tx = SignedQiOperation::decode(&bytes).unwrap();
        let h = tx.hash().unwrap();
        let m = Mock::default();
        m.state().chain = tx.transaction().chain_id;
        let mut scope = scope();
        scope.chain_id = tx.transaction().chain_id;
        m.state().receipts.insert(h.to_string(), receipt(h, 2));
        assert_eq!(
            observe_signed_candidates(&m.provider(), scope, std::slice::from_ref(&bytes))
                .await
                .unwrap()
                .canonical,
            Some(h)
        );
        m.state().receipts.insert(h.to_string(), receipt(h, 0));
        assert!(matches!(
            observe_signed_candidates(&m.provider(), scope, &[bytes]).await,
            Err(FamilyObservationError::Invalid)
        ));
    }
}
#[cfg(all(target_arch = "wasm32", feature = "browser"))]
mod browser {
    use super::*;
    use quai_sdk::browser_accounts::BrowserAccountBook;
    use quai_sdk::browser_qi::BrowserQiBook;
    use quai_sdk::browser_recovery::{BrowserRecoveryError, BrowserRecoverySession};
    use quai_sdk::wallet::account_custody::{ReservationId, ReservationState};
    use quai_sdk::wallet::discovery::Checkpoint;
    fn id() -> ReservationId {
        ReservationId([1; 16])
    }
    fn name() -> String {
        let mut b = [0; 16];
        quai_sdk::crypto::fill_random(&mut b).unwrap();
        format!("family-{:x}", u128::from_be_bytes(b))
    }
    async fn account(name: &str) -> BrowserAccountBook {
        let b = BrowserAccountBook::open(name, scope(), key().public_key())
            .await
            .unwrap();
        b.initialize(0).await.unwrap();
        b.reserve_nonce(id(), 0).await.unwrap();
        b.commit_signed(id(), &signed(2)).await.unwrap();
        b.commit_replacement(id(), signed(2).hash().unwrap(), &signed(3))
            .await
            .unwrap();
        b
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn account_candidate_replay_winner_missing_receipt_and_reorg_keep_signed_nonce() {
        let m = Mock::default();
        let p = m.provider();
        let n = name();
        let b = account(&n).await;
        let s = BrowserRecoverySession::for_account(&p, &b);
        let selected = signed(3).hash().unwrap();
        assert!(s.broadcast_candidate(id(), hash(7)).await.is_err());
        assert!(m.state().sends.is_empty());
        m.state().mode = 4;
        assert!(matches!(
            s.broadcast_candidate(id(), selected).await,
            Err(BrowserRecoveryError::Broadcast(
                quai_sdk::provider::BroadcastError::Ambiguous { .. }
            ))
        ));
        let reopened = BrowserAccountBook::open(&n, scope(), key().public_key())
            .await
            .unwrap();
        let s = BrowserRecoverySession::for_account(&p, &reopened);
        m.state().mode = 0;
        s.broadcast_candidate(id(), selected).await.unwrap();
        {
            let state = m.state();
            assert_eq!(state.sends.len(), 2);
            assert_eq!(state.sends[0], state.sends[1]);
        }
        m.state()
            .receipts
            .insert(selected.to_string(), receipt(selected, 0));
        assert_eq!(
            s.reconcile(id()).await.unwrap().inclusion.unwrap().0,
            selected
        );
        assert_eq!(
            reopened
                .snapshot()
                .await
                .unwrap()
                .book
                .operation(id())
                .unwrap()
                .state,
            ReservationState::Confirmed
        );
        m.state().receipts.clear();
        assert_eq!(
            s.reconcile(id()).await.unwrap().inclusion.unwrap().0,
            selected
        );
        assert!(s.broadcast_root(id()).await.is_err());
        m.state().mode = 1;
        assert!(s.reconcile(id()).await.unwrap().inclusion.is_none());
        let snapshot = reopened.snapshot().await.unwrap();
        assert_eq!(
            snapshot.book.operation(id()).unwrap().state,
            ReservationState::Submitted
        );
        assert_eq!(snapshot.book.operation(id()).unwrap().replacements.len(), 1);
        assert_eq!(snapshot.book.next_nonce(), 1);
        assert!(reopened.release_unsigned(id()).await.is_err());
        assert_eq!(s.signed_candidates(id()).await.unwrap().len(), 2);
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn account_reconciliation_race_and_cancellation_do_not_overwrite_new_custody() {
        use std::{future::Future, pin::Pin, task::Poll};
        async fn once<F: Future>(mut f: Pin<&mut F>) -> Poll<F::Output> {
            std::future::poll_fn(|cx| Poll::Ready(f.as_mut().poll(cx))).await
        }
        let m = Mock::default();
        let p = m.provider();
        let b = account(&name()).await;
        let h = signed(3).hash().unwrap();
        m.state().receipts.insert(h.to_string(), receipt(h, 0));
        let s = BrowserRecoverySession::for_account(&p, &b);
        m.state().stall = true;
        let mut f = Box::pin(s.reconcile(id()));
        assert!(once(f.as_mut()).await.is_pending());
        b.snapshot().await.unwrap();
        assert!(once(f.as_mut()).await.is_pending());
        b.mark_submitted(id()).await.unwrap();
        m.state().stall = false;
        assert!(matches!(
            f.await,
            Err(BrowserRecoveryError::Browser(
                quai_sdk::browser::BrowserError::StorageConflict
            ))
        ));
        let before = b.snapshot().await.unwrap().revision;
        m.state().stall = true;
        let mut f = Box::pin(s.reconcile(id()));
        assert!(once(f.as_mut()).await.is_pending());
        b.snapshot().await.unwrap();
        assert!(once(f.as_mut()).await.is_pending());
        drop(f);
        assert_eq!(b.snapshot().await.unwrap().revision, before);
        assert!(
            b.snapshot()
                .await
                .unwrap()
                .book
                .operation(id())
                .unwrap()
                .inclusion
                .is_none()
        );
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn qi_all_wire_forms_broadcast_and_reconcile_without_unlocking_keys() {
        use quai_sdk::wallet::{CandidateCoin, metadata::PublicAddress};
        for row in qi_vectors() {
            let tx =
                SignedQiOperation::decode(&get_bytes(row["signed"].as_str().unwrap()).unwrap())
                    .unwrap();
            let m = Mock::default();
            m.state().chain = tx.transaction().chain_id;
            let p = m.provider();
            let mut scope = scope();
            scope.chain_id = tx.transaction().chain_id;
            let b = BrowserQiBook::open(&name(), scope, hash(7)).await.unwrap();
            b.initialize().await.unwrap();
            let owners: BTreeMap<_, _> = tx
                .transaction()
                .inputs
                .iter()
                .map(|i| {
                    (
                        i.public_key.address(),
                        PublicAddress::imported(&i.public_key).unwrap(),
                    )
                })
                .collect();
            let coins: Vec<_> = tx
                .transaction()
                .inputs
                .iter()
                .map(|i| CandidateCoin {
                    outpoint: i.previous_output,
                    address: i.public_key.address().try_into().unwrap(),
                    denomination: quai_sdk::consensus::Denomination::new(14).unwrap(),
                    unlock_height: U256::ZERO,
                    expires_at: None,
                    reserved: false,
                })
                .collect();
            b.reserve(
                b.snapshot().await.unwrap().revision,
                id(),
                Checkpoint {
                    height: U256::from(16),
                    hash: hash(2),
                },
                U256::from(17),
                &coins,
                &owners.into_values().collect::<Vec<_>>(),
            )
            .await
            .unwrap();
            b.commit_signed(id(), &tx).await.unwrap();
            let s = BrowserRecoverySession::for_qi(&p, &b);
            m.state().mode = 4;
            assert!(s.broadcast_root(id()).await.is_err());
            m.state().mode = 0;
            assert_eq!(
                s.broadcast_root(id()).await.unwrap().transaction_hash,
                tx.hash().unwrap()
            );
            let h = tx.hash().unwrap();
            m.state().receipts.insert(h.to_string(), receipt(h, 2));
            assert_eq!(s.reconcile(id()).await.unwrap().inclusion.unwrap().0, h);
            m.state().mode = 1;
            assert!(s.reconcile(id()).await.unwrap().inclusion.is_none());
            let snapshot = b.snapshot().await.unwrap();
            assert!(coins.iter().all(|c| snapshot.book.claimed(c.outpoint)));
            assert!(b.release_unsigned(id()).await.is_err());
        }
    }
}
