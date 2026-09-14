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
pub use transport::{RemoteError, RpcError, Transport};

#[cfg(all(feature = "http", not(target_arch = "wasm32")))]
mod http;
#[cfg(all(feature = "http", not(target_arch = "wasm32")))]
pub use http::{HttpConfig, HttpTransport};

#[cfg(all(feature = "ws", not(target_arch = "wasm32")))]
mod websocket;
#[cfg(all(feature = "ws", not(target_arch = "wasm32")))]
pub use websocket::{WsConfig, WsSubscription, WsSubscriptionKind, WsTransport};
