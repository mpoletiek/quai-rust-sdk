# quai-keystore

Legacy Web3 v3 and `x-quais` JSON keystore interchange, published as an alpha under active development. Native full-wallet backups belong to `quai-wallet`; this legacy format cannot preserve accounts, payment channels, UTXOs, reservations or arbitrary key origins.

`Keystore::from_json(bytes, limits)` validates a bounded document before KDF work. `decrypt(Password::Text(...), limits)` uses NFKC-normalized UTF-8, matching pinned quais.js; `Password::Bytes(...)` preserves exact bytes. Decryption supports AES-128-CTR and scrypt or PBKDF2-HMAC-SHA256/SHA512. Scrypt derives 64 bytes as the JS implementation does, while the serialized v3 `dklen` remains 32. The legacy key MAC covers derived-key bytes 16–31 plus ciphertext and is checked in constant time before decryption.

The address is mandatory and must match the decrypted key, including after IV changes. Optional `x-quais` v0.1 mnemonic entropy uses AES-256-CTR with derived-key bytes 32–63. Its language and absolute path must derive the exact decrypted private key with an empty BIP39 passphrase. Unsupported extensions, PBKDF2 mnemonic metadata, unknown languages and mismatched derivation fail explicitly. This prevents the legacy format's unauthenticated mnemonic fields from being treated as verified recovery information. Nonempty BIP39 passphrases and arbitrary original seeds require native backups.

`encrypt` and `encrypt_with_mnemonic` work on native and browser Wasm and use fresh OS/Web Crypto salt, IV and UUID; mnemonic encryption has a separate fresh IV. Export uses the pinned JS defaults N=131072, r=8, p=1. There is no production API for injecting deterministic entropy or weakening export KDF parameters. Explicit `as_json()` exports encrypted JSON; key, password and mnemonic wrapper diagnostics are redacted.

All KDF calls are synchronous. Use an application-owned bounded worker thread or dedicated browser worker, not an async executor/UI thread. Wasm compilation and an actual Chromium dedicated-worker scrypt/mnemonic import test pass. A Chromium dedicated-worker export/import at the fixed production KDF cost also passes, including verified mnemonic recovery. Broad performance, interruption behavior during synchronous KDF work and other browsers remain separate qualification gates. Per-operation limits do not bound concurrent application calls; the caller must cap workers.

## Bounds and deliberate legacy restrictions

- At most 64 KiB JSON and 16 container levels; duplicate fields, including case-insensitive aliases, fail before conversion to a map. Unsupported field names/extensions fail rather than disappear.
- Exactly 32 ciphertext bytes and 16-byte IV; salt length 1–64 bytes. The account address is required even though JS allows its omission. JSON KDF numbers must be integer tokens, not strings/floats.
- Password input at most 1024 bytes and normalized output at most 4096 bytes. Empty passwords remain importable/exportable for compatibility; applications choose their password policy.
- Default scrypt working-memory ceiling 256 MiB plus 64 KiB, which admits the standard N=2^18, r=8 documents, and N*r*p at most 2^24; PBKDF2 at most 2,000,000 rounds. Caller limits have absolute ceilings of 1 GiB, 2^28 and 10,000,000 rounds. Memory budgeting uses `128*r*p*(N+2)`, covering B plus a V/T workspace for each parallel lane. This remains safe if another application dependency enables the additive Cargo `scrypt/parallel` feature; `p>1` may therefore be rejected under a policy that would allow sequential execution. These are denial-of-service bounds, not recommended encryption strengths.
- Legacy weak KDF settings may be imported within bounds. New exports always use the fixed stronger default above. Tests intentionally use already-public low-cost fixtures; never fund them.

Owned normalized passwords, derived keys, MAC input, plaintext and mnemonic entropy are guarded and wiped on drop. Normalized UTF-8 is built directly into one guarded 4096-byte allocation with a checked output limit, avoiding growth reallocations containing earlier normalized password bytes. AES/CTR zeroization features are enabled. **RustCrypto scrypt 0.12.0 internally allocates B/V/T work arrays without wiping them and exposes no caller-owned workspace API.** Backend/compiler temporaries are not covered by the SDK guards. This remains a documented security-review limitation; no claim of complete memory erasure is made. The legacy MAC does not authenticate all document fields and is not a substitute for native AEAD full-wallet backup.

## Evidence and provenance

The fixture generator `compatibility/scripts/generate-keystore.mjs` runs the pinned `quais@1.0.0-alpha.57` implementation. It captures 15 public cases: three key/password forms, ten mnemonic languages/paths and PBKDF2 SHA256/SHA512. PBKDF2 and deterministic mnemonic ciphertext are also constructed with Node's independent crypto backend. Tests compare Rust import and deterministic private test-only export, and reject changed passwords, key ciphertext, IV, address, mnemonic data/path/language, duplicate aliases and hostile KDF bounds. Linux native tests include a fresh production-cost export/import check, strict hostile JSON aliases/numeric/depth checks, normalization expansion boundaries and the parallel-workspace memory budget. Broader platform qualification is tracked separately.

Sources: pinned [quais.js keystore implementation](https://github.com/dominant-strategies/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/src/wallet/json-keystore.ts), [password normalization](https://github.com/dominant-strategies/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/src/wallet/utils.ts), RustCrypto [scrypt 0.12.0 source](https://docs.rs/scrypt/0.12.0/src/scrypt/lib.rs.html), [AES](https://docs.rs/aes/0.9.3/aes/) and [CTR](https://docs.rs/ctr/0.10.1/ctr/) primitive contracts. No code is copied from the Go node into this crate.


The bounded code review also inspected the installed scrypt 0.12.0 source at release
VCS commit `a795bf6f2dea6b9700fcc78f82a2ceaa19f36da7`. Its B/V/T allocations are
ordinary vectors in both sequential and optional parallel paths. A possible follow-up
is an upstream caller-owned workspace API, or a narrowly maintained vendor patch
wrapping all three vectors in `Zeroizing`, with identical KDF vectors and feature-matrix
tests. Such a patch must cover both paths and receive separate review; it would still
not prove erasure of SHA/Salsa/compiler temporaries. No backend fork, dependency
replacement or independent external security audit is claimed here.

The derive module exposes standalone bounded PBKDF2-HMAC-SHA256/SHA512 and scrypt, guarded output and coarse before/after lifecycle checkpoints. Run CPU work on caller-owned bounded workers.
See the [utility guide](https://github.com/mpoletiek/quai-rust-sdk/blob/main/docs/UTILITY_PARITY.md).
