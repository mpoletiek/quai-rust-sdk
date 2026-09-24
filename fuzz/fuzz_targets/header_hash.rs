#![no_main]
//! Header hash recomputation on hostile JSON: never panics, answers the same
//! way twice, and accepts only a header whose recomputed hash is the one it
//! reports.
use libfuzzer_sys::fuzz_target;
use quai_primitives::Hash32;
use quai_provider::header_hash::verify_header_hash;
use serde_json::Value;

fuzz_target!(|data: &[u8]| {
    let Ok(value) = serde_json::from_slice::<Value>(data) else {
        return;
    };
    let first = verify_header_hash(&value);
    assert_eq!(first, verify_header_hash(&value));
    if let Ok(header) = first {
        let reported = value["woHeader"]["headerHash"]
            .as_str()
            .and_then(|h| h.parse::<Hash32>().ok());
        assert_eq!(Some(header.header_hash), reported);
        assert!(!header.hash_verified || header.seal_hash.is_some());
    }
});
