# quai-primitives

Address, shard, exact amount, byte and text utilities for the Quai Rust SDK. This crate is
experimental and is not published or approved for production wallet use.

- `Address` stores any 20-byte address and enforces the pinned quais.js
  mixed-case Keccak checksum when parsing strings.
- `QuaiAddress` and `QiAddress` additionally validate the zone and ledger bit.
- `Zone`, `Region`, and `Shard` expose only published protocol identifiers.

Known zone encodings are not evidence that a network has activated those zones.
Parsing an address does not establish ownership, account existence, or whether
it is an appropriate transfer destination. In particular, the zero address is
representable, and applications must enforce their own destination policy.

```rust
use quai_primitives::{Address, Ledger, QuaiAddress, Shard, Zone};

let address: QuaiAddress = "0x007f000000000000000000000000000000000000".parse()?;
assert_eq!(address.zone(), Zone::Cyprus1);
assert_eq!(address.ledger(), Ledger::Quai);
assert_eq!(Shard::from(address.zone()).nickname(), "cyprus1");
let raw: Address = address.into();
assert_eq!(raw.zone()?, Zone::Cyprus1);
# Ok::<(), Box<dyn std::error::Error>>(())
```

String parsing accepts an optional lowercase `0x` prefix and exactly 40 ASCII
hex digits. All-lowercase and all-uppercase digits are accepted. Mixed case must
match the checksum; whitespace and uppercase `0X` prefixes are rejected.
`Display` produces the checksummed, `0x`-prefixed form. Address parsing and
formatting into an existing formatter use fixed stack buffers.

The test fixture includes 259 public checksum vectors from pinned quais.js,
with attribution and the upstream license in `tests/fixtures/`.

## Amounts and deployment prediction

`parse_units`/`parse_quai`/`parse_qi` produce nonnegative U256 chain amounts without
floating point or rounding. `parse_signed_units` and `SignedUnits::format` retain
the reference signed 512-bit display/conversion range. Units support 0..80
decimals; strings are capped at 512 bytes before parsing. Exact trailing zeros
are accepted; excess nonzero fractional digits, whitespace and exponent syntax
are rejected. Quai has 18 decimals; Qi has 3.

`contract_address(sender, nonce, init_code)` hashes exact init code with the
node's eight-byte big-endian nonce. It intentionally corrects JS's leading-zero
code stripping, which produces a different predicted address. `create2_address`
uses the full salt and exact init-code hash. Both functions return arbitrary
addresses; deployment still must satisfy active-zone and Quai-ledger rules.
The shared 30-case corpus and Go regression test verify both formulas, including
18 retained leading-zero divergences. Neither helper proves deployment success.

## Byte encodings and signed widths

The crate exposes bounded hex, byte concatenation/slicing/padding, Base64,
Base58, null-terminated UTF-8 bytes32 and two's-complement helpers. General byte
outputs are capped at 1 MiB; Base58 conversion is capped at 4096 bytes to bound
its quadratic radix work. `decode_base58_bytes` preserves leading zeros;
`decode_base58` returns a numeric U256 and rejects larger values. Hex requires
an even-width lowercase `0x` prefix. Base64 uses strict padded RFC 4648 decoding,
so Node Buffer's whitespace, missing-padding and trailing-bit coercions are an
explicit deviation. Rust byte slices replace JS BytesLike aliases/coercions.

`to_twos`/`from_twos` support explicit widths 1..=256 with signed range checks;
`mask` supports 0..=256. Values never silently truncate except through explicit
`mask`. `encode_bytes32` allows at most 31 UTF-8 bytes; decoding requires a final
zero byte and strict UTF-8, strips trailing padding and preserves internal nulls.
Eighty generated pinned-JS vectors cover these boundaries and zero preservation.

## Checked fixed-point arithmetic

`FixedPoint` stores an exact signed or unsigned integer field with an explicit
`FixedFormat` (8..256 bits, byte aligned, 0..80 decimals; default `fixed128x18`).
Parsing, byte import, exact scaled-unit import, addition, subtraction,
multiplication, division, rescaling, numeric comparison and chain-unit conversion
are supported. Arithmetic requires matching formats; comparison uses exact
numeric value across formats. Wide intermediates preserve valid results that
would otherwise overflow during multiplication or rescaling.

Multiplication, division and rescaling take `Rounding`: exact, toward zero,
floor, ceiling, nearest ties to even, or nearest ties toward positive infinity.
Overflow always fails, including rounding at a field boundary. Unsafe wrapping
arithmetic and implicit floating-point conversion are intentionally absent;
use exact decimal strings or integer units. `from_bytes` accepts at most 32
bytes, ignores redundant leading zeros and interprets signed patterns at the
full format width. `to_bytes` emits the complete field width. Guarded JavaScript
constructors and loose numeric/format coercions become validated Rust types.

The pinned reference's `floor` and `ceiling` discard their sign adjustment,
and its negative `round` truncates the adjusted negative integer toward zero.
Rust implements mathematical floor/ceiling and the selected rounding policy.
`compatibility/fixtures/fixed.json` retains these reference observations and
independent expectations, alongside matching parse, arithmetic and byte vectors.
`tests/fixed.rs` verifies the differences explicitly. Decimal display follows
the existing exact units formatter; no floating-point arithmetic is involved.

```rust
use quai_primitives::{FixedPoint, Rounding, Unit};
let format = "fixed128x18".parse()?;
let price = FixedPoint::parse("1.25", format)?;
let count = FixedPoint::parse("3", format)?;
let total = price.checked_mul(count, Rounding::Exact)?;
assert_eq!(total.to_chain_units(Unit::QUAI)?.to_string(), "3750000000000000000");
# Ok::<(), Box<dyn std::error::Error>>(())
```


### Text and strict checksum imports

`to_utf8_bytes(text, form)` and `to_utf8_code_points(text, form)` preserve exact
text with `None`, or apply explicit `Utf8Normalization::{Nfc,Nfd,Nfkc,Nfkd}`.
Input and normalized output each have a 1 MiB byte bound; expansion stops at the
limit. `UTF8_UNICODE_VERSION` exposes the library's table version (17.0.0 in the
locked build), independent of a browser's JavaScript engine. Code-point results
contain Unicode scalars and can occupy up to 4 MiB plus bounded temporary bytes.
`to_utf8_string` borrows only valid bounded UTF-8. It rejects invalid continuations,
overlong encodings, surrogate scalars and truncation; there is no silent skip or
replacement callback. Rust strings cannot represent unpaired UTF-16 surrogates.

`uuid_v4(&[u8; 16])` sets version/variant bits in a copy and returns canonical UUID
text. It never mutates caller bytes, truncates other input lengths or generates
entropy. The crypto crate's `fill_random` supplies explicit OS/Web Crypto entropy.

`Address::from_checksummed_str` requires exact prefixed checksum spelling, matching
`validateAddress`. Ordinary `parse::<Address>()` accepts uniform-case/unprefixed
input with mixed-case checksum checks; its `is_ok()` implements `isAddress`.
`Address::ledger` replaces raw `isQiAddress`/`isQuaiAddress` string heuristics;
`Address::zone` supplies the other part of `getAddressDetails`. Typed ledger
addresses add known-zone validation. `to_checksum` formats the already validated
20-byte value; arbitrary malformed strings are not silently repaired.

Dynamic `AddressLike`/Promise resolution stays in caller Rust code: await the
value, obtain the wallet/contract address, then pass a typed address. There is no
implicit network/name resolution. Transfer/conversion constructors choose the
actual signed wire type; address-only `getTxType` inference is deliberately
omitted because its Qi-to-Quai result is not the signed outer Qi conversion type.

Existing exact CREATE/CREATE2 predictors correspond to `getCreateAddress` and
`getCreate2Address`, with u64 nonces and fixed-size salt/hash inputs. Typed
`Address::ZERO`, `Hash32::ZERO`, SDK `U256::MAX`, and `parse_quai("1")` cover the
zero address/hash, maximum unsigned integer and 10^18 base-unit constants.

The `numeric` module supplies bounded exact integer parsing, byte/hex/quantity
conversion, safe-number bridges and hexadecimal shape validation. `ShardMetadata`
retains all published labels; typed shards determine hierarchy. See the
[declaration review](../../docs/DECLARATION_PARITY.md) for supported grammar and bounds.
