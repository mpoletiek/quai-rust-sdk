//! Shared transaction interchange tests in native and browser workers.
#![cfg(any(feature = "wallet", feature = "abi"))]
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
#[path = "fixtures/shared/crates/quai-consensus/tests/documents.rs"]
mod shared;
