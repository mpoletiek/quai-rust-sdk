# Quai Rust SDK documentation

This is the integration guide for the SDK in this repository, version
`0.1.0-alpha.1`. It covers the public crate layers, native and browser workflows,
recovery formats, limits, examples and verification. Exact Rust signatures and
field documentation are hosted on [docs.rs](https://docs.rs/quai-sdk) or can be
generated from the same checkout with `cargo doc`.
The companion [parity analysis](SDK_PARITY_ANALYSIS.md) identifies differences
from the pinned published `quais@1.0.0-alpha.57` SDK.

The SDK is an alpha. Implemented functionality and passing tests do not establish
identical JavaScript behavior or production qualification. The reviewed reference
capabilities and deliberate differences are documented in the comparison. The
repository is public and MIT licensed; all twelve crates are published on
[crates.io](https://crates.io/crates/quai-sdk).

## Contents

1. [Installation and feature selection](#installation-and-feature-selection)
2. [Architecture and API locations](#architecture-and-api-locations)
3. [Networks, routing and providers](#networks-routing-and-providers)
4. [Addresses, amounts and utilities](#addresses-amounts-and-utilities)
5. [Keys, HD derivation and signing](#keys-hd-derivation-and-signing)
6. [Qi discovery and fixed denominations](#qi-discovery-and-fixed-denominations)
7. [Native transaction workflows](#native-transaction-workflows)
8. [Payment codes](#payment-codes)
9. [Conversions and wrapped assets](#conversions-and-wrapped-assets)
10. [ABI, contracts and deployment](#abi-contracts-and-deployment)
11. [Persistence, backups and recovery](#persistence-backups-and-recovery)
12. [Browser integration](#browser-integration)
13. [Errors and operational limits](#errors-and-operational-limits)
14. [Examples, tests and qualification](#examples-tests-and-qualification)
15. [Detailed reference index](#detailed-reference-index)

## Installation and feature selection

The checked-in toolchain is Rust 1.97.1. Cargo resolves twelve workspace crates
through the root lockfile. Optional comparison tools require Node.js 20+ and
Python 3.10+. The application runtime does not depend on Node or quais.js.

```sh
git clone https://github.com/mpoletiek/quai-rust-sdk.git
cd quai-rust-sdk
cargo build --workspace --locked
cargo run -p quai-sdk --example offline_wallet --locked
```

For another Rust project, depend on the crates.io release:

```toml
[dependencies]
quai-sdk = { version = "=0.1.0-alpha.1", features = ["sqlite", "abi", "payments", "backup"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

Pre-release versions are only selected when requested explicitly. The `=` pin
keeps a later, possibly breaking alpha from being picked up automatically; commit
the application's lockfile for reproducible builds. To follow unreleased
development, use `git = "https://github.com/mpoletiek/quai-rust-sdk"` with a
reviewed `rev`, or `path = "../quai-rust-sdk/crates/quai-sdk"` for a local checkout.
The individual crates (`quai-primitives`, `quai-provider`, `quai-wallet` and so on)
are published at the same version for applications that do not need the facade.

| Facade feature | Capabilities and dependencies |
| --- | --- |
| Default: `http,wallet` | Native HTTP provider and offline wallet/signing types |
| `wallet` | Crypto, consensus, signer and wallet crate re-exports; portable discovery |
| `http` | Native HTTP transport; unavailable on browser Wasm |
| `ws` | Native WebSocket transport and subscriptions |
| `sqlite` | `wallet` plus native durable account/Qi sessions and reconciliation |
| `abi` | ABI/EIP-712, consensus, contract calls, artifacts and wrapper adapters |
| `payments` | BIP47 identities and channels; native channel orchestration with `sqlite` |
| `backup` | `wallet,payments` and authenticated portable full-wallet recovery |
| `keystore` | Legacy v3 JSON keystore import/export on native and Wasm |
| `browser` | Wasm Fetch, WebSocket, injected provider and IndexedDB adapters |

Browser builds must disable native defaults:

```sh
rustup target add wasm32-unknown-unknown
cargo check -p quai-sdk --no-default-features \
  --features browser,wallet,abi,payments,keystore,backup \
  --target wasm32-unknown-unknown --locked
```

Generate signature-level documentation for both targets:

```sh
cargo doc --workspace --all-features --no-deps --locked
cargo doc -p quai-sdk --no-default-features \
  --features browser,wallet,abi,payments,keystore,backup \
  --target wasm32-unknown-unknown --no-deps --locked
```

Open `target/doc/quai_sdk/index.html` for native APIs and
`target/wasm32-unknown-unknown/doc/quai_sdk/index.html` for browser APIs. A native
all-features build does not expose target-gated browser modules.

## Architecture and API locations

The facade re-exports lower layers without requiring the durable session layer.
Choose a layer according to whether the application needs primitives, explicit
signing/submission, or persisted wallet orchestration.

| Crate / facade path | Responsibilities | Source |
| --- | --- | --- |
| `quai-primitives` / `primitives` | Addresses, shards, amounts, fixed point, encoding, Unicode, CREATE prediction | [source](crates/quai-primitives/src/lib.rs) |
| `quai-crypto` / `crypto` | Guarded secp256k1 keys, ECDSA, BIP340, local multi-key signatures, hashing and entropy | [source](crates/quai-crypto/src/lib.rs) |
| `quai-consensus` / `consensus` | Typed Quai/Qi transactions, canonical protobuf, hashes and signature verification | [source](crates/quai-consensus/src/lib.rs) |
| `quai-wallet` / `wallet` | BIP39/BIP32/BIP44, selection, ownership metadata, allocation/custody journals, backups, SQLite | [source](crates/quai-wallet/src/lib.rs) |
| `quai-signer` / `signer` | Local/watch-only signer interfaces, personal messages and EIP-712 signing | [source](crates/quai-signer/src/lib.rs) |
| `quai-rpc` / `rpc` | Routing, strict RPC envelopes, transport trait, native HTTP/WS | [source](crates/quai-rpc/src/lib.rs) |
| `quai-provider` / `provider` | Typed node reads, explicit broadcast, confirmations, head replay, ETX observations | [source](crates/quai-provider/src/lib.rs) |
| `quai-abi` / `abi` | Solidity ABI, readable/JSON interfaces, events, artifacts and EIP-712 | [source](crates/quai-abi/src/lib.rs) |
| `quai-payments` / `payments` | BIP47 payment codes, key derivation and public channel state | [source](crates/quai-payments/src/lib.rs) |
| `quai-keystore` / `keystore` | Bounded legacy key interchange and mnemonic ownership validation | [source](crates/quai-keystore/src/lib.rs) |
| `quai-browser` / `browser` | Browser transports, wallet integration, timers and scoped snapshots | [source](crates/quai-browser/src/lib.rs) |
| `quai-sdk` | Native sessions, browser books, contract/wrapper composition | [source](crates/quai-sdk/src/lib.rs) |

Native facade modules are `accounts`, `qi`, `qi_discovery`, `payment_channels`,
`recovery`, `settlement` and `deployments`. Portable modules include `account_preflight`,
`qi_preflight`, `account_replacement`, `qi_replacement`, `candidate_observation`, `settlement_observation`,
`discovery`, `contracts` and `wrappers`. Browser modules are `browser_addresses`,
`browser_payments`, `browser_accounts`, `browser_qi`, `browser_backups` and
`browser_transactions`, `browser_qi_transactions` and `browser_recovery`.
Their precise feature gates are in the linked facade source.

## Networks, routing and providers

Use an explicit expected chain ID. Wallet state additionally binds a trusted
genesis and zone in `NetworkScope`; a network label or chain ID alone is not a
complete storage or recovery identity.

| Network | Expected chain ID | Gateway base | Cyprus-1 |
| --- | --- | --- | --- |
| Mainnet | `9` | `https://rpc.quai.network` | `/cyprus1` |
| Orchard | `15000` | `https://orchard.rpc.quai.network` | `/cyprus1` |
| Repository development harness | `1337` | Explicit loopback HTTP configuration | Patched isolated chain; separate genesis |

Orchard was reported under maintenance on September 13, 2026. Availability is
not guaranteed by this table. The retained [public mainnet read report](test-infra/reports/mainnet-public-read-recheck-2026-09-13.json)
records chain identity and contract reads, without spending funds.

```rust
use quai_sdk::{HttpConfig, HttpTransport, Provider, Routing, U256, Zone};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let zone = Zone::Cyprus1;
    let provider = Provider::new(
        HttpTransport::new(HttpConfig::default())?,
        Routing::with_pathing("https://rpc.quai.network", zone.into(), true)?,
        U256::from(9),
    );
    println!("{}", provider.block_number(zone.into()).await?);
    Ok(())
}
```

`Routing::direct` preserves an exact endpoint and port. Gateway pathing derives
shard paths from an immutable base while preserving its prefix/query. Do not
supply an already shard-suffixed base for gateway pathing. `Routing::explicit`
handles different local ports per shard and rejects unconfigured shards.
`parse_use_pathing("false")` parses an actual false value.

`Provider<T>` composes any supported `Transport`; the Wasm trait supports
non-`Send` futures. Typed methods cover chain/genesis/height/headers, balances,
nonces, gas, code/storage, calls/estimation/access lists, transactions/receipts,
logs, outpoints and wallet conversion queries. Further modules cover full block
and txpool inspection, external transactions, Qi fees and deployment/code
observations. Unknown or nullable node results remain distinguishable from zero.
Use explicit `BlockTag` values; not every node method supports every selector.

HTTP/WS envelopes validate IDs, result/error exclusivity, quantities and bounded
payloads. A successful chain check does not authenticate a malicious node or make
multiple RPC reads atomic. Raw method access does not imply a typed, qualified
SDK workflow for that method.

`HttpTransport::batch(endpoint, requests)` submits an explicit batch of up to 128
calls and 2 MiB of encoded request data. Results retain input order; each entry
can contain a remote error. Duplicate, missing, foreign or malformed response IDs
reject the entire batch. The configured response-size and total-time limits apply
to the batch. No failed batch is retried. `Transport::request_batch` advertises
this optional capability; `None` means no requests were sent.

`Provider::outpoints_many` groups each zone into pages of at most 32 addresses,
including chain-ID checks in each supported batch. Other transports use at most
four concurrent individual reads. Native `refresh_qi` uses eight-address pages
and retains its before/after head guard. This reduces HTTP overhead and public
gateway rate-limit pressure while preserving rejection of a moving view; it
does not make latest-only outpoint reads historical or atomic.

Subscriptions expose lag and disconnect. Native head-following can reconnect and
replay bounded canonical headers; replay/invalidation must be applied before
assuming continuity. Browser sockets require explicit reconnect and history
reconciliation. Receipt waits have bounded polls/time and canonical checks;
a receipt is an observation, not permanent finality or destination settlement.


### Indexed transaction waits and Qi response verification

`Provider::observe_transaction_confirmation` provides a portable one-shot
confirmation check without requiring receipts. Native `Provider::wait_for_transaction`
and browser `wait_for_transaction` add explicit deadlines/polling limits, including
Qi transactions. They check numbered block membership, transaction index,
refreshed fields and the sampled head; unknown or changed observations remain
pending. `Provider::transaction_block` exposes the block association check.

`Transaction::verified_qi` separately verifies supported Qi transfer, conversion
and wrapping signatures and locally computed IDs, preserving input order. It
complements `verified_quai`. Inclusion checks are node observations; signature
checks do not prove input existence, maturity, spendability or finality. Neither
operation changes custody or retries a submission. See [response parity](docs/TRANSACTION_RESPONSE_PARITY.md).


### General resource requests

`rpc::fetch::FetchClient` handles resources separately from JSON-RPC. Choose
`NativeFetch` with the `http` feature or `browser::BrowserResourceFetch` on Wasm.
`FetchRequest` supports validated headers, Basic authentication and byte/text/JSON
bodies; `FetchResponse` retains error bodies and offers strict text/JSON/status
inspection. Defaults permit one exchange and no redirects. `FetchConfig` and
`FetchHooks` express bounded retries, explicit redirects, custom gateways and
preflight/processing policy. `FetchCancellation` or dropping send releases active
local work; use a fresh token after cancellation.

`data_resource` decodes local data URIs. `ipfs_resource` requires a caller-selected
gateway and does not authenticate CID content. Browser CORS, opaque redirects
and compression negotiation have explicit limits. See [resource reference and
parity](docs/FETCH_PARITY.md) for bounds, examples and corrected source behavior.
Exact large JSON numbers must be strings; float round-trip support preserves
binary floats rather than arbitrary-precision integers.


### Network metadata

`provider::Network` carries an immutable name and exact chain ID; `NetworkRegistry`
provides bounded, atomic caller-owned aliases. `NetworkMatch` chooses name or
chain comparison explicitly. These labels never authenticate genesis or choose a
provider. `FeeData` is an optional exact gas-price view; live lookup still propagates
RPC errors.


### Block, receipt and log views

Mined full and hash-only blocks support exact `BlockTransactionId` lookup and
normalized `to_rpc_json()` export. `metadata()` exposes exact timestamp/size/entropy,
header maps and bounded interlink, manifest, uncle, work-share and outbound ETX
views. Outbound ETXs remain separate from transactions executed in the block.

Transactions, receipts and logs export normalized node JSON while preserving
exact quantities and top-level extensions. `Receipt::fee()` checks U256 overflow.
With feature `abi`, `contracts::decode_receipt_logs` preserves decoded, unknown
and malformed logs in order; `Contract::receipt_logs` restricts interpretation to
its bound emitter. See [response API and parity details](docs/RESPONSE_PARITY.md)
for limits, field mappings, canonicality and published lookup defects.


### Passive accounts and portable event delivery

`Provider::accounts(zone, max_accounts)` lists already exposed remote Quai accounts
without prompting for access. It preserves order and rejects duplicate, malformed,
wrong-ledger or oversized responses. Select a concrete account and trusted network
scope explicitly before constructing a remote signer.

`provider::event_hub::EventHub<K, E>` supplies bounded typed local `on`, `once`,
`emit`, `poll`, listener inspection/removal, pause/resume and close operations.
Applications explicitly feed transport notifications or canonical head updates.
Capacity failures leave all queues unchanged; no silent eviction or automatic
network subscription occurs. Limits count queued items, so bound payload sizes
and share large values with `Arc`/`Rc` as appropriate. Native WebSocket `is_open`
reports observed local session state, matching the browser socket convenience.
See [provider lifecycle and parity](docs/PROVIDER_PARITY.md) for API usage and the
mapping of JS initialization, callbacks, options and transport internals.

## Addresses, amounts and utilities

`Address` is 20 bytes. `QuaiAddress` and `QiAddress` add ledger and known-zone
validation. `Zone`, `Region` and `Shard` use protocol identifiers; a known zone
encoding does not establish that a network has activated it. Parse strings into
typed addresses before building a transaction.

Ordinary address parsing accepts uniform-case hex and enforces the checksum on
mixed-case input. `Address::from_checksummed_str` requires exact prefixed checksum
spelling. `Display` emits the checksummed address. Whitespace and uppercase `0X`
prefixes reject. Parsing is not ownership or destination-policy validation.

Use integer base units throughout signing:

| Asset | Display decimals | Smallest unit |
| --- | --- | --- |
| Quai | 18 | Its |
| Qi | 3 | Qit |
| WQUAI / WQI ERC-20 amount | 18 | Token atom; conversion rules differ by asset |

`parse_quai`, `parse_qi`, `parse_units` and matching formatters avoid floating
point. Unsigned chain amounts use `U256`; signed display values use
`SignedUnits`. Excess nonzero fractional precision, scientific notation and
implicit rounding reject. `FixedPoint` supports exact checked arithmetic,
explicit formats and rounding policies; overflow fails. Its floor/ceiling and
negative rounding intentionally correct recorded reference defects.

The primitives crate also provides bounded hex, Base64, Base58, byte padding,
slicing/concatenation, two's complement, bytes32 strings, Unicode normalization,
strict UTF-8 and UUID-v4 formatting from supplied entropy. Crypto supplies
Keccak/SHA-family hashes, RIPEMD-160 and OS/Web Crypto entropy. There are no
implicit floating-point, JavaScript truthiness or arbitrary object conversions.

CREATE/CREATE2 prediction uses exact init-code bytes. Quai CREATE uses the node's
eight-byte nonce representation. Leading zero code bytes are preserved, fixing a
pinned JS discrepancy. Predicted addresses must still satisfy deployment rules.

`consensus::document::TransactionDocument` adds exact JSON/protobuf interchange
for unsigned and verified signed account/Qi operations. It validates supplied
hash/sender claims, preserves full u64/U256 quantities and exact Qi data, and
exposes both first-output and all-output zone metadata. Raw protobuf DTOs retain
optional field presence; bounded helpers check encoding, while typed decoders
check semantics and signatures. `access_list_from_json` accepts ordered list or
explicit map normalization forms. See [transaction interchange and limits](docs/TRANSACTION_INTERCHANGE_PARITY.md).


### Numeric interchange and shard metadata

`primitives::numeric` provides exact signed integer parsing, unsigned byte/hex/RPC
quantity conversions, explicitly checked JavaScript-number bridges and
`HexFormat` validation. General signed parsing is bounded to 512 bits; transaction
quantities use U256. `SignedUnits::INT256_MIN` and `INT256_MAX` provide the signed
256-bit limits. `Shard::ALL`, `Shard::metadata` and `Zone::metadata` expose the
published labels while retaining typed hierarchy and zone validation.


See [numeric reference](docs/DECLARATION_PARITY.md).


### Checked and wrapping fixed-point arithmetic

`primitives::FixedPoint` offers `checked_add/sub/mul/div` and explicit
`wrapping_add/sub/mul/div`. Wrapping uses the configured integer field width;
multiplication/division truncate fractional scaled units toward zero. Format
mismatch and division by zero remain errors. `to_f64_lossy` is an explicitly
approximate presentation conversion; stored units remain exact. See the
[fixed-point comparison](docs/FIXED_POINT_PARITY.md) for the corrected source
signed-minimum behavior and exhaustive tests.

## Keys, HD derivation and signing

`Mnemonic::generate(language, word_count)` accepts 12/15/18/21/24 words using
secure entropy. `parse`, `from_entropy` and `entropy` validate BIP39 checksums and
support all ten pinned wordlists. NFKD normalization applies to mnemonic and
passphrase. The effective passphrase determines identity and is never silently
dropped during restore.

`wordlist::Wordlist` provides public word/index lookups, exact phrase conventions,
validated custom lists and bounded OWL/OWL-A imports with eager checksums.
`wordlist::CustomMnemonic` supports custom dictionary phrase/entropy/seed
conversion with an explicit passphrase; pass the seed to `HdWallet::from_seed`
and preserve it with a seed-origin encrypted backup. Phrase/word exports are
redacted zeroizing guards. Built-in Chinese mnemonic parsing also accepts
unseparated characters; entropy export never guesses a different language.
See [wordlist parity and limits](docs/WORDLIST_PARITY.md).

`HdWallet` accepts effective seeds of 16–64 bytes, mnemonics with explicit
passphrases, or a depth-zero master xprv. BIP44 paths are:

- Quai: `m/44'/994'/account'/change/index`
- Qi: `m/44'/969'/account'/change/index`

Accounts are hardened; receive (`change=false`) and change branches have
nonhardened children. `AccountPublic` permits watch-only derivation. Public
metadata preserves actual raw child indices, including skipped candidates.
Use bounded `Search` to find the requested zone and ledger; there is no guarantee
that an arbitrary child belongs to Cyprus-1 or to Qi.

`ExtendedPrivateKey` and `ExtendedPublicKey` support exact child derivation,
validated xprv/xpub import/export and depth/fingerprint/chain-code metadata.
`derive_path` requires a master; `derive_relative_path` handles explicit
subtrees. Public derivation rejects hardened steps. An invalid child fails at
its exact index rather than silently relabeling a later child.

`PublicAddress` retains imported or BIP44 ownership metadata.
`QiKeyResolver`/`QiKeyring` resolve HD, imported and BIP47 receive keys, validating
that the resolved point matches the transaction input. Watch-only types cannot
supply a signing secret.

Quai transactions use recoverable ECDSA. Ordinary Qi uses BIP340 for one input
and ordered local multi-key signing for multiple inputs. The SDK does not offer
a distributed MuSig coordinator or hardware-device protocol. Consensus types
validate canonical protobuf, chain, exact payload and recovered identity.
Separate types represent ordinary Qi, Qi conversion and Qi wrapping; arbitrary
special data cannot be smuggled through the ordinary transfer signer.

Local signer methods also support Quai personal messages, explicit Qi message
signing and EIP-712. A message signature does not authorize unrelated transaction
fields. Typed-data chain policy is explicit. Secrets use guarded wrappers with
redacted diagnostics and zeroization of owned buffers. Caller copies and every
compiler/dependency temporary cannot be guaranteed erased.


### Public roots and standalone KDFs

`wallet::ExtendedPublicKey::from_public_key_chain_code` creates a synthetic public
derivation root. `from_components` preserves checked supplied BIP32 metadata;
neither proves ancestry. Both support bounded public child derivation without
private material.

`keystore::derive` exposes standalone PBKDF2-HMAC-SHA256/SHA512 and scrypt with
exact byte inputs, parameter/output-work budgets and zeroizing derived output.
Run CPU work on a bounded native or dedicated browser worker. Optional Started/
Completed callbacks can cancel at checkpoints; they do not provide intermediate
progress or interrupt the underlying KDF loop. See [utility reference and parity](docs/UTILITY_PARITY.md).


### Curve arithmetic

`crypto::curve::CurveScalar` supports canonical zero-inclusive scalar parsing,
reduction, addition, multiplication and negation with guarded output. Validated
`PublicKey` values support multiplication, multiply-add, negation, X extraction
and even-Y lifting. Public field helpers and multipart/tagged SHA256 are bounded;
use `quais_tagged_sha256` for the reference's non-ASCII tag encoding and
`tagged_sha256` for UTF-8. These helpers do not create distributed signing sessions.


See [curve reference](docs/CURVE_AND_AGGREGATION_PARITY.md).


### Recovered signers

`crypto::recover_message_signer` recovers from exact personal-message bytes.
`recover_typed_data_signer` accepts a validated `abi::TypedData` document and
canonical signature. Both return an address that the caller must compare with
its expected signer; domain/chain authorization remains explicit. `VERSION` is
the Rust package version. See the [complete declaration mappings and numeric
boundaries](docs/DECLARATION_PARITY.md), including source coercions intentionally
rejected by Rust and the remaining platform/runtime differences.


### Specialized HD wallet parity

The [HD wallet review](docs/HD_WALLET_PARITY_REVIEW.md) maps Quai/Qi identity,
address lookup, channels, current scans, key ownership and transaction workflows.
Verified legacy wallet-JSON migration and cached address-status/gap views now
have native and browser-worker implementations. Authenticated Rust backups and typed current observations already exist;
these should not be confused with the reference's plaintext wallet schema or
mutable status cache.


### Signature metadata and shared-secret utilities

The crypto layer supports EIP-2098 compact signatures, explicit legacy EIP-155
metadata, checked chain/V conversion helpers, full SEC1 ECDH shared points and
public-point addition. Shared secret outputs use redacted zeroizing buffers.
See [crypto parity](docs/CRYPTO_PARITY.md) for exact format distinctions, key and
signature validation, published constructor defects and the complete API mapping.

## Qi discovery and fixed denominations

Qi outputs encode a denomination index, not an arbitrary output amount. The
fifteen values in **Qits** are:

```text
1, 5, 10, 50, 100, 500, 1000, 5000, 10000, 20000,
100000, 1000000, 10000000, 100000000, 1000000000
```

`Denomination` validates the index and exposes its exact value. Planners account
for denomination inventory and output address capacity, in addition to totals.

`qi_discovery::scan_and_refresh_qi` is the native node-backed workflow. The default
is **50 consecutive matching addresses with no current outpoints on each of the
receive and change branches**. Raw children skipped for zone/ledger mismatch do
not count. A funded match resets the gap. `QiScanOptions` bounds raw ranges and
matching queries; `gap_limit: None` explicitly deep-scans the selected range.
Reports expose stop reasons and continuation indices.

`refresh_qi` queries all persisted Qi addresses, including imported keys, known
payment receive exposures and allocated change beyond the gap. It validates
scope, duplicate outpoints, locks and sampled headers before a generation-checked
update. `qi_balance` classifies total, spendable, reserved, locked and expired
Qits; held claims take precedence.

Portable `discovery::discover_qi` and `AccountRpcSource` provide the corresponding
bounded read foundation for other runtimes. Optional caller-supplied address-use
hints can extend scanning past known fully spent addresses. A hint does not
create an output or prove a historical balance.

The node's latest-only outpoint API cannot prove historical address use or an
atomic historical snapshot. Before/after header checks reduce ordinary races.
A mnemonic-only gap scan can stop before old fully spent/burned ranges. Restore
retained cursors and known addresses, and use explicit deeper ranges as needed.
Ordinary current discovery does not require an external indexer; stronger
historical recovery requires independently qualified observations.


### Qi address inventory and cached usage

`wallet::qi_addresses::QiAddressBook` provides exact public origin lookup, HD
branch/account views, verified payment receive views and cached gap filters.
`discovery::refresh_qi_address_book` refreshes all registered origins with bounded
outpoint reads and commits only after the final network/head checks. Usage hints
are optional and work with worker-local callbacks. Errors and cancellation preserve
the prior cache. See [Qi address views](docs/QI_ADDRESS_VIEWS.md) for registration,
status transitions, reorg invalidation and the separation from allocation/custody.

## Native transaction workflows

Enable `sqlite`. `SqliteStore` stores public ownership, claims, exact signed
payloads and observation state under an explicit network scope. Keep secrets in
caller-owned guarded key objects or authenticated backups.

The common lifecycle is:

1. Initialize/import ownership and retained cursor/claim state.
2. Allocate fresh output/change addresses durably, then refresh observations.
3. Prepare under an operation ID and explicit fee/resource policy.
4. Review the frozen final transaction, destinations, amount, fee and digest.
5. Sign and persist exact verified bytes before returning them.
6. Broadcast explicitly using the same persisted operation ID.
7. Reconcile origin inclusion, replacements, reorgs and destination settlement.

A timeout after signing/submission never proves that funds are reusable. Reload
and inspect the operation; retries use its existing bytes. Signed claims remain
held until a separate validated lifecycle policy establishes what can happen.

### Quai accounts

`AccountSession` provides `prepare`, reserved-nonce preparation, explicit
cross-zone preparation, conversion and deployment preparation, `sign` and
`broadcast`. `AccountIntent` carries destination, exact value/data and ordered
access list; `FeePolicy` bounds gas/price/total cost. Nonces are durably allocated.

Pending-state observations are the default. If a node lacks working pending
reads, explicitly choose `AccountObservationPolicy::PinnedLatest`; nonce,
balance and simulation use a numeric block with head checks. This excludes
mempool effects. RPC failure does not silently select that fallback.

Access lists are preserved. Optional explicit access-list discovery estimates
the actual transaction and rechecks nonce/state races. Deployment reserves its
nonce before address grinding. Account candidate replacement is fee-only with
immutable payment intent and held nonce ownership.

### Qi payments, sweep and special operations

`QiSession::new` uses HD keys; `with_keys` accepts a mixed-origin resolver.
`QiChangePool::allocate` uses compact native allocation: it commits the examined
child range and ownership metadata before returning each address. It does not
burn unused trailing search candidates. A failed preparation still consumes all
addresses already returned; old burned ranges never rewind. The bounded search
holds a SQLite write lock. Allocate before refreshing because new ownership
metadata invalidates the old snapshot.

`prepare` selects inputs and converges the fee against the final payload under
`QiPolicy`. Supply distinct recipient address capacity for denomination outputs.
The prepared object exposes transaction, fee and signing digest. `sign` commits
custody, and `broadcast(id)` submits persisted bytes. Explicit cross-zone
preparation retains actual destination outputs in fee estimation.

`prepare_sweep` spends every eligible selected-scope coin or fails its bounds.
`SweepMode::PreserveDenominations` retains ordinary denomination constraints.
`SweepMode::Aggregate` explicitly permits consolidation, with fewer outputs than
inputs. The pinned node only permits aggregation as the first Qi transaction in
a block; preparation does not secure that position.

`prepare_special` handles native conversion/wrapping with an explicit authorized
Qit fee. `prepare_special_estimated` requires a selected compatible node fee
profile. Exact specialized data, claims and signed bytes survive backup/restart.
See the next section for profile and settlement boundaries.


### Threshold aggregation

`wallet::select_aggregate` applies an input denomination threshold, funds the exact
fee from eligible outside coins and appends the separately denominated fee refund.
Use `SweepMode::AggregateThreshold(AggregationPolicy::default())` in `quote_qi` or
native `prepare_sweep` for fee-converged preparation. Defaults are input index 6,
output index 14 and required UTXO-count reduction. Set `require_reduction: false`
explicitly for non-reducing plans. Larger denominations still require the node's
first-Qi block-position exception. See [complete mappings, bounds and examples of
reference differences](docs/CURVE_AND_AGGREGATION_PARITY.md).


### Unknown account nonce competitors

`provider.observe_account_replacements(&signed_original, trusted_genesis, request)`
scans an explicit bounded page for the original or a mined same-sender/nonce
transaction, including unregistered repricing, cancellation or changed recipients.
It verifies signatures and canonical associations without adopting candidates or
releasing claims. Native `wait_for_account_transaction` follows bounded pages
under an overall deadline; `quai_browser::wait_for_account_transaction` adds
window/worker deadlines and a page/poll budget. Both use the portable
`AccountReplacementTracker` for executor-independent composition.
See [account nonce replacement discovery](docs/ACCOUNT_NONCE_REPLACEMENTS.md) for
coverage, missing receipts, reorg behavior and the published-reference differences.

Native `AccountSession::observe_nonce(id, request)` combines both views. It first
reconciles durable candidates; when none is canonical it scans the bounded page and
returns `AccountNonceOutcome::Registered`, `Unregistered` (a verified same-nonce
transaction unknown to the store, such as one from another device) or `Unresolved`
with coverage. No claim is released. On mainnet it reported an externally signed
cancellation as `Unregistered` while the durable original remained held.


### Remote account signing and external custody

`rpc_signer::RpcAccountSigner` supports verified personal and typed-data signatures,
exact remote Quai transaction signing, finite-duration account unlocking and
wallet-mediated submission acknowledgements on native and Wasm transports.
`RpcSignerError::dispatched` preserves the distinction between preflight failure
and a potentially accepted request. No request is automatically retried.

Native and browser prepared account transactions accept verified external bytes
through `commit_external_signature`; the exact reviewed fields and live reservation
must still match before persistence. Wallet-mediated sends instead return a
`RemoteSendAcknowledgement` for independent signed-transaction observation and
explicit comparison with the original request. See [remote signer documentation](docs/RPC_SIGNER.md)
for construction, durable workflow, cancellation, limits and quais.js differences.

## Payment codes

`PaymentCode` validates BIP47 version-one payloads and Base58Check encodings.
`PrivatePaymentCode::from_seed` derives `m/47'/969'/account'`; master-xprv and
explicit payment-account-xprv imports are supported. Account imports validate
depth/index but cannot prove omitted ancestry. Payment children are `/index`
directly, matching executed reference behavior.

`send_public_key`, `receive_public_key` and `receive_key` derive matching sender
and receiver points. Bounded `search` selects a Qi zone match with cancellation
and continuation indices. Public codes expose linkage information, even though
they do not contain private keys.

Exchange peer codes explicitly out of band. `PaymentChannel` validates the local
owner and stores send/receive cursors per zone. Exhaustion never rewinds.
Native `SqliteStore::import_payment_channel` registers an owned channel;
`payment_channels::payment_intent` allocates fresh send destinations;
`scan_payment_channel` performs gap-50 or deep receive scans and persists verified
exposures. `continue_payment_channel` explicitly extends scanning beyond the highest
persisted receive exposure in the current zone, including after reopening or a
refresh failure after metadata import. Sparse imported metadata is not coverage;
use an explicit range with `gap_limit: None` for bounded deep recovery. A default
50-address gap can still miss funds beyond old spent or abandoned ranges.
`QiKeyring::load_payment_channel` enables spending those outputs. Native payment
intents use `allocate_payment_address_compact`; previously returned addresses and
legacy burned ranges stay consumed.

Browser `BrowserPaymentBook` persists the range before search and a completed
exposure before return. Each allocation has a caller-retained ID. Cancellation
or failure burns the range. BIP47 notification-transaction discovery and
blinding are not implemented.

Pelagus announces channels through a mailbox contract instead: senders call
`notify(sender, receiver)` and receivers read `getNotifications(receiver)`.
With `abi` and `payments`, `payment_mailbox::PaymentMailbox` prepares exact
`notify` account intents and returns validated, deduplicated sender codes,
bounded to `MAX_MAILBOX_NOTIFICATIONS`. `PELAGUS_MAILBOX_ADDRESS` is the
address Pelagus uses; identical runtime bytes were observed on mainnet and
Orchard. This is a wallet convention, not a protocol rule. Announcements are
unauthenticated and public: anyone can announce a code, and a `notify` links
both codes on-chain. A Pelagus recipient only discovers a channel after
`notify`, which cost 243,807 gas on mainnet. Native
`payment_channels::discover_mailbox_channels` reads announcements, registers at most
`max_channels` (1–64) announced senders, scans each and reports deferred, invalid and
duplicate entries. Because announcements are unauthenticated, each registration
persists metadata for a code anyone could have announced; keep the bound small.

## Conversions and wrapped assets

Qi-to-Quai refunds need separate output accounting. The pinned node omits refund
denominations at or below 500 Qits: a refund smaller than 1 Qi can report a
successful receipt while creating no outputs. Gas limits can further reduce
creation. Inspect `observe_conversion_qi_credit` and its `unobserved_qits` instead
of treating receipt success as full repayment. A refunded Qi output can be keyed
by an ETX hash with the Quai ledger bit; SDK validation checks its nonzero hash,
zone and actual Qi ownership rather than rejecting that bit.

Mainnet prices conversions per prime-block batch: concurrent conversions share
one cubic discount and conversions exceeding their slippage are refunded, which
single-transaction quotes cannot predict. `conversion_batch_discount_bps` reproduces
the pinned formula; see [conversion slippage](docs/conversions.md).

`Provider::qi_to_quai`, `quai_to_qi` and `calculate_conversion_amount` retain exact
integer quantities and unknown results. A historical selector is not a promise
of a historical quote: the pinned node's Quai-to-Qi path uses its current prime
terminus. Controller estimates are advisory settlement estimates.

| Operation | API path | Required accounting |
| --- | --- | --- |
| Quai → Qi | `AccountSession::prepare_conversion` | Native value, account fee/nonce, slippage and destination outputs |
| Qi → Quai | `QiSpecialIntent::Conversion` / Qi conversion transaction | Fixed-denomination inputs, 22-byte special data, refund/slippage, Qit fee |
| Qi → WQI backing | `QiSpecialIntent::Wrapping` / Qi wrapping transaction | 20-byte owner-contract data and same-zone beneficiary |
| Claim WQI backing | `WrappedQi::claim_deposit` | Explicit account contract call and access list |
| WQI → Qi | `WrappedQi::unwrap` | Qits → token atoms, destination ETX gas, redemption dust/locks |
| Quai → WQUAI | `WrappedQuai::deposit` | Exact native value attached to deposit call |
| WQUAI → Quai | `WrappedQuai::withdraw` | Exact token amount; zero native call value |

`Provider::estimate_quai_conversion_gas` preserves the raw node estimate.
`estimate_quai_conversion_gas_budget`, used by native `prepare_conversion` and
portable/browser `quote_quai_conversion`, also budgets origin intrinsic gas,
ETX creation, and the maximum denomination count for amounts at or below the
sampled nominal Qi quote. Controller discounts can increase the output count.
The caller's gas margin applies afterward, and all fee caps still apply before
nonce reservation. Missing/zero quotes fail explicitly. This empty-access-list
budget uses the pinned go-quai gas schedule; a future increase in the quote or a
different schedule remains an estimation risk. Follow destination receipts and
outpoints even when the origin succeeds: insufficient ETX gas can produce a
failed receipt with partial Qi outputs. See [funded Orchard evidence](test-infra/orchard/README.md).

Configured deployments in Cyprus-1:

| Asset | Network | Address |
| --- | --- | --- |
| WQI | Mainnet and Orchard | `0x002b2596EcF05C93a31ff916E8b456DF6C77c750` |
| WQUAI | Mainnet | `0x006C3e2AaAE5DB1bCd11A1a097cE572312EADdBB` |
| WQUAI | Orchard | `0x005c46f661Baef20671943f2b4c087Df3E7CEb13` |

`wrappers::{WQI_ADDRESS,WQUAI_MAINNET_ADDRESS,WQUAI_ORCHARD_ADDRESS}`
exposes explicit deployments. `WQUAI_ADDRESS` remains a mainnet compatibility alias. Constructors also
accept explicit deployments. `new` constructs an offline binding;
`new_verified` checks genesis, canonical sampled block, nonempty runtime and an
optional expected runtime hash. Mainnet reads observed code at both addresses;
the corrected Orchard deployment passed a funded 0.01 QUAI wrap/unwrap round trip
([evidence](test-infra/orchard/wquai-roundtrip-2026-09-14.json)). Configuration nevertheless
does not guarantee deployment availability. See [code preflight](docs/CONTRACT_CODE_PREFLIGHT.md).

`WrappedQi::unclaimed` queries backing; only the narrowly recognized no-balance
RPC error is treated as absent backing. Other errors propagate. `token()` on both
wrappers provides ERC-20 reads and transfer/approval intents. Calls do not send
implicitly. WQI claim/redemption includes the zone lockup contract access list;
external signing integrations must retain it.

One Qit corresponds to `10^15` WQI atoms. The explicit go-quai-v0.56 redemption
profile discards denomination indices 0–5; exact supported redemptions require
multiples of 1000 Qits and sufficient destination gas. `QiRedemptionPlan` exposes
the breakdown; the wrapper rejects dust loss under that profile.

`QiFeeProfile::V056ShaAnchored` requires asserted compatible node rules at/after
prime 1,755,000. Specialized estimation accounts for the actual shape, scaling,
special ETX gas, base-fee margin and rounding. A reported client-version string
is not sufficient software/fork attestation. Ordinary estimation must not be
used where the node drops specialized data.

`ConversionReference`, `observe_conversion`, provider ETX tracking and native
`settlement` correlate origin, changed-hash external execution, refunds and
redemption credit. Origin success does not prove destination spendability.
Refresh Qi outputs and their actual locks. Aggregate Quai balances cannot by
themselves attribute per-operation maturity.

## ABI, contracts and deployment

`AbiType`, `AbiValue` and `AbiCoder` encode/decode bounded canonical values.
Integers use exact values/decimal strings, tuples are positional, and loose JS
coercions are rejected. `default_values` constructs validated zero/empty values.
Packed Solidity encoding and Keccak/SHA256 helpers are separate; dynamic packed
fields can be ambiguous and there is no general packed decoder.

`AbiInterface` loads JSON or human-readable declarations with bounded nesting,
fragments and text. It supports canonical/minimal/full formatting, constructor,
function/error/event lookup, selector/topic calculation and overload ambiguity
errors. `parse_call`, `parse_log` and `parse_revert` distinguish unknown,
ambiguous and malformed data. Reverts include builtin `Error(string)` and
`Panic(uint256)`. Anonymous logs require an explicitly selected event.

Event filters use `AbiFilterValue::{Any,Exact,AnyOf}` in argument order. Indexed
compound values remain hashes on decode. `Contract::events_by_values` binds the
emitter and preserves removal/inclusion metadata. Unknown emitters and forged
revert bytes are not authenticated merely by matching an ABI.

`TypedData::from_json` rejects duplicate JSON keys and validates schema, primary
type, domain and message before caching the EIP-712 signing hash.
`TypedDataEncoder` exposes schema compilation, type/data encodings and hashes.
No address-name resolution or implicit network lookup occurs.

`Contract::prepare` builds `ContractCall`; `call`/`simulate` are explicit reads.
A call exposes destination, signature, data, value, arguments and access list.
With native sessions, `into_account_intent` preserves those fields for durable
preparation and signing. `Erc20` supplies balance/allowance reads and
transfer/approve intents.

`SolidityArtifact` loads an explicit contract entry or source/contract selection
from compiler output. Exact creation code is retained; supply externally linked
bytecode, since unresolved placeholders reject. Constructor arguments are appended
before Quai address grinding. `PreparedDeployment` exposes the predicted address,
salt/attempts and init data. Native deployment preparation couples a reserved
nonce, payability, fees and allowed-zone/ledger search; deployment observations
verify receipt/canonical code without treating simulation as execution.


### Contract fallback, event delivery and code waits

`Contract::prepare_fallback(data, value)` produces a `FallbackCall`;
`simulate_fallback` returns raw bytes and `estimate_fallback` returns gas.
`into_account_intent` carries exact calldata/value/access entries into native or
browser durable signing. `Contract::attach` and `connect` explicitly bind another
address/provider without I/O. ABI receive/fallback declaration order does not
change mutability.

`query_logs(range, topics, max_logs)` returns owned `ContractLog` values retaining
decoded, unknown and malformed events, including removal/association metadata.
The query bounds result count and aggregate decoded expansion and shares event
declarations. Applications feed these values into their own bounded event queues.

For an address with no known deployment transaction, construct `ContractCodeTarget`
with the trusted genesis and optional runtime hash, then use native/browser
`wait_for_contract_code` with explicit `CodeWaitConfig` deadline, interval and
poll limit. Empty/changed views remain pending; source errors stop and dropping
the future cancels reads. Code presence does not establish finality.
See [contract parity and limits](docs/CONTRACT_PARITY.md) for the complete mapping.


### ABI parameter reflection, gas hints and named results

The ABI layer now exposes named input/output parameter trees, individual fragment
formatting and exact function/constructor gas metadata. `AbiResult` supports eager
named decoding for calls, returns, custom errors and events, with checked slices,
name-preserving filtering and collision-safe object views. Parameter walks support
sync and non-Send async callbacks, exact named tuples and aggregate resource
limits. Gas hints remain metadata; contract transaction fee policy is unchanged.
See [ABI reflection and result parity](docs/ABI_REFLECTION_PARITY.md) for API
mappings, examples of the parameter syntax, limits and published JavaScript defects.


### EIP-712 utility parity

`TypedDataEncoder` exposes borrowed schema metadata, reusable encoders for primitive,
array and struct roots, and shape-checked value visitors. These utilities support
binding generation and deliberate value transformations before normal domain and
signer validation. See [typed-data parity](docs/TYPED_DATA_PARITY.md) for the complete
published API mapping, exact encoding rules, callback semantics and resource limits.

## Persistence, backups and recovery

Native SQLite schema v5 stores scoped public ownership, nonce/outpoint claims,
operations, signed candidate families, cursors and bounded observation caches.
Mutations use transactions and generation/revision fences. `SqliteStore::open`
uses a five-second lock wait; `open_with_busy_timeout` accepts zero through
60 seconds in whole milliseconds. Lock errors do not automatically retry actions. A reorg invalidates
coins and affected observations while retaining signed claims, exact bytes and
burned allocation ranges. Do not release an input because a latest query omitted
it or a transaction temporarily disappeared.

Portable journals expose the same core custody concepts without SQLite:

| Journal | Current frame | Bound / behavior |
| --- | --- | --- |
| HD allocation | `QADDRBK1`, sealed-history `QADDRBK2` | 4096 retained IDs; reserve range before search |
| Payment allocation | `QPAYABK1`, sealed-history `QPAYABK2` | 1024 retained IDs; exact owner/peer/direction/scope |
| Account custody | `QACCTBK1` | 256 retained IDs; durable nonce and signed candidates |
| Qi custody | `QQICUBK1` | 256 retained IDs; exact ownership/input claims and candidates |
| Public address | `QADDR001` | Imported/BIP44 metadata; no signing secret |
| Head replay | `QHEAD001` | Bounded ancestry and explicit continuity state |

Released/abandoned IDs remain consumed; capacity is not reclaimed by pruning.
Custody candidate families permit at most 32 replacement edges. Allocation
restore seals old ranges, abandons pending searches and advances to maximum
verified floors, preserving exhaustion. Older v1-only readers cannot read v2
allocation histories.

### Authenticated full-wallet backup

`BackupOrigin` represents an effective seed, master xprv, imported private key or
explicit payment-account xprv. Constructing from a mnemonic retains its effective
seed; this full-wallet format is recovery of identity/state, not the original
phrase/language/passphrase text. Preserve the desired user-facing mnemonic
separately in an appropriate guarded backup workflow.

`WalletBackup::capture` captures SQLite state with ownership proofs.
`capture_account_custody`, `capture_qi_custody` and `capture_portable` cover public
journals. `PortableWalletCapture` combines HD/account/Qi/payment journals,
additional owned addresses and optional `previous_inventory`. Current custody
journals must supply all operations: prior inventory intentionally contributes
addresses/channels/exposures/floors, not stale transaction custody.

`WalletBackup::encrypt(password, BackupKdf)` produces `EncryptedWalletBackup`.
`from_bytes` validates framing/bounds; `decrypt` authenticates and proves owned
metadata before yielding a guarded wallet. Encryption uses Argon2id and
XChaCha20-Poly1305 with authenticated framing and fresh randomness. Formats
QUAIWALT v1–v5 retain backwards decoding while adding channels, account origins
and candidate families. The format limits plaintext to 16 MiB, origins to 16
and records to 100,000. See the [full format specification](crates/quai-wallet/FULL_BACKUP_FORMAT.md).

Recovery preserves known address/payment inventory, burned floors, claims and
exact signed candidates. It discards chain observations for revalidation.
Portable recovery does not recreate missing allocation request-ID history;
retain public allocation journals when that exact history is needed. Neither
ownership proofs nor encryption prevent a caller from selecting an old authentic
backup; merge current state monotonically instead of resetting live cursors.

### Browser capture and restore

`capture_wallet_backup(BrowserWalletCaptureSources, origins)` reads every selected
journal then rechecks every revision. Unchanged monotonic revisions establish a
consistent cut for those selected public journals. A changed/corrupt/tombstoned
journal rejects; no write or retry occurs. It is not a chain snapshot or automatic
discovery of omitted stores.

`merge_wallet_backup(BrowserWalletRestoreTargets, backup)` validates all live
merges before committing in **one IndexedDB transaction**. All stores must share
one database name. Duplicate targets, cross-database targets, invalid ownership,
missing journals and stale revisions fail without partial updates. Up to 128
journals and 16 MiB combined candidate state are accepted. Account/Qi claims stay
held; pending allocations become abandoned and floors never rewind.

Cancellation before dispatch is read-only; cancellation after dispatch can commit
all updates. Re-read every selected revision/operation before deciding whether
to retry. See [atomic restore](docs/BROWSER_ATOMIC_RESTORE.md) and
[portable capture](docs/PORTABLE_WALLET_CAPTURE.md).

### Legacy interchange

`Keystore::from_json` and `decrypt` support bounded v3 JSON scrypt/PBKDF2 and
AES-CTR/MAC import, checking key/address and optional mnemonic ownership.
Native/Wasm export uses fresh OS/Web Crypto randomness. Unsupported lossy mnemonic exports reject.
Legacy keystores do not include wallet claims, allocation/channel history or a
fully authenticated public wallet state. Prefer full-wallet AEAD recovery for
that purpose. KDF bounds apply before expensive work; browser KDFs belong in a
worker. See [keystore documentation](crates/quai-keystore/README.md).


### Whole legacy wallet JSON migration

`wallet::full_backup::legacy::import_quais_json` verifies the original language,
passphrase and trusted public root, then proves every HD/imported/payment address
and returns an authenticated-backup-ready inventory with known index floors.
Cached chain observations are discarded. `export_quais_json` returns guarded
plaintext for representable identities and rejects loss of custody or burned
allocation ranges. See [legacy wallet migration](docs/LEGACY_WALLET_MIGRATION.md)
for API usage, resource limits and safe restoration.

Both pinned quais.js HD `xPub()` methods return xprv, despite their names. Rust's
`root_public_key` always returns a public key. Neuter the reference result before
using it as a public migration trust anchor; the Rust public-key importer rejects
raw xprv strings.

## Browser integration

`BrowserFetchTransport` and `BrowserWebSocketTransport` compose with the same
provider. They work in windows and dedicated workers without Tokio. Fetch uses
bounded streamed responses, an AbortController, explicit deadlines and no
credential/referrer forwarding; browser CORS/mixed-content rules still apply.
WebSocket messages are bounded after the browser assembles them; lag and
cancelled subscription ownership are explicit errors. No write is replayed
implicitly on reconnect.

`InjectedProvider` takes an explicitly selected JS provider object, endpoint
identity, shard and expected chain. Construction does not prompt for accounts.
Use `accounts` to read exposed accounts and `request_accounts` from an explicit
connection action. Personal/EIP-712 signatures are checked against the requested
account and exact digest. Context events and chain rechecks reject changed
accounts/chains during operations.

`sign_quai_transaction` requires already populated fields and rejects a wallet's
changed signed payload. `signed_submission_transport` explicitly enables raw
signed submission; generic injected transport is read-only. Wallet-mediated send
has its own observation/verification contract. Errors retain sanitized numeric
codes such as 4001/4200, not wallet messages or payloads. Dropping an injected
future cannot retract a wallet dialog or an accepted send.

`BrowserSnapshotStore` stores scoped public bytes or caller-encrypted envelopes.
It preserves tombstone revisions and uses compare-and-exchange across tabs.
`compare_exchange_snapshots` performs a bounded atomic multi-namespace transaction
within one named database. No plaintext private origin belongs in these stores.

`BrowserAddressBook` and `BrowserPaymentBook` implement durable fresh allocation.
`BrowserAccountBook` persists nonce custody and exact signing candidates.
`BrowserQiBook` persists ownership/input custody, validates selected discovery
inputs and signs ordinary/conversion/wrapping operations before returning bytes.
The new capture/restore coordinator composes these stores. `browser_transactions::BrowserAccountSession` now prepares same/cross-zone
Quai calls with exact-nonce fees, fixed review/signing and explicit persisted-root
submission. `account_preflight::quote_account` is portable, and contract/wrapper
intents retain their access lists. See [browser account workflow](docs/BROWSER_ACCOUNT_WORKFLOW.md).
Browser conversions and nonce-bound deployments share this workflow, with optional
access-list discovery that preserves required entries.
`browser_recovery::BrowserRecoverySession` selects exact persisted candidates and
reconciles account/Qi inclusions without unlocked keys; see
[browser candidate recovery](docs/BROWSER_CANDIDATE_RECOVERY.md). Both sessions now prepare [reviewed replacements](docs/BROWSER_REPLACEMENTS.md)
with exact field/ownership checks and durable candidate signing. Destination
settlement now reconstructs persisted signed intent through `observe_settlement`.
Browser destination views/cursors are in memory; native resume also persists
revalidated scan cursors. See [candidate recovery](docs/BROWSER_CANDIDATE_RECOVERY.md).
`browser_qi_transactions::BrowserQiSession` now combines current HD discovery and
persisted owners with exact denomination/fee planning, fresh change allocation,
revision-fenced custody and signing for transfers/conversions/wrapping. Portable
`qi_preflight::quote_qi` also supports explicit cross-zone and sweep policies. See
[the browser Qi workflow](docs/BROWSER_QI_WORKFLOW.md). Injected-wallet discovery, permission management
and automatic chain switching remain absent.

## Errors and operational limits

Errors are typed per layer: primitive/crypto/consensus validation, `RpcError`,
`ProviderError`, wallet `StorageError`, native session errors and browser errors.
Handle absence, unsupported capability, stale observation, conflict, exhaustion,
RPC rejection and timeout separately. Do not turn these into empty balances or
success. Debug output redacts guarded keys and backups; applications must also
avoid logging their own raw secret inputs.

| Resource | Representative bound |
| --- | --- |
| General byte utility output | 1 MiB; Base58 input 4096 bytes |
| Unit parsing | 0–80 decimals, bounded 512-byte text |
| Fixed point | 8–256 bits in byte increments; 0–80 decimals |
| Explicit address/payment search | Up to 100,000 attempts per bounded request |
| Native Qi transaction policy | Up to 1024 inputs/outputs; 1–32 fee rounds |
| Browser account/Qi retained operations | 256 IDs per journal |
| Browser HD/payment retained allocations | 4096 / 1024 IDs |
| Browser snapshot / coordinated restore | 16 MiB per store / total restore; up to 128 restore targets; 2048 database slots including tombstones |
| Full backup | 16 MiB plaintext; 16 private origins; 100,000 records |
| Signed replacement family | 32 edges in addition to the root |
| Native head replay | Bounded 4096-header ancestry; explicit recovery on missing continuity |

These are library policies, not claims about protocol-wide limits. Individual
APIs have additional bounds and accept explicit configuration within their
supported range. Bounds apply to encoded/application-owned data; cryptographic
working memory, browser/native networking and decoded structures need additional
memory. Strict finite limits and consumed IDs are deliberate Rust differences.

## Examples, tests and qualification

All checked-in seeds/private keys are public fixtures and must never be funded.
Examples separate offline intent/signing from read-only node inspection.

| Example | Feature selection | Purpose |
| --- | --- | --- |
| `offline_wallet` | default `wallet` | Derive both ledgers, sign/decode locally |
| `read_network` | `http` | Chain identity and block height |
| `qi_scan` | `sqlite,http` | Explicit scoped xpub current discovery |
| `payment_codes` | `payments` | Offline matching sender/receiver derivation |
| `wrapper_intents` | `abi,http` | Construct wrapping/ERC-20 calldata |
| `inspect_pool` | `http` | Typed pool inspection |
| `inspect_blocks` | `http` | Typed block inspection |
| `artifact_deployment` | `abi` | Artifact/constructor/deployment preparation |
| `watch_qi` | `http,wallet` | Bounded head/replay inspection |
| `qi_message` | `wallet` | Explicit Qi message signing/verification |

Run an example with `cargo run -p quai-sdk --features FEATURES --example NAME`.
Source files in [the examples directory](crates/quai-sdk/examples) document
positional arguments and fixtures. Mainnet read example:

```sh
cargo run -p quai-sdk --example read_network -- https://rpc.quai.network true 9
```

Core verification:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo test --workspace --no-default-features --locked
cargo doc --workspace --all-features --no-deps --locked
python3 test-infra/sync_package_files.py --check
```

Native RPC/WS tests use owned loopback listeners. Ignored live tests require
explicit endpoints/network configuration. Browser tests require the Wasm target,
matching wasm-bindgen tooling, Chromium and chromedriver; run
`python3 crates/quai-browser/tests/run_browser.py --help` for available suites.
The `worker` and `sdk-portable-capture` suites exercise actual IndexedDB atomicity.

The [compatibility harness](compatibility/README.md) pins the published JS
artifact, compiler and dependency graph. `npm ci --ignore-scripts`, verification,
fixture generation and tests run in `compatibility/`. Review expectation changes
before accepting regenerated fixtures. The [Go oracle](test-infra/go-oracle)
checks selected wire/consensus behavior independently. Bounded sanitizer fuzzing
and [package rehearsal](test-infra/package_rehearsal.py) complement deterministic
tests. Packaging checks archives and fresh extracted consumers without publishing.

Linux/macOS/Windows CI and Chromium worker evidence exist. Public mainnet reads
and patched isolated-chain transaction execution are separate evidence classes.
Funded Orchard evidence includes QUAI transfers, Quai-to-Qi creation/unlocking and
converted-output spending, WQUAI deposit/withdraw, and WQI backing/claim/redemption
followed by a mature redeemed-output spend. The [funded qualification report](test-infra/orchard/README.md)
records exact transactions, fee profiles and limitations. The [payment-code round trip](test-infra/orchard/payment-codes-2026-09-14.json)
exercised local send, receiver-key recovery, return spending and discovery.
The failed preparations' burned send ranges required explicit receive-scan
continuation beyond the first empty gap; this remains distinct from Pelagus testing.
The [Qi-to-Quai run](test-infra/orchard/qi-to-quai-2026-09-14.json) verified a 1 Qi
conversion, exact locked-to-unlocked account credit, and a subsequent 0.01 QUAI
spend whose fee-only replacement mined. The [transaction-scenario run](test-infra/orchard/transaction-scenarios-2026-09-14.json)
verified 46-input aggregation (including HD and BIP47 keys) in the first Qi block
position, and a 15-input sweep recovered after an injected lost acknowledgement.
The [isolated rollback test](test-infra/local-chain/recovery-2026-09-14.json) retained
custody through canonical-inclusion removal, reopening and restoration. The
[isolated refund test](test-infra/local-chain/refund-2026-09-14.json) verified strict
slippage, a real refunded Qi output, and its subsequent wallet-session spend.
Broader unmodified-node acceptance, consensus-driven competing-branch reorgs,
sustained fault/soak/performance tests, additional
browser engines/extensions and independent specialist security review remain
qualification work.

Funded **mainnet** qualification began on 2026-09-14 through `https://rpc.quai.network`
([record](test-infra/orchard/mainnet-2026-09-14.json), [checks](test-infra/orchard/mainnet-checks-2026-09-14.json),
[Pelagus](test-infra/orchard/mainnet-pelagus-2026-09-14.json)). It covers QUAI transfers,
WQUAI deposit/withdraw and approve/transferFrom/transfer, a fee-only replacement,
reconciliation after a withheld acknowledgement, discovery of an unregistered nonce
cancellation, a ground deployment with code wait, seed-only recovery of locked
conversion Qi, automatic specialized fee quotes (36 Qits for a 1 Qi conversion, 33
for a wrap), authenticated backup restore and six Quai-to-Qi conversions. Batch-wide
conversion discounts from concurrent third-party flow refunded three conversions;
three credited 3,416 Qits. Pelagus interoperability passed in both directions: the
SDK discovered a Pelagus sender through its mailbox contract, recovered 15 Qi, sent a
Pelagus-compatible `notify`, and returned 1 Qi from the received output. Qi spending
of conversion outputs, WQI and Qi-to-Quai on mainnet await lock expiry or activation.
Cross-zone qualification is deferred while only Cyprus-1 is available.


### Reference selection review and publishing

The [Qi selection review](docs/QI_SELECTION_PARITY_REVIEW.md) maps denominations,
coin metadata, selection results and fee adjustments, including reference bugs
that Rust corrects. [Publishing instructions](docs/PUBLISHING.md) cover package
metadata, docs.rs targets, extracted archive checks and first-release dependency
order, plus the procedure and rate limits observed for the `0.1.0-alpha.1` upload.
Later uploads still require separate authorization and a registry account.

Use [private vulnerability reporting](https://github.com/mpoletiek/quai-rust-sdk/security/advisories/new)
for security findings; follow the [security policy](SECURITY.md) and use public
toy reproductions instead of credentials or funded wallet material.

## Detailed reference index

These documents provide deeper contracts, binary field layouts and source-linked
evidence. They supplement the generated native/browser signature reference.

| Subject | Reference |
| --- | --- |
| Architecture | [architecture](docs/architecture.md) |
| Wallet derivation | [wallet crate](crates/quai-wallet/README.md) |
| Discovery | [discovery specification](crates/quai-wallet/DISCOVERY.md) |
| SQLite state | [storage specification](crates/quai-wallet/STORAGE.md) |
| Backup framing | [full backup format](crates/quai-wallet/FULL_BACKUP_FORMAT.md) |
| Native workflows | [wallet workflows](docs/WALLET_WORKFLOWS.md), [account workflow](docs/account-workflow.md), [Qi transactions](docs/qi-transactions.md), [conversions](docs/conversions.md) |
| Browser custody | [account](docs/BROWSER_ACCOUNT_CUSTODY.md), [Qi](docs/BROWSER_QI_CUSTODY.md), [allocation restore](docs/BROWSER_ALLOCATION_RESTORE.md) |
| Coordinated recovery | [capture](docs/PORTABLE_WALLET_CAPTURE.md), [atomic restore](docs/BROWSER_ATOMIC_RESTORE.md) |
| Contracts | [ABI](crates/quai-abi/README.md), [code preflight](docs/CONTRACT_CODE_PREFLIGHT.md) |
| RPC and browser policy | [RPC](crates/quai-rpc/README.md), [provider](crates/quai-provider/README.md), [browser](crates/quai-browser/README.md) |
| Key interchange | [keystore](crates/quai-keystore/README.md), [payment codes](crates/quai-payments/README.md) |
| Current status | [implementation](IMPLEMENTATION_STATUS.md), [gaps](docs/WALLET_GAPS.md), [feature review](docs/FEATURE_COMPLETENESS_REVIEW_2026-09-12.md) |
| Parity evidence | [analysis](SDK_PARITY_ANALYSIS.md), [machine-readable ledger](compatibility/parity.json), [reference lock](compatibility/reference-lock.json) |
| Test provenance | [retained reports](test-infra/reports), [isolated chain](test-infra/local-chain/README.md), [CI](.github/workflows/ci.yml) |
| Release/security | [security review](docs/SECURITY_REVIEW_2026-09-11.md), [dependency audit](docs/dependency-audit.md), [third-party notices](THIRD_PARTY_NOTICES.md) |
