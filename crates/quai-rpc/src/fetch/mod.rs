//! Bounded general resource fetching, separate from JSON-RPC submission policy.
mod client;
mod model;
pub use client::*;
pub use model::*;
#[cfg(all(feature = "http", not(target_arch = "wasm32")))]
mod native;
#[cfg(all(feature = "http", not(target_arch = "wasm32")))]
pub use native::NativeFetch;
