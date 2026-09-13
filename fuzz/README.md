# Sanitizer fuzz targets

This separate unpublished development workspace uses `cargo-fuzz 0.13.2` and
`libfuzzer-sys 0.4.13`. Its lockfile is independent of the runtime SDK lock.
Production crates remain built with their real signature/validation behavior:
commands explicitly use `--no-cfg-fuzzing` so dependency fuzz-only shortcuts are
not selected.

Targets:

- `transactions`: canonical Quai, ordinary Qi, explicit conversion and wrapping decoders;
  every successful decode must reproduce the exact input bytes and produce its
  transaction identity. Signature verification stays enabled.
- `abi`: bounded type/interface/typed-data parsing, RPC-document hash stability,
  and exact canonical ABI re-encoding for selected static/dynamic/nested types,
  plus bounded type-correct defaults, packed encoding/hash composition and Solidity artifact imports.
- `wallet_import`: legacy keystore parsing/resource bounds, public payment-code
  roundtrips, extended-key/mnemonic and public-key/signature import boundaries.
  It deliberately does not execute attacker-selected expensive KDF parameters.

- `encoding`: bounded hex/Base64/Base58 roundtrips, bytes32 UTF-8 and signed-width
  boundaries, strict UTF-8 and bounded normalization idempotence/scalar roundtrips, plus hostile decoder inputs.
- `head_state`: canonical bounded ancestry restoration, expected-network identity and
  exact byte roundtrips (163,887-byte mutation limit, including the full cursor).
- `fixed`: exact fixed-point import/format roundtrips, floor/ceiling ordering,
  and arithmetic checked against independent i128 calculations.

`seed-corpus.py` derives the checked-in starter corpus from already-public JS/Go
compatibility fixtures plus malformed inputs. It never reads wallets or nodes.
Only reviewed starter files belong in the repository; the smoke runner mutates
a temporary copy and retains crashes/logs under ignored `artifacts/`.

```sh
rustup toolchain install nightly-2026-09-11 --profile minimal --component rust-src
cargo install cargo-fuzz --version 0.13.2 --locked
cargo fetch --manifest-path fuzz/Cargo.toml --locked
python3 fuzz/seed-corpus.py
CARGO_NET_OFFLINE=true cargo +nightly-2026-09-11 fuzz build \
  --fuzz-dir fuzz --no-cfg-fuzzing --codegen-units 16
python3 fuzz/run-smoke.py --seconds 30
```

The runner requires Linux `nm` and the already-built x86_64 target binaries. It
checks for the AddressSanitizer runtime symbol, imposes a 1 GiB RSS ceiling,
five-second per-input timeout, 64 KiB mutation limit (163,887 bytes for head state) and deterministic initial
seed, then records binary/lock hashes and executions/coverage counters. It exits
nonzero on any crash or timeout. Those bounds cover a useful parser slice, not
all upper-bound payload sizes, all schemas, KDF memory behavior or persistence.
Use sustained runs, larger transaction sizes and additional stateful targets for
release qualification; retain and minimize every finding into a regression test.

In the development environment, official Rust nightly 1.100.0 (2026-09-11,
compiler commit 67eda617e) was installed only under `/tmp/quai-fuzz-toolchain`;
each component archive was checked against the SHA-256 in the verified official
manifest. System Rust was unchanged. `smoke-results.json` records completed
bounded runs, not a security-audit certification. No private fixture or live
transaction is involved.

References: [Rust Fuzz Book](https://rust-fuzz.github.io/book/cargo-fuzz/guide.html),
[pinned cargo-fuzz](https://crates.io/crates/cargo-fuzz/0.13.2), and
[libFuzzer runtime](https://crates.io/crates/libfuzzer-sys/0.4.13).

On 2026-09-11 the final 30-second-per-target run completed **875,310 executions**
across all three targets with exit 0 and AddressSanitizer active. The first run
inside the restricted sandbox could not complete LeakSanitizer process inspection;
it was not counted as passing. The successful rerun used permitted local process
inspection without disabling either sanitizer. This is a short smoke run, not
evidence of exhaustive parser coverage or leak freedom.

The September 12 extension ran the first four targets for 300 seconds each,
completing **11,116,595 executions** without a sanitizer failure. The retained
`test-infra/reports/extended-fuzz-2026-09-12.json` identifies each binary and lockfile.
The separate 300-second fixed-point run completed **1,101,348 executions** with
exit 0; it has its own report and binary identity. These are
bounded parser/arithmetic runs, not a substitute for stateful fault testing or
independent security review. `--report PATH` retains additional runs without
replacing the earlier smoke evidence. CI builds and runs all five targets.

The subsequent typed-value/default extension completed another 300-second ABI
run with **2,485,442 executions**, exit 0 and AddressSanitizer active. Its separate
`typed-abi-fuzz-2026-09-12.json` report binds the updated target and public corpus.

Artifact import coverage was subsequently added to the ABI target. Its separate
300-second run completed **2,034,432 executions** with exit 0; see
`artifact-fuzz-2026-09-12.json` for binary and lock identities.

Legacy wallet JSON migration extends `wallet_import` with four published-JavaScript
fixture seeds, bounded parsing, trusted root checks and representable export/import
round trips. Its 120-second run completed **42,643 executions** with AddressSanitizer
active and exit 0. See `test-infra/reports/legacy-wallet-fuzz-2026-09-13.json` for
binary and lock identities. This is bounded smoke coverage, not exhaustive review.

Typed node transaction, receipt and log JSON now exercise parse/export round trips
in the transaction target, alongside all existing canonical protobuf paths. The
120-second run completed **1,093,559 executions**, exit 0 with AddressSanitizer;
`test-infra/reports/response-fuzz-2026-09-13.json` retains binary/lock identities.
Public-node record and log seeds extend the initial corpus to 93 files. This does
not fuzz every provider workflow or establish consensus correctness.
