#![no_main]
//! Proofs from valid tries, then mutated: the verifier returns the true value
//! or an error, never another value and never a false absence.
//!
//! Input: mutation (1) ‖ position (2) ‖ entries of 36 bytes, each
//! base (1) ‖ shared (1) ‖ length (1) ‖ fill (1) ‖ path (32). An entry copies
//! `shared` leading nibbles from an earlier one, so tries grow extension nodes;
//! short values make embedded leaves. Paths are raw trie paths, walked as the
//! verifiers walk a hashed key.
use libfuzzer_sys::fuzz_target;
use quai_provider::state_proof::test_trie::TestTrie;
use quai_provider::state_proof::verify_path;

fn nibble(path: &[u8; 32], at: usize) -> u8 {
    (path[at / 2] >> if at.is_multiple_of(2) { 4 } else { 0 }) & 0x0f
}
fn set(path: &mut [u8; 32], at: usize, value: u8) {
    let shift = if at.is_multiple_of(2) { 4 } else { 0 };
    path[at / 2] = (path[at / 2] & !(0x0f << shift)) | ((value & 0x0f) << shift);
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 3 + 36 {
        return;
    }
    let (op, position) = (data[0], usize::from(u16::from_be_bytes([data[1], data[2]])));
    let mut entries: Vec<([u8; 32], Vec<u8>)> = Vec::new();
    for chunk in data[3..].chunks_exact(36).take(32) {
        let mut path: [u8; 32] = chunk[4..].try_into().unwrap();
        if !entries.is_empty() {
            let base = entries[usize::from(chunk[0]) % entries.len()].0;
            for at in 0..usize::from(chunk[1]) % 65 {
                set(&mut path, at, nibble(&base, at));
            }
        }
        let length = if chunk[2] & 0x80 != 0 {
            40
        } else {
            1 + usize::from(chunk[2]) % 3
        };
        let value = vec![chunk[3] | 1; length];
        entries.retain(|(p, _)| *p != path);
        entries.push((path, value));
    }
    let trie = TestTrie::from_paths(entries.clone());
    let root = trie.root();
    let (target, truth) = &entries[position % entries.len()];
    let proof = trie.prove(target);
    assert_eq!(
        verify_path(root, target, &proof).unwrap(),
        Some(truth.clone())
    );

    let mut changed = proof.clone();
    let node = position % changed.len();
    match op % 6 {
        0 => {
            let at = usize::from(op) * 7 % changed[node].len();
            changed[node][at] ^= 1 << (op % 8);
        }
        1 => changed.truncate(node),
        2 => {
            changed.remove(node);
        }
        3 => changed.insert(node, proof[proof.len() - 1 - node].clone()),
        4 => changed.swap(node, (node + 1 + usize::from(op)) % proof.len()),
        _ => changed = trie.prove(&entries[usize::from(op) % entries.len()].0),
    }
    let result = verify_path(root, target, &changed);
    assert!(
        result.is_err() || result == Ok(Some(truth.clone())),
        "a changed proof showed {result:?}"
    );

    // A path beside the target, if absent, proves absence.
    let mut beside = *target;
    set(&mut beside, 63, nibble(target, 63) ^ (1 + op % 15));
    if !entries.iter().any(|(p, _)| *p == beside) {
        assert_eq!(
            verify_path(root, &beside, &trie.prove(&beside)).unwrap(),
            None
        );
    }
});
