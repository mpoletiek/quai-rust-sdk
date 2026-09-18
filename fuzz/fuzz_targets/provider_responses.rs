#![no_main]
//! Typed provider parsers for node responses: headers, blocks, outpoints and
//! pool content. They run on every poll, head notification, Qi refresh and
//! pending-transaction check, and feed reorg, balance and nonce decisions. The
//! first byte selects the parser; the rest must be JSON, as the transport
//! guarantees after envelope decoding.
use libfuzzer_sys::fuzz_target;
use quai_primitives::{Hash32, Zone};
use quai_provider::{MinedBlock, ZoneHeader, fuzz_internals};
use serde_json::Value;

fuzz_target!(|data: &[u8]| {
    let Some((&which, rest)) = data.split_first() else {
        return;
    };
    let Ok(value) = serde_json::from_slice::<Value>(rest) else {
        return;
    };
    match which % 4 {
        0 => {
            if let Ok(header) = ZoneHeader::try_from(value) {
                assert_ne!(header.hash, Hash32::ZERO);
            }
        }
        1 => {
            let selector = match which / 4 % 3 {
                0 => MinedBlock::Latest,
                1 => MinedBlock::Number(u64::from(which)),
                _ => MinedBlock::Hash(Hash32::from_bytes([which; 32])),
            };
            if let Ok((block, _, _)) = fuzz_internals::block_fields(value, Zone::Cyprus1, selector)
                && let MinedBlock::Number(number) = selector
            {
                assert_eq!(block.number, number, "block returned for another height");
            }
        }
        2 => {
            if let Ok(outpoints) = fuzz_internals::parse_outpoints(value) {
                let mut seen = std::collections::BTreeSet::new();
                for outpoint in &outpoints {
                    assert!(seen.insert((outpoint.outpoint.tx_hash, outpoint.outpoint.index)));
                }
            }
        }
        _ => {
            if let Ok(entries) = fuzz_internals::pool_entries(value, Zone::Cyprus1, 64) {
                assert!(entries.len() <= 64);
                assert!(entries.iter().all(|(_, sender, _, _)| sender.zone() == Zone::Cyprus1));
            }
        }
    }
});
