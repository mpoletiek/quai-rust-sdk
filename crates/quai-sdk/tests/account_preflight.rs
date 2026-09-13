//! Exact-nonce portable preparation and actual browser custody orchestration.
#![cfg(feature = "wallet")]
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
use quai_sdk::account_preflight::{
    AccountIntent, AccountNonce, AccountObservationPolicy, AccountPreflightError, FeePolicy,
    quote_account,
};
use quai_sdk::consensus::{AccessTuple, SignedQuaiTransaction};
use quai_sdk::crypto::SecretKey;
use quai_sdk::primitives::{Hash32, QuaiAddress};
use quai_sdk::provider::RpcData;
use quai_sdk::rpc::{Endpoint, RpcError, Transport};
use quai_sdk::wallet::discovery::NetworkScope;
use quai_sdk::{Provider, Routing, U256, Zone};
use serde_json::{Value, json};
#[cfg(target_arch = "wasm32")]
type Shared = std::rc::Rc<std::cell::RefCell<State>>;
#[cfg(not(target_arch = "wasm32"))]
type Shared = std::sync::Arc<std::sync::Mutex<State>>;
#[derive(Default)]
struct State {
    calls: Vec<(String, Value)>,
    mode: u8,
    headers: u8,
    genesis: u8,
    sends: Vec<String>,
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
            U256::from(9),
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
fn sender() -> QuaiAddress {
    QuaiAddress::try_from(key().public_key().address()).unwrap()
}
fn intent() -> AccountIntent {
    AccountIntent {
        to: sender(),
        value: U256::from(7),
        data: RpcData::new(vec![1, 2]).unwrap(),
        access_list: vec![AccessTuple {
            address: sender().address(),
            storage_keys: vec![hash(4), hash(3)],
        }],
    }
}
fn fee() -> FeePolicy {
    FeePolicy {
        max_gas: 30_000,
        max_gas_price: U256::from(3),
        max_total_fee: U256::from(60_000),
        gas_margin_bps: 1000,
    }
}
fn unhex(s: &str) -> Vec<u8> {
    let h = s.strip_prefix("0x").unwrap();
    (0..h.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap())
        .collect()
}
impl Transport for Mock {
    async fn request(&self, _: &Endpoint, method: &str, params: Value) -> Result<Value, RpcError> {
        let stall;
        {
            let mut s = self.state();
            s.calls.push((method.into(), params.clone()));
            match method {
                "quai_chainId" => return Ok(json!(if s.mode == 1 { "0xa" } else { "0x9" })),
                "quai_getHeaderByNumber" if params[0] == "0x0" => {
                    s.genesis += 1;
                    let h = if s.mode == 2 && s.genesis > 1 {
                        hash(9)
                    } else {
                        hash(1)
                    };
                    return Ok(
                        json!({"woHeader":{"hash":h.to_string(),"number":"0x0","location":"0x","parentHash":Hash32::ZERO.to_string()}}),
                    );
                }
                "quai_getHeaderByNumber" => {
                    s.headers += 1;
                    return Ok(
                        json!({"gasLimit":"0x100000","stateLimit":"0x100000","woHeader":{"hash":hash(if s.mode==3&&s.headers>1{9}else{2}).to_string(),"number":"0x10","parentHash":hash(1).to_string(),"primeTerminusNumber":"0x4","location":"0x0000"}}),
                    );
                }
                "quai_getTransactionCount" => {
                    if s.mode == 4 && params[1] == "pending" {
                        return Err(RpcError::Timeout);
                    }
                    return Ok(json!("0x5"));
                }
                "quai_gasPrice" => return Ok(json!(if s.mode == 5 { "0x4" } else { "0x2" })),
                "quai_getBalance" => return Ok(json!(if s.mode == 6 { "0x1" } else { "0xf4240" })),
                "quai_estimateGas" => {
                    stall = s.mode == 7;
                    if !stall {
                        return Ok(json!("0x5209"));
                    }
                }
                "quai_sendRawTransaction" => {
                    let wire = params[0].as_str().unwrap().to_owned();
                    s.sends.push(wire.clone());
                    if s.mode == 8 {
                        return Err(RpcError::Timeout);
                    }
                    let signed = SignedQuaiTransaction::decode(&unhex(&wire)).unwrap();
                    return Ok(json!(signed.hash().unwrap().to_string()));
                }
                _ => panic!("unexpected test RPC {method}"),
            }
        }
        assert!(stall);
        std::future::poll_fn(|_cx| {
            if self.state().mode == 7 {
                std::task::Poll::Pending
            } else {
                std::task::Poll::Ready(Ok(json!("0x5209")))
            }
        })
        .await
    }
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn quotes_exact_retained_nonce_access_order_and_rounded_fee() {
    for observation in [
        AccountObservationPolicy::Pending,
        AccountObservationPolicy::PinnedLatest,
    ] {
        let m = Mock::default();
        let p = m.provider();
        let q = quote_account(
            &p,
            scope(),
            sender(),
            intent(),
            AccountNonce::AtLeast(8),
            observation,
            fee(),
        )
        .await
        .unwrap();
        assert_eq!(q.transaction().nonce, 8);
        assert_eq!(q.transaction().gas_limit, 23_102);
        assert_eq!(q.maximum_fee(), U256::from(46_204));
        assert_eq!(
            q.signing_digest(),
            q.transaction().signing_digest().unwrap()
        );
        assert_eq!(q.transaction().access_list, intent().access_list);
        let s = m.state();
        let (_, request) = s
            .calls
            .iter()
            .find(|(method, _)| method == "quai_estimateGas")
            .unwrap();
        assert_eq!(request[0]["nonce"], "0x8");
        assert_eq!(request[0]["input"], "0x0102");
        assert_eq!(
            request[0]["accessList"][0]["storageKeys"],
            json!([hash(4).to_string(), hash(3).to_string()])
        );
        let block = if observation == AccountObservationPolicy::Pending {
            "pending"
        } else {
            "0x10"
        };
        for (method, params) in &s.calls {
            if matches!(
                method.as_str(),
                "quai_estimateGas" | "quai_getBalance" | "quai_getTransactionCount"
            ) {
                assert_eq!(params[1], block);
            }
        }
        assert!(s.sends.is_empty());
    }
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn rejects_changed_identity_heads_limits_and_unsupported_pending_without_fallback() {
    for mode in 1..=6 {
        let m = Mock::default();
        m.state().mode = mode;
        let observation = if mode == 4 {
            AccountObservationPolicy::Pending
        } else {
            AccountObservationPolicy::PinnedLatest
        };
        assert!(
            quote_account(
                &m.provider(),
                scope(),
                sender(),
                intent(),
                AccountNonce::AtLeast(0),
                observation,
                fee()
            )
            .await
            .is_err()
        );
        assert!(m.state().sends.is_empty());
        if mode == 4 {
            assert!(!m.state().calls.iter().any(|(method, params)| method
                == "quai_getHeaderByNumber"
                && params[0] == "latest"));
        }
    }
    let m = Mock::default();
    let mut low = fee();
    low.max_total_fee = U256::from(46_203);
    assert!(matches!(
        quote_account(
            &m.provider(),
            scope(),
            sender(),
            intent(),
            AccountNonce::AtLeast(0),
            AccountObservationPolicy::Pending,
            low
        )
        .await,
        Err(AccountPreflightError::FeeLimit)
    ));
    assert!(
        quote_account(
            &m.provider(),
            scope(),
            sender(),
            intent(),
            AccountNonce::Exact(4),
            AccountObservationPolicy::Pending,
            fee()
        )
        .await
        .is_err()
    );
    let fresh = Mock::default();
    let mut bad = fee();
    bad.gas_margin_bps = 10_001;
    assert!(
        quote_account(
            &fresh.provider(),
            scope(),
            sender(),
            intent(),
            AccountNonce::AtLeast(0),
            AccountObservationPolicy::Pending,
            bad
        )
        .await
        .is_err()
    );
    assert!(fresh.state().calls.is_empty());
}

#[cfg(all(target_arch = "wasm32", feature = "backup", feature = "browser"))]
mod browser {
    use super::*;
    use quai_sdk::browser_accounts::BrowserAccountBook;
    use quai_sdk::browser_transactions::{BrowserAccountSession, BrowserTransactionError};
    use quai_sdk::signer::LocalSigner;
    use quai_sdk::wallet::account_custody::{ReservationId, ReservationState};
    fn id(n: u8) -> ReservationId {
        ReservationId([n; 16])
    }
    fn name() -> String {
        let mut b = [0; 16];
        quai_sdk::crypto::fill_random(&mut b).unwrap();
        format!("account-session-{:x}", u128::from_be_bytes(b))
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn prepare_review_sign_and_ambiguous_submission_recover_exact_bytes_after_reopen() {
        let m = Mock::default();
        let p = m.provider();
        let name = name();
        let book = BrowserAccountBook::open(&name, scope(), key().public_key())
            .await
            .unwrap();
        book.initialize(8).await.unwrap();
        let session = BrowserAccountSession::new(&p, &book);
        let prepared = session.prepare(id(1), intent(), fee()).await.unwrap();
        assert_eq!(prepared.transaction().nonce, 8);
        assert_eq!(prepared.maximum_fee(), U256::from(46_204));
        assert!(m.state().sends.is_empty());
        assert_eq!(book.snapshot().await.unwrap().book.next_nonce(), 9);
        let signer = LocalSigner::new(key(), scope().chain_id).unwrap();
        let signed = prepared.sign(&signer).await.unwrap();
        assert_eq!(
            book.snapshot()
                .await
                .unwrap()
                .book
                .operation(id(1))
                .unwrap()
                .payload,
            Some(signed.signed_bytes().unwrap())
        );
        m.state().mode = 8;
        assert!(matches!(
            session.broadcast(id(1)).await,
            Err(BrowserTransactionError::Broadcast(
                quai_sdk::provider::BroadcastError::Ambiguous { .. }
            ))
        ));
        assert_eq!(m.state().sends.len(), 1);
        assert_eq!(
            book.snapshot()
                .await
                .unwrap()
                .book
                .operation(id(1))
                .unwrap()
                .state,
            ReservationState::Submitted
        );
        let reopened = BrowserAccountBook::open(&name, scope(), key().public_key())
            .await
            .unwrap();
        m.state().mode = 0;
        let result = BrowserAccountSession::new(&p, &reopened)
            .broadcast(id(1))
            .await
            .unwrap();
        assert_eq!(result.transaction_hash, signed.hash().unwrap());
        assert_eq!(m.state().sends.len(), 2);
        {
            let state = m.state();
            assert_eq!(state.sends[0], state.sends[1]);
        }
        assert!(reopened.release_unsigned(id(1)).await.is_err());
        assert!(session.prepare(id(1), intent(), fee()).await.is_err());
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn preparation_races_cancellation_and_fee_failures_cannot_reserve_unreviewed_nonces() {
        use std::{future::Future, pin::Pin, task::Poll};
        async fn once<F: Future>(mut f: Pin<&mut F>) -> Poll<F::Output> {
            std::future::poll_fn(|cx| Poll::Ready(f.as_mut().poll(cx))).await
        }
        let m = Mock::default();
        let p = m.provider();
        let book = BrowserAccountBook::open(&name(), scope(), key().public_key())
            .await
            .unwrap();
        book.initialize(8).await.unwrap();
        let session = BrowserAccountSession::new(&p, &book);
        m.state().mode = 7;
        let mut work = Box::pin(session.prepare(id(2), intent(), fee()));
        assert!(once(work.as_mut()).await.is_pending());
        book.snapshot().await.unwrap();
        assert!(once(work.as_mut()).await.is_pending());
        assert!(
            m.state()
                .calls
                .iter()
                .any(|(method, _)| method == "quai_estimateGas")
        );
        book.reserve_nonce(id(3), 5).await.unwrap();
        m.state().mode = 0;
        assert!(matches!(
            work.await,
            Err(BrowserTransactionError::Custody(
                quai_sdk::browser_accounts::BrowserAccountError::Browser(
                    quai_sdk::browser::BrowserError::StorageConflict
                )
            ))
        ));
        let state = book.snapshot().await.unwrap();
        assert!(state.book.operation(id(2)).is_none());
        assert_eq!(state.book.next_nonce(), 9);
        let before = state.revision;
        m.state().mode = 7;
        let mut cancelled = Box::pin(session.prepare(id(4), intent(), fee()));
        assert!(once(cancelled.as_mut()).await.is_pending());
        book.snapshot().await.unwrap();
        assert!(once(cancelled.as_mut()).await.is_pending());
        drop(cancelled);
        assert_eq!(book.snapshot().await.unwrap().revision, before);
        m.state().mode = 6;
        assert!(session.prepare(id(4), intent(), fee()).await.is_err());
        assert_eq!(book.snapshot().await.unwrap().revision, before);
        m.state().mode = 0;
        let repaired = session
            .prepare_reserved(id(3), intent(), fee())
            .await
            .unwrap();
        assert_eq!(repaired.transaction().nonce, 8);
        assert_eq!(book.snapshot().await.unwrap().book.next_nonce(), 9);
        let mut other = intent();
        other.to = "0x0100000000000000000000000000000000000000"
            .parse()
            .unwrap();
        assert!(session.prepare(id(5), other.clone(), fee()).await.is_err());
        let cross = session
            .prepare_cross_zone(id(5), other, fee())
            .await
            .unwrap();
        assert_eq!(cross.transaction().nonce, 9);
        assert!(m.state().sends.is_empty());
    }
}

#[cfg(feature = "abi")]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn wrapper_calls_feed_portable_account_preparation_without_losing_value_or_access() {
    use quai_sdk::wrappers::{WQI_ADDRESS, WQUAI_ADDRESS, WrappedQi, WrappedQuai};
    let m = Mock::default();
    let p = m.provider();
    let wquai = WrappedQuai::new(WQUAI_ADDRESS.parse().unwrap(), &p).unwrap();
    let deposit = wquai.deposit(U256::from(7)).unwrap().into_account_intent();
    let q = quote_account(
        &p,
        scope(),
        sender(),
        deposit,
        AccountNonce::Exact(5),
        AccountObservationPolicy::Pending,
        fee(),
    )
    .await
    .unwrap();
    assert_eq!(q.transaction().value, U256::from(7));
    assert_eq!(q.transaction().data, vec![0xd0, 0xe3, 0x0d, 0xb0]);
    let wqi = WrappedQi::new(WQI_ADDRESS.parse().unwrap(), &p).unwrap();
    let claim = wqi.claim_deposit().unwrap().into_account_intent();
    let expected = claim.access_list.clone();
    assert!(!expected.is_empty());
    let q = quote_account(
        &p,
        scope(),
        sender(),
        claim,
        AccountNonce::Exact(6),
        AccountObservationPolicy::Pending,
        fee(),
    )
    .await
    .unwrap();
    assert_eq!(q.transaction().access_list, expected);
    assert_eq!(q.transaction().value, U256::ZERO);
}

#[cfg(all(target_arch = "wasm32", feature = "backup", feature = "browser"))]
#[wasm_bindgen_test::wasm_bindgen_test]
async fn actual_fetch_preflight_and_indexeddb_signing_preserve_the_estimated_nonce() {
    use quai_sdk::browser::{BrowserConfig, BrowserFetchTransport};
    use quai_sdk::browser_accounts::BrowserAccountBook;
    use quai_sdk::browser_transactions::BrowserAccountSession;
    use quai_sdk::wallet::account_custody::ReservationId;
    let endpoint = format!(
        "{}/account-preflight",
        option_env!("QUAI_BROWSER_FIXTURE_URL").unwrap_or("http://127.0.0.1:18080")
    );
    let provider = Provider::new(
        BrowserFetchTransport::new(BrowserConfig::default()).unwrap(),
        Routing::direct(&endpoint, Zone::Cyprus1.into()).unwrap(),
        scope().chain_id,
    );
    let mut random = [0; 16];
    quai_sdk::crypto::fill_random(&mut random).unwrap();
    let book = BrowserAccountBook::open(
        &format!("account-fetch-{:x}", u128::from_be_bytes(random)),
        scope(),
        key().public_key(),
    )
    .await
    .unwrap();
    book.initialize(8).await.unwrap();
    let session = BrowserAccountSession::new(&provider, &book)
        .with_observation_policy(AccountObservationPolicy::PinnedLatest);
    let prepared = session
        .prepare(ReservationId([9; 16]), intent(), fee())
        .await
        .unwrap();
    assert_eq!(prepared.transaction().nonce, 8);
    assert_eq!(prepared.transaction().gas_limit, 23_102);
    let signer = quai_sdk::signer::LocalSigner::new(key(), scope().chain_id).unwrap();
    let signed = prepared.sign(&signer).await.unwrap();
    assert_eq!(
        book.snapshot()
            .await
            .unwrap()
            .book
            .operation(ReservationId([9; 16]))
            .unwrap()
            .payload,
        Some(signed.signed_bytes().unwrap())
    );
}
