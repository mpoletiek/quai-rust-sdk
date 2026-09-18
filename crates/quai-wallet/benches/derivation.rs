//! HD derivation and coin-selection baselines.
//!
//! This is the SDK's dominant CPU cost. A Qi wallet discovery at the default
//! gap limit matches roughly 100 addresses across both branches, and each match
//! costs a run of derivations rather than one: zone lives in address byte 0 and
//! the ledger flag in bit 7 of byte 1, and only 9 of 256 byte-0 values name a
//! valid zone, so a usable address appears once per 256 * 2 = 512 candidates.
//!
//! `search` is therefore reported with `Throughput::Elements(max_attempts)` so
//! the headline number is candidates per second, which stays comparable even
//! though the number of matches per run varies with the seed.
//!
//! All seeds and mnemonics here are deterministic public fixtures. Never fund
//! any address these produce.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use quai_consensus::{Denomination, OutPoint};
use quai_crypto::U256;
use quai_primitives::{Hash32, QiAddress, Zone};
use quai_wallet::{
    AccountPublic, CandidateCoin, CoinType, HdWallet, Search, SelectionRequest, select_fewest,
};
use std::hint::black_box;

const SEED: [u8; 64] = [0x2b; 64];
const BENCH_ZONE: Zone = Zone::Cyprus1;

/// A syntactically valid Cyprus-1 Qi address. Zone is byte 0 and the Qi ledger
/// is bit 7 of byte 1. This is a shape fixture for the selector, not a key.
fn fixture_address(tag: u8) -> QiAddress {
    let mut bytes = [0u8; 20];
    bytes[0] = BENCH_ZONE.byte();
    bytes[1] = 0x80;
    bytes[19] = tag;
    QiAddress::try_from(bytes).expect("fixture address is a valid Cyprus-1 Qi address")
}

/// Selection requires unique outpoints, a nonzero hash, and hash byte 2 equal to
/// the owner's zone byte. Vary the low bytes so every coin is distinct.
fn fixture_coins(count: usize) -> Vec<CandidateCoin> {
    let address = fixture_address(1);
    (0..count)
        .map(|i| {
            let mut hash = [0x33u8; 32];
            hash[2] = BENCH_ZONE.byte();
            hash[28..32].copy_from_slice(&(i as u32).to_be_bytes());
            CandidateCoin {
                outpoint: OutPoint {
                    transaction_hash: Hash32::from(hash),
                    index: (i % 65_536) as u16,
                },
                address,
                // Cycle the low denominations so the buckets are genuinely
                // populated rather than all landing in one.
                denomination: Denomination::new((i % 8) as u8).expect("index under 15"),
                unlock_height: U256::ZERO,
                expires_at: None,
                reserved: false,
            }
        })
        .collect()
}

fn fixture_request() -> SelectionRequest {
    SelectionRequest {
        zone: BENCH_ZONE,
        candidate_height: U256::from(1_000u64),
        target: U256::from(1_000u64),
        fee: U256::from(10u64),
        max_fee: U256::from(1_000u64),
        max_inputs: 4096,
        max_outputs: 4096,
    }
}

fn account() -> AccountPublic {
    HdWallet::from_seed(&SEED, CoinType::Qi)
        .expect("fixture seed builds a wallet")
        .account_public(0)
        .expect("account zero derives")
}

fn seed_and_wallet(c: &mut Criterion) {
    let mut group = c.benchmark_group("wallet_init");
    group.bench_function("hd_wallet_from_seed", |b| {
        b.iter(|| HdWallet::from_seed(black_box(&SEED), CoinType::Qi).expect("builds"));
    });
    group.bench_function("account_public", |b| {
        let wallet = HdWallet::from_seed(&SEED, CoinType::Qi).expect("builds");
        b.iter(|| wallet.account_public(black_box(0)).expect("derives"));
    });
    group.finish();
}

fn derivation(c: &mut Criterion) {
    let account = account();
    let mut group = c.benchmark_group("derivation");

    // The atomic unit: one CKDpub step plus keccak plus the zone/ledger check.
    // Most calls return InvalidDerivedAddress, which is the expected case -- the
    // address is only usable when byte 0 names a zone and the ledger bit agrees.
    group.throughput(Throughput::Elements(1));
    group.bench_function("derive_address", |b| {
        let mut index = 0u32;
        b.iter(|| {
            index = index.wrapping_add(1) & 0x7fff_ffff;
            let _ = black_box(account.derive_address(false, black_box(index)));
        });
    });
    group.finish();

    // Time to find the next usable address from a moving start index, which is
    // the shape of a gap scan.
    //
    // Deliberately NOT reported as throughput against `max_attempts`: `search`
    // returns on the first match, so `max_attempts` is a cap rather than work
    // performed. Dividing by it would report a number that changes with the cap
    // while the measured work stayed identical. Per-candidate cost belongs to
    // `derivation/derive_address` above; this benchmark owns the end-to-end
    // latency a caller actually waits on, and the expected ratio between them is
    // the 1-in-512 hit rate.
    let mut group = c.benchmark_group("address_search");
    group.sample_size(20);
    let mut start_index = 0u32;
    group.bench_function("next_usable_cyprus1", |b| {
        b.iter(|| {
            // Advance the start so successive iterations do not all resolve to
            // the same cached-looking index.
            start_index = start_index.wrapping_add(997) & 0x000f_ffff;
            let search = Search {
                zone: BENCH_ZONE,
                start_index,
                max_attempts: 10_000,
            };
            let _ = black_box(account.search(false, search, || false));
        });
    });
    group.finish();
}

fn coin_selection(c: &mut Criterion) {
    let mut group = c.benchmark_group("selection");
    group.sample_size(30);
    // Selection was reviewed as O(n) bucketed rather than quadratic. These sizes
    // exist to hold that line: the per-element cost should stay flat as n grows.
    for count in [100usize, 1_000, 10_000] {
        let coins = fixture_coins(count);
        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(BenchmarkId::new("select_fewest", count), &coins, |b, c| {
            b.iter(|| {
                let request = fixture_request();
                let _ = black_box(select_fewest(black_box(c), black_box(&request)));
            });
        });
    }
    group.finish();
}

criterion_group!(benches, seed_and_wallet, derivation, coin_selection);
criterion_main!(benches);
