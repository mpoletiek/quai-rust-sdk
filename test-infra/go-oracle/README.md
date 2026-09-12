# Independent Go transaction oracle

This development harness verifies the repository's public JS/Rust transaction
fixtures through the pinned Go node library. It does not initialize a node,
create chain state, mine, fund wallets, submit transactions or contact RPC
endpoints. **Passing means agreement on wire data and signatures, not node
acceptance.** The fixture private-key fields are ignored.

The source pin is
`f3f345c877300c044e3e0081a48bf3cf786fb9cc` of
[`dominant-strategies/go-quai`](https://github.com/dominant-strategies/go-quai/tree/f3f345c877300c044e3e0081a48bf3cf786fb9cc).
The wrapper requires that exact commit and a completely clean checkout,
including absence of untracked files. The Go requirement's timestamp is the
commit's UTC committer timestamp. A local replacement is made only in a
temporary copy of this module; no replacement path is committed.

## What it checks

For each vector in `compatibility/fixtures/transactions.json`, the harness uses
Go's `core/types` and generated protobuf implementation to:

1. Unmarshal signed protobuf and call `Transaction.ProtoDecode` in the explicit
   Cyprus-1 context used by the current fixtures.
2. Compare `ProtoEncodeTxSigningData` bytes and `Signer.Hash` with the unsigned
   fixture bytes and signing digest.
3. Compare `ProtoEncode` bytes and `Transaction.Hash` with the signed bytes and
   location-adjusted transaction ID.
4. Recover and compare the Quai sender using the Go signer, or verify the
   Qi Schnorr signature with Go's btcec implementation and the decoded input
   key, or ordered `musig2.AggregateKeys(keys, false)` for multiple inputs.

The JSON report records each result, the fixture SHA256, verified source commit,
Go version and enabled-CGO setting. Any mismatch exits nonzero. Regression
tests also corrupt expected hashes/digests and truncate signed bytes to check
that the harness fails rather than reporting success.

The `qi-1` and `qi-2` fixtures contain one and two data bytes respectively. They
are valid wire/signature comparison inputs but are **known to violate** the
pinned Go validator's permitted data lengths (zero, 20 or 22). The report marks
this explicitly. `ValidateQiTxInputs` and `ProcessQiTx` are not called: they
require node state. Even the other fixtures have no established UTXO existence,
balance, nonce, gas, maturity, fork or activation validity. There is no mempool
or block-acceptance claim, and no distributed MuSig protocol qualification.

## Reproduction

Use Go 1.24 or newer, Python 3 and a working C/C++ toolchain. The wrapper uses the installed toolchain and
disables automatic toolchain downloads. The checked-in report, when present,
records the exact toolchain used for that run. CGO is required by the pinned node's Litecoin secp256k1 dependency. The wrapper
sets `CGO_ENABLED=1`; this is recorded rather than assumed to be identical to
every node binary's build profile. Go VCS stamping is disabled for the temporary
harness copy; the wrapper verifies and records the node source commit itself.

Prepare the pinned source once in a separate checkout:

```sh
git clone https://github.com/dominant-strategies/go-quai.git /tmp/quai-go-oracle-source
git -C /tmp/quai-go-oracle-source checkout --detach f3f345c877300c044e3e0081a48bf3cf786fb9cc
```

From the Rust repository root:

```sh
python3 test-infra/go-oracle/run.py \
  --source /tmp/quai-go-oracle-source --test \
  > /tmp/quai-go-oracle-result.json
```

An existing clean `/tmp/quai-sdk-research-go` checkout is the default source.
`--fixtures` selects a different public fixture file; `--cache` selects another
cache root (default `/tmp/quai-go-oracle-cache`). Inputs are intended to be
reviewed test fixtures, not an untrusted public service.

First execution downloads the locked module dependencies from the Go proxy and
verifies them through the Go checksum database. Later builds use the same
`go.mod`/`go.sum` with `-mod=readonly`. `go mod verify` runs before building.
The wrapper isolates `GOMODCACHE`, `GOCACHE`, `GOPATH`, temporary build source and
executable under `/tmp`, disables ambient workspaces and Go environment files,
and removes the temporary module/executable on exit. The source checkout and
Rust workspace manifests are not modified.

To deliberately refresh only this oracle's Go lock files, run
`python3 test-infra/go-oracle/run.py --source PATH --update-lock --test`.
Review both lock-file changes before retaining them. Ordinary runs do not
modify these files. A source-pin change requires separately reviewing the
constant in `run.py`, the report constant in `main.go`, the Go requirement and
all resulting compatibility differences.

## Licensing and distribution boundary

The harness source is MIT licensed. It imports the Go project as a separate
development tool; it is not a dependency of any Rust crate, and its executable
must not be bundled into the SDK's crates.io artifacts. The upstream root
[`LICENSE`](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/LICENSE)
is GPL-3.0, while imported core/types and crypto source files retain LGPL-3.0-or-later
headers. Other modules retain their own licenses. This harness does not
relicense upstream code or copy its implementation into Rust. Any distribution
of a compiled Go oracle needs its applicable upstream notices/source obligations
handled independently of the MIT Rust packages.

## Recorded result

[`RESULTS.json`](RESULTS.json) records a successful run with Go
`go1.26.5-X:nodwarf5`, GCC 15.3.0 and CGO enabled. All 14 transaction fixtures matched signing
protobuf/digest, signed protobuf/hash and the applicable signature checks.
The two known Qi data-length failures remain flagged. All eleven ordered aggregate cases also matched, and Go verified both JS and
Rust signatures. The four oracle regression tests passed, including tampered
signatures. The report records hashes of both captured input files. This is an offline
library verification result, not a transaction-acceptance result.

## Ordered aggregation oracle

The optional aggregate fixture mode compares full compressed aggregate public
keys with the node's btcec `musig2.AggregateKeys(keys, false)` and verifies both
JS MuSig and Rust local aggregate signatures. It preserves order and duplicates.
The eleven cases include reversed lists, repeated keys and all-identical lists.
Its regression test mutates a Rust signature and requires rejection.

Verify the captured public fixtures:

```sh
python3 test-infra/go-oracle/run.py --test \
  --aggregates crates/quai-crypto/tests/fixtures/ordered-musig-rust.json \
  > /tmp/quai-go-oracle-aggregate-result.json
```

To exercise fresh Rust signing before Go verification:

```sh
cargo run -p quai-crypto --example generate_local_aggregate_signatures \
  > /tmp/quai-rust-local-aggregate-signatures.json
python3 test-infra/go-oracle/run.py --test \
  --aggregates /tmp/quai-rust-local-aggregate-signatures.json \
  > /tmp/quai-go-oracle-aggregate-result.json
```

The Rust example contains only public toy keys and makes no network calls.
Fresh signatures use OS randomness, so the aggregate fixture SHA256 changes on
regeneration. This checks signature compatibility for one local owner of all
keys; it does not qualify a distributed signing protocol.

## Explicit conversion wire fixtures

`CONVERSION-RESULTS.json` verifies eight pinned JavaScript conversion fixtures;
`CONVERSION-RUST-RESULTS.json` verifies eight corresponding envelopes freshly
signed by Rust (single-input, ordered/reversed/repeated-key Qi and account
conversions). Both reports compare protobuf, digests, IDs and signatures through
the same pinned Go library. The Rust fixture reference is an explicit additional
allowed reference string; unknown references still fail. Neither report invokes
node state processing or establishes conversion settlement.

```sh
python3 test-infra/go-oracle/run.py \
  --fixtures crates/quai-consensus/tests/conversion-vectors.json
cargo run -p quai-consensus --example conversion_oracle > /tmp/quai-rust-conversions.json
python3 test-infra/go-oracle/run.py --fixtures /tmp/quai-rust-conversions.json
```

The harness's `--test` default fixture regression suite still targets the ordinary
transaction fixture corpus, including its two deliberate invalid data-length
cases. Conversion fixture commands above run independent library verification
without that corpus-specific count assertion.
