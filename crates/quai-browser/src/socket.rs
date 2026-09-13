//! Non-Send browser/worker WebSocket RPC and bounded notification streams.
use crate::{BrowserSocketConfig, decode_response, encode};
use quai_rpc::{Endpoint, RpcError, Transport};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{cell::Cell, fmt, rc::Rc};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/src/socket.js")]
extern "C" {
    #[wasm_bindgen(catch, js_name = openSocket)]
    async fn open_socket(
        url: &str,
        timeout: u32,
        max_bytes: usize,
        max_request: usize,
        max_flight: usize,
        max_subs: usize,
        max_queue: usize,
        max_queued_bytes: usize,
        controller: &JsValue,
    ) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch, js_name = acknowledgeSocket)]
    fn acknowledge_socket(handle: &JsValue) -> Result<(), JsValue>;
    #[wasm_bindgen(js_name = closeSocket)]
    fn close_socket(handle: &JsValue);
    #[wasm_bindgen(js_name = socketOpenState)]
    fn socket_open_state(handle: &JsValue) -> bool;
    #[wasm_bindgen(catch, js_name = socketNextId)]
    fn socket_next_id(handle: &JsValue) -> Result<f64, JsValue>;
    #[wasm_bindgen(catch, js_name = socketRequest)]
    async fn socket_request(
        handle: &JsValue,
        id: f64,
        body: &str,
        kind: &str,
        controller: &JsValue,
    ) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch, js_name = socketNext)]
    async fn socket_next(
        handle: &JsValue,
        id: &str,
        controller: &JsValue,
    ) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch, js_name = acknowledgeNotification)]
    fn acknowledge_notification(handle: &JsValue, id: &str) -> Result<(), JsValue>;
    #[wasm_bindgen(catch, js_name = acknowledgeSubscription)]
    fn acknowledge_subscription(handle: &JsValue, id: &str) -> Result<(), JsValue>;
    #[wasm_bindgen(js_name = forgetSubscription)]
    fn forget_subscription(handle: &JsValue, id: &str);
    #[wasm_bindgen(js_name = dropSubscription)]
    fn drop_subscription(handle: &JsValue, id: &str);
}
#[wasm_bindgen(module = "/src/bridge.js")]
extern "C" {
    #[wasm_bindgen(catch, js_name = newAbort)]
    fn new_abort() -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_name = abort)]
    fn abort(controller: &JsValue);
    #[wasm_bindgen(js_name = errorKind)]
    fn error_kind(error: &JsValue) -> String;
}
struct Abort(JsValue);
impl Abort {
    fn new() -> Result<Self, RpcError> {
        new_abort().map(Self).map_err(convert)
    }
}
impl Drop for Abort {
    fn drop(&mut self) {
        abort(&self.0);
    }
}
fn convert(error: JsValue) -> RpcError {
    match error_kind(&error).as_str() {
        "closed" => RpcError::Disconnected,
        "subscription_closed" => RpcError::SubscriptionClosed,
        "lagged" => RpcError::SubscriptionLagged,
        "busy" => RpcError::AtCapacity,
        "timeout" => RpcError::Timeout,
        "response_size" => RpcError::ResponseTooLarge,
        "request_size" => RpcError::RequestTooLarge,
        "invalid" => RpcError::InvalidResponse("invalid browser WebSocket message"),
        "id_exhausted" => RpcError::RequestIdExhausted,
        _ => RpcError::Transport,
    }
}
struct Session {
    handle: JsValue,
    endpoint: Endpoint,
    config: BrowserSocketConfig,
    active: Cell<usize>,
}
struct Permit(Rc<Session>);
impl Drop for Permit {
    fn drop(&mut self) {
        self.0.active.set(self.0.active.get() - 1);
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        close_socket(&self.handle);
    }
}
/// One explicit browser/worker WebSocket connection. Clones share bounded requests
/// and subscriptions. No reconnect, retry, chain inference or write replay occurs.
/// Last-owner drop closes the socket; an active subscription also owns the session.
#[derive(Clone)]
pub struct BrowserWebSocketTransport(Rc<Session>);
impl fmt::Debug for BrowserWebSocketTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BrowserWebSocketTransport")
            .field("endpoint", &self.0.endpoint)
            .field("open", &self.is_open())
            .finish()
    }
}
impl BrowserWebSocketTransport {
    /// Connect to one exact ws/wss endpoint with a bounded opening deadline.
    /// Cancellation closes an incomplete connection. Works without Window or Tokio.
    pub async fn connect(
        endpoint: Endpoint,
        config: BrowserSocketConfig,
    ) -> Result<Self, RpcError> {
        config.validate().map_err(|_| RpcError::InvalidConfig)?;
        if !matches!(endpoint.scheme(), "ws" | "wss") {
            return Err(RpcError::InvalidConfig);
        }
        let guard = Abort::new()?;
        let handle = open_socket(
            endpoint.as_str(),
            config.rpc.request_timeout_ms,
            config.rpc.max_response_bytes,
            config.rpc.max_request_bytes,
            config.rpc.max_in_flight,
            config.max_subscriptions,
            config.max_notifications,
            config.max_queued_bytes,
            &guard.0,
        )
        .await
        .map_err(convert)?;
        acknowledge_socket(&handle).map_err(convert)?;
        Ok(Self(Rc::new(Session {
            handle,
            endpoint,
            config,
            active: Cell::new(0),
        })))
    }
    /// Current local connection status; this is not a liveness or chain check.
    pub fn is_open(&self) -> bool {
        socket_open_state(&self.0.handle)
    }
    /// Close this shared session and fail pending operations without replay.
    pub fn close(&self) {
        close_socket(&self.0.handle);
    }
    async fn send(&self, method: &str, params: Value, kind: &str) -> Result<Value, RpcError> {
        if method.is_empty()
            || method.len() > 128
            || !method
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.'))
            || !(params.is_array() || params.is_object())
        {
            return Err(RpcError::InvalidConfig);
        }
        if self.0.active.get() >= self.0.config.rpc.max_in_flight {
            return Err(RpcError::AtCapacity);
        }
        self.0.active.set(self.0.active.get() + 1);
        let _permit = Permit(self.0.clone());
        let id = socket_next_id(&self.0.handle).map_err(convert)?;
        if !(1.0..=9_007_199_254_740_991.0).contains(&id) || id.fract() != 0.0 {
            return Err(RpcError::RequestIdExhausted);
        }
        let body = encode(
            &json!({"jsonrpc":"2.0","id":id as u64,"method":method,"params":params}),
            self.0.config.rpc.max_request_bytes,
        )?;
        let guard = Abort::new()?;
        let result = socket_request(&self.0.handle, id, &body, kind, &guard.0)
            .await
            .map_err(convert)?;
        let text = result
            .as_string()
            .ok_or(RpcError::InvalidResponse("invalid WebSocket bridge result"))?;
        let decoded = decode_response(text.as_bytes(), id as u64);
        if kind == "subscribe"
            && let Ok(Value::String(id)) = &decoded
        {
            acknowledge_subscription(&self.0.handle, id).map_err(convert)?;
        }
        if matches!(decoded, Err(RpcError::InvalidResponse(_))) {
            self.close();
        }
        decoded
    }
    /// Subscribe with explicit node parameters, e.g. `["newHeads"]`. No unbounded
    /// pre-registration queue exists: the bridge registers before delivering its reply.
    /// A cancelled/timed-out subscribe closes the session to avoid an unknown remote ID.
    pub async fn subscribe(&self, params: Value) -> Result<BrowserSubscription, RpcError> {
        if !params.is_array() || params.as_array().is_none_or(|p| p.is_empty()) {
            return Err(RpcError::InvalidConfig);
        }
        let id = self.send("quai_subscribe", params, "subscribe").await?;
        let id = id
            .as_str()
            .ok_or(RpcError::InvalidResponse("invalid subscription ID"))?
            .to_owned();
        Ok(BrowserSubscription {
            transport: self.clone(),
            id,
            active: true,
        })
    }
}
impl Transport for BrowserWebSocketTransport {
    async fn request(
        &self,
        endpoint: &Endpoint,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        if endpoint != &self.0.endpoint || matches!(method, "quai_subscribe" | "quai_unsubscribe") {
            return Err(RpcError::InvalidConfig);
        }
        self.send(method, params, "request").await
    }
}
/// One bounded subscription. Queue overflow is explicit and requires backfill;
/// receipt of a notification is never a canonicality or finality assertion.
/// Drop attempts bounded unsubscribe; if cleanup capacity is exhausted it closes
/// the shared session. Use explicit unsubscribe when completion matters.
pub struct BrowserSubscription {
    transport: BrowserWebSocketTransport,
    id: String,
    active: bool,
}
impl fmt::Debug for BrowserSubscription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BrowserSubscription")
            .field("active", &self.active)
            .finish_non_exhaustive()
    }
}
impl BrowserSubscription {
    /// Await one notification with the configured request deadline. Quiet timeout
    /// does not end the subscription. Cancelling before delivery releases wait
    /// capacity; cancellation after JS delivery marks the stream lagged so a
    /// notification cannot disappear without an explicit continuity error.
    pub async fn next(&mut self) -> Result<Value, RpcError> {
        if !self.active {
            return Err(RpcError::SubscriptionClosed);
        }
        let guard = Abort::new()?;
        let raw = socket_next(&self.transport.0.handle, &self.id, &guard.0)
            .await
            .map_err(convert)?;
        let raw = raw.as_string().ok_or(RpcError::InvalidResponse(
            "invalid subscription bridge result",
        ))?;
        #[derive(Deserialize)]
        struct Params {
            subscription: String,
            result: Value,
        }
        #[derive(Deserialize)]
        struct Envelope {
            jsonrpc: String,
            method: String,
            params: Params,
        }
        let envelope: Envelope = serde_json::from_str(&raw).map_err(|_| {
            self.transport.close();
            RpcError::InvalidResponse("malformed subscription envelope")
        })?;
        if envelope.jsonrpc != "2.0"
            || envelope.method != "quai_subscription"
            || envelope.params.subscription != self.id
        {
            self.transport.close();
            return Err(RpcError::InvalidResponse("subscription identity mismatch"));
        }
        acknowledge_notification(&self.transport.0.handle, &self.id).map_err(convert)?;
        Ok(envelope.params.result)
    }
    /// End locally and request remote unsubscribe exactly once. Failure remains
    /// explicit; disconnect/reconnect and application backfill are caller-controlled.
    pub async fn unsubscribe(mut self) -> Result<(), RpcError> {
        forget_subscription(&self.transport.0.handle, &self.id);
        let result = match self
            .transport
            .send("quai_unsubscribe", json!([self.id]), "request")
            .await
        {
            Ok(Value::Bool(true)) => Ok(()),
            Ok(_) => {
                self.transport.close();
                Err(RpcError::InvalidResponse(
                    "unsubscribe was not acknowledged",
                ))
            }
            Err(error) => {
                self.transport.close();
                Err(error)
            }
        };
        self.active = false;
        result
    }
}
impl Drop for BrowserSubscription {
    fn drop(&mut self) {
        if self.active {
            drop_subscription(&self.transport.0.handle, &self.id);
        }
    }
}
