//! Shared actual WebSocket coverage in both window and dedicated-worker runtimes.
use quai_browser::{BrowserConfig, BrowserSocketConfig, BrowserWebSocketTransport};
use quai_rpc::{Endpoint, RpcError, Transport};
use serde_json::json;
use wasm_bindgen_test::*;
fn endpoint(path: &str) -> Endpoint {
    let base = option_env!("QUAI_BROWSER_FIXTURE_URL").unwrap_or("http://127.0.0.1:18080");
    Endpoint::parse(&format!("{}{}", base.replacen("http://", "ws://", 1), path)).unwrap()
}
fn config() -> BrowserSocketConfig {
    BrowserSocketConfig::default().with_rpc(BrowserConfig::default().with_request_timeout_ms(200))
}
#[wasm_bindgen_test(async)]
async fn websocket_provider_reads_immediate_subscription_and_explicit_reconnect() {
    let url = endpoint("/socket/normal");
    let transport = BrowserWebSocketTransport::connect(url.clone(), config())
        .await
        .unwrap();
    let provider = quai_provider::Provider::new(
        transport.clone(),
        quai_rpc::Routing::direct(url.as_str(), quai_primitives::Zone::Cyprus1.into()).unwrap(),
        quai_rpc::U256::from(15000),
    );
    assert_eq!(
        provider
            .chain_id(quai_primitives::Zone::Cyprus1.into())
            .await
            .unwrap(),
        quai_rpc::U256::from(15000)
    );
    let mut sub = transport.subscribe(json!(["newHeads"])).await.unwrap();
    assert_eq!(sub.next().await.unwrap()["number"], "0x10");
    sub.unsubscribe().await.unwrap();
    assert!(matches!(
        transport
            .request(&endpoint("/socket/other"), "quai_chainId", json!([]))
            .await,
        Err(RpcError::InvalidConfig)
    ));
    transport.close();
    assert!(!transport.is_open());
    assert!(matches!(
        transport.request(&url, "quai_chainId", json!([])).await,
        Err(RpcError::Disconnected)
    ));
    let reopened = BrowserWebSocketTransport::connect(url.clone(), config())
        .await
        .unwrap();
    assert_eq!(
        reopened
            .request(&url, "quai_chainId", json!([]))
            .await
            .unwrap(),
        json!("0x3a98")
    );
    reopened.close();
}
#[wasm_bindgen_test(async)]
async fn websocket_timeouts_cancellation_and_capacity_leave_no_request_slot_leak() {
    let url = endpoint("/socket/normal");
    let mut limits = config();
    limits.rpc.max_in_flight = 1;
    limits.rpc.request_timeout_ms = 200;
    let transport = BrowserWebSocketTransport::connect(url.clone(), limits)
        .await
        .unwrap();
    assert!(matches!(
        transport.request(&url, "fixture_never", json!([])).await,
        Err(RpcError::Timeout)
    ));
    let mut pending = Box::pin(transport.request(&url, "fixture_never", json!([])));
    assert!(futures_util::poll!(&mut pending).is_pending());
    assert!(matches!(
        transport.request(&url, "quai_chainId", json!([])).await,
        Err(RpcError::AtCapacity)
    ));
    drop(pending);
    assert_eq!(
        transport
            .request(&url, "quai_chainId", json!([]))
            .await
            .unwrap(),
        json!("0x3a98")
    );
    let mut sub = transport.subscribe(json!(["quiet"])).await.unwrap();
    let mut waiting = Box::pin(sub.next());
    assert!(futures_util::poll!(&mut waiting).is_pending());
    drop(waiting);
    assert!(matches!(sub.next().await, Err(RpcError::Timeout)));
    sub.unsubscribe().await.unwrap();
    transport.close();
}
#[wasm_bindgen_test(async)]
async fn websocket_lag_and_disconnect_are_explicit_without_silent_continuity() {
    let url = endpoint("/socket/normal");
    let mut limits = config();
    limits.max_notifications = 1;
    let transport = BrowserWebSocketTransport::connect(url.clone(), limits)
        .await
        .unwrap();
    let mut sub = transport.subscribe(json!(["burst"])).await.unwrap();
    transport
        .request(&url, "quai_chainId", json!([]))
        .await
        .unwrap();
    assert!(matches!(
        sub.next().await,
        Err(RpcError::SubscriptionLagged)
    ));
    sub.unsubscribe().await.unwrap();
    let mut quiet = transport.subscribe(json!(["quiet"])).await.unwrap();
    assert!(matches!(
        transport.request(&url, "fixture_close", json!([])).await,
        Err(RpcError::Disconnected)
    ));
    assert!(matches!(quiet.next().await, Err(RpcError::Disconnected)));
}
#[wasm_bindgen_test(async)]
async fn websocket_rejects_malformed_duplicate_binary_and_oversize_wire_responses() {
    for path in [
        "/socket/wrong-id",
        "/socket/duplicate-id",
        "/socket/binary",
        "/socket/oversize",
    ] {
        let url = endpoint(path);
        let mut limits = config();
        limits.rpc.max_response_bytes = 128;
        let transport = BrowserWebSocketTransport::connect(url.clone(), limits)
            .await
            .unwrap();
        let result = transport.request(&url, "quai_chainId", json!([])).await;
        if path.ends_with("oversize") {
            assert!(matches!(result, Err(RpcError::ResponseTooLarge)));
        } else {
            assert!(matches!(result, Err(RpcError::InvalidResponse(_))));
        }
        assert!(!transport.is_open());
    }
    let url = endpoint("/socket/duplicate-notification");
    let transport = BrowserWebSocketTransport::connect(url, config())
        .await
        .unwrap();
    let mut sub = transport.subscribe(json!(["newHeads"])).await.unwrap();
    assert!(matches!(
        sub.next().await,
        Err(RpcError::InvalidResponse(_))
    ));
    assert!(!transport.is_open());
}
#[wasm_bindgen_test(async)]
async fn websocket_remote_errors_preserve_rpc_identity_without_closing() {
    let url = endpoint("/socket/remote");
    let transport = BrowserWebSocketTransport::connect(url.clone(), config())
        .await
        .unwrap();
    assert!(
        matches!(transport.request(&url,"quai_chainId",json!([])).await,Err(RpcError::Remote(error)) if error.code==-32000)
    );
    assert!(transport.is_open());
    transport.close();
}
