#![no_main]
//! WebSocket session dispatch.
//!
//! Every frame a node sends over a WebSocket passes through one stateful
//! dispatcher that routes replies to pending requests and notifications to
//! subscriptions. A routing mistake there is response confusion: one request
//! receiving another's answer. The first byte selects a synthetic session
//! (which requests are outstanding, subscribing or unsubscribing, and which
//! subscriptions are live); the rest is frames separated by 0xff, a byte that
//! never occurs in valid UTF-8 JSON.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&layout, rest)) = data.split_first() else {
        return;
    };
    let frames: Vec<&[u8]> = rest.split(|byte| *byte == 0xff).take(16).collect();
    quai_rpc::fuzz_internals::dispatch_frames(layout, &frames);
});
