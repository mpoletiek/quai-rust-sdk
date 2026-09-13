# Signature and signing-key parity

This review covers the published `quais@1.0.0-alpha.57` Signature and SigningKey
APIs. Rust separates validated keys, mathematical signatures, legacy metadata and
secret output buffers. These utilities do not change Quai transaction signing,
network selection or wallet authorization.

| Published operation | Rust equivalent |
| --- | --- |
| `SigningKey(privateKey)` | `SecretKey::from_bytes`; validates a nonzero scalar below the secp256k1 order |
| `privateKey` | Explicit `SecretKey::export_bytes()` into a redacted zeroizing buffer |
| `publicKey`, `compressedPublicKey` | `public_key().to_uncompressed()` / `to_compressed()`, always derived from the actual scalar |
| `sign(digest)` | `sign_prehash` over an exact 32-byte digest with deterministic low-S ECDSA |
| `computePublicKey` | Explicit secret import plus derivation, or `PublicKey::from_sec1_bytes` and selected serialization |
| `recoverPublicKey` | `RecoverableSignature::recover_prehash` with explicit public-key serialization |
| `computeSharedSecret` | `SecretKey::ecdh_shared_point` returns a guarded 65-byte uncompressed SEC1 shared point |
| `addPoints` | `PublicKey::add_point`, rejecting the identity result |
| Signature `r`, `s`, `yParity` | `r`, `s`, `recovery_id`; parity serialization rejects mathematical recovery IDs 2/3 |
| `serialized` | `to_quais_bytes` emits R/S plus normalized 27/28 |
| `compactSerialized`, `yParityAndS` | `to_eip2098`; the latter is bytes 32..64 of that representation |
| `Signature.from` | Explicit `from_quais_bytes`, `from_eip2098`, `from_compact` or `SignatureMetadata::from_rs_v` constructors |
| `v`, `networkV`, `legacyChainId` | `SignatureMetadata::v`, `network_v`, `legacy_chain_id` |
| `getNormalizedV`, `getChainId`, `getChainIdV` | `normalized_v`, `legacy_chain_id`, `legacy_chain_v` with checked U256 arithmetic |
| `clone`, `toJSON` | Rust Copy/Clone for public signature metadata; `SignatureMetadata::to_json` emits compatible field names and exact decimal legacy V |

## Compact formats and legacy metadata

Raw compact `r || s` bytes and EIP-2098 `r || yParityAndS` are distinct formats.
The latter stores parity in S's top bit. Rust extracts that bit and validates
nonzero R/S scalars, curve-order bounds and low S. It never silently normalizes a
received high-S signature or fabricates a default zero signature. Full recovery
IDs 2/3 remain supported mathematically, but cannot be represented by Quai's
parity-only 65-byte or EIP-2098 forms.

`SignatureMetadata` retains a validated parity signature and optional EIP-155 V.
It accepts explicit R/S plus parity 0/1, normalized 27/28 or legacy V >= 35. Legacy
chain zero remains `Some(0)`, distinct from absent legacy metadata. `network_v`
is separate from normalized signature serialization; it is not inserted into
Quai's protobuf signature or treated as the current signer/network chain ID.
For a legacy 65-byte input, callers explicitly split R/S and pass the final byte
to `from_rs_v`; ordinary `from_quais_bytes` does not guess legacy transaction context.

The free `legacy_chain_id` helper returns zero for 27/28 and rejects parity-only
0/1, matching the published helper. `legacy_chain_v` requires normalized 27/28
and rejects U256 overflow. The published helper accepts invalid normalized values;
Rust rejects them. The largest representable result is tested for both parities.
JSON exports include `_type`, R/S hex, normalized V and optional decimal networkV;
input objects must be parsed explicitly and supplied through validated constructors.
No arbitrary JavaScript object/property coercion is performed.

## Keys, ECDH and point arithmetic

Key import distinguishes a private scalar from compressed/uncompressed public
SEC1 bytes. It does not guess whether a 32-byte input is a secret, an x-only key
or an address. Raw 64-byte X/Y coordinates require explicitly adding the standard
0x04 SEC1 prefix before public import. All imported public points must be valid
nonidentity secp256k1 points.

The published SigningKey constructor accepts a separate cached compressed key
without proving it belongs to the private scalar. As a result, its displayed
public key can disagree with the key recovered from its signatures. Rust derives
the public key from the validated secret and offers no unverified cache override.
The published Signature constructor also permits zero R/S and certain high-S
values; Rust validates the group-order and low-S requirements.

`ecdh_shared_point` matches the full published SEC1 output. The existing
`ecdh_shared_x` returns its 32-byte X coordinate for protocols that specify that
form. Both are shared secret material and require the protocol's prescribed KDF
before use as encryption keys. Full-point output uses `SecretBytes<65>`; the
default `SecretBytes` remains 32 bytes. These buffers redact diagnostics, have no
Clone/Copy/serialization, and zeroize their owned bytes on drop. Named scalar,
point and SEC1 encoding intermediates are guarded; compiler/backend transient
copies and caller-created copies cannot be guaranteed erased.

`add_point` performs ordinary curve addition and rejects infinity. It does not
implement a signing protocol or prevent rogue-key attacks. The separate
`OrderedKeyAggregate` implements the SDK's supported ordered MuSig aggregation.
The extra Rust shared-X, tweak, Schnorr and aggregation APIs remain available.

## Evidence

[Published public-toy vectors](../compatibility/fixtures/crypto-utils.json) cover six
key pairs, sixteen signatures across both parities and fifteen legacy V values.
[Four source regressions](../compatibility/scripts/crypto-utils.test.mjs) verify
exact bytes and identify permissive signature/cached-key behavior.
[Four shared native/worker tests](../crates/quai-crypto/tests/metadata.rs) verify
formats, recovery, JSON metadata, overflow, ECDH symmetry/X extraction, point
addition, rejection boundaries and redacted outputs. A native unit check verifies
both secret-buffer sizes retain zeroizing guards. Existing curve/signature vectors
remain regression evidence, and sanitizer fuzzing exercises the new formats and
curve identities. No real private wallet material or funded transaction is used.
