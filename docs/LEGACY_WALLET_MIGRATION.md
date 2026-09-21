# Legacy quais.js wallet JSON migration

With the wallet `backup` feature, `full_backup::legacy` imports version-one
`SerializedQuaiHDWallet` and `SerializedQiHDWallet` documents from the pinned
`quais@1.0.0-alpha.57` SDK. It returns a `WalletBackup` ready for authenticated
encryption and monotonic restoration. It performs no RPC, signing, submission or
storage writes.

## Required identity and network

Supply `LegacyWalletIdentity { language, passphrase, expected_root }`. The importer
parses the document's mnemonic in the explicit language and applies the original
BIP39 passphrase. The expected root is a trusted **public** coin-level xpub,
represented by `ExtendedPublicKey`. The document cannot supply its own trust
anchor. This also detects wrong passphrases for empty/import-only wallets, where
there may be no HD address available to verify identity.

The pinned JavaScript `QuaiHDWallet.xPub()` and `QiHDWallet.xPub()` methods return
**xprv strings** in the package's compiled `lib/`, contrary to their names and
comments. Treat those raw values as secrets. The published `src/` for the same
release already neuters them, so a build made from source returns an xpub; see
the reference-half divergence in [../SDK_PARITY_ANALYSIS.md](../SDK_PARITY_ANALYSIS.md).

Derive a public trust anchor in a way that holds either way. Calling `.neuter()`
unconditionally throws once `xPub()` starts returning an xpub, because
`fromExtendedKey` then yields an `HDNodeVoidWallet`, which has no such method:

```javascript
const node = HDNodeWallet.fromExtendedKey(wallet.xPub());
const expectedRoot = (node.neuter ? node.neuter() : node).extendedKey;
```

Rust's `HdWallet::root_public_key().export()` returns only an xpub;
`ExtendedPublicKey::import` rejects an xprv. The source defect is covered by
[executable reference tests](../compatibility/scripts/hd-wallet-workflows.test.mjs).

Legacy JSON has no network identity. Supply an explicit `NetworkScope` containing
chain ID and trusted genesis. All address zones in the document belong to that
network; the supplied zone is also retained for an empty wallet. Chain ID and
genesis must be nonzero. Do not infer the network from address prefixes alone.

```rust
use quai_wallet::full_backup::legacy::{import_quais_json, LegacyWalletIdentity};
use quai_wallet::{ExtendedPublicKey, Language};

// legacy_json, network, original_passphrase and trusted_root are explicit inputs.
let root = ExtendedPublicKey::import(&trusted_root)?;
let backup = import_quais_json(
    &legacy_json,
    network,
    LegacyWalletIdentity {
        language: Language::English,
        passphrase: &original_passphrase,
        expected_root: &root,
    },
)?;
// Encrypt with backup.encrypt(password, chosen_kdf), then restore/merge normally.
```

Non-English mnemonics and nonempty passphrases can be imported correctly when the
caller supplies their actual context. The reference JSON omits that context;
its own deserializer can therefore change identity or fail. No importer can infer
a missing passphrase from an empty document. Retain the trusted public root and
original private context independently.

## What migration preserves

| Legacy record | Verification and retained state |
| --- | --- |
| BIP44 external/change address | Exact coin/account/branch/raw child, compressed public key, address and zone; known derivation floor at least index + 1 |
| Direct imported Qi key | Private scalar must reproduce the exact public key/address; retained as a guarded backup origin and separate public metadata |
| BIP47 receive address | Explicit owner account and peer code must reproduce the receiving key; retain owned address, exposure and receive floor |
| BIP47 sender address | Explicit owner account and peer must reproduce the destination public key; retain the send exposure/floor without claiming local ownership |
| Empty peer registration | Retain an account-zero channel when no address record identifies another account |

All records are verified before a backup is returned. Duplicate or unknown JSON
fields, duplicate channel keys/addresses/exposures, forged keys or paths, invalid
versions/coin types and malformed checkpoints reject the whole import. No partial
store update can occur. Imported private keys in the legacy `derivationPath` field
are treated as secrets, never ordinary public metadata.

Cached statuses and `lastSyncedBlock` are checked for shape and discarded. The
legacy serializer does not carry a matching UTXO snapshot, so they cannot establish
recovery coverage. The imported backup has no UTXO cache, nonce reservations or
signed claims. Existing live journals retain their newer claims and index floors
when merged using the normal restore APIs. Rescan known origins and explicit deep
ranges as needed; an empty gap is not a historical completeness guarantee.

Known floors are derived from the supplied address inventory. Missing addresses,
unserialized exposure history or absent pending transactions cannot be inferred.
Do not replace a live allocator with a fresh one initialized from older JSON.

## Explicit legacy export

`export_quais_json(&backup, network, coin, &mnemonic)` returns a `SecretString`
containing compatible plaintext. Export selects the explicit network and ledger,
verifies the original mnemonic identity and emits unknown/null usage observations.
It includes the mnemonic and direct imported private keys because the old schema
requires them. Its diagnostics are redacted and its owned text zeroizes on drop;
caller copies and input buffers remain the caller's responsibility.

The pinned JavaScript importer only restores English mnemonics with an empty
passphrase. Legacy export therefore requires that identity. It also rejects
selected-network custody operations, advanced nonce floors, HD/payment burned
ranges beyond the exported indexes, or other state that cannot be represented
without losing recovery protection. Use authenticated Rust backups for those
wallets. These errors deliberately prevent silent downgrade; export is not the
recommended general backup format.

| Resource bound | Maximum |
| --- | ---: |
| Input/output JSON | 1 MiB |
| Combined owned addresses and send exposures | 1,024 |
| Distinct HD accounts | 64 |
| Account/peer channel pairs | 64 |
| Direct imported private keys plus seed origin | 15 + 1 |
| Individual private text field | 4,096 bytes |

Output uses a preallocated, bounded, zeroizing buffer. Parsing guards private
phrase/path text and returns redacted errors. The public APIs expose neither an
implicit plaintext serializer nor a replacement for authenticated custody.

[Reference-generated vectors](../compatibility/fixtures/legacy-wallets.json)
cover multiple accounts/zones, HD/change/imported/payment keys, sender and receiver
accounts, a passphrase-protected empty wallet and French identity. Compatible
normalized exports are restored by the published JS SDK during fixture generation.
[Native and worker tests](../crates/quai-sdk/tests/legacy_wallets.rs) compare Rust
exports with those vectors and test forgery, duplicates, bounds, identity mismatch,
monotonic merge and downgrade rejection. The wallet-import fuzz target exercises
parsing, ownership checks and import/export roundtrips using public toy origins.
