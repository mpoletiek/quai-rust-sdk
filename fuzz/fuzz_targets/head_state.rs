#![no_main]
use libfuzzer_sys::fuzz_target;
use quai_primitives::{Hash32, Zone};
use quai_provider::{HeadTracker, MAX_HEAD_STATE_BYTES};
fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_HEAD_STATE_BYTES { return; }
    let genesis = Hash32::from_bytes([1;32]);
    if let Ok(tracker) = HeadTracker::from_state(data, Zone::Cyprus1, genesis) {
        assert_eq!(tracker.export_state(), data);
        assert!(HeadTracker::from_state(data, Zone::Cyprus2, genesis).is_err());
        assert!(HeadTracker::from_state(data, Zone::Cyprus1, Hash32::from_bytes([2;32])).is_err());
        assert_eq!(HeadTracker::from_state(&tracker.export_state(), Zone::Cyprus1, genesis).unwrap().checkpoint(), tracker.checkpoint());
    }
});
