# Encrypted seed backup version 1

`SeedBackup` encrypts the original 16–64 seed bytes and coin/preferred-account
metadata. It preserves HD identity, including a BIP39 passphrase already applied
to that seed. It **is not a complete wallet backup**: it has no original mnemonic,
language, imported private keys, BIP47/payment-channel state, issued indexes,
outpoints, reservations, history or synchronization checkpoints. Restoring it
requires the appropriate subsequent discovery/recovery workflow. It must never
be labeled a complete Qi snapshot.

The format uses Argon2id v1.3 followed by XChaCha20Poly1305. Native encryption
always obtains a fresh 16-byte salt and 24-byte nonce from the operating system;
there is no public deterministic-nonce encryption API. Passwords are exact bytes,
1–1024 bytes long, with no implicit Unicode normalization. Password strength is
the application's/user's responsibility: the KDF slows guessing but cannot make
a weak password strong. The backup password and a mnemonic's BIP39 passphrase
are independent inputs.

The complete envelope is exactly **151 bytes**, with all integers big-endian.

| Byte offsets, half-open | Meaning |
| --- | --- |
| 0..8 | ASCII `QUAISEED` |
| 8 | Format version, 1 |
| 9 | KDF identifier 1: Argon2id version 0x13, 32-byte output |
| 10 | AEAD identifier 1: XChaCha20Poly1305 |
| 11 | Reserved; must be zero |
| 12..16 | Argon2 memory cost in KiB |
| 16..20 | Argon2 pass count |
| 20..24 | Argon2 lane count |
| 24..40 | Random salt |
| 40..64 | Random nonce |
| 64..135 | Encrypted 71-byte payload |
| 135..151 | Poly1305 authentication tag |

The entire 64-byte header is AEAD associated data. The decrypted payload is one
seed-length byte, two coin-number bytes (994/969), four account-index bytes
(less than 2^31), then the original seed padded to 64 bytes with zeros. Readers
require every unused padding byte to be zero. Coin/account and seed length are
encrypted, and no secret-bearing generic JSON serializer is exposed.

## Resource bounds and secret ownership

Default parameters are **64 MiB, three passes, four lanes**, matching the second
recommended Argon2id profile in [RFC 9106 §4](https://www.rfc-editor.org/rfc/rfc9106.html#section-4).
Version-one readers accept only memory costs 65,536–262,144 KiB, 3–6 passes,
1–4 lanes, and a memory-cost/pass product no greater than 786,432 KiB-passes.
Unknown identifiers, nonzero reserved bytes, wrong lengths and out-of-range
parameters are rejected **before KDF allocation**. Input length is checked before
copying the envelope into its fixed buffer. No attacker-provided payload length
controls an allocation.

The KDF uses fallible allocation of an explicitly zeroizing block arena. The
Argon2 convenience allocator does not erase its arena on drop; this implementation
does not use that allocator. The derived key, plaintext payload and seed storage
are zeroizing buffers, and the cipher enables its key-zeroization feature.
Seed/plaintext exports are explicit and `Debug` is redacted. Caller-owned input
buffers, caller-created copies and compiler/dependency temporaries remain subject
to the limitations documented in [README.md](README.md).

These are per-operation limits. Encryption/decryption are synchronous CPU/memory
work with no mid-KDF cancellation API; applications must bound concurrent jobs
and choose an appropriate CPU executor. Parsing an encrypted envelope validates
structure, not authenticity. Successful decryption authenticates its contents.
Wrong passwords, corrupted ciphertext, malformed format and unsupported version
all produce `BackupError::UnlockFailed`; actual allocation/randomness failures
remain separate operational errors. No file reads or writes are performed.

Encryption is currently exposed on native targets. Browser entropy and large
memory/cancellation behavior are not qualified by these offline tests.

## Independent vector and tests

`tests/backup-vector.json` is a deterministic **public toy fixture**, independently
generated using Python `cryptography 50.0.0` Argon2id and `libsodium 1.0.22`
XChaCha20Poly1305. Rust checks the entire envelope byte-for-byte and decrypts it.
Never reuse the fixture seed, password, salt or nonce for actual funds.

```sh
python3 crates/quai-wallet/tests/generate-backup-vector.py
cargo test -p quai-wallet backup::tests -- --test-threads=1
```

Regeneration requires the two reference implementations named above; ordinary
Rust tests use the checked-in fixture and require neither Python nor network
access. Tests use the production minimum KDF cost, exercise varying seed lengths
and coin/account metadata, wrong passwords, modified parameters/salt/nonce/
ciphertext/tag, oversized input and extreme KDF parameters, diagnostic redaction,
and fresh randomness. No reduced-cost test configuration is accepted by readers.

Primary implementation references:
[Argon2 0.6.0](https://docs.rs/argon2/0.6.0/argon2/),
[XChaCha20Poly1305 0.11.0](https://docs.rs/chacha20poly1305/0.11.0/chacha20poly1305/),
[cryptography Argon2id](https://cryptography.io/en/latest/hazmat/primitives/key-derivation-functions/#argon2id),
[libsodium XChaCha20Poly1305](https://doc.libsodium.org/secret-key_cryptography/aead/chacha20-poly1305/xchacha20-poly1305_construction).

The separate [QUAIWALT native-state format](FULL_BACKUP_FORMAT.md) includes explicit
secret origins and the supported native database state. This QUAISEED envelope
remains seed-only; its scope and guarantees have not been broadened.
