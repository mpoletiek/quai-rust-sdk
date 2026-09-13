# EIP-712 encoder and traversal parity

The Rust typed-data API covers the published `quais@1.0.0-alpha.57`
`TypedDataEncoder` operations with an immutable validated schema, exact values
and explicit resource policy. It does not resolve names or perform network calls.

| Published API | Rust API |
| --- | --- |
| Constructor, `from(types)` | `TypedDataEncoder::new(&TypedDataTypes)` |
| `types`, `primaryType`, `getPrimaryType(types)` | Borrowed `types()` and `primary_type()` on a validated encoder |
| `encodeType(name)` | `encode_type(name)` with canonical dependency ordering |
| `getEncoder(type)` | `encoder(type)` returns a cloneable `TypedValueEncoder` borrowing its immutable schema; call `encode(value)` |
| `encodeData(type, value)` | `encode_data(type, value)` accepts primitive, array and declared struct roots |
| Instance `encode(value)` | `encode_data(encoder.primary_type(), value)` |
| Instance `hash(value)`, `hashStruct(type, value)` | `hash(value)`, `hash_struct(type, value)` |
| Static `encode(domain, types, value)` | `TypedDataEncoder::new(types)?.signing_preimage(domain, value)` |
| Static `hash(domain, types, value)` | `hash_typed_data(domain, types, value)` or compiled `signing_hash` |
| Static `hashStruct(type, types, value)` | Construct the schema and call `hash_struct` |
| Static `hashDomain(domain)` | `hash_domain(domain)` |
| `getPayload(domain, types, value)` | `TypedData::from_json` validates the explicit RPC document, then `to_rpc_json()` emits normalized exact values |
| `visit(value, callback)`, `_visit(type, value, callback)` | `visit(value, callback)`, `visit_type(type, value, callback)` |

## Type encodings

An EIP-712 primitive or array encoder returns one 32-byte word. Dynamic bytes and
strings hash their contents; arrays hash their element words, including hashes
of any struct elements. A declared struct root returns its type hash followed by
one word per field. These encodings differ from ordinary Solidity ABI dynamic
encoding. `hash_struct` hashes the entire returned encoding, matching the
published helper even for primitive/array roots.

Resolution validates the type expression before use. Unknown structs reject even
inside empty or zero-length arrays. Explicit integer widths, canonical array
length spelling, at most 16 array dimensions and existing schema limits remain
required. Rust rejects bare `uint`/`int` typed-data aliases, JavaScript coercions,
extra object fields and out-of-range integer/address/byte values. Integers retain
exact decimal strings or exact supported JSON integer values, without JavaScript
number precision loss. No mutable singleton or callback-based encoder cache is
needed; a resolved encoder borrows immutable compiled metadata.

```rust
use quai_abi::{TypedDataEncoder, TypedDataTypes};
use serde_json::json;
let types: TypedDataTypes = serde_json::from_value(json!({
    "Order": [{"name":"amount", "type":"uint256"}]
}))?;
let encoder = TypedDataEncoder::new(&types)?;
let integers = encoder.encoder("uint256[2]")?;
let array_hash_word = integers.encode(&json!(["1", "2"]))?;
assert_eq!(array_hash_word.len(), 32);
let fields = encoder.types();
assert_eq!(fields["Order"][0].name, "amount");
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Value traversal

Visitors process primitive leaves in schema declaration order and array index
order, preserving objects and arrays in the result. The callback receives the
canonical primitive type name and borrowed leaf value. It may replace leaves
with arbitrary JSON values; numeric/address/byte validity must be checked by
encoding the result before signing. Traversal performs no implicit coercion,
address lookup or hashing.

The complete input shape is validated before the first callback. Missing/extra
fields and fixed-array length mismatches reject without callback work. Published
JavaScript instead silently skips extra fields and may invoke earlier callbacks
before discovering a later malformed array. Rust maps also preserve fields named
`__proto__` or `constructor`. Callback failures stop traversal immediately;
previous callback side effects cannot be rolled back.

Both original input and aggregate transformed output obey 32,768 visited nodes,
one MiB of combined string/key bytes and 64 nested levels. Object keys count
against the output budget. Each returned callback value is checked before it is
retained, preventing aggregate output growth across many leaves. Memory allocated
by the caller inside its callback is outside the SDK's allocation control.

The published visitor is synchronous. Rust's corresponding methods are also
synchronous; asynchronous ABI parameter traversal is separately available through
`AbiParameter::walk_async`. Neither facility authorizes the transformed values
for signing; normal domain policy and explicit signer validation still apply.

## Evidence and limits

[107 published encoding/hash vectors](../compatibility/fixtures/typed-data-utils.json)
cover all integer/fixed-byte widths, primitive roots, nested arrays, empty arrays
and struct arrays. [Three source tests](../compatibility/scripts/typed-data-utils.test.mjs)
verify encoder equivalence, leaf order and permissive/partial callback behavior.
[Four shared Rust tests](../crates/quai-abi/tests/typed_utils.rs) cover exact bytes,
borrowed schema metadata, traversal, strict invalid-input policy and aggregate
resource limits on native and Chromium worker targets. Existing typed-data
schema/domain/signature vectors remain regression evidence. ABI sanitizer fuzzing
now also exercises arbitrary-type resolution, encoding and visitor properties.
These checks establish offline behavior, not independent security audit or
funded-chain acceptance.
