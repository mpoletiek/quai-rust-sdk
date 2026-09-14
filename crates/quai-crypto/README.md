# quai-crypto

Experimental cryptographic foundations for the Quai Rust SDK. This crate is an
alpha; it is not independently audited or qualified for production funds.

The public API provides:

- A validated secp256k1 `SecretKey` with redacted diagnostics and backend
  zeroization on drop; OS-backed generation fails on entropy errors. It has no
  `Clone`, `Copy`, `Display` or serde implementation. Explicit `export_bytes`
  returns a redacted, non-Clone `SecretBytes` buffer for encrypted backup integration; callers must
  protect that deliberately exposed copy from logs and ordinary serialization.
- Validated SEC1 `PublicKey` parsing, canonical compressed/uncompressed encoding,
  and raw address derivation. Address derivation does not grind for a zone or
  promise that the address is a Quai account.
- Deterministic recoverable low-S ECDSA over exact 32-byte prehashes. Recovery IDs
  remain zero through three until explicitly converted to quais.js's 65-byte
  parity form; unsupported bits and high S are rejected, not silently discarded.
- Raw-message BIP340 signing and verification. Public signing always obtains
  fresh 32-byte auxiliary randomness from the OS and verifies the result before
  returning it. No implicit SHA256 pass is applied to the message.
- Guarded raw ECDH X-coordinate derivation and additive public/secret scalar
  tweaks using maintained k256 operations, including identity/zero rejection.
- Ordered MuSig public-key aggregation and local signing when one process owns
  every input key; key order and duplicates are preserved.
- Keccak256, SHA256, SHA512, RIPEMD160, HMAC-SHA256/HMAC-SHA512 and constant-time HMAC tag
  verification. Derived HMAC output bytes remain the caller's responsibility.

Password-based KDF policies, encrypted key storage, HD derivation, typed-data
hashing, distributed multisignature protocols and transaction preimages remain
outside this crate. The wallet and consensus crates own their respective next
steps. No transaction encoding is inferred from Ethereum familiarity.

## Compatibility choices

`hash_message(message: &[u8])` uses the actual pinned quais.js prefix,
`\x19Ethereum Signed Message:\n`, followed by the decimal byte length and exact
message bytes. UTF8 text callers use `.as_bytes()`; a string such as `0x4243`
is not automatically decoded as hex. This follows
[quais.js message hashing](https://github.com/dominant-strategies/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/src/hash/message.ts)
and its
[prefix constant](https://github.com/dominant-strategies/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/src/constants/strings.ts).

ECDSA nonce derivation applies RFC6979 `bits2octets` reduction using the backend's
scalar operation before calling its deterministic signer. This matters for a
32-byte digest at or above the group order: ecdsa 0.16's convenience method
passes unreduced digest bytes to its nonce generator, while the pinned noble
implementation reduces them. The wrapper's explicit reduction matches quais.js
and retains verification/recovery against the original digest. Fixtures cover
zero, all-ones, a patterned digest and `n-1`, `n`, `n+1` for four public test keys.
See [RFC6979 section 2.3.4](https://www.rfc-editor.org/rfc/rfc6979#section-2.3.4).

Public keys accept only canonical SEC1 shapes: 33 bytes starting with 02/03 or
65 bytes starting with 04. The API deliberately does not reinterpret raw
64-byte coordinates or 32-byte inputs as public/private keys interchangeably.
`SchnorrPublicKey` separately models BIP340's 32-byte x-only form.

## Backend and limits

The implementation uses RustCrypto k256 0.13.4 with ECDSA/Schnorr/ECDH/std and no
PKCS8 or serde features, plus getrandom, zeroize, sha2 and hmac. RustCrypto
[documents the backend's NCC Group audit](https://github.com/RustCrypto/elliptic-curves/blob/5ac8f5d77f11399ff48d87b0554935f6eddda342/k256/README.md),
including corrected findings and processor limitations; that audit does not
cover this SDK wrapper. The
[OS entropy API](https://docs.rs/getrandom/0.4.3/getrandom/fn.fill.html)
reports failures instead of supplying a fallback.

Stored secret scalars, temporary signing keys and explicitly owned random
buffers zeroize on drop. Caller-owned import buffers, copies made before
import, registers, compiler spills, crash dumps and all cryptographic internal
intermediates are not guaranteed erased by this API. No browser/wasm randomness
or side-channel qualification has been completed.

The tests contain public toy keys that must never be funded. Tests compare
24 deterministic ECDSA vectors from the pinned JS package, official BIP340
vectors including invalid encodings and non-32-byte messages, standard hash/MAC
vectors, malformed keys, high S, recovery-ID corruption and OS-backed signing.
They do not establish acceptance of a Quai or Qi transaction by any node.

## Ordered local aggregation

`OrderedKeyAggregate` supports untweaked aggregation of 2 through 1024 public
keys. The cap is a resource policy, not a consensus input limit; one input uses
ordinary Schnorr. The context preserves compressed SEC1 key order and duplicate
keys, matching the pinned node's `musig2.AggregateKeys(keys, false)`.

The implementation delegates public aggregation and coefficients to
[musig2 0.4.1](https://docs.rs/musig2/0.4.1/musig2/struct.KeyAggContext.html)
with only its k256 backend enabled. It uses the ordered `KeyAgg list` hash and
`KeyAgg coefficient` tagged hashes, with coefficient one for the first key
distinct from the first input, following
[BIP327 key aggregation](https://github.com/bitcoin/bips/blob/master/bip-0327.mediawiki).
No point tweaks or sorting are applied. Eleven pinned JS cases cover reversed
order, repeated keys and all-identical key lists. Rust and the independent Go
oracle match their full compressed aggregate points and verify their signatures.

`sign_local` requires the exact count and order of locally owned secret keys.
It computes the weighted scalar sum using constant-time k256 scalar operations,
checks its full public point against the library aggregate, then invokes the
existing OS-randomized BIP340 signer and verifies the result. It never exposes
an aggregate secret or partial signature. Public coefficients come from musig2;
private scalars are never passed to its `aggregated_seckey` helper, whose owned
scalar intermediates do not provide the erasure guarantees required here.
Named private copies, products, accumulator and encoded aggregate bytes use
zeroizing guards, including on errors. The compiler/backend erasure limits
above still apply.

This is local signing by an owner of all keys. It does not implement a
distributed MuSig session, nonce exchange, partial-signature validation or nonce
reuse prevention across participants. Those protocols remain separate work.
The independent [Go oracle](https://github.com/mpoletiek/quai-rust-sdk/blob/main/test-infra/go-oracle/README.md) verifies both
JS protocol signatures and captured signatures produced by this local Rust API.
Neither musig2 nor this integration has been independently audited for the SDK.


`ripemd160` hashes exact bytes into `[u8; 20]`. The reference text `id` helper is
`keccak256(text.as_bytes())`: no hex decoding or implicit normalization occurs.
`MESSAGE_PREFIX` retains the pinned Ethereum personal-sign prefix as bytes; the
message hash uses byte length. Crypto backends are fixed Rust implementations,
without JavaScript global registration/locking hooks.

`fill_random(&mut bytes)` fills at most 65,536 caller-owned bytes using the native
OS or browser Web Crypto. An oversize request leaves the destination untouched;
a backend failure clears partial output and returns `RandomnessUnavailable`.
No PRNG fallback exists. Successful caller buffers remain caller-owned sensitive
material. Native boundary and failure tests plus actual Chromium worker execution
cover this API; those checks do not constitute statistical entropy certification
or qualification of every browser engine.

## Compact signatures, legacy metadata and full ECDH points

`RecoverableSignature::from_eip2098` / `to_eip2098` handle `r || yParityAndS`,
separately from raw compact `r || s`. Both scalars must be nonzero, in range and
low S. Recovery IDs 2/3 cannot fit parity-only representations and reject.

`SignatureMetadata` wraps a validated signature and optional EIP-155 V. Construct
from exact R/S/V or an existing parity signature; inspect `v`, `network_v` and
`legacy_chain_id`, or export `to_json`. Free `normalized_v`, `legacy_chain_id` and
`legacy_chain_v` helpers use checked U256 quantities. This metadata does not alter
Quai protobuf signing or authorize another network. No default zero signature,
unverified public-key cache or arbitrary JS object coercion is accepted.

`SecretKey::ecdh_shared_point` returns the full uncompressed SEC1 shared point in
`SecretBytes<65>`, matching published SigningKey ECDH output. Existing
`ecdh_shared_x` returns its X coordinate in the default 32-byte secret buffer.
Both require the calling protocol's KDF, redact diagnostics and zeroize owned
buffers. Backend/compiler transients and caller copies cannot be guaranteed erased.
`PublicKey::add_point` adds validated points and rejects infinity; use the separate
ordered aggregation API when a MuSig signing protocol is required.

The `curve` module supplies guarded zero-inclusive scalar arithmetic, validated
point multiplication/lifting, public field utilities and bounded multipart/tagged
hashes. See the [curve adapter review](https://github.com/mpoletiek/quai-rust-sdk/blob/main/docs/CURVE_AND_AGGREGATION_PARITY.md)
for source mappings, tag encoding and protocol/secret-handling limits.
