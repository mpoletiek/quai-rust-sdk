# quai-wallet foundation

This crate implements offline BIP39/BIP32 derivation for Quai coin type 994 and
Qi coin type 969, bounded discovery, and optional native wallet-state persistence
and authenticated backup. Signing workflows and provider adapters live in other
SDK crates. This crate does not implement verified chain-proof recovery. Bounded watch-only discovery and durable fresh
address allocation are documented in [DISCOVERY.md](DISCOVERY.md). Optional native
`sqlite` storage provides public metadata, snapshot generations, durable outpoint
and nonce reservations, and validated signed-payload recovery; see [STORAGE.md](STORAGE.md).

Implemented behavior:

- All ten pinned quais.js wordlists, including Portuguese, with validated
  entropy/word/checksum conversion and NFKD mnemonic/passphrase normalization.
- BIP32 master keys for all seed lengths from 16 through 64 bytes, xprv/xpub
  import/export, exact hardened/nonhardened children, absolute master paths, and bounded relative subtree paths.
- BIP44 roots `m/44'/994'` and `m/44'/969'`, hardened accounts, external/change
  branches, and nonhardened address indexes.
- Bounded zone-and-ledger search with cooperative cancellation, actual child
  indexes and resumable continuation metadata; no global index mutation.
- Watch-only account xpubs. Import checks depth and account child metadata;
  the declared coin ancestry must come from a trusted source because xpub bytes
  cannot prove their complete origin path.
- Authenticated encrypted **seed-only** backups with explicit coin/account
  metadata. See [the binary format and limits](BACKUP_FORMAT.md).

```rust
use quai_wallet::{CoinType, HdWallet, Language, Mnemonic, Search};
use quai_primitives::Zone;

# fn main() -> Result<(), Box<dyn std::error::Error>> {
// Public BIP39 test fixture. Never fund this wallet.
let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
let mnemonic = Mnemonic::parse(Language::English, phrase)?;
let wallet = HdWallet::from_mnemonic(&mnemonic, "", CoinType::Quai)?;
let found = wallet.search(0, false, Search {
    zone: Zone::Cyprus1, start_index: 0, max_attempts: 10_000,
}, || false)?;
assert_eq!(found.address.zone, Zone::Cyprus1);
let watch = wallet.account_public(0)?;
assert_eq!(watch.derive_address(false, found.address.index)?, found.address);
# Ok(())
# }
```

Search is synchronous CPU work. Applications choose their executor and can
check an atomic cancellation flag through the callback before every candidate.
Treat caller-owned address index allocation transactionally if multiple tasks
share an account. Search does not establish historical address use or perform
wallet recovery.

## Secret handling and reference differences

Mnemonic, seed, extended private key and exported secret text omit their values
from `Debug` and have no `Clone`, `Display`, or generic serialization. Phrase and
private extended-key exports require explicit `expose()` calls. All public
address records contain only public keys and explicit BIP44 metadata; private
keys are never placed in derivation-path fields.

Owned mnemonic word indexes use bip39's enabled zeroization implementation.
Normalized phrase/passphrase strings, seed buffers, private import decoding and
private export buffers use `zeroize::Zeroizing`. The master HMAC result is
explicitly wiped after copying its scalar/chain data into an extended key;
the latter erases its scalar bytes on drop. BIP32 private scalars use k256's
zeroizing signing-key storage. Import seed/phrase/passphrase slices are borrowed:
the caller owns and must erase its original buffers. These measures cannot
guarantee erasure of compiler temporaries or internal buffers in cryptographic
dependencies; no all-memory-erasure claim is made.

Passphrases are used explicitly and not retained. Restoring the same mnemonic
with a different passphrase intentionally derives a different wallet. There is
no legacy serializer which silently discards the passphrase or language.

Only canonical xprv/xpub version prefixes are supported in this foundation.
Watch-only import rejects private input. Invalid root metadata is rejected.
Exact-index BIP32 derivation returns an error for a rare invalid child rather
than silently incrementing its index. Paths are bounded to the BIP32 depth
limit. Mnemonic parsing accepts surrounding/repeated Unicode whitespace; this
is more permissive than some reference wordlist splitters, and seeds derive
from the canonical validated phrase. Native authenticated full-wallet backups and portable mnemonic generation are
available; complete browser wallet restore/reservation integration is separate.

The underlying bip32 0.5.3 master API accepts only 16/32/64-byte seeds. This
crate applies the standard HMAC-SHA512 `Bitcoin seed` master derivation for the
complete reference-compatible range and delegates curve/scalar validation and
child arithmetic to bip32. Relevant primary sources:
[BIP32](https://github.com/bitcoin/bips/blob/master/bip-0032.mediawiki),
[BIP39](https://github.com/bitcoin/bips/blob/master/bip-0039.mediawiki),
[bip32](https://docs.rs/bip32/0.5.3/bip32/),
[bip39 languages](https://docs.rs/bip39/2.2.2/bip39/enum.Language.html),
[zeroize](https://docs.rs/zeroize/1.9.0/zeroize/),
[ECDSA signing-key source](https://docs.rs/ecdsa/0.16.9/src/ecdsa/signing.rs.html).

## Reproducing tests

The pinned JS oracle is installed with lifecycle scripts disabled as described
in `compatibility/README.md`. From the repository root:

```sh
node crates/quai-wallet/tests/generate-reference.mjs
cargo test -p quai-wallet --locked
```

Review fixture changes before accepting regeneration. Fixtures contain only
public test secrets: all 20,480 words in ten lists, 50 language/entropy/Unicode
mnemonic cases, 24 extended-key cases (including intermediate seed lengths),
and six Quai/Qi zone-and-change derivations. Tests also cover invalid mnemonic
checksums, malformed extended keys/root metadata, secret diagnostic redaction,
private-input rejection by watch-only import, hardened public derivation,
index/depth boundaries, cancellation and resumable search. These are offline
conformance tests, not node qualification or a security audit of the wallet.

Native authenticated database-state backup with explicit seed/master-xprv/imported-key
owners is documented in [FULL_BACKUP_FORMAT.md](FULL_BACKUP_FORMAT.md). It includes
burned ranges, retained claims and SQLite-registered seed/master-owned BIP47 channels,
invalidates snapshots on restore, and excludes external application-held channels
and other uninventoried state. The `sqlite` feature enables durable payment cursors
and ownership-verified QUAIWALT v2 channel backups; legacy v1 decoding is preserved.

Current gap-50 Qi RPC discovery, mixed-origin sessions, payment channel orchestration,
sweep/aggregation and recovery are described in the [facade workflow guide](../../docs/WALLET_WORKFLOWS.md).

## Extended key metadata and subtree paths

`ExtendedPrivateKey::metadata()` and `ExtendedPublicKey::metadata()` return the
same `ExtendedKeyMetadata`: depth, serialized child number (including the
hardened bit), node/parent fingerprints and chain code. `child_index()` removes
the hardened bit; `is_hardened()` reports it. Diagnostics redact these values.
Fingerprints are four-byte routing hints and cannot authenticate ancestry.
Full paths are not recoverable from xpub/xprv bytes; callers retain their origin
path, while `DerivedAddress::path()` formats known BIP44 address origins.

`derive_path("m/...")` requires a master node. On an imported account or other
subtree use `derive_relative_path("0/7")`, or an exact `derive_child`. Relative
paths must be nonempty without a leading slash or `m`; the complete path and
remaining BIP32 depth budget are checked before derivation. Public derivation
rejects any hardened step. The APIs preserve exact indices and return errors
for invalid children rather than silently skipping them. These explicit APIs
replace the reference's single absolute/relative `derivePath` entry point.

The pinned metadata generator covers 40 master, leaf and hardened-node cases,
including hardened index 2^31-1, across all supported seed lengths in the existing
reference fixtures. Tests compare private/public/imported metadata, relative
subtree results and parent fingerprints, plus malformed paths and depth 255.
Run `node compatibility/scripts/generate-key-metadata.mjs` to reproduce vectors.

## Mnemonic entropy and generation

`Mnemonic::entropy()` returns `MnemonicEntropy`, an explicitly exposed,
zeroizing buffer containing the original 16–32 entropy bytes without checksum
bits. It has no `Clone`, `Display` or serialization and redacts `Debug`. Use
`export.expose()` only when needed; caller-created copies remain the caller's
responsibility. `Mnemonic::parse(language, phrase)?.entropy()` is the inverse of
`Mnemonic::from_entropy(language, bytes)?.phrase()` for every supported language
and entropy length. To validate a phrase, use the result of `Mnemonic::parse`.
`Mnemonic::language().word_list()` exposes the selected public wordlist.

`Mnemonic::generate(language, word_count)` supports 12, 15, 18, 21 and 24 words
on native and browser wasm. Generation uses OS randomness or Web Crypto and
fails when secure entropy is unavailable. It performs no network request or
account prompt. Create an HD wallet explicitly with
`HdWallet::from_mnemonic(&mnemonic, passphrase, coin)`, retaining the guarded
mnemonic for the application's backup flow. Passphrases are caller-owned inputs,
not a mutable `password` property on the mnemonic.

The pinned `fromMnemonicReactNative` helpers use an optional JavaScript native
crypto bridge. Rust uses the same BIP39/BIP32 APIs on native and WebAssembly;
there is no React Native bridge or mutable provider field. Lower-level arbitrary
HD paths use `ExtendedPrivateKey::from_seed(mnemonic.to_seed(passphrase).expose())`
and `derive_path`. Import xprv and xpub with their separate validated key types.
Browser generation/entropy restoration is tested in a Chromium dedicated worker;
React Native, other browser engines and suspended execution remain separate
platform qualifications.
