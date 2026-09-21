//! HD derivation and coin-selection baselines.
//!
//! This is the SDK's dominant CPU cost. A Qi wallet discovery at the default
//! gap limit matches roughly 100 addresses across both branches, and each match
//! costs a run of derivations rather than one.
//!
//! The 1-in-512 hit rate comes from `AccountPublic::search` requiring a
//! *specific* zone: one byte-0 value in 256, and the ledger flag in bit 7 of
//! byte 1, so 256 * 2. It does NOT come from "9 of 256 byte-0 values are valid
//! zones" -- that ratio gives 9/512, about 1 in 57, which is the rate at which
//! `derive_address` succeeds for *any* zone. Those are different quantities and
//! conflating them is how a cost model ends up wrong.
//!
//! Note what the atomic unit actually is. `derive_address` performs **two**
//! CKDpub steps, because it re-derives the change-level node on every call
//! (`hd.rs`), while `search` hoists that node out of its loop and pays **one**
//! per candidate. Use `derivation/ckdpub_step` as the per-candidate unit;
//! `derive_address` is roughly 1.9x it and is not the right multiplier for a
//! scan cost model.
//!
//! All seeds and mnemonics here are deterministic public fixtures. Never fund
//! any address these produce.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use quai_consensus::{Denomination, OutPoint};
use quai_crypto::U256;
use quai_primitives::{Hash32, QiAddress, Zone};
use quai_wallet::{
    AccountPublic, CandidateCoin, CoinType, ExtendedPublicKey, HdWallet, Search, SelectionRequest,
    select_fewest,
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

    // Two CKDpub steps plus keccak and the zone/ledger check: it re-derives the
    // change-level node every call. This is the cost of a one-off address
    // lookup, NOT the per-candidate cost inside a scan -- see `ckdpub_step`.
    // Most calls return InvalidDerivedAddress, which is expected: the address is
    // usable only when byte 0 names a zone and the ledger bit agrees.
    group.throughput(Throughput::Elements(1));
    group.bench_function("derive_address", |b| {
        let mut index = 0u32;
        b.iter(|| {
            index = index.wrapping_add(1) & 0x7fff_ffff;
            let _ = black_box(account.derive_address(false, black_box(index)));
        });
    });
    // The genuine per-candidate unit, matching what `search` pays per attempt:
    // a single CKDpub step from an already-derived branch node. Multiply this by
    // the measured attempt count to model a scan, rather than using
    // `derive_address`, which does twice the curve work.
    // Rebuild the branch node through the public xpub API, which is exactly what
    // `search` hoists out of its loop.
    let branch = ExtendedPublicKey::import(&account.export())
        .expect("account xpub round-trips")
        .derive_child(0, false)
        .expect("receive branch derives");
    group.bench_function("ckdpub_step", |b| {
        let mut index = 0u32;
        b.iter(|| {
            index = index.wrapping_add(1) & 0x7fff_ffff;
            branch
                .derive_child(black_box(index), false)
                .expect("derives")
        });
    });
    group.finish();

    // Time to find the next usable address from a moving start index, which is
    // the shape of a gap scan.
    //
    // Attempts are Geometric(1/512), so this latency is close to exponential and
    // strongly right-skewed: its mean is roughly 1.35x its median. A cost model
    // must use the mean, which is what criterion reports. Sampled widely enough
    // that the confidence interval is narrow enough to regress against; at a
    // small sample size the interval spans tens of percent and the benchmark
    // cannot detect a real change.
    //
    // Deliberately NOT reported as throughput against `max_attempts`: `search`
    // returns on the first match, so `max_attempts` is a cap rather than work
    // performed. Dividing by it would report a number that changes with the cap
    // while the measured work stayed identical. Per-candidate cost belongs to
    // `derivation/ckdpub_step` above; this benchmark owns the end-to-end latency
    // a caller actually waits on, and the expected ratio between them is the
    // 1-in-512 hit rate.
    let mut group = c.benchmark_group("address_search");
    group.sample_size(100);
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
    // Selection is O(n) bucketed rather than quadratic, and these sizes hold
    // that line. They run to the selector's own 100,000 cap because the claim
    // that per-element cost stays flat was previously only checked to 10,000,
    // and the constant factor above that is where the dedupe set showed up.
    for count in [100usize, 1_000, 10_000, 100_000] {
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

/// Fee convergence re-chooses against one validated snapshot.
///
/// The interesting ratio is this against `selection/select_fewest` at the same
/// size: a round that re-validated the whole snapshot would make eight rounds
/// cost eight selections, and the point of hoisting the validation is that it
/// does not.
fn selection_fee_rounds(c: &mut Criterion) {
    let mut group = c.benchmark_group("selection_fee");
    group.sample_size(20);
    for count in [10_000usize, 100_000] {
        let coins = fixture_coins(count);
        for rounds in [1u8, 8] {
            group.throughput(Throughput::Elements(count as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("select_with_fee_{rounds}_rounds"), count),
                &coins,
                |b, c| {
                    b.iter(|| {
                        let request = fixture_request();
                        let mut seen = 0u8;
                        // Every quote but the last must exceed the fee currently
                        // held, or `select_with_fee` converges immediately and
                        // both arms measure a single round. The fixture starts
                        // at 10 with a 1,000 ceiling, so ramp in tens and stay
                        // under it: call n quotes 10*(n+2), which is above the
                        // 10*(n+1) set by the previous call. The final call
                        // quotes zero to converge.
                        let _ = black_box(quai_wallet::select_with_fee(
                            black_box(c),
                            black_box(&request),
                            rounds,
                            |_| {
                                seen += 1;
                                Ok(if seen < rounds {
                                    U256::from(10u64 * (u64::from(seen) + 2))
                                } else {
                                    U256::ZERO
                                })
                            },
                        ));
                    });
                },
            );
        }
    }
    group.finish();
}

/// The window scan, sequential against parallel.
///
/// This is the grind the README advertises, and it had no bench at all, which
/// is how the parallel path went on deriving one index at a time — skipping the
/// batch normalization the sequential path had always used — without anything
/// noticing. Both variants grind the same fixed window, so the parallel/
/// sequential ratio is the speedup, and a regression in either shows up against
/// `address_search/next_usable_cyprus1` in the same run.
fn window_scan(c: &mut Criterion) {
    let account = account();
    let mut group = c.benchmark_group("window_scan");
    group.sample_size(10);
    // Wide enough to cross several batches at the 1-in-512 hit rate.
    let window = || Search {
        zone: BENCH_ZONE,
        start_index: 0,
        max_attempts: 4_096,
    };
    group.throughput(Throughput::Elements(4_096));
    group.bench_function("sequential", |b| {
        b.iter(|| {
            let _ = black_box(account.search_window(
                black_box(false),
                black_box(window()),
                black_box(8),
                || false,
            ));
        });
    });
    #[cfg(all(feature = "rayon", not(target_arch = "wasm32")))]
    group.bench_function("parallel", |b| {
        b.iter(|| {
            let _ = black_box(account.search_window_parallel(
                black_box(false),
                black_box(window()),
                black_box(8),
                || false,
            ));
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    seed_and_wallet,
    derivation,
    coin_selection,
    selection_fee_rounds,
    window_scan
);
criterion_main!(benches);
