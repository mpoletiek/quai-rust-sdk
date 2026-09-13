# Quai Rust SDK documentation

This is the integration guide for the SDK in this repository, version
`0.1.0-alpha.1`. It covers the public crate layers, native and browser workflows,
recovery formats, limits, examples and verification. Exact Rust signatures and
field documentation are generated from the same checkout with `cargo doc`.
The companion [parity analysis](SDK_PARITY_ANALYSIS.md) identifies differences
from the pinned published `quais@1.0.0-alpha.57` SDK.

The SDK is an alpha. Implemented functionality and passing tests do not establish
complete quais.js parity or production qualification. The repository is public
and MIT licensed. All crates currently have `publish = false`; no crates.io
release is available from this project.

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

For another Rust project:

```toml
[dependencies]
quai-sdk = { git = "https://github.com/mpoletiek/quai-rust-sdk", branch = "main", features = ["sqlite", "abi", "payments", "backup"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

Replace `branch` with a reviewed `rev` and commit the application's lockfile for
reproducible builds. A local checkout can use
`path = "../quai-rust-sdk/crates/quai-sdk"` instead.

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
| `keystore` | Legacy v3 JSON keystore import and native export |
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
`qi_preflight`, `candidate_observation`, `discovery`, `contracts` and `wrappers`. Browser modules are `browser_addresses`,
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

Subscriptions expose lag and disconnect. Native head-following can reconnect and
replay bounded canonical headers; replay/invalidation must be applied before
assuming continuity. Browser sockets require explicit reconnect and history
reconciliation. Receipt waits have bounded polls/time and canonical checks;
a receipt is an observation, not permanent finality or destination settlement.

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

## Keys, HD derivation and signing

`Mnemonic::generate(language, word_count)` accepts 12/15/18/21/24 words using
secure entropy. `parse`, `from_entropy` and `entropy` validate BIP39 checksums and
support all ten pinned wordlists. NFKD normalization applies to mnemonic and
passphrase. The effective passphrase determines identity and is never silently
dropped during restore.

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
`QiChangePool::allocate` burns bounded ranges before exposing addresses. Allocate
before refreshing because new ownership metadata invalidates the old snapshot.

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
exposures. `QiKeyring::load_payment_channel` enables spending those outputs.

Browser `BrowserPaymentBook` persists the range before search and a completed
exposure before return. Each allocation has a caller-retained ID. Cancellation
or failure burns the range. Automatic notification-transaction discovery,
blinding and peer-code exchange are not implemented.

## Conversions and wrapped assets

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

Confirmed configured addresses on both mainnet and Orchard, Cyprus-1:

| Asset | Address |
| --- | --- |
| WQI | `0x002b2596EcF05C93a31ff916E8b456DF6C77c750` |
| WQUAI | `0x006C3e2AaAE5DB1bCd11A1a097cE572312EADdBB` |

`wrappers::{WQI_ADDRESS,WQUAI_ADDRESS}` exposes the constants. Constructors also
accept explicit deployments. `new` constructs an offline binding;
`new_verified` checks genesis, canonical sampled block, nonempty runtime and an
optional expected runtime hash. Mainnet reads observed code at both addresses;
the last retained Orchard check found empty WQUAI code. Configuration therefore
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

## Persistence, backups and recovery

Native SQLite schema v5 stores scoped public ownership, nonce/outpoint claims,
operations, signed candidate families, cursors and bounded observation caches.
Mutations use transactions and generation/revision fences. A reorg invalidates
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
Native export uses fresh randomness. Unsupported lossy mnemonic exports reject.
Legacy keystores do not include wallet claims, allocation/channel history or a
fully authenticated public wallet state. Prefer full-wallet AEAD recovery for
that purpose. KDF bounds apply before expensive work; browser KDFs belong in a
worker. See [keystore documentation](crates/quai-keystore/README.md).

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
[browser candidate recovery](docs/BROWSER_CANDIDATE_RECOVERY.md). Replacement
preparation and destination settlement still require integration.
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
Funded Orchard/unmodified-node acceptance, mature WQI unlock spending,
aggregation placement, broader reorg/fault/soak/performance tests, additional
browser engines/extensions and independent specialist security review remain
qualification work. No mainnet transaction was submitted during this review.

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
