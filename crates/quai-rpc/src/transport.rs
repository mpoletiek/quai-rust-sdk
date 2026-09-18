use crate::Endpoint;
use serde::Deserialize;
use serde_json::Value;
use std::fmt;
use thiserror::Error;

/// JSON-RPC error details. Message/data are available explicitly but redacted in diagnostics.
#[derive(Clone, Deserialize)]
pub struct RemoteError {
    /// Remote machine-readable error code.
    pub code: i64,
    /// Untrusted remote message; may echo sensitive request values.
    pub message: String,
    /// Untrusted remote details; inspect explicitly rather than logging blindly.
    pub data: Option<Value>,
}

impl fmt::Display for RemoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "remote JSON-RPC error (code {})", self.code)
    }
}
impl fmt::Debug for RemoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RemoteError")
            .field("code", &self.code)
            .finish_non_exhaustive()
    }
}
impl std::error::Error for RemoteError {}

/// Bounded transport/protocol failures; no raw URL or response body is displayed.
#[derive(Clone, Debug, Error)]
#[non_exhaustive]
pub enum RpcError {
    /// A transport configured for fail-fast concurrency is at its active-operation limit.
    #[error("RPC transport concurrency limit reached")]
    AtCapacity,
    /// The WebSocket session closed. Pending requests are never replayed automatically.
    #[error("RPC WebSocket session disconnected")]
    Disconnected,
    /// A bounded subscription queue overflowed; backfill or resubscription is required.
    #[error("RPC subscription lagged; notification continuity is lost")]
    SubscriptionLagged,
    /// Subscription ended locally or was explicitly unsubscribed.
    #[error("RPC subscription closed")]
    SubscriptionClosed,
    /// Outgoing encoded JSON exceeded the configured message limit.
    #[error("RPC request exceeds size limit")]
    RequestTooLarge,
    /// Total request deadline, including time waiting for a concurrency permit.
    #[error("RPC request timed out")]
    Timeout,
    /// Network/TLS failure; details are intentionally not echoed.
    #[error("RPC transport failed")]
    Transport,
    /// HTTP failure without exposing an arbitrary response body.
    #[error("RPC HTTP status {0}")]
    HttpStatus(u16),
    /// Response length exceeded the configured bound.
    #[error("RPC response exceeds size limit")]
    ResponseTooLarge,
    /// JSON-RPC envelope could not be validated.
    #[error("invalid JSON-RPC response: {0}")]
    InvalidResponse(&'static str),
    /// Remote JSON-RPC failure.
    #[error(transparent)]
    Remote(#[from] RemoteError),
    /// Request ID space exhausted; never wrap and reuse an ID.
    #[error("RPC request ID space exhausted")]
    RequestIdExhausted,
    /// Invalid transport settings or endpoint scheme.
    #[error("invalid HTTP transport configuration")]
    InvalidConfig,
}

/// A transport independent of routing and key material.
///
/// Implementations must bound untrusted data and correlate response IDs.
/// Dropping a returned future cancels the caller's wait; it does not establish
/// whether a state-changing request was accepted by a remote node.
#[allow(async_fn_in_trait)]
pub trait Transport {
    /// Optional explicit batch capability, bounded by the concrete transport.
    /// `None` means unsupported and guarantees that no requests were sent.
    /// An outer error leaves acceptance unknown; callers must not silently replay.
    /// Per-request remote errors retain their input positions.
    #[cfg(not(target_arch = "wasm32"))]
    fn request_batch(
        &self,
        _endpoint: &Endpoint,
        _requests: Vec<(&str, Value)>,
    ) -> impl std::future::Future<Output = Option<BatchResult>> + Send {
        async { None }
    }
    /// Optional browser-compatible batch capability; no requests on `None`.
    #[cfg(target_arch = "wasm32")]
    async fn request_batch(
        &self,
        _endpoint: &Endpoint,
        _requests: Vec<(&str, Value)>,
    ) -> Option<BatchResult> {
        None
    }
    /// Send a request to this explicit endpoint. Params must be an array or object.
    /// Native futures are `Send`; browser transports may use thread-local browser APIs.
    #[cfg(not(target_arch = "wasm32"))]
    fn request(
        &self,
        endpoint: &Endpoint,
        method: &str,
        params: Value,
    ) -> impl std::future::Future<Output = Result<Value, RpcError>> + Send;
    /// Send a request using a browser-compatible future without a `Send` requirement.
    #[cfg(target_arch = "wasm32")]
    async fn request(
        &self,
        endpoint: &Endpoint,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError>;
}

/// Entire batch transport/envelope result, containing ordered per-request results.
pub type BatchResult = Result<Vec<Result<Value, RpcError>>, RpcError>;

#[cfg(all(any(feature = "http", feature = "ws"), not(target_arch = "wasm32")))]
pub(crate) fn decode_response(bytes: &[u8], expected_id: u64) -> Result<Value, RpcError> {
    #[derive(Default)]
    enum ResultSlot {
        #[default]
        Absent,
        Present(Value),
    }
    fn slot<'de, D: serde::Deserializer<'de>>(d: D) -> Result<ResultSlot, D::Error> {
        Value::deserialize(d).map(ResultSlot::Present)
    }
    #[derive(Deserialize)]
    struct Envelope {
        jsonrpc: String,
        id: u64,
        #[serde(default, deserialize_with = "slot")]
        result: ResultSlot,
        // Option would accept error:null, which is not a valid error object.
        #[serde(default, deserialize_with = "slot")]
        error: ResultSlot,
    }
    let envelope: Envelope = serde_json::from_slice(bytes)
        .map_err(|_| RpcError::InvalidResponse("malformed envelope"))?;
    if envelope.jsonrpc != "2.0" {
        return Err(RpcError::InvalidResponse("wrong protocol version"));
    }
    if envelope.id != expected_id {
        return Err(RpcError::InvalidResponse("request ID mismatch"));
    }
    match (envelope.result, envelope.error) {
        (ResultSlot::Present(result), ResultSlot::Absent) => Ok(result),
        (ResultSlot::Absent, ResultSlot::Present(error)) => {
            let error: RemoteError = serde_json::from_value(error)
                .map_err(|_| RpcError::InvalidResponse("malformed error"))?;
            Err(RpcError::Remote(error))
        }
        _ => Err(RpcError::InvalidResponse(
            "expected exactly one result or error",
        )),
    }
}
