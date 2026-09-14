# Transaction, protobuf and access-list parity

The published `quais@1.0.0-alpha.57` transaction classes map to concrete unsigned
and verified signed Rust types. `quai_consensus::document::TransactionDocument`
adds bounded JSON/protobuf interchange across Quai and all supported Qi operations.
It has unsigned Quai, unsigned Qi, signed Quai and signed Qi-operation variants.

| Published API | Rust equivalent |
| --- | --- |
| Abstract/Quai/Qi transaction constructors | `QuaiTransaction`, `QiTransaction`, typed conversion/wrapping builders; `TransactionDocument` state variants |
| `from`, `fromProto` | Concrete canonical decoders or `TransactionDocument::decode`, `from_proto`, `from_json` |
| `toJSON`, `toProtobuf` | `to_json`, `to_proto(include_signature)` |
| `serialized`, `unsignedSerialized`, `digest`, `hash` | `to_bytes`, `unsigned_bytes`, `signing_digest`, optional verified `hash` |
| `isSigned`, `type`, `typeName`, `inferType(s)` | Enum state, `is_signed`, fixed `type_id`, `type_name`; a single supported kind per variant |
| `chainId`, fields, signature | Concrete typed fields, immutable signed payload getters, verified signature getters |
| `originZone`, `destZone`, `isExternal` | `origin_zone`, `destination_zone`, `is_external` with explicit unknown origin |
| `clone` | Rust Clone; signed payload fields remain immutable |
| `ProtoTransaction`, encode/decode helpers | Raw public `proto` DTOs plus bounded `encode_proto_transaction`, `decode_proto_transaction` |
| Access-list types and `accessListify` | `AccessTuple`, ordered vectors, `access_list_from_json`, `access_list_to_json` |

## State, identity and operation semantics

Unsigned account documents may carry `claimed_sender`. It is a display/routing
assertion, not proof, and is absent from wire bytes. Decoding unsigned bytes cannot
recover it. Signed import recovers the sender from the exact preimage and rejects
any conflicting supplied sender. A supplied hash requires a signed document and
must equal the locally computed transaction ID. Qi signature import verifies
Schnorr/ordered-key aggregation instead of treating any string as signed.

Unsigned fields remain editable; serialization revalidates size and operation
rules. Signed variants expose immutable verified payloads: changing a field
requires a new signature. Explicit type zero/two replaces nullable inferred type
and prevents a Qi object from claiming an account wire kind. Unsigned account
serialization is not node acceptance validation; signing adds chain/address checks.

Qi document import selects transfer, wrapping or conversion by exact data length
0/20/22 and runs the corresponding typed builder/signature checks. Other lengths
reject, including the two retained JS fixtures known to be node-invalid. Input
order, duplicate-key participants, fixed denominations, outpoint identities and
output order are preserved. User-created nonzero locks reject. This does not
prove input existence, spendability, maturity, fee sufficiency or node acceptance.

`destination_zone` follows the source's first-output convention. Qi transactions
can have other destinations, so Rust additionally exposes `destination_zones`
and `has_cross_zone_outputs`. `is_external` returns None when an unsigned account
origin is unknown, and false for creation without a destination. A claimed
unsigned sender does not become verified merely because a zone can be derived.

## Exact JSON and raw protobuf

The JSON API takes a caller-parsed `serde_json::Value`; duplicate text keys already
lost by that parser cannot be detected. It rejects unknown fields and bounds
aggregate key/string bytes to 4 MiB, nodes to 65,536 and depth to 64 before
conversion. Required quantities accept exact unsigned decimal/hex strings or
integer u64 JSON values. Floating, fractional, negative and coerced inputs reject.
Full u64 nonce/gas and U256 amounts are preserved. Nonces above JavaScript's safe
integer range export as decimal strings; other amounts use published decimal
string fields. Absent optional fields normalize to explicit null/empty values.

JSON signature import accepts validated R/S/V objects, EIP-2098 or normalized
65-byte signatures for Quai, and exact 64-byte signatures for Qi. Supplied parity
and optional legacy V must agree. Legacy metadata is not part of Quai wire
signatures and normalizes away on export; it never changes the transaction's
chain ID. Unknown signature object fields reject. Address exports use checksums,
public keys compressed SEC1, and hashes/storage slots lowercase fixed-width hex.
These display normalizations do not change signed bytes.

Qi JSON data is exact hex, null when empty. The published constructor instead
passes its JSON hex string to a typed-array length constructor, which can discard
data or throw. Rust imports the intended bytes. Empty output locks export as
`0x`; nonempty locks are rejected instead of silently disappearing on re-encoding.
The published JSON constructors also accept conflicting supplied hashes, and a
Qi signature string is not cryptographically verified by its transaction class.

Raw protobuf DTOs preserve explicit optional field presence, including zero and
empty bytes. The bounded helpers enforce canonical encoding, unknown/duplicate
field rejection, the 1 MiB byte cap and 16,384 nested-message cap. They do not
validate transaction semantics or signatures; use concrete types or
`TransactionDocument::from_proto` for that. Direct prost trait decoding bypasses
the SDK's allocation preflight and should not be used on untrusted bytes.

The published generic decoder inserts absent defaults; raw decode/re-encode then
changes the original bytes. Rust retains presence. Raw DTOs can represent node
ETX/work fields, while user transaction documents accept only supported account
and Qi operations. Reading an ETX is separate from constructing one for submission.

## Access-list normalization

Ordered object/tuple lists preserve duplicate addresses, duplicate slots and
input order. This is required to preserve signing bytes. Map input is an explicit
normalization request: Rust sorts decoded address bytes, sorts and deduplicates
decoded storage-key bytes, and rejects duplicate map keys that decode to the same
address. It avoids JavaScript locale sorting and the source's hex-case duplicate
bug. Invalid mixed-case address checksums reject instead of being silently fixed.
Imports share the JSON budget and exports enforce the transaction message budget.
Once normalized, the result is an ordinary ordered `Vec<AccessTuple>`.

## Evidence

[Four source regressions](../compatibility/scripts/transaction-documents.test.mjs)
exercise typed and raw wire behavior, hash/signature handling, Qi data/locks and
access-list normalization. [Five native/worker tests](../crates/quai-consensus/tests/documents.rs)
cover 24 supported signed/unsigned vectors from existing independent JS/Go
transaction, conversion and wrapping fixtures, and reject two unsupported data
length fixtures. They check JSON/protobuf/byte roundtrips, identity conflicts,
full u64 values, all-output zone metadata, exact locks and resource boundaries.
Existing signing tests and sanitizer fuzzing remain separate regression evidence.
No new funded transaction or production acceptance is claimed.
