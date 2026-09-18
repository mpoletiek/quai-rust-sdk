//! Explicit shard routing and a bounded JSON-RPC transport.
//!
//! Routing has no network side effects. Native HTTP and WebSocket transports are
//! optional; native futures are Send while browser adapters have a separate bound.

/// General resource requests with explicit limits and lifecycle.
pub mod fetch;

mod quantity;
mod routing;
mod transport;

pub use quantity::{QuantityError, U256, parse_quantity};
pub use routing::{Endpoint, RouteError, Routing, parse_use_pathing};
pub use transport::{BatchResult, MAX_BATCH_CALLS, RemoteError, RpcError, Transport};

#[cfg(not(target_arch = "wasm32"))]
mod dyn_transport;
#[cfg(not(target_arch = "wasm32"))]
pub use dyn_transport::DynTransport;

#[cfg(all(feature = "http", not(target_arch = "wasm32")))]
mod http;
#[cfg(all(feature = "http", not(target_arch = "wasm32")))]
pub use http::{HttpConfig, HttpTransport};

#[cfg(all(feature = "ws", not(target_arch = "wasm32")))]
mod websocket;
#[cfg(all(feature = "ws", not(target_arch = "wasm32")))]
pub use websocket::{WsConfig, WsSubscription, WsSubscriptionKind, WsTransport};

/// Internal decoders exposed for fuzzing only.
///
/// Not public API: the `fuzzing` feature is off by default, these items are
/// hidden from documentation, and their signatures may change without notice.
/// They exist so the fuzz harness can reach the JSON-RPC envelope decoders,
/// which every byte a remote node sends passes through, without widening the
/// supported surface for ordinary consumers.
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzz_internals {
    /// Decode one JSON-RPC response envelope against its expected request ID.
    #[cfg(all(any(feature = "http", feature = "ws"), not(target_arch = "wasm32")))]
    pub fn decode_response(
        bytes: &[u8],
        expected_id: u64,
    ) -> Result<serde_json::Value, crate::RpcError> {
        crate::transport::decode_response(bytes, expected_id)
    }

    /// Decode a JSON-RPC batch response against its issued ID window.
    #[cfg(all(feature = "http", not(target_arch = "wasm32")))]
    pub fn decode_batch(bytes: &[u8], first: u64, count: usize) -> crate::BatchResult {
        crate::http::decode_batch(bytes, first, count)
    }

    /// Route WebSocket frames through the session dispatcher against a
    /// synthetic session that `layout` selects, asserting reply routing.
    #[cfg(all(feature = "ws", not(target_arch = "wasm32")))]
    pub fn dispatch_frames(layout: u8, frames: &[&[u8]]) {
        crate::websocket::fuzz_dispatch(layout, frames)
    }
}
