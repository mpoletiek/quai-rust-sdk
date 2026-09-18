//! Cryptographic primitive baselines.
//!
//! These exist to make performance claims falsifiable. Deterministic public
//! fixtures only; never fund any key that appears here.
//!
//! Two groups are load bearing for the SDK's cost model:
//!
//! - `sign_prehash` and `sign_schnorr` are the direct beneficiaries of k256's
//!   `precomputed-tables` feature, because `ecdsa::hazmat` computes
//!   `R = ProjectivePoint::mul_by_generator(k)`.
//! - `public_key_from_compressed` measures point decompression, which HD
//!   derivation currently pays once per candidate address for no reason.
//!   Compare it against `public_key_from_uncompressed`, which does not
//!   decompress, to size that waste.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use quai_crypto::{PublicKey, SecretKey, keccak256, sha256};
use std::hint::black_box;

/// Deterministic nonzero scalar; a public fixture, never a funded key.
fn fixture_key(seed: u8) -> SecretKey {
    let mut bytes = [0u8; 32];
    bytes[31] = seed;
    bytes[0] = 0x11;
    SecretKey::from_bytes(&bytes).expect("fixture scalar is in range")
}

fn hashing(c: &mut Criterion) {
    let mut group = c.benchmark_group("hash");
    // 64 bytes is the address-derivation size: keccak over an uncompressed
    // public key minus its 0x04 prefix.
    for size in [32usize, 64, 1024, 65536] {
        let data = vec![0xa5u8; size];
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::new("keccak256", size), &data, |b, d| {
            b.iter(|| keccak256(black_box(d)));
        });
        group.bench_with_input(BenchmarkId::new("sha256", size), &data, |b, d| {
            b.iter(|| sha256(black_box(d)));
        });
    }
    group.finish();
}

fn signing(c: &mut Criterion) {
    let key = fixture_key(7);
    let public = key.public_key();
    let digest = keccak256(b"quai benchmark payload");
    let signature = key.sign_prehash(&digest).expect("fixture signs");

    let mut group = c.benchmark_group("signature");
    group.bench_function("ecdsa_sign_prehash", |b| {
        b.iter(|| key.sign_prehash(black_box(&digest)).expect("signs"));
    });
    group.bench_function("ecdsa_verify_prehash", |b| {
        b.iter(|| signature.verify_prehash(black_box(&digest), black_box(&public)));
    });
    group.bench_function("ecdsa_recover_prehash", |b| {
        b.iter(|| {
            signature
                .recover_prehash(black_box(&digest))
                .expect("recovers")
        });
    });
    group.bench_function("schnorr_sign", |b| {
        b.iter(|| {
            key.sign_schnorr(black_box(b"quai benchmark payload"))
                .expect("signs")
        });
    });
    group.finish();
}

fn point_operations(c: &mut Criterion) {
    let key = fixture_key(11);
    let tweak = fixture_key(29);
    let public = key.public_key();
    let compressed = public.to_compressed();
    let uncompressed = public.to_uncompressed();

    let mut group = c.benchmark_group("point");
    // The asymmetry between these two is the cost of decompression: a modular
    // square root. HD derivation pays it per candidate via a to_bytes /
    // from_sec1_bytes round trip over a point it already holds in affine form.
    group.bench_function("from_compressed_33b", |b| {
        b.iter(|| PublicKey::from_sec1_bytes(black_box(&compressed)).expect("valid point"));
    });
    group.bench_function("from_uncompressed_65b", |b| {
        b.iter(|| PublicKey::from_sec1_bytes(black_box(&uncompressed)).expect("valid point"));
    });
    group.bench_function("to_compressed", |b| {
        b.iter(|| black_box(public).to_compressed());
    });
    group.bench_function("address", |b| {
        b.iter(|| black_box(public).address());
    });
    // Public-key tweak addition: one base-point multiply plus a point add. This
    // is the same shape as a BIP32 CKDpub step, so it bounds what derivation
    // could cost if it used the generator table.
    group.bench_function("public_add_tweak", |b| {
        b.iter(|| public.add_tweak(black_box(&tweak)).expect("valid tweak"));
    });
    group.bench_function("secret_add_tweak", |b| {
        b.iter(|| key.add_tweak(black_box(&tweak)).expect("valid tweak"));
    });
    group.finish();
}

fn key_agreement(c: &mut Criterion) {
    let key = fixture_key(13);
    let peer = fixture_key(17).public_key();
    let mut group = c.benchmark_group("ecdh");
    group.bench_function("shared_x", |b| {
        b.iter(|| key.ecdh_shared_x(black_box(&peer)));
    });
    group.finish();
}

criterion_group!(benches, hashing, signing, point_operations, key_agreement);
criterion_main!(benches);
