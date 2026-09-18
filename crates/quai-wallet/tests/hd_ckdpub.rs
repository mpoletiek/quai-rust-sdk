//! Differential check of the scan-path CKDpub against the `bip32` path.
//!
//! `AccountPublic::derive_address` and `search` derive child points through an
//! internal CKDpub over `quai-crypto`, because `bip32` multiplies with k256's
//! generic `Mul` and never reaches the precomputed generator table, computes a
//! parent fingerprint a scan never reads, and hands back a compressed point the
//! caller immediately decompresses again.
//!
//! Reimplementing a derivation primitive is only acceptable with evidence that
//! it agrees bit for bit with the implementation it replaces, on every index,
//! including the indexes that must be rejected. `ExtendedPublicKey::derive_child`
//! still goes through `bip32`, so both paths are reachable from the public API
//! and can be compared directly.

use quai_wallet::{AccountPublic, CoinType, ExtendedPublicKey, HdWallet, Search, WalletError};

const SEED: [u8; 64] = [0x2b; 64];

fn account(coin: CoinType) -> AccountPublic {
    HdWallet::from_seed(&SEED, coin)
        .expect("fixture seed builds a wallet")
        .account_public(0)
        .expect("account zero derives")
}

/// The branch node as `bip32` produces it, for the reference side.
fn bip32_branch(account: &AccountPublic, change: bool) -> ExtendedPublicKey {
    ExtendedPublicKey::import(&account.export())
        .expect("account xpub round-trips")
        .derive_child(u32::from(change), false)
        .expect("branch derives")
}

/// Every index agrees: the same point where the address is usable, and the same
/// rejection where it is not.
fn assert_agrees_over(coin: CoinType, change: bool, indexes: impl Iterator<Item = u32>) {
    let account = account(coin);
    let branch = bip32_branch(&account, change);
    let ledger = match coin {
        CoinType::Quai => quai_primitives::Ledger::Quai,
        CoinType::Qi => quai_primitives::Ledger::Qi,
    };

    let mut usable = 0u32;
    let mut rejected = 0u32;
    for index in indexes {
        // Reference: bip32's CKDpub, then decompress.
        let reference = branch
            .derive_child(index, false)
            .expect("nonhardened child derives")
            .public_key()
            .expect("child point is valid");

        match account.derive_address(change, index) {
            Ok(derived) => {
                // Bit-equality of the derived point is the property that matters.
                assert_eq!(
                    derived.public_key,
                    reference.to_compressed(),
                    "point diverged at index {index}"
                );
                assert_eq!(
                    derived.address,
                    reference.address(),
                    "address diverged at {index}"
                );
                assert_eq!(derived.index, index);
                assert_eq!(derived.change, change);
                usable += 1;
            }
            Err(WalletError::InvalidDerivedAddress) => {
                // Both paths must agree this index is unusable, and for the same
                // reason: an unknown zone byte, or the wrong ledger bit.
                let address = reference.address();
                assert!(
                    address.zone().is_err() || address.ledger() != ledger,
                    "index {index} was rejected but the reference address is usable"
                );
                rejected += 1;
            }
            Err(other) => panic!("unexpected error at index {index}: {other:?}"),
        }
    }
    // Sanity: the sample must actually exercise both outcomes, or the test is
    // asserting nothing. Roughly 9 zone bytes in 256 pass, halved by the ledger
    // bit, so usable addresses are about 1 in 57 for a given coin.
    assert!(usable > 0, "sample found no usable address");
    assert!(rejected > usable, "sample should be mostly rejections");
}

#[test]
fn the_scan_path_matches_bip32_bit_for_bit_on_both_ledgers_and_branches() {
    for coin in [CoinType::Qi, CoinType::Quai] {
        for change in [false, true] {
            assert_agrees_over(coin, change, 0..2_000);
        }
    }
}

#[test]
fn agreement_holds_across_sparse_and_high_indexes() {
    // Indexes spread across the nonhardened range, including the boundary, so
    // agreement is not an artifact of small contiguous values.
    let sparse = (0..400).map(|i: u32| i.wrapping_mul(2_654_435_761) & 0x7fff_ffff);
    assert_agrees_over(CoinType::Qi, false, sparse);
    assert_agrees_over(CoinType::Qi, false, 0x7fff_fc00u32..0x8000_0000);
}

#[test]
fn a_hardened_index_is_refused_on_both_paths() {
    let account = account(CoinType::Qi);
    let branch = bip32_branch(&account, false);
    // The public API refuses a hardened public child explicitly.
    assert!(matches!(
        branch.derive_child(0, true),
        Err(WalletError::HardenedPublicChild)
    ));
    // And an index with the hardened bit set is not silently masked.
    assert!(account.derive_address(false, 1 << 31).is_err());
    assert!(branch.derive_child(1 << 31, false).is_err());
}

#[test]
fn search_finds_the_same_address_the_reference_path_would() {
    // `search` is the grind loop, so it exercises the new CKDpub thousands of
    // times per call. Whatever it returns must be reproducible through bip32.
    let account = account(CoinType::Qi);
    let branch = bip32_branch(&account, false);
    for zone in [
        quai_primitives::Zone::Cyprus1,
        quai_primitives::Zone::Paxos2,
    ] {
        let found = account
            .search(
                false,
                Search {
                    zone,
                    start_index: 0,
                    max_attempts: 100_000,
                },
                || false,
            )
            .expect("a usable address exists in this range");
        assert_eq!(found.address.zone, zone);

        let reference = branch
            .derive_child(found.address.index, false)
            .expect("child derives")
            .public_key()
            .expect("point is valid");
        assert_eq!(found.address.public_key, reference.to_compressed());
        assert_eq!(found.address.address, reference.address());

        // And nothing usable was skipped: every earlier index must be unusable
        // for this zone, or the search returned the wrong match.
        for index in 0..found.address.index {
            let earlier = branch
                .derive_child(index, false)
                .expect("child derives")
                .public_key()
                .expect("point is valid")
                .address();
            assert!(
                earlier.zone().ok() != Some(zone)
                    || earlier.ledger() != quai_primitives::Ledger::Qi,
                "search skipped a usable address at index {index}"
            );
        }
    }
}
