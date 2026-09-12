# quai-primitives

Initial address and shard foundations for the Quai Rust SDK. This crate is
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
