//! Real Chromium wasm tests using local HTTP and an explicitly supplied synthetic wallet.
#![cfg(target_arch = "wasm32")]
use quai_browser::{
    BrowserConfig, BrowserError, BrowserFetchTransport, InjectedProvider, fill_random,
};
use quai_primitives::{QuaiAddress, Zone};
use quai_rpc::{Endpoint, RpcError, Transport, U256};
use serde_json::json;
use wasm_bindgen::prelude::*;
use wasm_bindgen_test::*;
wasm_bindgen_test_configure!(run_in_browser);
#[wasm_bindgen(module = "/tests/fixture.js")]
extern "C" {
    #[wasm_bindgen(js_name=fixtureProvider)]
    fn fixture_provider(mode: &str) -> JsValue;
    #[wasm_bindgen(js_name=listenerCount)]
    fn listener_count(provider: &JsValue) -> usize;
    #[wasm_bindgen(js_name=emitContextChange)]
    fn emit_context_change(provider: &JsValue, event: &str);
    #[wasm_bindgen(js_name=callCount)]
    fn call_count(provider: &JsValue) -> usize;
    #[wasm_bindgen(js_name=callJson)]
    fn call_json(provider: &JsValue, index: usize) -> String;
    #[wasm_bindgen(js_name=finishProvider)]
    fn finish_provider(provider: &JsValue);
    #[wasm_bindgen(js_name=setSignature)]
    fn set_signature(provider: &JsValue, signature: &str);
}
fn endpoint(path: &str) -> Endpoint {
    Endpoint::parse(&format!(
        "{}{}",
        option_env!("QUAI_BROWSER_FIXTURE_URL").unwrap_or("http://127.0.0.1:18080"),
        path
    ))
    .unwrap()
}
fn injected(mode: &str, config: BrowserConfig) -> (InjectedProvider, JsValue) {
    let value = fixture_provider(mode);
    let provider = InjectedProvider::new(
        value.clone(),
        endpoint("/rpc"),
        Zone::Cyprus1.into(),
        U256::from(15000),
        config,
    )
    .unwrap();
    (provider, value)
}
fn public_fixture_signature(digest: &[u8; 32]) -> [u8; 65] {
    let mut bytes = [0; 32];
    bytes[30..].copy_from_slice(&805u16.to_be_bytes());
    quai_crypto::SecretKey::from_bytes(&bytes)
        .unwrap()
        .sign_prehash(digest)
        .unwrap()
        .to_quais_bytes()
        .unwrap()
}
fn install_signature(provider: &JsValue, signature: [u8; 65]) {
    let hex: String = signature.iter().map(|b| format!("{b:02x}")).collect();
    set_signature(provider, &format!("0x{hex}"));
}
#[wasm_bindgen_test(async)]
async fn browser_fetch_real_http_routing_and_envelopes() {
    let transport = BrowserFetchTransport::new(BrowserConfig::default()).unwrap();
    assert_eq!(
        transport
            .request(
                &endpoint("/prefix/cyprus1?token=PUBLIC"),
                "quai_chainId",
                json!([])
            )
            .await
            .unwrap(),
        json!("0x3a98")
    );
    assert!(matches!(
        transport
            .request(&endpoint("/wrong-id"), "quai_chainId", json!([]))
            .await,
        Err(RpcError::InvalidResponse(_))
    ));
    assert!(matches!(
        transport
            .request(&endpoint("/duplicate-id"), "quai_chainId", json!([]))
            .await,
        Err(RpcError::InvalidResponse(_))
    ));
    assert!(matches!(
        transport
            .request(&endpoint("/redirect"), "quai_chainId", json!([]))
            .await,
        Err(RpcError::Transport)
    ));
}
#[wasm_bindgen_test(async)]
async fn browser_fetch_stream_limit_deadline_and_drop_release_capacity() {
    let transport = BrowserFetchTransport::new(BrowserConfig {
        max_response_bytes: 100,
        request_timeout_ms: 100,
        max_in_flight: 1,
        ..Default::default()
    })
    .unwrap();
    assert!(matches!(
        transport
            .request(&endpoint("/oversize"), "quai_chainId", json!([]))
            .await,
        Err(RpcError::ResponseTooLarge)
    ));
    assert!(matches!(
        transport
            .request(&endpoint("/slow"), "quai_chainId", json!([]))
            .await,
        Err(RpcError::Timeout)
    ));
    let endpoint = endpoint("/slow");
    let mut pending = Box::pin(transport.request(&endpoint, "quai_chainId", json!([])));
    assert!(futures_util::poll!(&mut pending).is_pending());
    assert!(matches!(
        transport.send(&endpoint, "quai_chainId", json!([])).await,
        Err(BrowserError::Busy)
    ));
    drop(pending);
    assert!(matches!(
        transport.send(&endpoint, "quai_chainId", json!([])).await,
        Err(BrowserError::Rpc(RpcError::Timeout))
    ));
}
#[wasm_bindgen_test(async)]
async fn injected_construction_and_read_never_prompt_and_preserve_shard() {
    let (provider, value) = injected("normal", BrowserConfig::default());
    assert_eq!(call_count(&value), 0);
    assert_eq!(
        provider
            .request(&endpoint("/rpc"), "quai_blockNumber", json!([]))
            .await
            .unwrap(),
        json!("0x17")
    );
    assert_eq!(call_count(&value), 2);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&call_json(&value, 1)).unwrap(),
        json!({"method":"quai_blockNumber","params":[],"shard":"0x00"})
    );
    assert!(matches!(
        provider
            .read(&endpoint("/rpc"), "quai_requestAccounts", json!([]))
            .await,
        Err(BrowserError::AuthorizationRequired)
    ));
    assert!(matches!(
        provider
            .read(&endpoint("/other"), "quai_blockNumber", json!([]))
            .await,
        Err(BrowserError::InvalidConfig)
    ));
    assert_eq!(call_count(&value), 2);
}
#[wasm_bindgen_test(async)]
async fn injected_chain_mismatch_and_user_rejection_fail_closed() {
    let (provider, value) = injected("wrong_chain", BrowserConfig::default());
    assert!(matches!(
        provider.request_accounts().await,
        Err(BrowserError::ChainMismatch)
    ));
    assert_eq!(call_count(&value), 1);
    let (provider, _) = injected("denied", BrowserConfig::default());
    let error = provider.request_accounts().await.unwrap_err();
    assert!(matches!(error, BrowserError::Provider(4001)));
    assert!(!format!("{error:?}").contains("SECRET"));
}
#[wasm_bindgen_test(async)]
async fn explicit_accounts_and_personal_sign_use_exact_wallet_arguments() {
    let (provider, value) = injected("normal", BrowserConfig::default());
    let address: QuaiAddress = "0x0049cda3305ccb9cb23e7ce2528cef555e9a5b32"
        .parse()
        .unwrap();
    assert_eq!(provider.request_accounts().await.unwrap(), vec![address]);
    let expected = public_fixture_signature(&quai_crypto::hash_message(b"hello"));
    install_signature(&value, expected);
    let signature = provider.personal_sign(address, b"hello").await.unwrap();
    assert_eq!(signature, expected);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&call_json(&value, 5)).unwrap(),
        json!({"method":"personal_sign","params":["0x68656c6c6f","0x0049cda3305ccb9cb23e7ce2528cef555e9a5b32"],"shard":"0x00"})
    );
}
#[wasm_bindgen_test(async)]
async fn injected_limits_accessors_timeout_and_future_drop() {
    for mode in ["oversize", "accessor"] {
        let (provider, _) = injected(
            mode,
            BrowserConfig {
                max_response_bytes: 100,
                ..Default::default()
            },
        );
        assert!(
            provider
                .read(&endpoint("/rpc"), "quai_blockNumber", json!([]))
                .await
                .is_err()
        );
    }
    let (provider, value) = injected(
        "hang",
        BrowserConfig {
            request_timeout_ms: 30,
            max_in_flight: 1,
            ..Default::default()
        },
    );
    assert!(matches!(
        provider.accounts().await,
        Err(BrowserError::Rpc(RpcError::Timeout))
    ));
    let mut pending = Box::pin(provider.accounts());
    assert!(futures_util::poll!(&mut pending).is_pending());
    assert!(matches!(provider.accounts().await, Err(BrowserError::Busy)));
    drop(pending);
    finish_provider(&value);
    assert!(matches!(
        provider.accounts().await,
        Err(BrowserError::Rpc(RpcError::Timeout))
    ));
}
#[wasm_bindgen_test]
fn browser_entropy_is_explicit_and_bounded() {
    let mut bytes = [0; 32];
    fill_random(&mut bytes).unwrap();
    assert_ne!(bytes, [0; 32]);
    assert!(fill_random(&mut vec![0; 65537]).is_err());
}

#[wasm_bindgen_test(async)]
async fn signing_requires_exposed_matching_zone_and_rejects_unicode_signature() {
    let address: QuaiAddress = "0x0049cda3305ccb9cb23e7ce2528cef555e9a5b32"
        .parse()
        .unwrap();
    let (provider, value) = injected("unavailable", BrowserConfig::default());
    assert!(matches!(
        provider.personal_sign(address, b"hi").await,
        Err(BrowserError::AccountUnavailable)
    ));
    assert_eq!(call_count(&value), 2);
    let wrong_zone: QuaiAddress = "0x1000000000000000000000000000000000000000"
        .parse()
        .unwrap();
    assert!(matches!(
        provider.personal_sign(wrong_zone, b"hi").await,
        Err(BrowserError::InvalidConfig)
    ));
    assert_eq!(call_count(&value), 2);
    let (provider, _) = injected("unicode_signature", BrowserConfig::default());
    assert!(matches!(
        provider.personal_sign(address, b"hi").await,
        Err(BrowserError::InvalidResult)
    ));
}

#[wasm_bindgen_test(async)]
async fn injected_signatures_verify_exact_message_account_and_typed_domain() {
    use quai_signer::{DomainPolicy, TypedData};
    let address: QuaiAddress = "0x0049cda3305ccb9cb23e7ce2528cef555e9a5b32"
        .parse()
        .unwrap();
    let (provider, wallet) = injected("normal", BrowserConfig::default());
    install_signature(
        &wallet,
        public_fixture_signature(&quai_crypto::hash_message(b"different")),
    );
    assert!(matches!(
        provider.personal_sign(address, b"hello").await,
        Err(BrowserError::InvalidResult)
    ));
    let document = json!({"types":{"Transfer":[{"name":"amount","type":"uint256"}]},"primaryType":"Transfer","domain":{"chainId":"15000"},"message":{"amount":"9007199254740993"}});
    let data = TypedData::from_json(&serde_json::to_vec(&document).unwrap()).unwrap();
    let signature = public_fixture_signature(data.signing_hash().bytes());
    install_signature(&wallet, signature);
    assert_eq!(
        provider
            .sign_typed_data(address, &data, DomainPolicy::RequireChainId)
            .await
            .unwrap(),
        signature
    );
    let payload: serde_json::Value = serde_json::from_str(&call_json(&wallet, 5)).unwrap();
    assert_eq!(payload["method"], "quai_signTypedData_v4");
    assert_eq!(payload["shard"], "0x00");
    let encoded: serde_json::Value =
        serde_json::from_str(payload["params"][1].as_str().unwrap()).unwrap();
    assert_eq!(encoded["message"]["amount"], "9007199254740993");
    let mut wrong = document.clone();
    wrong["domain"]["chainId"] = json!(9);
    let wrong = TypedData::from_json(&serde_json::to_vec(&wrong).unwrap()).unwrap();
    let before = call_count(&wallet);
    assert!(matches!(
        provider
            .sign_typed_data(address, &wrong, DomainPolicy::RequireChainId)
            .await,
        Err(BrowserError::ChainMismatch)
    ));
    assert_eq!(before, call_count(&wallet));
    let mut unbound = document;
    unbound["domain"] = json!({});
    let unbound = TypedData::from_json(&serde_json::to_vec(&unbound).unwrap()).unwrap();
    assert!(matches!(
        provider
            .sign_typed_data(address, &unbound, DomainPolicy::RequireChainId)
            .await,
        Err(BrowserError::ChainMismatch)
    ));
    install_signature(
        &wallet,
        public_fixture_signature(unbound.signing_hash().bytes()),
    );
    assert!(
        provider
            .sign_typed_data(address, &unbound, DomainPolicy::AllowUnbound)
            .await
            .is_ok()
    );
    // A valid signature from a different key must never pass account binding.
    let other = quai_crypto::SecretKey::from_bytes(&[1; 32])
        .unwrap()
        .sign_prehash(data.signing_hash().bytes())
        .unwrap()
        .to_quais_bytes()
        .unwrap();
    install_signature(&wallet, other);
    assert!(matches!(
        provider
            .sign_typed_data(address, &data, DomainPolicy::RequireChainId)
            .await,
        Err(BrowserError::InvalidResult)
    ));
}

#[wasm_bindgen_test(async)]
async fn account_chain_and_disconnect_events_invalidate_pending_signatures_and_cleanup() {
    let (provider, value) = injected("change_during_sign", BrowserConfig::default());
    assert!(provider.monitors_context_events());
    assert_eq!(listener_count(&value), 3);
    let address: QuaiAddress = "0x0049cda3305ccb9cb23e7ce2528cef555e9a5b32"
        .parse()
        .unwrap();
    install_signature(
        &value,
        public_fixture_signature(&quai_crypto::hash_message(b"hello")),
    );
    assert!(matches!(
        provider.personal_sign(address, b"hello").await,
        Err(BrowserError::ContextChanged)
    ));
    assert_eq!(provider.context_revision(), 1);
    let clone = provider.clone();
    emit_context_change(&value, "disconnect");
    assert_eq!(clone.context_revision(), 2);
    drop(provider);
    assert_eq!(listener_count(&value), 3);
    drop(clone);
    assert_eq!(listener_count(&value), 0);
}

#[wasm_bindgen_test(async)]
async fn indexeddb_snapshots_are_atomic_scoped_persistent_and_keep_tombstone_revisions() {
    use quai_browser::{BrowserSnapshotStore, BrowserStorageScope};
    let scope = BrowserStorageScope {
        chain_id: U256::from(15000),
        genesis: quai_primitives::Hash32::from_bytes([1; 32]),
        zone: Zone::Cyprus1,
        wallet: quai_primitives::Hash32::from_bytes([2; 32]),
    };
    let mut random = [0; 8];
    fill_random(&mut random).unwrap();
    let name = format!("snapshot-test-{}", u64::from_be_bytes(random));
    let first = BrowserSnapshotStore::open(&name, scope, 1024)
        .await
        .unwrap();
    let second = BrowserSnapshotStore::open(&name, scope, 1024)
        .await
        .unwrap();
    assert!(first.read().await.unwrap().is_none());
    let (a, b) = futures_util::future::join(
        first.compare_exchange(None, Some(b"public-a")),
        second.compare_exchange(None, Some(b"public-b")),
    )
    .await;
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(
        matches!(a, Err(BrowserError::StorageConflict))
            || matches!(b, Err(BrowserError::StorageConflict))
    );
    let record = second.read().await.unwrap().unwrap();
    assert_eq!(record.revision, 1);
    assert!(matches!(
        record.bytes.as_deref(),
        Some(b"public-a") | Some(b"public-b")
    ));
    let other = BrowserSnapshotStore::open(
        &name,
        BrowserStorageScope {
            wallet: quai_primitives::Hash32::from_bytes([3; 32]),
            ..scope
        },
        1024,
    )
    .await
    .unwrap();
    assert!(other.read().await.unwrap().is_none());
    assert!(
        first
            .compare_exchange(Some(1), Some(&[0; 1025]))
            .await
            .is_err()
    );
    assert_eq!(first.compare_exchange(Some(1), None).await.unwrap(), 2);
    assert!(matches!(
        second.compare_exchange(None, Some(b"stale")).await,
        Err(BrowserError::StorageConflict)
    ));
    drop(first);
    drop(second);
    let reopened = BrowserSnapshotStore::open(&name, scope, 1024)
        .await
        .unwrap();
    let record = reopened.read().await.unwrap().unwrap();
    assert_eq!(record.revision, 2);
    assert!(record.bytes.is_none());
    assert_eq!(
        reopened
            .compare_exchange(Some(2), Some(b"restored"))
            .await
            .unwrap(),
        3
    );
}

#[path = "support/socket.rs"]
mod socket_tests;
