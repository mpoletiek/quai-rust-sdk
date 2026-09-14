# Explicit fixed-point wrapping and float conversion

The final deviation audit found five implemented quais.js methods whose behavior
had previously been omitted: `FixedNumber.addUnsafe`, `subUnsafe`, `mulUnsafe`,
`divUnsafe` and `toUnsafeFloat`. They now have explicit Rust counterparts on
`quai_primitives::FixedPoint`:

| Reference | Rust behavior |
| --- | --- |
| `addUnsafe` | `wrapping_add`: scaled-integer addition modulo 2^width |
| `subUnsafe` | `wrapping_sub`: scaled-integer subtraction modulo 2^width |
| `mulUnsafe` | `wrapping_mul`: multiply, truncate fractional scaled units toward zero, wrap |
| `divUnsafe` | `wrapping_div`: divide with scale, truncate toward zero, wrap; zero divisor fails |
| `toUnsafeFloat` | `to_f64_lossy`: explicitly approximate presentation value |

Signed results use two's-complement interpretation; unsigned results use the
nonnegative residue. Both operands must have the same validated format. These
methods are ordinary safe Rust functions; the names make numeric overflow policy
explicit. Existing `checked_*` methods still reject overflow and expose the
chosen precision policy. Lossy floats never change stored units and must not
replace exact amounts in transaction authorization.

The source mishandles a negative arithmetic result congruent to the minimum
signed value. For example, in `fixed8x0`, `-127 + -1` through `addUnsafe` becomes
positive 128, which exceeds the declared signed range. Rust returns -128. Source
construction of -128 is valid; the defect is in arithmetic result normalization.
The comparison retains 26 such differences across 632 operations and checks Rust
against independent `BigInt.asIntN`/`asUintN` expectations.

Five shared native/Chromium tests check 158 value pairs in five formats, all
131,072 combinations of signed/unsigned eight-bit operands, overflow/precision
policy, mismatched formats and zero division, and exact IEEE-754 bits of lossy
conversions. Three source tests isolate wrapping behavior and the reference bug.
The fixed fuzz target checks reversible modular addition, result range/format,
byte/text round trips and agreement with successful checked arithmetic.

The same audit checked remaining rows without a runtime Rust mapping. Published
`waitForBlock` throws `NOT_IMPLEMENTED`; historical `getTransactionResult` has no
JSON-RPC implementation; `Typed.tuple` throws before constructing a named tuple,
and a generic `Typed.from('tuple', ...)` has null name metadata. These are
source placeholders, not missing working behavior. Rust named tuple metadata is
available through `AbiParameter`, while validated values use `AbiValue` and
static type identity rather than a JS brand symbol. The source regression is in
`compatibility/scripts/final-utilities.test.mjs`.
