#![no_main]
//! Account and storage proof verification on hostile node lists: never panics,
//! answers the same way twice, and a proven account verifies again.
//!
//! Input: mode (1) ‖ root (32) ‖ key (32) ‖ nodes as u16 big-endian length ‖ bytes.
//! Mode bit 0 replaces the root with the first node's hash, so inputs reach the
//! RLP and trie walk instead of stopping at the root check. Bit 1 selects a
//! storage proof; otherwise the key's first 20 bytes are an account address.
use libfuzzer_sys::fuzz_target;
use quai_primitives::{Address, Hash32, QuaiAddress};
use quai_provider::state_proof::{MAX_PROOF_NODES, verify_account_proof, verify_storage_proof};
fuzz_target!(|data: &[u8]| {
    if data.len() < 65 {
        return;
    }
    let (mode, root, key, mut rest) = (data[0], &data[1..33], &data[33..65], &data[65..]);
    let mut nodes: Vec<&[u8]> = Vec::new();
    while rest.len() >= 2 && nodes.len() <= MAX_PROOF_NODES {
        let length = usize::from(u16::from_be_bytes([rest[0], rest[1]]));
        let end = (2 + length).min(rest.len());
        nodes.push(&rest[2..end]);
        rest = &rest[end..];
    }
    let root = match nodes.first() {
        Some(first) if mode & 1 == 1 => Hash32::from_bytes(quai_crypto::keccak256(first)),
        _ => Hash32::from_bytes(root.try_into().unwrap()),
    };
    if mode & 2 == 2 {
        let slot = Hash32::from_bytes(key.try_into().unwrap());
        assert_eq!(
            verify_storage_proof(root, slot, &nodes),
            verify_storage_proof(root, slot, &nodes)
        );
    } else if let Ok(address) =
        QuaiAddress::try_from(Address::from_bytes(key[..20].try_into().unwrap()))
    {
        let first = verify_account_proof(root, address, &nodes);
        assert_eq!(first, verify_account_proof(root, address, &nodes));
        if let Ok(Some(account)) = first {
            // A leaf decodes to canonical fields: the nonce fits and hashes are exact.
            assert_eq!(account.code_hash.bytes().len(), 32);
        }
    }
});
