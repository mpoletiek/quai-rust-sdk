# quai-payments

Experimental BIP47 version-one payment codes and local Qi channel derivation.
This crate is not independently audited or production-qualified. It performs
no network operations, notification broadcasts, automatic discovery or sends.

## Derivation and API

`PaymentCode` validates the exact 80-byte payload or bounded Base58Check form:
prefix 0x47, payload version 1, features 0, a valid compressed secp256k1 public
key, a 32-byte chain code and thirteen zero reserved bytes. Public codes have
redacted Debug and explicit `to_base58`/`to_bytes` exports. They reveal extended
public-key and wallet-linkage information. A code alone does not encode or prove
its originating coin path.

`PrivatePaymentCode::from_seed(seed, account)` accepts every standard BIP32 seed
length from 16 through 64 bytes and derives `m/47'/969'/account'`. Account and
payment child indices are strictly below 2^31. The notification child is `/0`;
receive payment children are `/index` directly. This follows the actual pinned
[account construction](https://github.com/dominant-strategies/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/src/wallet/qi-wallets/bip47-self-qi-wallet.ts)
and [payment-code derivation](https://github.com/dominant-strategies/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/src/wallet/payment-codes.ts).
A QiHDWallet comment mentions an extra `/0/index` branch, which the executed
implementation does not use; this crate does not introduce that extra branch.

The sender uses its notification secret `a` and recipient child `B_i` to derive
`x(a * B_i)`. The recipient uses its child secret `b_i` and sender notification
point `A` to obtain the same X coordinate. Both hash that exact 32-byte value
with SHA256, require the result to be a valid nonzero scalar without modular
reduction, and add its generator multiple to `B_i`. The receiver also adds that
scalar to `b_i` to obtain its spendable key, checking the resulting public key.
See [BIP47](https://github.com/bitcoin/bips/blob/master/bip-0047.mediawiki).

`send_public_key`, `receive_public_key` and `receive_key` expose those operations.
`notification_key` explicitly returns a guarded secret wrapper for a protocol
that needs the notification child. Public key/address derivation by itself
does not promise a valid deployed zone or Qi ledger address. No private payment
account, notification secret or shared scalar has implicit serde, Display or
ordinary Clone/Copy support.

The implementation delegates BIP32 to bip32 0.5.3 and ECDH/additive operations
to maintained RustCrypto k256 through quai-crypto. It uses no custom curve or
big-integer implementation. Named master-HMAC output, imported scalar copies,
ECDH output and SHA256 tweak bytes are guarded. `SecretBytes` redacts Debug,
zeroizes on drop and has no Clone/Copy/Display/serde. Explicitly borrowed secret
bytes remain the caller's responsibility; registers, compiler spills and every
backend transient copy cannot be guaranteed erased. The backend's rare invalid
BIP32 child returns an error at its exact index rather than silently relabeling
a later child.

## Bounded Qi search and public channel state

`PrivatePaymentCode::search(peer, direction, PaymentSearch, cancelled)` is
stateless. It checks cancellation before each candidate, seeks a Qi-ledger
address in the requested deployed zone, and returns the exact ground index,
public key, destination, attempt count and first unexamined index. Cancellation
and exhaustion errors retain the same continuation metadata. The local maximum
is 100,000 attempts per explicit call; this is a resource policy, smaller than
the pinned JS convenience search cap of 10,000,000. Callers should choose short
chunks suitable for their runtime and retain continuation metadata.

`PaymentChannel` stores public state only and borrows a supplied private owner
for each operation. `from_json(owner, bytes)` verifies the exact local payment
code and account against that owner before accepting a channel. The maximum
input is 4096 bytes, and unknown/duplicate fields, unsupported versions/schemes,
invalid codes, wrong cursor arity and hardened/overflowing indices are rejected.

The stable JSON schema is:

```json
{
  "schemaVersion": 1,
  "scheme": "bip47-quai-969-v1",
  "account": 0,
  "localPaymentCode": "<validated Base58Check public code>",
  "counterpartyPaymentCode": "<validated Base58Check public code>",
  "sendNext": [0,0,0,0,0,0,0,0,0],
  "receiveNext": [0,0,0,0,0,0,0,0,0]
}
```

Cursor order is `Zone::ALL`: 00, 01, 02, 10, 11, 12, 20, 21, 22. A null cursor
means that direction/zone exhausted the nonhardened index space. Cursors retain
actual derivation indices, including skipped candidates.

`reserve_next` advances the selected cursor after finding a destination and
also retains progress past known nonmatching candidates on cancellation or
exhaustion. This explicit reservation operation differs from JS's
reuse-until-status-changes convenience method; successful reservations produce
distinct indices. Persist authenticated state before using a returned address
externally. The state contains peer relationships and must be protected by the
wallet's backup policy even though it contains no private keys.

Ownership validation does not authenticate imported cursor history, prevent
backup rollback, prove that an index was unused, or verify the intended identity
of a peer. Those guarantees require the wallet's authenticated storage and
application policy. Restoring a channel requires its original owner seed/master/account;
public metadata alone cannot derive receiving secrets.

## Validation and reproduction

The locked JS generator captures four public seed/account pairs, including a
17-byte seed and account 2^31−1, 24 matching sender/receiver payment keys at exact
indices including 2^31−1, and six first-match searches across three zones and
both directions. Tests also cover every standard seed length, all frame fields,
checksum/curve failures, wrong-owner/account restore, duplicate/unknown JSON,
precise cancellation/exhaustion continuation and exhausted cursor restoration.

The independent published
[Alice/Bob vectors](https://gist.github.com/SamouraiDev/6aad669604c5930864bd/beddb67cf219ead27b8b1380a3b084b8dde51093)
verify both payment codes, notification keys, ten ECDH X coordinates and ten
Bitcoin P2PKH destinations. Coin-zero derivation/address conversion is used only
inside these tests; the production constructor selects Quai coin969, and the
public search API selects Qi. Those tests establish compatibility of the shared
BIP47 cryptography, not Bitcoin network support in this crate.

```sh
node crates/quai-payments/tests/generate-fixtures.mjs
cargo test -p quai-payments
cargo clippy -p quai-payments -p quai-crypto --all-targets -- -D warnings
```

Every included seed/key is publicly known test material and must never be
funded. These results do not establish node acceptance, notification-protocol
privacy or production wallet safety. Version-two/Bitmessage features, blinded
notification construction, notification transaction parsing and automatic peer discovery remain separate
work. Native gap scanning and spend reservations live in the facade; actual
Chromium worker tests cover derivation, allocation and recovery. Other engines
and production notification protocols remain unqualified.

`from_master_xprv` derives the same payment identity as its original seed.
`from_account_xprv` validates depth/account while explicitly trusting the omitted
ancestry; full-wallet backup v3 and later support these explicit account origins.
The [facade workflow guide](../../docs/WALLET_WORKFLOWS.md) covers registered
channel send destinations, current receive scanning and spending through a
verified local keyring. Those network/storage workflows live outside this crate.
