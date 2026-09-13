# quai-abi

Experimental bounded Solidity ABI encoding and EIP-712 hashing for Quai Rust applications.
The implementation matches the pinned quais.js encoder for supported valid
inputs. It is not independently audited or production-qualified. Signing and
application authorization belong to the signer layer.

## API

`TypedDataEncoder::new(&TypedDataTypes)` validates a schema, infers its unique
primary type and caches canonical type strings and hashes. Its methods expose
`encode_type`, `encode_data`, `hash_struct`, primary `hash`, `signing_preimage`
and `signing_hash`. Hash methods return the common `quai_primitives::Hash32`.
Schema field order is significant; named dependencies are appended in lexical
order. Array hashes stream encoded words rather than buffering their
concatenation. An array element may itself be an array, struct or primitive.

`hash_domain` handles the five standard domain fields in their prescribed order:
name, version, chainId, verifyingContract, salt. Known null fields are omitted.
`hash_typed_data` combines schema compilation and domain-separated hashing.
No address name resolution or network access occurs.

For untrusted JSON, use `TypedData::from_json(&[u8])`. It requires the four RPC
document fields `types`, `primaryType`, `domain` and `message`, rejects duplicate
keys at every level during parsing, and checks `primaryType` against the schema.
The optional `types.EIP712Domain` declaration must exactly match the non-null
domain fields in canonical order. A validated document is immutable and caches
its signing hash. Lower-level `serde_json::Value` APIs cannot recover duplicate
keys discarded by somebody else's JSON parser; their inputs are still checked
for shape, size and all declared value constraints before hashing.

```rust
use quai_abi::TypedData;

let document = TypedData::from_json(br#"{
  "types": {"Message": [{"name":"amount", "type":"uint256"}]},
  "primaryType": "Message",
  "domain": {"name":"Example", "chainId":9000},
  "message": {"amount":"1000000000000000000"}
}"#)?;
let digest = document.signing_hash();
assert_eq!(digest.bytes().len(), 32);
# Ok::<(), quai_abi::TypedDataError>(())
```

The digest is `keccak256(0x1901 || domainSeparator || hashStruct(message))`.
Signing uses ECDSA over that exact prehash; personal-message hashing must not be
applied to it. Domains provide context but do not implement application replay
prevention, nonce management, deadlines, chain selection or contract-policy
checks. A signer must apply those policies before authorizing a signature.

## Compatibility and strict policies

The source reference is
[quais.js TypedDataEncoder at 94e32c7](https://github.com/dominant-strategies/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/src/hash/typed-data.ts),
with fixtures generated from locked `quais@1.0.0-alpha.57`. Hash construction
follows [EIP-712](https://eips.ethereum.org/EIPS/eip-712); the pinned implementation
is the compatibility reference where behavior differs.

Supported primitives are address, bool, bytes, string, bytes1 through bytes32,
and int8/uint8 through int256/uint256 in multiples of eight. Signed integers use
256-bit sign extension, fixed bytes right-pad with zeros, and addresses left-pad
with zeros. Dynamic bytes and strings hash their exact contents. Addresses are
validated through the common address/checksum parser, without imposing a
particular ledger or deployed-zone requirement. Strings are exact UTF-8 bytes;
no Unicode normalization is performed.

The pinned JS implementation rejects recursive schemas and `int`/`uint`
aliases. This crate does too. Fixed zero-length arrays match the pinned encoder.
The following intentional stricter policies avoid JavaScript coercion or
ambiguous unsigned content:

- Struct and field identifiers must be ASCII Solidity-style identifiers of at
  most 128 bytes; primitive names cannot be redefined as structs.
- Every declared struct must be reachable from one primary type, and all cycles
  are rejected, including disconnected cycles.
- Struct objects must contain exactly their declared fields. Missing bool fields
  do not silently become false, and unknown fields cannot be displayed while
  being omitted from the signed hash.
- Bools require JSON booleans; strings require JSON strings. JavaScript truthy
  values and numeric values used as strings are rejected.
- Integers accept exact decimal or `0x`/`0X` hexadecimal strings with an optional
  leading minus, at most 80 bytes, and JSON integer tokens in the JS safe range
  ±(2^53−1). Use strings for larger values. Whitespace, plus signs, binary/octal
  prefixes, fractional/exponent tokens and floating-point tokens such as `1.0`
  are rejected. A numeric string's declared signed/unsigned range is enforced.
- Bytes require even-length `0x` hexadecimal strings; fixed widths must match
  exactly. Array lengths use canonical decimal spelling (`[02]` is rejected).
- Unknown domain keys are rejected even if their value is null. Explicit JSON
  domain declarations must match the canonical field list rather than supplying
  a different field order or silently changing the separator.

The captured fixture file records whether each stricter policy case is accepted
by JS. This is deliberate input-policy tightening, not a claim that these
behaviors are identical to the JavaScript API.

## Resource boundaries

These fixed local limits are not protocol or consensus limits:

| Resource | Limit |
| --- | ---: |
| Raw JSON document bytes | 1 MiB |
| Combined value key/string bytes per encoding input | 1 MiB |
| Visited JSON values per encoding input | 32,768 |
| JSON/schema/encoding recursion depth | 64 |
| Struct declarations | 128 |
| Total declared fields | 1,024 |
| Combined schema name/field/type-expression bytes | 65,536 |
| Type-expression bytes | 256 |
| Array dimensions per expression | 16 |
| Declared fixed-array elements | 32,768 |

Raw JSON size is checked before parsing; the parser checks depth and node count
while constructing its tree. Schema limits are checked before cached encodings
are allocated, and graph traversal memoizes dependency closures. Cached type
strings are bounded by roughly nine MiB in the maximum schema. Borrowed value
inputs undergo a bounded iterative traversal before recursive encoding. Domain
and message are separate encoding inputs. Values already allocated by callers
remain the callers' allocation responsibility. Errors contain categories only,
not copies of potentially sensitive message values.

The crate uses tiny-keccak and the existing ruint unsigned arithmetic backend;
it does not implement curve arithmetic, retain signing keys, or depend on
quai-crypto at runtime. The cryptographic dependency appears only in tests to
check signature interoperability. No browser/runtime-specific qualification
has been performed.

## Validation and reproduction

From the repository root, after installing the compatibility lockfile:

```sh
node crates/quai-abi/tests/generate-fixtures.mjs
cargo test -p quai-abi
cargo clippy -p quai-abi --all-targets -- -D warnings
```

The fixture includes 43 valid vectors, 36 shared rejection cases and eight
explicit stricter-policy cases. Valid vectors cover the canonical EIP-712 mail
example, every integer width and fixed-byte width, integer extrema, empty and
nested arrays, struct arrays, dependency diamonds, UTF-8, null/empty/full domains
and exact signing preimages. Tests also reject the adjacent out-of-range value
at every signed/unsigned width, malformed/duplicate JSON, excessive resources,
unknown or duplicate schema fields, wrong primary/domain declarations and
changes to signing context. All 43 deterministic signatures match JS and recover
the expected address through quai-crypto.

Fixture signing uses public toy key 1. It must never be funded. Tests establish
agreement with these references, not node transaction acceptance or production
wallet safety. Name-resolution visitors remain separate work. Packed ABI and
readable interface declarations are described below.

## Solidity ABI codec and contract interfaces

`AbiType` parses primitive names, `int`/`uint` aliases, fixed/dynamic arrays and
nested tuple expressions such as `(uint256,string[])[][2]`. Types are opaque
validated objects; tuple/array constructors apply the same shape limits. ABI
aliases normalize to int256/uint256, independently of EIP-712's alias policy.
The supported primitives match the pinned JS AbiCoder: uint/int widths, bool,
address, bytes widths, bytes and string. Fixed-point and `function` types are not
implemented by the pinned JS coder and are rejected here. Source-level storage
specifiers and named parameters are not part of this type-parser grammar.

`AbiCoder::encode(types, values)` validates the full value tree and measures the
complete output before allocating one zeroed output buffer. It writes canonical
heads, offsets and tails into that buffer. `decode(types, bytes)` checks total
size, primitive padding, sign extension, exact consecutive tail offsets, UTF-8,
array counts and remaining node capacity before allocating decoded containers.
It rejects gaps, noncanonical bools, aliased/reordered tails, out-of-bounds
lengths, dirty padding, unconsumed data and partial words. Empty dynamic bodies
may legitimately share an offset; the exact canonical cursor calculation
preserves that case. No selector is assumed for bare parameter encoding.

Values use JSON arrays for tuples and arrays, booleans for bool, strings for
UTF-8 and hexadecimal bytes, and the exact integer input policy described above.
Decoding returns integers as decimal strings and addresses as checksum strings.
Objects keyed by tuple parameter names are not accepted by this positional API.
Encoding/decoding policy is stricter than JS's permissive decoder; captured
`decodePolicy` fixtures record those differences explicitly.

Zero-sized arrays and tuples remain bounded by value-node count even when they
consume no wire bytes. Valid nonempty arrays of zero-sized elements retain their
length and shape. The pinned JS encoder emits these cases, but its decoder
rejects some of its own outputs with BUFFER_OVERRUN. These fixtures have a
`referenceDecodeError` marker; their bytes agree with JS encoding and their Rust
roundtrip follows the formal ABI definition. This is not JS decoder agreement.

A maintained-backend assessment included alloy-dyn-abi 1.7.3, at source commit
`87080853083c909b34f8c8cff3622570c114985d`. Its
[zero-sized sequence decoder](https://github.com/alloy-rs/core/blob/87080853083c909b34f8c8cff3622570c114985d/crates/dyn-abi/src/dynamic/token.rs)
collapses some such arrays to empty sequences to avoid allocation attacks.
This crate uses a local bounded structural codec to preserve exact semantics,
with existing ruint and tiny-keccak primitives; it does not add Alloy as a
runtime dependency. This local codec needs independent review and sustained
fuzzing before production qualification. The specification reference is the
[Solidity contract ABI specification](https://docs.soliditylang.org/en/latest/abi-spec.html).

`AbiInterface::from_json` parses a bounded standard JSON ABI array and rejects
duplicate JSON keys/declarations, unknown fields, invalid mutability and
ambiguous lookups. It supports functions, custom errors, events, constructors,
fallback and receive declarations, plus legacy consistent constant/payable
metadata. JSON tuple `components` resolve recursively. Functions expose exact
selector-prefixed call encoding/decoding and bare return encoding/decoding.
Errors expose selector-prefixed encoding/decoding. Matching a four-byte selector
never authenticates the source of data; known selector collisions are tested
and selector lookup reports ambiguity.

Events encode/decode indexed and non-indexed fields, including anonymous logs.
Topic count and non-anonymous topic0 must match exactly. Indexed compound values
remain `AbiEventValue::IndexedHash`, since their original values cannot be
recovered from a topic. `indexed_event_topic` implements the specification's
special in-place encoding: top-level string/bytes hash their contents, nested
members pad to words, and arrays/tuples concatenate members without lengths.
This differs from ordinary ABI encoding and packed encoding. Compound indexed
hashes can be ambiguous, so they should not be used as unique value encodings.
JS fixtures cover primitive, string/bytes and anonymous event topics; compound
indexed-topic tests use explicit specification-derived preimages because JS's
interface encoder does not implement every compound indexed case.

The ABI limits are 1 MiB input/output parameter bytes and combined value text,
32,768 decoded/encoded value nodes, nesting depth 64, 4096 bytes per type or
canonical signature, 1024 type syntax nodes/parameters/interface entries, and
bounded static-shape expansion. Selector-prefixed payloads add four bytes. Raw
JSON interface input also uses the bounded duplicate-rejecting parser. These
are local resource limits, not contract or node consensus limits.

Additional validation includes 45 ABI encoding/roundtrip vectors, ten shared
encoder rejection cases, five explicit stricter decoder cases, all integer and
fixed-byte widths, nested static/dynamic tuples/arrays, every truncation of a
representative dynamic payload, pointer alias/gap attacks, malformed lengths,
UTF-8/padding, zero-size expansion attacks, selector collisions and a deterministic
arbitrary-byte rejection corpus. Call/return/error/event fixtures are generated
through pinned JS Interface. Regenerate them with:

```sh
node crates/quai-abi/tests/generate-abi-fixtures.mjs
```

## Values with explicit ABI types

`AbiValue::new(AbiType, Value)` validates and retains an immutable type/value
pair. All integer widths, fixed/dynamic bytes, strings, addresses, booleans,
arrays and tuples use the existing bounded codec contract. `encode` encodes one
parameter sequence; `expect_type` rejects a mismatched expected type. Debug
shows only the canonical type and encoded size. Values are borrowed immutably;
owned parts can be extracted, but modified values require construction again.

`AbiValue::default_for` supplies type-correct zero/empty values and checks node,
text and encoded-size limits before allocating the default tree. Dynamic arrays
are empty; fixed arrays and tuples preserve their complete declared shape.
`AbiType::integer_bounds` returns exact decimal bounds for every integer width.
Type predicates, `array_info` and `tuple_components` expose structural metadata.

These APIs replace JS `Typed` width-specific constructors with an explicit
validated type. They validate values immediately; the pinned constructors leave
range/length checks to the encoder. The reference's default/min/max helpers
return the constant zero, which Rust intentionally corrects. Guard tokens, brand
symbols, duck typing, inferred array/tuple types, loose truthiness, and tuple-name
metadata are not mirrored. Tuples remain positional. Transaction overrides use
provider `CallRequest`/transaction intents, not an ABI value wrapper. The 426
reference encoder cases and 64 bound cases retain constructor observations and
stub results; tests verify exact encoding and deliberate validation differences.

```rust
use quai_abi::{AbiType, AbiValue};
use serde_json::json;
let value = AbiValue::new("uint16".parse()?, json!("65535"))?;
assert_eq!(value.encode()?.len(), 32);
assert_eq!(value.abi_type().integer_bounds(), Some(("0".into(), "65535".into())));
assert!(AbiValue::new("uint16".parse()?, json!("65536")).is_err());
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Solidity artifacts

`SolidityArtifact::from_json` loads one contract entry containing `abi` and
creation `bytecode` or `evm.bytecode`. Bytecode accepts the standard hex string
or `{object: ...}` form with an optional lowercase `0x` prefix. Exact bytes,
including leading zeros and compiler metadata, are retained. `from_compilation_json`
selects an exact source and contract name from a full compiler output; it never
chooses a contract heuristically or reads a source path from disk.

The document uses the existing 1 MiB, depth/node-bounded, duplicate-rejecting
parser, including ignored metadata. Missing/empty creation code, unresolved
link placeholders and conflicting top-level/nested bytecode are rejected. Matching
bytecode at both locations is accepted. ABI and code are not authenticated against
one another, and runtime/immutable/library fixups are not inferred or applied.
`init_data` validates and appends constructor arguments before Quai grinding.
Native value/payability, sender, nonce, fees and network checks remain in the
SDK deployment preparation phase. `interface`, `init_code` and `into_parts`
connect to that phase directly. Debug reports only creation byte count.

Twelve reference factory cases compare exact pre-grinding constructor payloads.
Tests also cover explicit compiler-output selection, duplicate nested/ignored
fields, missing contracts, resource limits, malformed hex and argument arity.

## Parameter defaults

`AbiCoder::default_values(&types)` returns the zero/empty values for a complete
parameter list. Fixed arrays and tuples retain their shape; dynamic arrays are
empty, bytes are hex strings, addresses are the zero address and integers are
exact decimal strings (`"0"`). The pinned JS `AbiCoder.getDefaultValue` returns
numeric zero; Rust keeps the same representation used by its decoder. Tuple
results are positional vectors rather than JS named-property `Result` objects.

Field count, visited value nodes, total string bytes and total ABI encoding size
are checked across the whole sequence before allocating nested values. This also
bounds repeated empty tuples, whose ABI encoding consumes zero bytes. The shared
single-value constructor remains `AbiValue::default_for`. Tests compare 107 pinned
JS default sequences and their complete encoded bytes, plus independent aggregate
budget boundaries.


### Packed encoding and hashes

`solidity_packed`, `solidity_packed_keccak256` and `solidity_packed_sha256` accept
validated `AbiType` slices and exact JSON values. Scalars use their declared byte
widths; integer/address/bool/fixed-bytes array elements use 32 bytes with the
appropriate left or right padding. Dynamic data carries no length prefix.
341 pinned JS cases match bytes and both hashes; 172 invalid or stricter cases
are rejected, including 66 that JS accepts through coercion or widened array
integer ranges. Rust always checks the declared element width and boolean kind.
Shared ABI field, node, text and canonical-encoded-size limits apply before the
packed output allocation; tuples, including tuple types in empty arrays, fail.

```rust
use quai_abi::{AbiType, solidity_packed};
let types: Vec<AbiType> = ["int16", "bytes1", "uint16", "string"]
    .iter().map(|s| s.parse().unwrap()).collect();
let values = serde_json::json!(["-1", "0x42", "3", "Hello, world!"]);
let bytes = solidity_packed(&types, values.as_array().unwrap()).unwrap();
assert_eq!(quai_primitives::hexlify(&bytes).unwrap(),
           "0xffff42000348656c6c6f2c20776f726c6421");
```

These helpers preserve pinned quais.js extensions: nested arrays flatten, and
string/bytes array elements remain raw and unpadded. Those inputs are not a
Solidity compiler compatibility claim. See the
[Solidity packed-mode specification](https://docs.soliditylang.org/en/latest/abi-spec.html#non-standard-packed-mode)
for the compiler's scope. Packed dynamic fields are ambiguous: `["a", "bc"]`
and `["ab", "c"]` produce the same bytes. Use canonical `AbiCoder` or EIP-712
for structured signing where field boundaries must be preserved. No packed
decoder is provided.

### Event filters and interface decoding

`AbiEvent::encode_filter_topics` accepts `AbiFilterValue` entries in full event
argument order. Use `Any` for non-indexed fields, `Exact(&value)` for one value
and `AnyOf(&values)` for 1–128 alternatives. Omitted suffixes are wildcards;
trailing wildcard topics are trimmed. Nonanonymous signature topics are inserted
automatically. All alternatives share the ABI node, text and encoded-size limits.
The SDK's `Contract::events_by_values` applies these filters to the bound emitter,
decodes canonical results and retains removal/inclusion metadata.

The 246 pinned filter cases include 174 canonical topic matches and 72 rejected
inputs. For 64 negative-integer cases, pinned `encodeFilterTopics` throws while
its `encodeEventLog` supplies the correct sign-extended word used by Rust. Rust
also rejects JS integer-width overflow, boolean coercion and empty/oversized OR
lists. Explicit `Exact` makes indexed arrays/tuples possible without confusing
array values with OR lists. These compound topics use the special event encoding,
not packed encoding; a hash neither recovers nor uniquely identifies its input.

`AbiInterface::parse_call`, `parse_log` and `parse_revert` select declarations and
decode the entire canonical payload. Unknown declarations return `NotFound`,
selector collisions return `Ambiguous`, and malformed encodings fail. Anonymous
logs require explicit `AbiEvent::decode_log`; automatic matching never guesses.
`ParsedCall` and `ParsedLog` borrow declarations and own positional values.
Indexed compound fields use `AbiEventValue::IndexedHash(Hash32)`; Rust pattern
matching replaces JS `Indexed` constructors, nullable hashes and marker tests.

`ParsedRevert` distinguishes builtin `Error(String)` and `Panic(U256)` from
custom errors, exposes signature/name/selector, and retains unknown panic codes.
Builtin declarations need not appear in the ABI. Custom selectors colliding with
a builtin are ambiguous. Revert data is unauthenticated, potentially forwarded
or forged; the caller retains the RPC error, transaction value and other context.
There is no synthetic JS `CallExceptionError` object. Return bytes have no
selector: use an explicitly selected function's `decode_returns`. The pinned
`parseCallResult` itself is unimplemented; Rust does not guess a return schema.

Interface lookup uses names/canonical signatures or typed selectors/topic hashes,
never argument coercion to select an overload. `is_ok()` on an unambiguous lookup
provides a presence check; each declaration exposes its name. Ordered `functions`,
`errors` and `events` iterators replace callbacks; constructor, fallback mutability
and receive are explicit getters. `AbiCoder` is a stateless standalone codec,
including the parameter encode/decode operations; no replaceable instance coder
is stored. Explicit JSON/readable parsing or ordinary Rust cloning replaces
`Interface.from`.


## Readable declarations and formatting

`AbiInterface::from_human_readable(&[&str])` accepts explicit function, event,
error, constructor, fallback and receive fragments, plus bare function signatures.
Named nested tuples and arrays, indexed event parameters, function returns,
mutability and ordinary source storage/visibility modifiers are supported.
Every declaration is validated; malformed fragments are never silently skipped.
Duplicate overload signatures, duplicate field names and conflicting modifiers
fail. This parses ABI declarations, not Solidity source: comments, function
bodies, struct definitions and gas annotations are unsupported.

```rust
use quai_abi::AbiInterface;
let interface = AbiInterface::from_human_readable(&[
    "function balanceOf(address owner) view returns (uint balance)",
    "event Transfer(address indexed from, address indexed to, uint amount)",
])?;
assert_eq!(interface.function("balanceOf")?.selector(), [0x70, 0xa0, 0x82, 0x31]);
let full = interface.format_human_readable(false)?;
let minimal = interface.format_human_readable(true)?;
let json = interface.format_json()?;
let restored = AbiInterface::from_json(json.as_bytes())?;
assert_eq!(restored.format_human_readable(false)?, full);
# Ok::<(), quai_abi::AbiError>(())
```

Readable formatting preserves declaration order, canonical types, indexed flags,
mutability and returns. Full mode retains parameter names including tuple fields
and return names; minimal mode omits names and comma spacing. JSON export retains
validated source metadata including `internalType`, names, optional legacy flags
and original type spelling. It does not reproduce JavaScript object-key order,
constructor `"undefined"` mutability or the pinned formatter's lost indexed flags
on event arrays. The independent fixtures record those two upstream JSON
normalizations explicitly. Source-only memory/calldata/storage, payable address
annotations and public/external visibility do not affect ABI encoding. Fallback
byte parameter names are omitted, matching the pinned fragment representation.

Import is bounded to 1,024 fragments, 4,096 ASCII bytes per fragment and 65,536
combined bytes, with at most 1,024 parameter/array syntax nodes per fragment and
64 nesting levels. The existing JSON/type/expanded-value limits also apply.
Formatted output is capped at one MiB. A large named JSON declaration may format
to more than the readable import budget; JSON remains its lossless metadata
round-trip format. `format_json` does not establish contract provenance, deployed
code or execution behavior. Contract calls use the same compiled ABI after
readable import or JSON restore.

Tests cover 180 pinned full/minimal/selector cases, 31 explicit rejection cases,
additional depth/width/count limits, source metadata and overload order. The same
suite runs through the SDK in actual Chromium workers; a contract facade test
checks exact calldata, maximum U256 return decoding and explicit block selection.
