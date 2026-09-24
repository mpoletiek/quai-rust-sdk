# quai-rust-sdk

A modular Rust SDK for building Quai Network applications and wallets on the Quai
account ledger and Qi UTXO ledger. It includes native HTTP/WebSocket providers,
local signing, HD wallets, durable wallet operations, contracts, payment codes,
and browser adapters.

[![crates.io](https://img.shields.io/crates/v/quai-sdk.svg)](https://crates.io/crates/quai-sdk)
[![docs.rs](https://img.shields.io/docsrs/quai-sdk)](https://docs.rs/quai-sdk)

**Pre-release:** `0.1.0-alpha.14` is published on
[crates.io](https://crates.io/crates/quai-sdk). It is not production-qualified for
real-fund custody and has had no external security audit; read [SECURITY.md](SECURITY.md)
and the [wallet gaps](docs/WALLET_GAPS.md) before integrating it. The
[SDK guide](SDK_DOCUMENTATION.md) covers every workflow, and the
[quais.js comparison](SDK_PARITY_ANALYSIS.md) records gaps and Rust differences.
API documentation is on [docs.rs](https://docs.rs/quai-sdk). Dated reviews and the
original plan are kept under [docs/history](docs/history/).

## Getting started

The tested native baseline is **Rust 1.97.1 on Linux x86-64**. The checked-in
`rust-toolchain.toml` selects it for rustup users. Node.js 20+ and Python 3.10+
are needed for the optional compatibility and node-test tooling.

```sh
git clone https://github.com/mpoletiek/quai-rust-sdk.git
cd quai-rust-sdk
cargo build --workspace --locked
cargo run --locked -p quai-sdk --example offline_wallet
```

The offline example derives wallet identities and signs a transaction without
submitting it. Its deterministic keys are public fixtures: **never fund them**.

For an application outside this workspace, depend on the crates.io release:

```toml
[dependencies]
quai-sdk = { version = "=0.1.0-alpha.14", features = ["sqlite", "abi"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

Structs the SDK may extend are `#[non_exhaustive]`: build inputs such as
`FeePolicy` or `QiPolicy` with their constructors and `with_*` methods, and
read outputs by field. Types whose fields a protocol fixes, such as
transactions, outpoints and `NetworkScope`, stay plain structs. CI rejects a
change to the published API unless the changelog declares it under `Breaking:`.

Pre-release versions are only selected when requested explicitly. The `=` pin
keeps a later pre-release, which may break the API, from being picked up
automatically; commit your application's lockfile as well. To follow unreleased
development, use `git = "https://github.com/mpoletiek/quai-rust-sdk"` with a
reviewed `rev`, or `path = "../quai-rust-sdk/crates/quai-sdk"` for a neighboring
checkout.

### Build settings for wallet scanning

**Cargo ignores a dependency's release profile.** Only your binary's profile
applies, so this workspace's own settings do not reach you and the SDK cannot
choose them on your behalf. If your application derives addresses -- any Qi
wallet scan or restore does, heavily -- set them yourself:

```toml
[profile.release]
lto = "fat"
codegen-units = 1
```

Measured on one BIP32 public-derivation step, the unit a scan repeats hundreds
of times per usable address: **72.4 us at Cargo's release defaults, 61.3 us with
the settings above -- about 18%.** The gain is cross-crate optimization inside
`k256`'s field arithmetic and the SHA-2 implementation, so it is not something
this SDK can obtain for you: adding `#[inline]` to its own wrappers was tried
and measured no change. The cost is roughly double the build time, which is why
it is a choice rather than a recommendation for every project.

Address derivation dominates Qi wallet CPU for a Quai-specific reason. The zone
lives in address byte 0 and the ledger in bit 7 of byte 1, and both come from
the Keccak hash of the derived point, so a usable address cannot be chosen --
it is ground for, at roughly one candidate in 512. A default restore performs
about 51,200 derivations where a standard BIP44 gap scan performs 40.

Scans grind in slices of about 20 ms and yield to the executor between them,
so a scan does not stall other tasks on its thread. With the `rayon` feature, a
scan can opt in with `with_grinding(Grinding::Parallel)` on its options: the
same addresses, found about 4x faster on six cores, using the whole rayon pool
while it runs.

### Read a node

The read-only example defaults to Orchard, expected chain ID `15000`, and Cyprus-1
gateway pathing:

```sh
cargo run --locked -p quai-sdk --example read_network
```

Its explicit arguments are `URL USE_PATHING EXPECTED_CHAIN_ID`:

```sh
# Direct local or mainnet node: preserve the supplied URL and port.
cargo run --locked -p quai-sdk --example read_network -- http://127.0.0.1:9200 false 9

# Orchard gateway: derive the Cyprus-1 route from the base URL.
cargo run --locked -p quai-sdk --example read_network -- https://orchard.rpc.quai.network true 15000
```

Substitute your own endpoint and expected chain ID. These commands read chain identity and height;
they do not load keys or send transactions. The separately patched disposable
harness uses port `19200` and chain ID `1337`, with its own documented genesis.

An application can configure the same routing directly:

```rust
use quai_sdk::{HttpConfig, HttpTransport, Provider, Routing, U256, Zone};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let zone = Zone::Cyprus1;
    let provider = Provider::new(
        HttpTransport::new(HttpConfig::default())?,
        Routing::with_pathing("http://127.0.0.1:9200", zone.into(), false)?,
        U256::from(9), // Expected network; use your node's actual chain ID.
    );
    let height = provider.block_number(zone.into()).await?;
    println!("Cyprus-1 height: {height}");
    Ok(())
}
```

Rust uses boolean `false`. For text configuration,
`parse_use_pathing("false")` returns false rather than applying JavaScript string
truthiness. Direct routing preserves the exact endpoint. Gateway routing derives
shard URLs from an immutable base while preserving its prefix/query and rejects
an already shard-suffixed base. Use `Routing::explicit` when local shards have
different ports; a missing shard is an error. See [RPC routing](crates/quai-rpc/README.md).

## Features and workspace

The facade crate is named **`quai-sdk`**. Its default features are `http` and `wallet`.

| Facade feature | Enables |
| --- | --- |
| `http` | Native HTTP transport and provider integration |
| `wallet` | Keys, mnemonic/HD derivation, consensus and local signers |
| `ws` | Native WebSocket transport and subscriptions |
| `sqlite` | Native durable wallet state and account/Qi session workflows; includes `wallet` |
| `abi` | ABI/EIP-712 support and contract, token, event and deployment helpers |
| `payments` | BIP47 codes and derivation; with `sqlite`, channel scan and send preparation |
| `keystore` | Legacy v3 JSON keystore import and native export |
| `backup` | Portable authenticated full-wallet capture/restore; includes `wallet,payments` |
| `browser` | Wasm Fetch and injected-wallet adapters |
| `rayon` | Opt-in parallel address grinding for scans (native only); includes `wallet` |
| `test-fixtures` | `new` constructors for provider observation outputs, for a consumer's own tests; they check no invariant |

For a browser build, disable native defaults and select the capabilities you need:

```sh
rustup target add wasm32-unknown-unknown
cargo check -p quai-sdk --no-default-features \
  --features browser,wallet,abi,payments,keystore \
  --target wasm32-unknown-unknown --locked
```

SQLite sessions and native HTTP/WS transports are not browser persistence or
transport implementations. See [browser support and real-browser tests](crates/quai-browser/README.md).

Contract bindings include exact fallback/receive intents, lossless bounded event
queries and code-appearance waits ([contract operations](docs/CONTRACT_PARITY.md)).
General resource fetching supports native HTTP and browser workers with bounded
bodies, cancellation and explicit hooks/gateways ([resource fetching](docs/FETCH_PARITY.md)).

| Crate | Responsibility |
| --- | --- |
| [`quai-primitives`](crates/quai-primitives/README.md) | Ledger/zone addresses, hashes, exact amounts, CREATE/CREATE2 prediction |
| [`quai-crypto`](crates/quai-crypto/README.md) | Guarded keys, recoverable ECDSA, BIP340, ordered local multi-key signing |
| [`quai-consensus`](crates/quai-consensus/README.md) | Bounded canonical transaction encoding, signing digests and verified identities |
| [`quai-wallet`](crates/quai-wallet/README.md) | Mnemonic/HD/watch-only wallets, authenticated backups, Qi selection and SQLite state |
| `quai-signer` | Chain-bound local/watch-only signing, personal messages and typed data |
| [`quai-abi`](crates/quai-abi/README.md) | Bounded ABI, interfaces and EIP-712 encoding |
| [`quai-rpc`](crates/quai-rpc/README.md) | Routing, strict RPC envelopes and bounded native HTTP/WebSocket transports |
| [`quai-provider`](crates/quai-provider/README.md) | Typed reads, explicit signed submission, receipt and conversion observations |
| [`quai-browser`](crates/quai-browser/README.md) | Browser Fetch, injected signatures and worker execution |
| [`quai-payments`](crates/quai-payments/README.md) | BIP47 codes, validated channels and bounded address derivation |
| [`quai-keystore`](crates/quai-keystore/README.md) | Bounded AES-CTR/scrypt/PBKDF2 interchange and mnemonic ownership verification |
| `quai-sdk` | Application facade, durable account/Qi sessions and contract helpers |

## Wallet workflows and security

Account and Qi sessions separate preparation, signing and explicit submission.
They freeze the reviewed payload, reserve its nonce or inputs, persist verified
signed bytes, then submit those exact bytes. A timeout or cancellation may happen
after the node receives a transaction: signed reservations remain held for recovery.
Unsigned prepared objects are bound to the exact open wallet-store handle.

- [Current wallet workflows](docs/WALLET_WORKFLOWS.md): gap-50 Qi scans, payment codes, mixed-origin sends, conversions, wrappers, sweep and recovery.
- [Account workflow](docs/account-workflow.md): nonce reservation, fee limits, deployment and restart behavior.
- [Qi workflow](docs/qi-transactions.md): change allocation, qualified discovery, bounded fee convergence and input claims.
- [Conversion support](docs/conversions.md): signed intents, correlation, refunds and unverified spendability.
- [State proofs](docs/STATE_PROOFS.md): balances, nonces, code hashes and token slots a node must prove against a block's state root.
- [Full-wallet backups](crates/quai-wallet/FULL_BACKUP_FORMAT.md): supported origins, durable state, channel recovery and format limits.
- [Security policy and limits](SECURITY.md): threat boundaries, secret handling and unresolved release gates.

The HTTP transport verifies TLS, disables redirects, automatic retries and environment
proxies (an explicit HTTP(S) or SOCKS5 proxy is opt-in), bounds responses/concurrency/deadlines, and
redacts credentials in diagnostics. The WebSocket transport pings idle connections
so a half-open socket fails rather than going silent.
There is no automatic failover. Chain/genesis checks detect configuration mistakes;
they do not authenticate remote observations. Raw `Transport::request` bypasses typed
provider checks. Conversion receipt success alone does not prove maturity or spendability.

Remaining concerns include unwiped upstream scrypt workspace memory, malicious
source observations, copied-wallet rollback, full reorg/recovery behavior and
specialist external review; see [SECURITY.md](SECURITY.md). No external security
audit or production certification has occurred.

## Validation and node evidence

CI runs the workspace suite on Linux, macOS and Windows with strict Clippy,
rustdoc, format, no-default-feature, semver, Chromium worker, fuzz-smoke and
dependency-advisory checks. Two inactive optional unmaintained dependencies are
documented in the [advisory report](docs/dependency-audit.md).

Read-only HTTP and WebSocket head notifications were exercised against a
mainnet node and the Orchard gateway. The [isolated harness](test-infra/local-chain/README.md)
verified transfers, deployments, multi-input Qi, replacement and conversion behavior
on explicitly patched development profiles. The [high-level Qi run](test-infra/local-chain/HIGHLEVEL.md)
also verified fee convergence, persisted signatures, process restart and exact outputs.

The pinned node's pending-state account RPC failures and isolated-chain patches
are documented in the retained reports. Current preparation has explicit
pending/latest policy, and later isolated-chain account/deployment checks have
their own evidence. Patched-node success does not qualify unmodified networks,
funded Orchard transactions or mature redemption spend. Funded mainnet
qualification is recorded separately in the [mainnet harness](test-infra/orchard/README.md);
ordinary tests never send mainnet transactions.

Run the standard checks from the repository root:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo test --workspace --no-default-features --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps --locked
npm ci --prefix compatibility --ignore-scripts
npm run verify --prefix compatibility
npm test --prefix compatibility
python3 -m unittest discover -s test-infra/tests -v
python3 -m unittest discover -s test-infra/local-chain -p 'test_*.py' -v
```

Native mock transports need ephemeral loopback sockets. Ordinary tests do not send
live transactions; live tests are explicitly gated. Browser tests require Chrome and
chromedriver. Go acceptance/oracle tools build the pinned node source separately.
See [browser tests](crates/quai-browser/README.md), [sanitizer fuzzing](fuzz/README.md),
[JS compatibility](compatibility/README.md), [Go oracle](test-infra/go-oracle/README.md)
and [node probes](test-infra/README.md) for their additional setup and commands.
CI is configured in [.github/workflows/ci.yml](.github/workflows/ci.yml); configuration
alone does not establish macOS, Windows or remote CI qualification.

## Roadmap and contributing

The [architecture](docs/architecture.md), [wallet gaps](docs/WALLET_GAPS.md) and
[current parity analysis](SDK_PARITY_ANALYSIS.md) track scope; the original plan
and its audit are in [docs/history](docs/history/). Remaining production qualification includes funded unmodified-node
acceptance, mature redemption spend, broader engine/extension interoperability,
sustained fault/fuzz/performance work and independent security review. See the
[changelog](CHANGELOG.md) and [publishing guide](docs/PUBLISHING.md).

Changes should document API behavior and limitations, include meaningful regressions
or independent compatibility evidence, and pass the relevant checks above. Keep
signing/submission explicit and preserve ambiguous signed claims. Use public test
fixtures only; do not commit real keys, wallet exports, database files or credentials.
Read [SECURITY.md](SECURITY.md) before reporting a security issue, and never include
secrets in a public issue. Use [private vulnerability reporting](https://github.com/mpoletiek/quai-rust-sdk/security/advisories/new) for security reports.

## License

First-party SDK code is licensed under the **[MIT License](LICENSE)**.
Third-party code, reference data and tools retain their original licenses and
attribution; see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). Referencing or
building go-quai does not relicense its code under MIT.
