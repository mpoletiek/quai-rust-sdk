//! Shared dictionary/phrase/seed tests in native and actual browser workers.
#![cfg(feature = "wallet")]
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
#[path = "fixtures/shared/crates/quai-wallet/tests/wordlists.rs"]
mod shared;
