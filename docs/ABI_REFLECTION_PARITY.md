# ABI reflection and named values

This review compares the published `quais@1.0.0-alpha.57` ABI implementation with
`quai-abi`. Rust exposes validated immutable declarations, explicit metadata and
ordinary slices instead of JavaScript inheritance, proxy properties and deferred
access errors. Encoding remains canonical and bounded; metadata does not establish
contract behavior, event provenance or transaction authorization.

## Parameter and fragment metadata

`AbiParameter::from_json(bytes, allow_indexed)` and `from_human_readable(text,
allow_indexed)` validate a single parameter. `AbiFunction`, `AbiCustomError`,
`AbiEvent` and `AbiConstructor` expose `input_parameters`; functions also expose
`output_parameters`. Each tree retains names, canonical ABI types, tuple components,
array element structure, explicit indexed flags and optional compiler `internalType`.
`internalType` is descriptive metadata and never overrides the encoding type.

| Published API | Rust equivalent and deliberate differences |
| --- | --- |
| `ParamType.name`, `type`, `baseType` | `name`, `abi_type().canonical_name()`, `is_array` and `is_tuple`; scalar base types use the canonical type name |
| `arrayChildren`, `arrayLength` | `array_child`; `array_length` returns `None` for non-arrays, `Some(None)` for dynamic arrays and `Some(Some(n))` for fixed arrays, including zero |
| `components`, `indexed`, `isIndexable` | `components`, `indexed`, `indexed().is_some()`; components belong to the tuple node reached through any array children |
| `ParamType.from`, `isParamType` | Validated constructors and Rust's static `AbiParameter` type |
| Parameter `format` | `AbiFormat::{Signature,Full,Minimal,Json}`; explicit choice, canonical aliases and preserved outer array indexed flags |
| Function/error/event/constructor `format` | Individual compiled declaration `format(AbiFormat)`; constructor signature mode rejects because it has no callable selector |
| Fragment `name`, `inputs`, `outputs`, `selector`, `topicHash`, `anonymous` | Typed declaration getters and named parameter trees; selector/topic helpers are `function_selector` and `signature_hash` |
| Function `constant`, `payable`, `stateMutability`; constructor/fallback `payable` | Explicit `StateMutability`; constant means Pure or View and payable means Payable |
| Fragment construction and runtime `is*` guards | `AbiInterface::from_json/from_human_readable`, typed lookup methods and Rust types; malformed entries reject the entire import |
| `Interface.fragments` and common `Fragment`/`NamedFragment` views | `declarations()` borrows original validated JSON in input order; typed function/event/error iterators provide compiled views |
| Fallback and receive fragments | `fallback_mutability`, `has_receive`, original declaration metadata and interface formatting; source fallback bytes parameter names have no encoding role |
| `StructFragment` | Standalone named tuple `AbiParameter`, e.g. `(uint256 value) Record`; contract JSON ABIs do not gain a nonstandard struct entry |

The published `StructFragment` retains a name and input list but its formatter
throws `@TODO`. Rust's named tuple representation provides the same field metadata
and supports formatting/encoding through `AbiParameter` and `AbiCoder`. Solidity
source bodies, comments and `struct Name(...)` declaration syntax remain outside
the contract ABI parser. Runtime JavaScript constructor guards and prototype
inheritance have no corresponding requirement in a statically typed Rust API.

## Gas annotations

Functions and constructors accept trailing `@123` annotations and JSON `gas`
metadata. `gas_hint()` returns an optional exact U256. JSON null means absent;
nonnegative integral JSON numbers fitting u64, decimal strings and `0x` strings
are accepted, with U256 overflow rejected. Negative values, floats, signs, empty
strings, duplicate annotations and trailing modifiers reject. Human-readable
formatting prints canonical decimal hints; individual JSON formatting uses decimal
strings. Interface JSON export preserves the validated original representation.

Gas hints **never set a transaction's gas limit** or bypass fee estimation.
Selectors and encoded argument/return bytes are independent of this metadata.
The published fragment formatter throws a BigInt serialization error for JSON
when gas hints are present; Rust exports them successfully.

## Named decoded results

Function `decode_call_named` and `decode_returns_named`, custom error `decode_named`
and event `decode_log_named` return `AbiResult<T>`. Event values retain the existing
`AbiEventValue::IndexedHash` variant for unrecoverable indexed compound fields.
These entry points perform all selector, topic and canonical encoding checks
before returning a result. No deferred decoding errors remain to be discovered
through property access.

`values()` borrows ordered fields, `get_value(name)` performs exact name lookup,
`names()` exposes aligned optional unique names, and `to_object()` borrows a
`BTreeMap` only if every field has a unique nonempty name. `slice` and `filter`
retain existing names; `into_values` transfers ownership of positional values.
Nested decoded tuples remain arrays; their names are available in the parameter
tree, where callers can walk or interpret them explicitly.

`from_items` accepts arbitrary already decoded Rust values and optional names.
Empty or duplicate names become unnamed, matching published Result lookup.
Duplicate names in an ABI schema itself continue to reject at import. Unlike
JavaScript object accumulation, Rust maps retain keys such as `__proto__` and
`constructor`; method names such as `length` and `then` cannot intercept lookup.
The published `toObject` silently omits keys inherited from Object.prototype.

Inherited JavaScript Array operations are standard Rust slice/iterator/Vec
operations. Indexing uses checked `get` or explicit Rust indexing; slices use
checked half-open usize ranges without negative-index coercion. The published
Result is frozen despite inherited mutator declarations; Rust requires consuming
or copying values into an owned Vec before mutation. Locale display, prototype
symbols and Array species constructors are language facilities, not SDK wire
features. No JS locale coercion or deferred error text is reproduced.

The generic container limits fields to 1,024 and combined name bytes to 65,536;
it does not measure heap bytes inside arbitrary `T`. ABI decoding separately
bounds JSON nodes, text and encoded bytes. This distinction also applies when
callers construct a result around their own values.

## Synchronous and asynchronous walking

`AbiParameter::walk` and `walk_async` validate the complete input shape before
calling a leaf processor. Tuple arrays and exact named objects are accepted;
objects require every field to be named and reject missing/extra keys. Fixed
arrays require their declared length. Output uses positional arrays. Leaf
callbacks receive the canonical `AbiType` and borrowed input value, and may
transform values into other JSON kinds without implicit ABI coercion.

Both input and combined output obey the 32,768-node, one-MiB string/key and
64-level limits. Output is checked after each leaf callback, so repeated large
callback outputs cannot accumulate past the aggregate budget. The SDK cannot
limit memory already allocated by caller code inside a callback.

Async traversal processes leaves sequentially in declaration/index order and
requires neither Send nor a specific executor. It supports browser-local Rc
state. The published async walker runs leaf promises concurrently; callers needing
concurrent resolution can prefetch and then walk resolved values. Dropping the
Rust future stops subsequent callbacks; callbacks already performed are not undone.
Synchronous Rust walking also accepts exact named tuple objects, a convenience
beyond the published synchronous walker. Walking validates structure, not the
numeric range or address validity of transformed leaves; use `AbiCoder::encode`
to validate values intended for encoding.

## Codec and error conventions

`AbiCoder` remains stateless. Its fixed node/depth/byte limits replace mutable
process-wide inflation settings and a cached singleton. `default_values` returns
exact positional values; `AbiResult::from_items` can attach explicit names.
`AbiInterface::parse_revert` and `ParsedRevert` expose built-in Error/Panic and
custom error metadata, with typed failure for malformed or unknown bytes. The
caller retains its action and transaction context rather than receiving a
JavaScript `CallExceptionError` object with interpolated potentially sensitive
arguments. Revert bytes do not authenticate their origin.

## Evidence

[Published vectors](../compatibility/fixtures/abi-reflection.json) cover parameter
formatting, gas hints and named call/return/error/event bytes.
[Source regression tests](../compatibility/scripts/abi-reflection.test.mjs) verify
four published behaviors, including the upstream formatter/result defects.
[Seven shared Rust tests](../crates/quai-abi/tests/reflection.rs) exercise metadata,
encodings, invalid inputs, name collisions, sync/async walking and aggregate
budgets. They run with the existing three readable-ABI tests in the actual browser
worker. The ABI fuzz target additionally exercises the new parsers, round trips,
walks and named results. These are offline ABI checks, not funded-chain acceptance.
