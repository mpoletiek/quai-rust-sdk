# Curve adapters and threshold Qi aggregation

Reference: published `quais@1.0.0-alpha.57`, specifically
`lib/esm/crypto/musig.js` and `lib/esm/transaction/coinselector-aggregate.js`.
The `musigCrypto` export is an object containing 26 helpers; its two inventory
rows alone do not describe those behaviors. The Rust mappings below cover each
helper. These primitives do not supply a distributed MuSig nonce coordinator.
The existing `OrderedKeyAggregate` supports the documented local all-keys signing
workflow; a distributed application must implement its protocol's nonce rules.

## Curve helper mapping

| Reference | Rust |
| --- | --- |
| `read32b`, `write32b` | `U256::from_be_bytes`, `U256::to_be_bytes::<32>`; fixed-size unsigned input, no arbitrary-integer truncation |
| `readScalar`, `isScalar` | `curve::CurveScalar::from_bytes`; canonical `[0,n)`, including zero |
| `readSecret`, `isSecret` | `SecretKey::from_bytes`; canonical nonzero scalar |
| `scalarAdd`, `scalarMultiply`, `scalarNegate`, `scalarMod` | `CurveScalar::{add,multiply,negate,reduce_bytes}`, with explicit guarded export |
| `secp256k1Right`, `jacobiSymbol` | `curve::{public_curve_rhs,public_jacobi_symbol}` |
| `isPoint`, `isXOnlyPoint` | `PublicKey::{from_sec1_bytes,lift_x}` validation results |
| `pointNegate`, `pointX`, `hasEvenY` | `PublicKey::{negate,x_coordinate,has_even_y}` |
| `pointMultiplyUnsafe`, `pointMultiplyAndAddUnsafe` | `PublicKey::{multiply,multiply_add}`, using ordinary k256 operations |
| `pointAdd`, `pointAddTweak` | `PublicKey::{add_point,add_tweak}`; tweak requires a nonzero `SecretKey` |
| `pointCompress`, `getPublicKey` | `PublicKey::{to_compressed,to_uncompressed}`, `SecretKey::public_key` |
| `liftX` | Source actually parses SEC1 and returns uncompressed SEC1. Compose Rust `from_sec1_bytes` and `to_uncompressed`; Rust `lift_x` additionally supports real X-only lifting to even Y |
| `sha256`, `taggedHash` | `curve::{sha256_parts,quais_tagged_sha256}`; `tagged_sha256` additionally offers exact UTF-8 tag encoding |
| `N` | `curve::CURVE_ORDER`; `FIELD_PRIME` is also public |

`CurveScalar` has zeroizing storage, redacted Debug and no implicit Clone, Copy
or serde. `export_bytes` returns a `SecretBytes` guard. Reduction is not unbiased
random-key generation. Inputs, compiler spills and backend transient copies are
outside the erasure guarantee. Point results are explicitly public; protocols
using shared points as secrets should use the guarded ECDH APIs instead.

Invalid points and infinity return errors. Negation, X and parity methods only
accept an already validated public key; reference structure-only helpers can
accept off-curve encodings. Public field operations reduce arbitrary U256 input,
including the prime itself, and terminate with bounded 256-bit arithmetic. Their
variable-time implementation is for public values only. The source Jacobi loop
assumes a nonzero reduced input after its initial zero check; passing the prime
can enter a zero-shifting loop. We do not reproduce that behavior.

Multipart hashes stream at most 1024 parts and 1 MiB combined message bytes.
Tags are bounded to 1024 UTF-8 input bytes; there is no global tag cache.
The reference uses one truncated first UTF-16 code unit per Unicode code point
for tags. `quais_tagged_sha256` reproduces that behavior, including supplementary
characters; `tagged_sha256` uses UTF-8. ASCII BIP340/MuSig tags are identical.
Unpaired UTF-16 surrogates cannot be represented by a Rust `str`.

## Threshold aggregation

`quai_wallet::select_aggregate` and
`SweepMode::AggregateThreshold(AggregationPolicy)` add the reference's input
threshold and separate fee-input policy. Defaults aggregate input denominations
up to index 6, emit denominations up to index 14 and require fewer output UTXOs
than selected input UTXOs. `require_reduction: false` explicitly permits the
reference's warning-only non-reducing plans.

The selector validates snapshot outpoints, then excludes foreign-zone, reserved,
locked and expired coins. It selects small coins in stable ascending order,
leaves larger coins unused unless needed for fees, and puts fee inputs before
aggregation inputs. Aggregation denominations come first; any fee refund is
separately denominated and appended, so the complete output sequence need not
be descending. `change_outputs` is empty because all outputs are sweep outputs.
`CoinSelection::input_value` is the exact selected sum; the source mutable
`totalInputValue` counts every available coin before selection. Rust callers can
sum a supplied snapshot separately when they need that different metric.

The requested fee is always covered exactly. With three five-Qit coins and a
six-Qit fee, the reference selects all three but emits ten Qits, leaving only a
five-Qit fee. Rust emits nine Qits, leaving the requested six. This can increase
the output count; the default reduction policy then rejects that plan. The
explicit non-reducing policy permits it. Reduction compares all selected inputs,
whereas the source warning compares only its aggregation subset.

Bounds are 100,000 candidate records, 4096 selected inputs/outputs, the existing
protobuf message limit and the caller's fee cap. Denomination grouping and fee
selection are linear; duplicate detection uses an ordered set. The selector
never reserves inputs or allocates addresses. `quote_qi` and native
`QiSession::prepare_sweep` pass the mode through each fee-convergence round;
existing portable custody preparation consumes the resulting reviewed quote.
Fresh output addresses and durable claims retain their existing explicit APIs.

Increasing denominations still requires the pinned node's first-Qi-transaction
block exception. Neither this pure selector nor a successful RPC fee estimate
arranges that position or proves eventual network acceptance.

## Evidence

- `compatibility/fixtures/curve.json`: 49 scalar pairs, five reductions, sixteen
  point/scalar cases, seven public field cases and twenty multipart/tagged hashes.
- `compatibility/fixtures/aggregation.json`: 129 source cases, including the
  underpaid-fee regression. Successful covered cases retain exact input/output
  order; source errors are checked and the shortfall case is corrected explicitly.
- Six shared curve tests and five aggregation tests run natively and in Chromium
  workers. Source tests separately pin three curve and three aggregation behaviors.
- `crates/quai-sdk/tests/qi_preflight.rs` checks re-selection after a node fee quote
  without broadcasting. The wallet-import fuzz target checks scalar/point
  identities and bounded aggregation value conservation/spendability.

These are implementation and isolated test results, not funded public-network
acceptance, a distributed signing protocol audit or proof of cryptographic safety.
