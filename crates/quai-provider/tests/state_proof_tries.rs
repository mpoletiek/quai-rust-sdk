//! Proofs from tries with embedded leaves and extension nodes, which the
//! captured mainnet proofs lack, verify through the public API, and every
//! changed byte fails.
#![cfg(feature = "test-fixtures")]
use quai_primitives::{Hash32, QuaiAddress};
use quai_provider::state_proof::test_trie::TestTrie;
use quai_provider::state_proof::{
    Account, EMPTY_CODE_HASH, EMPTY_TRIE_ROOT, verify_account_proof, verify_storage_proof,
};
use quai_rpc::U256;

/// Slots whose Keccak-256 hashes agree on their first nine nibbles, in pairs
/// (mined offline). Under a two-leaf branch that deep, a leaf holding a small
/// value encodes to fewer than 32 bytes, so its parent embeds it.
const PAIRED_SLOTS: [u64; 4] = [247_476, 515_151, 268_005, 645_179];

fn slot(n: u64) -> Hash32 {
    Hash32::from_bytes(U256::from(n).to_be_bytes())
}

#[test]
fn a_storage_trie_with_embedded_leaves_and_extensions_verifies() {
    let slots = PAIRED_SLOTS
        .iter()
        .map(|&n| (slot(n), U256::from(n % 97 + 1)))
        .collect::<Vec<_>>();
    let trie = TestTrie::storage(&slots);
    let (embedded, extensions) = trie.shapes();
    assert!(embedded >= 4 && extensions >= 2, "{embedded} {extensions}");
    let root = trie.root();
    for (slot, value) in &slots {
        let proof = trie.prove_slot(*slot);
        assert_eq!(verify_storage_proof(root, *slot, &proof), Ok(*value));
        for node in 0..proof.len() {
            for at in 0..proof[node].len() {
                let mut changed = proof.clone();
                changed[node][at] ^= 0x01;
                assert!(verify_storage_proof(root, *slot, &changed).is_err());
            }
        }
    }
    // A slot beside a pair, and one far away, prove zero.
    for absent in [slot(1), slot(247_477)] {
        assert_eq!(
            verify_storage_proof(root, absent, &trie.prove_slot(absent)),
            Ok(U256::ZERO)
        );
    }
}

#[test]
fn a_state_trie_proves_accounts_and_absence() {
    let storage = TestTrie::storage(&[(slot(3), U256::from(42))]);
    let token: QuaiAddress = "0x006C3e2AaAE5DB1bCd11A1a097cE572312EADdBB"
        .parse()
        .unwrap();
    let holder: QuaiAddress = "0x002360Bc8E2A359bE7335B06De43F1c7F040f15a"
        .parse()
        .unwrap();
    let absent: QuaiAddress = "0x0011111111111111111111111111111111111111"
        .parse()
        .unwrap();
    let accounts = [
        (
            token,
            Account::new(
                1,
                U256::ZERO,
                storage.root(),
                Hash32::from_bytes([5; 32]),
                U256::from(1),
            ),
        ),
        (
            holder,
            Account::new(
                9,
                U256::from(10).pow(U256::from(20)),
                EMPTY_TRIE_ROOT,
                EMPTY_CODE_HASH,
                U256::ZERO,
            ),
        ),
    ];
    let state = TestTrie::accounts(&accounts);
    for (address, account) in &accounts {
        let proven = verify_account_proof(state.root(), *address, &state.prove_account(*address));
        assert_eq!(proven, Ok(Some(*account)));
    }
    assert_eq!(
        verify_account_proof(state.root(), absent, &state.prove_account(absent)),
        Ok(None)
    );
    assert_eq!(
        verify_storage_proof(storage.root(), slot(3), &storage.prove_slot(slot(3))),
        Ok(U256::from(42))
    );
    let empty = TestTrie::storage(&[]);
    assert_eq!(empty.root(), EMPTY_TRIE_ROOT);
    assert!(empty.prove_slot(slot(3)).is_empty());
}
