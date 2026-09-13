//! Public-toy remote wallet protocol tests shared by native and browser workers.
#![cfg(feature = "wallet")]
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
use quai_sdk::consensus::{AccessTuple, QuaiTransaction, SignedQuaiTransaction};
use quai_sdk::crypto::{SecretKey, hash_message};
use quai_sdk::primitives::{Hash32, QuaiAddress, hexlify};
use quai_sdk::rpc::{Endpoint, RemoteError, RpcError, Transport};
use quai_sdk::rpc_signer::*;
use quai_sdk::signer::{DomainPolicy, TypedData};
use quai_sdk::wallet::discovery::NetworkScope;
use quai_sdk::{Provider, Routing, U256, Zone};
use serde_json::{Value, json};
#[cfg(target_arch = "wasm32")]
type Shared = std::rc::Rc<std::cell::RefCell<State>>;
#[cfg(not(target_arch = "wasm32"))]
type Shared = std::sync::Arc<std::sync::Mutex<State>>;
#[derive(Default)]
struct State {
    mode: u8,
    dispatched: usize,
    calls: Vec<(String, Value)>,
    reply: Value,
    observed: Value,
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
}
fn key(n: u16) -> SecretKey {
    let mut b = [0; 32];
    b[30..].copy_from_slice(&n.to_be_bytes());
    SecretKey::from_bytes(&b).unwrap()
}
fn address() -> QuaiAddress {
    key(805).public_key().address().try_into().unwrap()
}
fn scope() -> NetworkScope {
    NetworkScope {
        chain_id: U256::from(9),
        genesis: Hash32::from_bytes([1; 32]),
        zone: Zone::Cyprus1,
    }
}
fn tx() -> QuaiTransaction {
    QuaiTransaction {
        chain_id: U256::from(9),
        nonce: 0,
        to: Some(address().address()),
        value: U256::from(9007199254740993u64),
        gas_limit: 25000,
        gas_price: U256::from(2),
        data: vec![0, 1, 2],
        access_list: vec![AccessTuple {
            address: address().address(),
            storage_keys: vec![Hash32::from_bytes([3; 32])],
        }],
    }
}
fn setup(mode: u8, reply: Value) -> (RpcAccountSigner<Mock>, Mock) {
    let mock = Mock::default();
    {
        let mut s = mock.state();
        s.mode = mode;
        s.reply = reply;
    }
    (
        RpcAccountSigner::new(
            mock.clone(),
            Routing::direct("https://remote.invalid", Zone::Cyprus1.into()).unwrap(),
            scope(),
            address(),
        )
        .unwrap(),
        mock,
    )
}
impl Transport for Mock {
    async fn request(&self, _: &Endpoint, method: &str, params: Value) -> Result<Value, RpcError> {
        let result = {
            let mut s = self.state();
            s.calls.push((method.into(), params));
            match method {
                "quai_chainId" => Some(Ok(json!(if s.mode == 2 && s.dispatched > 0 {
                    "0xa"
                } else {
                    "0x9"
                }))),
                "quai_getHeaderByNumber" => Some(Ok(
                    json!({"woHeader":{"hash":if s.mode==3 && s.dispatched>0 {Hash32::from_bytes([2;32])}else{scope().genesis}.to_string(),"number":"0x0","location":"0x","parentHash":Hash32::ZERO.to_string()}}),
                )),
                "quai_accounts" => Some(Ok(match s.mode {
                    1 => json!([]),
                    4 if s.dispatched > 0 => json!([]),
                    5 => json!([address().to_string(), address().to_string()]),
                    6 => json!([key(130).public_key().address().to_string()]),
                    7 => json!(vec![address().to_string(); 1025]),
                    _ => json!([address().to_string()]),
                })),
                "quai_getTransactionByHash" => Some(Ok(s.observed.clone())),
                "personal_sign"
                | "quai_sign"
                | "quai_signTypedData_v4"
                | "quai_signTransaction"
                | "quai_sendTransaction"
                | "personal_unlockAccount" => {
                    s.dispatched += 1;
                    match s.mode {
                        8 => Some(Err(RpcError::Remote(RemoteError {
                            code: 4001,
                            message: "PUBLIC password echo".into(),
                            data: Some(json!("PUBLIC secret")),
                        }))),
                        9 => None,
                        _ => Some(Ok(s.reply.clone())),
                    }
                }
                _ => panic!("unexpected method {method}"),
            }
        };
        match result {
            Some(r) => r,
            None => std::future::pending().await,
        }
    }
}
fn signature(digest: &[u8; 32], scalar: u16) -> Value {
    json!(
        hexlify(
            &key(scalar)
                .sign_prehash(digest)
                .unwrap()
                .to_quais_bytes()
                .unwrap()
        )
        .unwrap()
    )
}
fn encoded(tx: &QuaiTransaction) -> Value {
    json!(hexlify(&tx.sign(&key(805)).unwrap().signed_bytes().unwrap()).unwrap())
}
fn typed(chain: u64) -> TypedData {
    TypedData::from_json(&serde_json::to_vec(&json!({"types":{"Transfer":[{"name":"amount","type":"uint256"}]},"primaryType":"Transfer","domain":{"name":"Public toy","chainId":chain},"message":{"amount":"9007199254740993"}})).unwrap()).unwrap()
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn messages_typed_data_legacy_and_unlock_match_exact_rpc_and_recover() {
    let (signer, mock) = setup(0, signature(&hash_message(b"hello"), 805));
    signer.sign_message(b"hello").await.unwrap();
    assert!(mock.state().calls.contains(&(
        "personal_sign".into(),
        json!(["0x68656c6c6f", address().to_string().to_ascii_lowercase()])
    )));
    let data = typed(9);
    mock.state().reply = signature(data.signing_hash().bytes(), 805);
    signer
        .sign_typed_data(&data, DomainPolicy::RequireChainId)
        .await
        .unwrap();
    assert!(mock.state().calls.contains(&(
        "quai_signTypedData_v4".into(),
        json!([
            address().to_string().to_ascii_lowercase(),
            data.to_rpc_json().unwrap()
        ])
    )));
    mock.state().reply = signature(&hash_message(b"legacy"), 805);
    signer
        .legacy_sign(b"legacy", Hash32::from_bytes(hash_message(b"legacy")))
        .await
        .unwrap();
    assert!(mock.state().calls.contains(&(
        "quai_sign".into(),
        json!([address().to_string().to_ascii_lowercase(), "0x6c6567616379"])
    )));
    mock.state().reply = json!(true);
    assert!(signer.unlock("PUBLIC", 60).await.unwrap());
    assert!(mock.state().calls.contains(&(
        "personal_unlockAccount".into(),
        json!([address().to_string().to_ascii_lowercase(), "PUBLIC", 60])
    )));
    mock.state().reply = json!(false);
    assert!(!signer.unlock("PUBLIC", 1).await.unwrap());
    assert!(!format!("{signer:?}").contains("PUBLIC"));
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn exact_transactions_preserve_zero_nonce_large_values_deployment_and_access_order() {
    for to in [Some(address().address()), None] {
        let mut t = tx();
        t.to = to;
        let (signer, mock) = setup(0, encoded(&t));
        let signed = signer.sign_transaction(&t).await.unwrap();
        assert_eq!(signed.transaction(), &t);
        let s = mock.state();
        let dto = &s
            .calls
            .iter()
            .find(|(m, _)| m == "quai_signTransaction")
            .unwrap()
            .1[0];
        assert_eq!(dto["nonce"], "0x0");
        assert_eq!(dto["value"], "0x20000000000001");
        assert_eq!(dto["data"], "0x000102");
        assert_eq!(
            dto["accessList"][0]["storageKeys"][0],
            Hash32::from_bytes([3; 32]).to_string()
        );
        assert_eq!(dto.get("to").is_some(), to.is_some());
        assert_eq!(s.dispatched, 1);
    }
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn changed_transaction_fields_and_bad_signatures_never_become_verified() {
    for mode in 0..9 {
        let original = tx();
        let mut altered = original.clone();
        match mode {
            0 => altered.chain_id = U256::from(10),
            1 => altered.nonce = 1,
            2 => altered.to = None,
            3 => altered.value += U256::from(1),
            4 => altered.gas_limit += 1,
            5 => altered.gas_price += U256::from(1),
            6 => altered.data.push(3),
            7 => altered.access_list.clear(),
            _ => altered.access_list[0].storage_keys.push(Hash32::ZERO),
        }
        let (signer, mock) = setup(0, encoded(&altered));
        assert!(
            signer
                .sign_transaction(&original)
                .await
                .unwrap_err()
                .dispatched
        );
        assert_eq!(mock.state().dispatched, 1);
    }
    for reply in [
        Value::Null,
        json!("0x00"),
        json!("x".repeat(132)),
        signature(&hash_message(b"other"), 805),
        signature(&hash_message(b"hello"), 1),
    ] {
        let (signer, mock) = setup(0, reply);
        assert!(signer.sign_message(b"hello").await.unwrap_err().dispatched);
        assert_eq!(mock.state().dispatched, 1);
    }
    let (signer, _) = setup(0, json!("0x00"));
    assert!(signer.sign_transaction(&tx()).await.unwrap_err().dispatched);
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn bounds_domain_and_unavailable_accounts_fail_before_dispatch() {
    for mode in [1, 5, 6, 7] {
        let (signer, mock) = setup(mode, Value::Null);
        assert!(!signer.sign_message(b"hello").await.unwrap_err().dispatched);
        assert_eq!(mock.state().dispatched, 0);
    }
    let (signer, mock) = setup(0, Value::Null);
    assert!(
        !signer
            .sign_message(&vec![0; MAX_SIGNER_PARAMS / 2 + 1])
            .await
            .unwrap_err()
            .dispatched
    );
    assert!(
        !signer
            .sign_typed_data(&typed(10), DomainPolicy::RequireChainId)
            .await
            .unwrap_err()
            .dispatched
    );
    for ttl in [0, 3601] {
        assert!(!signer.unlock("PUBLIC", ttl).await.unwrap_err().dispatched);
    }
    assert!(
        !signer
            .unlock(&"x".repeat(1025), 60)
            .await
            .unwrap_err()
            .dispatched
    );
    let mut t = tx();
    t.chain_id = U256::from(10);
    assert!(!signer.sign_transaction(&t).await.unwrap_err().dispatched);
    t = tx();
    t.access_list[0].storage_keys = vec![Hash32::ZERO; 8193];
    assert!(!signer.sign_transaction(&t).await.unwrap_err().dispatched);
    assert!(mock.state().calls.is_empty());
    let mut bad = scope();
    bad.genesis = Hash32::ZERO;
    assert!(
        RpcAccountSigner::new(
            mock,
            Routing::direct("https://remote.invalid", Zone::Cyprus1.into()).unwrap(),
            bad,
            address()
        )
        .is_err()
    );
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn context_changes_and_remote_rejections_are_dispatched_without_retry_or_payload_logs() {
    for mode in [2, 3, 4, 8] {
        let (signer, mock) = setup(mode, signature(&hash_message(b"hello"), 805));
        let err = signer.sign_message(b"hello").await.unwrap_err();
        assert!(err.dispatched);
        assert!(!format!("{err:?} {err}").contains("PUBLIC"));
        assert_eq!(mock.state().dispatched, 1);
    }
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn cancellation_keeps_recovery_identity_and_never_resubmits() {
    use std::{future::Future, task::Poll};
    let (signer, mock) = setup(9, Value::Null);
    let t = tx();
    let identity = RemoteSendIdentity::new(scope(), address(), &t).unwrap();
    let mut future = Box::pin(signer.send_transaction(&t));
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    assert_eq!(mock.state().dispatched, 1);
    drop(future);
    assert_eq!(mock.state().dispatched, 1);
    assert_eq!(identity.nonce, 0);
    assert_eq!(identity.signing_digest, t.signing_digest().unwrap());
}
fn rpc_transaction(signed: &SignedQuaiTransaction) -> Value {
    let tx = signed.transaction();
    let compact = signed.signature().to_compact();
    json!({"type":"0x0","hash":signed.hash().unwrap().to_string(),"blockHash":null,"blockNumber":null,"transactionIndex":null,"from":signed.from().to_string(),"to":tx.to.map(|a|a.to_string()),"chainId":format!("{:#x}",tx.chain_id),"nonce":format!("{:#x}",tx.nonce),"gas":format!("{:#x}",tx.gas_limit),"gasPrice":format!("{:#x}",tx.gas_price),"value":format!("{:#x}",tx.value),"input":hexlify(&tx.data).unwrap(),"accessList":tx.access_list.iter().map(|a|json!({"address":a.address.to_string(),"storageKeys":a.storage_keys.iter().map(ToString::to_string).collect::<Vec<_>>()})).collect::<Vec<_>>(),"v":format!("{:#x}",signed.signature().recovery_id()),"r":format!("{:#x}",U256::from_be_slice(&compact[..32])),"s":format!("{:#x}",U256::from_be_slice(&compact[32..]))})
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn sends_require_independent_observation_and_retain_hash_after_context_failure() {
    for change in [false, true] {
        let requested = tx();
        let mut actual = requested.clone();
        if change {
            actual.value += U256::from(1);
        }
        let signed = actual.sign(&key(805)).unwrap();
        let hash = signed.hash().unwrap();
        let (signer, mock) = setup(0, json!(hash.to_string()));
        let ack = signer.send_transaction(&requested).await.unwrap();
        assert_eq!(ack.reported_hash(), hash);
        assert!(ack.observe(signer.provider()).await.unwrap().is_none());
        mock.state().observed = rpc_transaction(&signed);
        let observation = ack.observe(signer.provider()).await.unwrap().unwrap();
        assert_eq!(observation.matches_request, !change);
        assert_eq!(
            observation.signed.signed_bytes().unwrap(),
            signed.signed_bytes().unwrap()
        );
        mock.state().observed["value"] = json!("0x1");
        assert!(ack.observe(signer.provider()).await.is_err());
        let alien = Provider::new(
            mock.clone(),
            Routing::direct("https://remote.invalid", Zone::Cyprus1.into()).unwrap(),
            U256::from(10),
        );
        assert!(ack.observe(&alien).await.is_err());
        let (signer, mock) = setup(3, json!(hash.to_string()));
        let error = signer.send_transaction(&requested).await.unwrap_err();
        assert!(error.dispatched);
        assert_eq!(error.reported_hash, Some(hash));
        assert_eq!(mock.state().dispatched, 1);
    }
    for reply in [
        Value::Null,
        json!(Hash32::ZERO.to_string()),
        json!(Hash32::from_bytes([128; 32]).to_string()),
        json!("0xzz"),
    ] {
        let (signer, _) = setup(0, reply);
        assert!(signer.send_transaction(&tx()).await.unwrap_err().dispatched);
    }
}
