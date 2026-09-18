#![no_main]
//! JSON-RPC envelope decoding.
//!
//! Every byte a remote node sends reaches the SDK through these two decoders,
//! which made them the largest untrusted-input surface with no fuzz coverage.
//! The properties asserted here are the ones a response-confusion attack would
//! have to break: a decoded value never comes from the wrong request, and a
//! batch never returns a row for an ID outside the window that was issued.

use libfuzzer_sys::fuzz_target;
use quai_rpc::fuzz_internals::{decode_batch, decode_response};

fuzz_target!(|data: &[u8]| {
    // Derive the expected ID and batch window from the input so the fuzzer can
    // steer correlation, rather than pinning them to one constant.
    let (expected_id, first, count, body) = match data {
        [a, b, c, rest @ ..] => (u64::from(*a), u64::from(*b), usize::from(*c), rest),
        _ => (0, 0, 0, data),
    };

    // Single response. A decoded result must never be produced for a mismatched
    // ID: that is the property that stops one request's reply satisfying another.
    if let Ok(value) = decode_response(body, expected_id) {
        // The envelope carried this exact ID, and it parsed as JSON.
        let parsed: serde_json::Value =
            serde_json::from_slice(body).expect("a decoded envelope is valid JSON");
        assert_eq!(
            parsed.get("id").and_then(serde_json::Value::as_u64),
            Some(expected_id),
            "decode_response returned a value for a mismatched id"
        );
        // Success and failure are mutually exclusive in a decoded envelope.
        assert!(
            parsed.get("error").is_none_or(serde_json::Value::is_null),
            "decode_response returned a result alongside an error"
        );
        // Decoding is deterministic.
        let again = decode_response(body, expected_id).expect("decoding is deterministic");
        assert_eq!(value, again);
        // A different expected ID must not also accept the same envelope.
        let other = expected_id.wrapping_add(1);
        assert!(
            decode_response(body, other).is_err(),
            "the same envelope decoded under two different ids"
        );
    }

    // Batch response. `batch` bounds requests to 1..=128, so mirror that here
    // rather than exploring counts the transport would never issue.
    if (1..=128).contains(&count) {
        if let Ok(rows) = decode_batch(body, first, count) {
            assert_eq!(
                rows.len(),
                count,
                "a decoded batch must have exactly one row per issued request"
            );
            // Every row must correspond to an ID inside the issued window, and
            // each ID must appear exactly once. Reordering by the server is
            // allowed; duplicates, gaps and foreign IDs are not.
            let parsed: Vec<serde_json::Value> =
                serde_json::from_slice(body).expect("a decoded batch is a JSON array");
            let mut seen: Vec<u64> = parsed
                .iter()
                .filter_map(|row| row.get("id").and_then(serde_json::Value::as_u64))
                .collect();
            assert_eq!(seen.len(), count, "every row carries an id");
            seen.sort_unstable();
            seen.dedup();
            assert_eq!(seen.len(), count, "ids within a batch must be unique");
            let last = first
                .checked_add(count as u64 - 1)
                .expect("the window was decoded, so it cannot overflow");
            assert!(
                seen.iter().all(|id| (first..=last).contains(id)),
                "a batch row carried an id outside the issued window"
            );
            let again = decode_batch(body, first, count).expect("decoding is deterministic");
            assert_eq!(rows.len(), again.len());
        }
    }
});
