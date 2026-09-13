//! Shared ABI declaration/formatting tests in native facade and actual worker.
#![cfg(feature = "abi")]
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
#[path = "fixtures/shared/crates/quai-abi/tests/human.rs"]
mod shared;
