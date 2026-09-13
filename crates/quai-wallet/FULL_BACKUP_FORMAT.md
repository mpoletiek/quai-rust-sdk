# QUAIWALT v1–v5 — authenticated wallet state and secret origins

This is a separate format from the existing fixed-size `QUAISEED` v1 seed backup.
`full_backup::{BackupOrigin, WalletBackup, EncryptedWalletBackup}` is available
with the portable `backup` feature, including browser workers. Native `sqlite`
enables that feature and adds atomic capture/restore for all scopes in one selected
wallet database and explicit supplied secret origins. It does **not** inventory arbitrary
application state, external payment channels, other databases, devices or files.
Do not describe it as a complete backup of an application that owns such state.

## Supported origin matrix

| Origin | Preserved | Verification and restore access |
| --- | --- | --- |
| Original BIP32 seed, 16–64 bytes | Exact bytes and representation | Re-derive BIP44 994/969 accounts and child keys; explicit `expose_seed` |
| BIP39 mnemonic plus passphrase | Effective 64-byte seed after wordlist/passphrase normalization | Same HD identity; original phrase, language label and passphrase are not retained |
| Depth-zero master xprv | Full private master and chain code | Re-derive accounts/children; explicit guarded `export_master_xprv` |
| Imported or generated standalone private key | Exact scalar through guarded `SecretBytes` | Validate public ownership; recover guarded `SecretKey` |
| BIP44 account/coin-level xprv | Unsupported | Reject; never reinterpret as a master |
| Watch-only/public-only ownership | Unsupported in this encrypted spending-backup API | Every included HD/imported public record must have a supplied secret owner |
| SQLite-registered BIP47 channels backed by supplied seeds or master xprvs | v2 preserves all owner/peer/account identities, eighteen cursors, burned exposure ranges and public destinations | Re-derive m/47'/969'/account'; validate exact owner, peer and every receive/send point |
| Depth-three BIP47 account xprv | v3 preserves exact key and asserted account | Validate hardened account and explicit m/47'/969' ancestry assertion; never reinterpret as a BIP44 master |
| External application-held payment channels | Not inventoried | Explicitly register/import into this database before capture, or back up separately |

A seed origin can cover both coin types and all included accounts/zones. A backup
may contain up to 16 origins and 64 scopes. All supplied origins use redacted
Debug and zeroizing secret ownership; none implements generic wallet serialization,
Clone or Display. Explicit secret exports remain the caller's responsibility.

## Public state included

Capture reads every scope in the selected SQLite database within one read
transaction. It retains chain/genesis/zone identity, immutable public address and
HD-origin metadata, account xpub bindings, burned receive/change derivation bounds,
account nonce cursors, operation IDs and states, held Qi claims and their owners,
account nonce allocations, caller-observed inclusion metadata and canonical signed
payload bytes when available, including verified ordinary Qi, conversions and
wrapping. Older SDKs without specialized decoding will reject those operations. Released operation IDs remain consumed. Schema v2 also
captures every registered payment channel and public exposure in that same transaction.

Capture rejects unknown native database tables or columns. This catches unsupported
state added to that database; it cannot detect channels or other data held elsewhere
by the application. It does not serialize configuration credentials, RPC URLs,
mnemonic text, wallet UI preferences, scan checkpoints or UTXO snapshots. Public
transaction payloads may contain public application calldata and must not contain
secrets intended to stay off chain.

`WalletBackup::restore` proves each included address and cursor against the supplied
secret roots before writing. HD proof checks coin/account/branch/actual child index,
public key and address; imported-key proof matches the full public key. Cursor
xpubs must match a derived account. Signed bytes are re-decoded and their signatures,
chain identity, sender/nonce or exact Qi input/owner set and transaction hash checked.
Public fields cannot substitute for possession of the corresponding backup secret.
For v2, every channel must match a supplied seed/master-derived private payment owner.
Every exposure is re-derived using its exact peer, direction, zone and child index;
its burned range must contain that index and not exceed the stored channel cursor.
Only verified receive exposures can prove ownership of corresponding imported-public
address metadata. Send destinations are peer-owned and never establish local ownership.

Restore merges in one SQLite writer transaction. Cursor values never decrease.
An existing operation must exactly match the backup record; conflicting records
fail the whole transaction rather than downgrading a newer signed/submitted state.
Existing operations absent from the backup are retained. Claim collisions fail
atomically. Restoring cannot release a signed claim, overwrite a transaction, rewind
nonce allocation or make burned address ranges available again. Existing metadata
must also agree with any newly restored account-xpub binding. Registered channels
merge each of their eighteen cursors monotonically, with exhaustion permanent.
Exposure collisions must match exactly. Existing target channels/exposures omitted
from an older backup are preserved, including when restoring v1 into a schema-v2
database. Channel imports invalidate all existing target scopes on affected networks.

All restored scopes have their UTXOs and scan checkpoints invalidated and their
local generation incremented. They require a fresh qualified scan. `RestoreReport`
reports new generations and signed/hash-only operations from the backup that remain
held for reconciliation. Existing target operations may have additional outstanding
reconciliation requirements. Block inclusion observations are not treated as finality
proofs. A hash-only signed operation remains reserved but lacks bytes for rebroadcast.
The facade reconciles canonical origin inclusion without releasing signed claims.
Signed replacement graphs and automatic terminal release remain unimplemented.

## Binary envelope

All integers are unsigned big endian. Ciphertext is authenticated with
Argon2id v1.3 and XChaCha20-Poly1305, the same maintained RustCrypto primitives and
bounded KDF arena handling as [QUAISEED](BACKUP_FORMAT.md).

| Bytes | Field |
| --- | --- |
| 0–7 | ASCII `QUAIWALT` |
| 8–11 | `[version, 1, 1, 0]`: version 1 or 2, Argon2id-v19 ID, XChaCha ID, reserved |
| 12–15 | Argon2 memory KiB |
| 16–19 | Iterations |
| 20–23 | Lanes |
| 24–39 | Fresh OS-random 16-byte salt |
| 40–63 | Fresh OS-random 24-byte nonce |
| 64–67 | Plaintext/ciphertext byte length, at most 16 MiB |
| 68 onward | Encrypted plaintext of exactly that length |
| Final 16 bytes | Poly1305 authentication tag |

The entire 68-byte header is associated data. Parsers check magic, IDs, reserved
byte, total length and KDF limits before copying a ciphertext or allocating KDF
memory. Passwords are exact bytes, 1–1024 bytes, without normalization. Default
Argon2id cost is 64 MiB / 3 passes / 4 lanes. Accepted costs remain 64–256 MiB,
3–6 passes, 1–4 lanes and at most 768 MiB-passes. Wrong password, corruption,
unsupported encrypted content and invalid decrypted ownership share `UnlockFailed`.
Resource failures remain distinct and contain no secret diagnostics.

The encrypted body starts with a u32 extension bitmap: zero for v1 and exactly one
for v2. Writers retain v1 when no registered channels/exposures exist; v2 is emitted
when channel state is present. Existing v1 fixtures and decoding remain unchanged.
Unknown bits, origins, fields or trailing bytes fail; no extension is skipped.
The body then contains:

1. u16 origin count. Each origin: u8 kind (`1` seed, `2` master xprv, `3` standalone
   scalar), u16 byte length, bytes. Master xprv bytes are canonical UTF-8/ASCII text.
2. u16 scope count. Each scope begins with chain ID32, genesis32 and zone1.
3. Per scope, four u32-counted collections in order: addresses, derivation cursors,
   nonce cursors, operations.

Address records contain address20, compressed public key33, origin kind1 (`0`
imported, `1` BIP44); BIP44 adds coin u16, account u32, change bool1, index u32.
Derivation records contain coin u16, account u32, change bool1, xpub u16-length text,
next raw index u32. Nonce cursors contain address20 and next nonce u64.

Operation records contain ID16, kind1 (`0` Qi, `1` Quai), state1 (`0` Reserved,
`1` Signed, `2` Submitted, `3` Confirmed, `4` Released), optional transaction hash,
optional inclusion `(hash32,height32)`, u16 Qi-claim count with each claim
`(hash32,index u16,owner20)`, optional `(account address20,nonce u64)`, and u32 signed
payload length followed by bytes (zero means absent). Optional values and booleans
use exactly `0` or `1`. Payloads retain canonical signed transaction order.

After the core scope collections, v2 appends two u32-counted collections:

1. Channels (maximum 1,024): network64 `(chain32,genesis32)`, local payment payload80,
   peer payload80, account u32, source local generation u64, u16-length channel JSON
   (1–4,096 bytes). JSON follows the strict `bip47-quai-969-v1` schema and nine-zone
   order from `quai-payments`; no unknown fields or schema versions are skipped.
2. Exposures: network64, local80, peer80, account u32, direction1 (`0` send, `1` receive),
   zone1, raw child index u32, compressed point33, Qi address20, burned start/end u32.
   Ranges are half-open; end may equal 2^31 (exhausted).

Source generations are local bookkeeping, not a freshness proof. Restore increments
existing local channel generations and does not copy a remote generation over them.
Channel ownership verification allows at most 8,192 origin/channel combinations.

The body is bounded to 16 MiB and 100,000 total public records/claims/channels/exposures. An operation
has at most 4,096 Qi claims and at most 1 MiB signed bytes. Aggregate signed payload
size is checked before capture loads payloads. Decryption decrements a global
record budget before constructing collections and rejects impossible counts against
remaining bytes. Ownership checking permits at most 4,096 distinct accounts,
8,192 origin/account combinations and 200,000 public-child verification attempts.

The plaintext writer reserves its full fixed capacity before writing any secret,
preventing reallocations from leaving earlier secret copies behind. Decrypted body
buffers, serialized bodies, secret origin buffers, KDF keys and Argon2 working arenas
are zeroizing. Public metadata and transaction records are ordinary public types.
Zeroization cannot guarantee removal of caller copies, compiler spills, crash dumps
or every temporary inside dependencies. Run KDF/large verification work on a CPU
worker and bound concurrent backups at the application layer.

The format authenticates backup content; it cannot prove that a successfully
authenticated old backup is the latest one. Import into an existing database
preserves counters and rejects state conflicts. Restoring onto an empty database
cannot recover address ranges or broadcasts that happened after that backup.
Keep backups current and use external trusted recovery bounds/chain reconciliation
before exposing new addresses or spending after a stale-device restore.

## Validation

`tests/full-backup-vector.json` is an independent public toy vector generated by
`tests/generate-full-backup-vector.py` using Python cryptography Argon2id and system
libsodium XChaCha20-Poly1305 (generator versions recorded in the fixture). No real
wallet input is read or written by that generator. Runtime APIs never accept a
caller-supplied salt or nonce. `full-backup-v2-vector.json` and its separate Python
generator independently encode and encrypt a registered channel with a burned cursor;
public payment-code identity comes from the pinned quais.js payment fixture.

Tests cover exact vector ciphertext, successful authentication, wrong passwords,
tampering, authenticated unsupported extension bits, fresh salt/nonce generation,
header/length/KDF bounds, effective English/Japanese/Portuguese mnemonic identities,
master-xprv and generated-imported-key preservation, full public-state round trips,
monotonic index/nonce restoration, retained signed claims and bytes, wrong-root
rejection, unsupported non-master origins/tables, malformed cursor data and rollback
after metadata insertion when an operation conflicts. QUAISEED support remains a
separate unchanged API with its own independent fixture. V2 tests additionally cover
registered receive ownership, tampered channel/exposure records, unsupported payment
origins, independent ciphertext, authenticated header downgrade rejection, v1 import
into a channel-bearing target and stale restore without address reuse.


## Portable decryption and inspection

The `backup` feature enables the same bounded QUAIWALT v1–v5 decryption,
ownership verification and fresh encryption on native Rust and WebAssembly.
`scope_state` returns borrowed addresses, exact raw derivation cursors, nonce
cursors and original reservation records with all signed candidate bytes.
`payment_channels` and `payment_exposures` retain their network, owner, peer,
account, direction and burned-range context. Access does not mutate the backup,
allocate indexes/nonces, submit transactions or establish current spendability.
Origin exports retain their guarded secret wrappers and the backup's Debug stays
redacted. Run memory-hard operations in an application-managed worker.

Existing native capture and monotonic restore still use SQLite transactions.
Shared public records, payment proofs and candidate validation have moved out of
the backend; no database schema or encrypted-format change was made. Portable
inspection is not a live wallet-store restore: applications must not overwrite a
newer cursor or release a signed claim based on an older backup. Complete browser
reservation and merge integration remains separate work.

Two independent Python/Argon2/libsodium v1/v2 vectors and three deterministic
native-Rust v3/v4/v5 fixtures execute in native and actual Chromium worker tests.
The latter are cross-platform interoperability evidence, not independent oracles.
They retain payment account/exposure ownership, account candidates/nonces, and Qi
candidates/claims plus a burned HD change range. Public test secrets only.
`test-infra/generate_portable_backup_fixtures.py` explicitly regenerates v3/v4/v5
through native tests; it never reads a user's wallet or exposes a production API
for deterministic encryption randomness.
