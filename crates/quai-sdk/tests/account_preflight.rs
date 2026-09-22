//! Exact-nonce portable preparation and actual browser custody orchestration.
#![cfg(feature = "wallet")]
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
use quai_sdk::account_preflight::{
    AccountAccessListPolicy, AccountIntent, AccountNonce, AccountObservationPolicy,
    AccountPreflightError, FeePolicy, QuaiConversionIntent, quote_account,
    quote_account_with_access, quote_quai_conversion,
};
use quai_sdk::consensus::{
    AccessTuple, ConversionSlippage, MIN_QUAI_CONVERSION_VALUE, QuaiToQiTransaction,
    SignedQuaiTransaction,
};
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
    AccountIntent::new(sender(), U256::from(7))
        .with_data(RpcData::new(vec![1, 2]).unwrap())
        .with_access_list(vec![AccessTuple {
            address: sender().address(),
            storage_keys: vec![hash(4), hash(3)],
        }])
}
fn conversion_fee() -> FeePolicy {
    {
        let mut updated = fee();
        updated.max_gas = 500_000;
        updated.max_total_fee = U256::from(1_000_000);
        updated
    }
}
fn fee() -> FeePolicy {
    FeePolicy::new(30_000, U256::from(3), U256::from(60_000)).with_gas_margin_bps(1000)
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
                "quai_quaiToQi" => return Ok(json!("0x5e7f4")),
                "quai_gasPrice" => return Ok(json!(if s.mode == 5 { "0x4" } else { "0x2" })),
                "quai_getBalance" => {
                    return Ok(json!(match s.mode {
                        6 => "0x1",
                        11 => "0x20000000000000000",
                        _ => "0xf4240",
                    }));
                }
                "quai_createAccessList" => {
                    let mut entries = params[0]["accessList"].as_array().unwrap().clone();
                    if s.mode == 12 {
                        entries.clear();
                    } else if s.mode == 13 {
                        entries[0]["storageKeys"] = json!([]);
                    } else {
                        entries.push(json!({"address": sender().to_string(), "storageKeys": [hash(7).to_string()]}));
                    }
                    return Ok(json!({"accessList": entries, "gasUsed": "0x5209"}));
                }
                "quai_estimateGas" => {
                    stall = s.mode == 7;
                    if !stall {
                        return Ok(json!(match s.mode {
                            9 => "0x0",
                            10 => "0xffffffffffffffff",
                            // Grown estimate still inside the parent's 23,102 limit.
                            12 => "0x5a3c",
                            _ => "0x5209",
                        }));
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
    // Unlike pinned JS population, an explicit zero is never replaced with the
    // observed pending nonce (five in this fixture).
    for nonce in [0, 4] {
        assert!(
            quote_account(
                &m.provider(),
                scope(),
                sender(),
                intent(),
                AccountNonce::Exact(nonce),
                AccountObservationPolicy::Pending,
                fee()
            )
            .await
            .is_err()
        );
    }
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

#[cfg(feature = "abi")]
fn deployment(nonce: u64) -> quai_sdk::contracts::PreparedDeployment {
    quai_sdk::contracts::prepare_deployment(
        &quai_sdk::abi::AbiInterface::from_json(b"[]").unwrap(),
        &[0, 0x60, 0, 0x60, 0, 0xf3],
        &[],
        sender(),
        scope().chain_id,
        nonce,
        U256::ZERO,
        quai_sdk::contracts::DeploymentSearch::new(0, 10_000),
        || false,
    )
    .unwrap()
}
#[cfg(feature = "abi")]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn deployment_quotes_keep_nonce_prediction_init_code_and_required_access() {
    let m = Mock::default();
    let draft = deployment(8);
    let predicted = draft.address();
    let init = draft.init_data().to_vec();
    let quote = quai_sdk::account_preflight::quote_deployment(
        &m.provider(),
        scope(),
        sender(),
        draft,
        AccountObservationPolicy::Pending,
        fee(),
    )
    .await
    .unwrap();
    assert_eq!(quote.transaction().to, None);
    assert_eq!(quote.transaction().nonce, 8);
    assert_eq!(quote.transaction().data, init);
    assert_eq!(
        quote.transaction().access_list[0].address,
        predicted.address()
    );
    assert_eq!(
        quai_sdk::primitives::contract_address(sender().address(), 8, &init),
        predicted.address()
    );
    assert_eq!(quote.transaction().data[0], 0);
    let other = "0x0000000000000000000000000000000000000001"
        .parse()
        .unwrap();
    let fresh = Mock::default();
    assert!(
        quai_sdk::account_preflight::quote_deployment(
            &fresh.provider(),
            scope(),
            other,
            deployment(8),
            AccountObservationPolicy::Pending,
            fee()
        )
        .await
        .is_err()
    );
    assert!(fresh.state().calls.is_empty());
    assert!(
        quai_sdk::account_preflight::quote_deployment(
            &fresh.provider(),
            scope(),
            sender(),
            deployment(4),
            AccountObservationPolicy::Pending,
            fee()
        )
        .await
        .is_err()
    );
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn access_discovery_preserves_required_coverage_and_estimates_the_final_list() {
    for mode in [0, 12, 13] {
        let m = Mock::default();
        m.state().mode = mode;
        let result = quote_account_with_access(
            &m.provider(),
            scope(),
            sender(),
            intent(),
            AccountNonce::Exact(8),
            AccountObservationPolicy::PinnedLatest,
            fee(),
            AccountAccessListPolicy::Discover,
        )
        .await;
        let state = m.state();
        let (_, discover) = state
            .calls
            .iter()
            .find(|(method, _)| method == "quai_createAccessList")
            .unwrap();
        assert_eq!(discover[0]["nonce"], "0x8");
        assert_eq!(discover[1], "0x10");
        if mode == 0 {
            let quote = result.unwrap();
            assert_eq!(quote.transaction().access_list.len(), 2);
            assert_eq!(quote.transaction().access_list[1].storage_keys, [hash(7)]);
            let (_, estimate) = state
                .calls
                .iter()
                .find(|(method, _)| method == "quai_estimateGas")
                .unwrap();
            assert_eq!(estimate[0]["accessList"].as_array().unwrap().len(), 2);
        } else {
            assert!(matches!(result, Err(AccountPreflightError::Invalid)));
            assert!(
                !state
                    .calls
                    .iter()
                    .any(|(method, _)| method == "quai_estimateGas")
            );
        }
    }
}

fn conversion() -> QuaiConversionIntent {
    QuaiConversionIntent::new(
        "0x00edf2d16afbc028fb1e879559b07997af79539f"
            .parse()
            .unwrap(),
        U256::from(MIN_QUAI_CONVERSION_VALUE),
        ConversionSlippage::new(1234).unwrap(),
    )
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn conversion_preserves_exact_native_value_recipient_slippage_and_nonce() {
    let m = Mock::default();
    m.state().mode = 11;
    let q = quote_quai_conversion(
        &m.provider(),
        scope(),
        sender(),
        conversion(),
        AccountNonce::AtLeast(8),
        AccountObservationPolicy::PinnedLatest,
        conversion_fee(),
    )
    .await
    .unwrap();
    let typed = QuaiToQiTransaction::new(q.transaction().clone()).unwrap();
    assert_eq!(typed.destination(), conversion().destination);
    assert_eq!(typed.slippage(), conversion().slippage);
    let signed = typed.sign(&key()).unwrap();
    assert_eq!(
        SignedQuaiTransaction::decode(&signed.signed_bytes().unwrap())
            .unwrap()
            .hash()
            .unwrap(),
        signed.hash().unwrap()
    );
    let state = m.state();
    let (_, params) = state
        .calls
        .iter()
        .find(|(method, _)| method == "quai_estimateGas")
        .unwrap();
    assert_eq!(params[0]["to"], conversion().destination.to_string());
    assert_eq!(params[0]["value"], format!("{:#x}", conversion().value));
    assert_eq!(params[0]["input"], "0x04d2");
    assert_eq!(params[0]["nonce"], "0x8");
    assert_eq!(params[0]["txType"], 0);
    assert_eq!(params[1], "0x10");
    assert!(state.sends.is_empty());
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn conversion_invalid_intents_and_fee_arithmetic_fail_closed() {
    for wrong_zone in [false, true] {
        let m = Mock::default();
        let mut bad = conversion();
        if wrong_zone {
            bad.destination = "0x0180000000000000000000000000000000000000"
                .parse()
                .unwrap();
        } else {
            bad.value -= U256::from(1);
        }
        assert!(matches!(
            quote_quai_conversion(
                &m.provider(),
                scope(),
                sender(),
                bad,
                AccountNonce::AtLeast(0),
                AccountObservationPolicy::Pending,
                conversion_fee()
            )
            .await,
            Err(AccountPreflightError::Invalid)
        ));
        assert!(m.state().calls.is_empty());
    }
    let m = Mock::default();
    assert!(matches!(
        quote_quai_conversion(
            &m.provider(),
            scope(),
            sender(),
            conversion(),
            AccountNonce::Exact(5),
            AccountObservationPolicy::Pending,
            conversion_fee()
        )
        .await,
        Err(AccountPreflightError::InsufficientBalance)
    ));
    for mode in [9, 10, 0] {
        let m = Mock::default();
        m.state().mode = mode;
        let mut i = intent();
        if mode == 0 {
            i.value = U256::MAX;
        }
        let mut limits = fee();
        limits.max_gas = u64::MAX;
        assert!(matches!(
            quote_account(
                &m.provider(),
                scope(),
                sender(),
                i,
                AccountNonce::Exact(5),
                AccountObservationPolicy::Pending,
                limits
            )
            .await,
            Err(AccountPreflightError::FeeLimit)
        ));
        // The balance now arrives with the nonce, in one round trip, so it is
        // in hand here; the fee bound still decides first, which is why this
        // reports FeeLimit rather than InsufficientBalance for `U256::MAX`.
    }
}

fn replacement_policy() -> quai_sdk::account_replacement::ReplacementPolicy {
    let mut fees = fee();
    fees.max_total_fee = U256::from(100_000);
    fees.max_gas_price = U256::from(5);
    quai_sdk::account_replacement::ReplacementPolicy::new(5, fees)
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn replacement_quotes_change_only_price_and_use_confirmed_nonce_admission() {
    use quai_sdk::account_replacement::quote_account_replacement;
    for conversion_mode in [false, true] {
        let m = Mock::default();
        if conversion_mode {
            m.state().mode = 11;
        }
        let p = m.provider();
        let quote = if conversion_mode {
            quote_quai_conversion(
                &p,
                scope(),
                sender(),
                conversion(),
                AccountNonce::Exact(8),
                AccountObservationPolicy::Pending,
                conversion_fee(),
            )
            .await
            .unwrap()
        } else {
            quote_account(
                &p,
                scope(),
                sender(),
                intent(),
                AccountNonce::Exact(8),
                AccountObservationPolicy::Pending,
                fee(),
            )
            .await
            .unwrap()
        };
        let parent = quote.transaction().sign(&key()).unwrap();
        if !conversion_mode {
            m.state().mode = 4;
        } // Pending nonce RPC is unavailable; replacement must use confirmed admission.
        let mut replacement_limits = replacement_policy();
        if conversion_mode {
            replacement_limits.fees.max_gas = 500_000;
            replacement_limits.fees.max_total_fee = U256::from(2_000_000);
        }
        m.state().calls.clear();
        let replacement = quote_account_replacement(
            &p,
            scope(),
            &parent,
            AccountObservationPolicy::Pending,
            replacement_limits,
        )
        .await
        .unwrap();
        // A conversion replacement checks the parent's gas limit against the
        // same origin-cost budget the original used, which reads the quote;
        // the raw estimator omits those costs.
        assert_eq!(
            m.state()
                .calls
                .iter()
                .any(|(method, _)| method == "quai_quaiToQi"),
            conversion_mode
        );
        let mut expected = parent.transaction().clone();
        expected.gas_price = U256::from(3);
        assert_eq!(replacement.transaction(), &expected);
        assert_eq!(
            replacement.maximum_fee(),
            U256::from(expected.gas_limit) * U256::from(3)
        );
        assert_eq!(replacement.parent_hash(), parent.hash().unwrap());
        assert_eq!(
            replacement.signing_digest(),
            expected.signing_digest().unwrap()
        );
        let mut stale = expected.clone();
        stale.nonce = 4;
        assert!(matches!(
            quote_account_replacement(
                &p,
                scope(),
                &stale.sign(&key()).unwrap(),
                AccountObservationPolicy::Pending,
                replacement_limits
            )
            .await,
            Err(AccountPreflightError::Invalid)
        ));
        if !conversion_mode {
            for mode in [3, 6, 10] {
                m.state().mode = mode;
                m.state().headers = 0;
                assert!(
                    quote_account_replacement(
                        &p,
                        scope(),
                        &parent,
                        AccountObservationPolicy::PinnedLatest,
                        replacement_limits
                    )
                    .await
                    .is_err()
                );
            }
        }
    }
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
    async fn external_signature_commit_preserves_review_and_durable_nonce_claim() {
        let m = Mock::default();
        let p = m.provider();
        let name = name();
        let book = BrowserAccountBook::open(&name, scope(), key().public_key())
            .await
            .unwrap();
        book.initialize(8).await.unwrap();
        let session = BrowserAccountSession::new(&p, &book);
        let prepared = session.prepare(id(90), intent(), fee()).await.unwrap();
        let mut changed = prepared.transaction().clone();
        changed.value += U256::from(1);
        assert!(
            prepared
                .commit_external_signature(&changed.sign(&key()).unwrap())
                .await
                .is_err()
        );
        assert_eq!(
            book.snapshot()
                .await
                .unwrap()
                .book
                .operation(id(90))
                .unwrap()
                .state,
            ReservationState::Reserved
        );
        let signed = prepared.transaction().sign(&key()).unwrap();
        prepared.commit_external_signature(&signed).await.unwrap();
        let reopened = BrowserAccountBook::open(&name, scope(), key().public_key())
            .await
            .unwrap();
        let snapshot = reopened.snapshot().await.unwrap();
        let operation = snapshot.book.operation(id(90)).unwrap();
        assert_eq!(operation.state, ReservationState::Signed);
        assert_eq!(operation.payload, Some(signed.signed_bytes().unwrap()));
        assert!(reopened.release_unsigned(id(90)).await.is_err());
        assert!(m.state().sends.is_empty());
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
    #[cfg(feature = "abi")]
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn deployment_requires_retained_nonce_and_signs_the_discovered_access_list() {
        let m = Mock::default();
        let p = m.provider();
        let book = BrowserAccountBook::open(&name(), scope(), key().public_key())
            .await
            .unwrap();
        book.initialize(8).await.unwrap();
        let session = BrowserAccountSession::new(&p, &book)
            .with_access_list_policy(AccountAccessListPolicy::Discover);
        assert!(
            session
                .prepare_deployment(id(11), deployment(8), fee())
                .await
                .is_err()
        );
        assert!(m.state().calls.is_empty());
        assert_eq!(book.reserve_nonce(id(11), 5).await.unwrap(), 8);
        assert!(
            session
                .prepare_deployment(id(11), deployment(9), fee())
                .await
                .is_err()
        );
        assert!(m.state().calls.is_empty());
        let before = book.snapshot().await.unwrap().revision;
        m.state().mode = 12;
        assert!(
            session
                .prepare_deployment(id(11), deployment(8), fee())
                .await
                .is_err()
        );
        assert_eq!(book.snapshot().await.unwrap().revision, before);
        m.state().mode = 0;
        let prepared = session
            .prepare_deployment(id(11), deployment(8), fee())
            .await
            .unwrap();
        let signer = LocalSigner::new(key(), scope().chain_id).unwrap();
        let signed = prepared.sign(&signer).await.unwrap();
        assert_eq!(signed.transaction().access_list.len(), 2);
        let expected = quai_sdk::primitives::contract_address(
            sender().address(),
            8,
            &signed.transaction().data,
        );
        assert_eq!(signed.transaction().access_list[0].address, expected);
        assert_eq!(signed.transaction().to, None);
        assert_eq!(
            session.broadcast(id(11)).await.unwrap().transaction_hash,
            signed.hash().unwrap()
        );
        assert_eq!(book.snapshot().await.unwrap().book.next_nonce(), 9);
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn conversion_reprepare_sign_and_replay_retain_the_reviewed_destination() {
        let m = Mock::default();
        m.state().mode = 11;
        let p = m.provider();
        let name = name();
        let book = BrowserAccountBook::open(&name, scope(), key().public_key())
            .await
            .unwrap();
        book.initialize(8).await.unwrap();
        let session = BrowserAccountSession::new(&p, &book);
        session
            .prepare_conversion(id(10), conversion(), conversion_fee())
            .await
            .unwrap();
        let reopened = BrowserAccountBook::open(&name, scope(), key().public_key())
            .await
            .unwrap();
        let session = BrowserAccountSession::new(&p, &reopened);
        let prepared = session
            .prepare_conversion_reserved(id(10), conversion(), conversion_fee())
            .await
            .unwrap();
        assert_eq!(prepared.transaction().nonce, 8);
        assert_eq!(reopened.snapshot().await.unwrap().book.next_nonce(), 9);
        let signed = prepared
            .sign(&LocalSigner::new(key(), scope().chain_id).unwrap())
            .await
            .unwrap();
        assert_eq!(
            signed.transaction().to,
            Some(conversion().destination.address())
        );
        assert_eq!(
            signed.transaction().data,
            conversion().slippage.to_be_bytes()
        );
        m.state().mode = 8;
        assert!(session.broadcast(id(10)).await.is_err());
        m.state().mode = 11;
        assert_eq!(
            session.broadcast(id(10)).await.unwrap().transaction_hash,
            signed.hash().unwrap()
        );
        let state = m.state();
        assert_eq!(state.sends.len(), 2);
        assert_eq!(state.sends[0], state.sends[1]);
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn reviewed_account_replacement_keeps_nonce_fields_and_exact_candidate_after_restart() {
        use quai_sdk::browser_recovery::BrowserRecoverySession;
        let m = Mock::default();
        let p = m.provider();
        let name = name();
        let book = BrowserAccountBook::open(&name, scope(), key().public_key())
            .await
            .unwrap();
        book.initialize(8).await.unwrap();
        let session = BrowserAccountSession::new(&p, &book);
        let signer = LocalSigner::new(key(), scope().chain_id).unwrap();
        let root = session
            .prepare(id(12), intent(), fee())
            .await
            .unwrap()
            .sign(&signer)
            .await
            .unwrap();
        let reviewed = session
            .prepare_replacement(id(12), root.hash().unwrap(), replacement_policy())
            .await
            .unwrap();
        let candidate = reviewed.sign(&signer).await.unwrap();
        assert_eq!(candidate.transaction(), reviewed.transaction());
        assert_eq!(candidate.transaction().nonce, 8);
        assert_eq!(
            candidate.transaction().access_list,
            root.transaction().access_list
        );
        assert_eq!(book.snapshot().await.unwrap().book.next_nonce(), 9);
        m.state().mode = 8;
        assert!(
            BrowserRecoverySession::for_account(&p, &book)
                .broadcast_candidate(id(12), candidate.hash().unwrap())
                .await
                .is_err()
        );
        let reopened = BrowserAccountBook::open(&name, scope(), key().public_key())
            .await
            .unwrap();
        m.state().mode = 0;
        assert_eq!(
            BrowserRecoverySession::for_account(&p, &reopened)
                .broadcast_candidate(id(12), candidate.hash().unwrap())
                .await
                .unwrap()
                .transaction_hash,
            candidate.hash().unwrap()
        );
        {
            let state = m.state();
            assert_eq!(state.sends[0], state.sends[1]);
        }
        let snapshot = book.snapshot().await.unwrap();
        book.observe_inclusion(
            snapshot.revision,
            id(12),
            candidate.hash().unwrap(),
            quai_sdk::wallet::discovery::Checkpoint {
                height: U256::from(16),
                hash: hash(2),
            },
        )
        .await
        .unwrap();
        assert!(reviewed.sign(&signer).await.is_err());
        assert!(
            session
                .prepare_replacement(id(12), root.hash().unwrap(), replacement_policy())
                .await
                .is_err()
        );
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

#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn a_replacement_is_allowed_while_the_estimate_still_fits_the_parent_limit() {
    // The parent's limit already includes the preparation margin. Applying the
    // margin again refused any replacement once the estimate grew, leaving the
    // nonce stuck behind an underpriced parent.
    use quai_sdk::account_replacement::quote_account_replacement;
    let m = Mock::default();
    let p = m.provider();
    let quote = quote_account(
        &p,
        scope(),
        sender(),
        intent(),
        AccountNonce::Exact(8),
        AccountObservationPolicy::Pending,
        fee(),
    )
    .await
    .unwrap();
    let parent = quote.transaction().sign(&key()).unwrap();
    assert_eq!(parent.transaction().gas_limit, 23_102);
    m.state().mode = 12;
    let replacement = quote_account_replacement(
        &p,
        scope(),
        &parent,
        AccountObservationPolicy::Pending,
        replacement_policy(),
    )
    .await
    .unwrap();
    assert_eq!(replacement.transaction().gas_limit, 23_102);
    // The grown estimate is reported, so a wallet can see the headroom left.
    assert_eq!(replacement.estimated_gas(), 0x5a3c);
}
