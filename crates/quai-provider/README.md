# quai-provider

Typed wallet/application RPCs and explicit signed submission over configured shard routes. Every network method checks the routed endpoint's chain ID first. Lookup hashes and account/Qi transaction chain IDs are also checked against the request. RPC response validation is not cryptographic verification or proof of finality.

The provider currently exposes:

- Chain ID, block number, running zones and Quai account balance.
- Account transaction count, gas price, bytecode and storage words.
- Account-only `call` and `estimate_gas`, using an explicit sender and block selector.
- Explicit-zone transaction and receipt lookup with typed Quai, external and Qi variants.
- Current-head indexed Qi outpoints, preserving denomination and lock values.
- Zone headers with location/height validation, immutable signed Quai submission and native receipt confirmation polling.

`BlockTag::Number` rejects values above the node's signed 64-bit range before I/O. This matters because go-quai can interpret a 66-character selector as a hash. Nonces, gas and transaction indices enforce their wire integer widths; account values and prices retain 256 bits. `Hash32` is a primitive, whereas `RpcData` distinguishes bytes from quantities and caps decoded data at 1 MiB. Collections have explicit limits: 65,536 entries for outpoints/inputs/outputs/logs and 4,096 access-list entries with 65,536 total storage keys.

`CallRequest::try_from(serde_json::Value)` is a strict adapter for external request objects. It rejects unknown fields, conflicting `data`/`input` usage, unsupported transaction kinds and Ethereum dynamic-fee fields. The request uses `input`, `gasPrice`, and numeric `txType: 0`; both simulation methods send an explicit second block-selector argument. Contract creation requires nonempty input. Same-zone and cross-zone account calls route by sender. Ordinary and profile-selected specialized Qi estimates and conversion APIs are separate typed methods; state overrides and block-hash simulation selectors remain outside this request model. Simulation does not submit anything, and the node may replace a supplied nonce with its state nonce.

The response parsers preserve top-level unknown fields in `Extensions`, whose debug output omits values. Known signatures and public keys are checked for structural encoding only; they are not cryptographically verified. Unknown transaction types, partial inclusion metadata, incorrect lookup hashes, malformed log associations and duplicate outpoints are rejected. Pending and missing transactions remain `Option` values; neither establishes rejection. Receipt outcomes distinguish status from historical post-state roots. Quai log blooms are **10,240 bytes**, as verified against both captured nodes and pinned source; Ethereum's 256-byte bloom assumption is wrong here. ETX receipts can report zero cumulative gas despite nonzero gas used.

`outpoints` operates at current head only. An empty result does not establish address-index readiness, and a returned outpoint does not establish key ownership, maturity, nonexpiry or spendability. Wallet recovery and reservation logic live in quai-wallet and the native quai-sdk sessions.

## Explicit submission and confirmation

`broadcast(&SignedQuaiTransaction)` checks the signed transaction's chain ID before any RPC, computes its expected ID and canonical protobuf before submission, then checks the endpoint chain at the recovered sender's zone. It submits exactly one `quai_sendRawTransaction` request and requires the returned ID to match. There is no automatic retry. Errors distinguish preflight from ambiguous submit-stage outcomes; every send-stage error preserves the expected transaction ID for reconciliation, including malformed or conflicting acknowledgements. Acknowledgement does not establish inclusion. Retain `signed.hash()` before awaiting: dropping the future cancels waiting but cannot roll back a possibly accepted transaction.

`wait_for_receipt(zone, hash, WaitConfig)` requires a positive confirmation count, timeout and poll interval. It tolerates missing or reorged receipt observations, verifies the containing block by canonical number/hash, re-reads the receipt, and verifies the observed head hash before returning. Failed execution receipts are returned with their explicit outcome. RPC failures stop the wait without automatic retry. The overall timeout covers cooperative RPC polling and delays; dropping the future stops polling. Separate RPC reads cannot eliminate races or prove finality, and chain-ID checks cannot authenticate a dishonest node. The native `polling` feature uses Tokio time; `http` enables it. Portable single-poll checks and bounded browser waiting are described below.

Broadcast and confirmation regressions use only mock transports. **No real transaction was submitted during this increment**, including to mainnet. Funded disposable-testnet acceptance remains a separate gate.

## Grouped current outpoint reads

`outpoints_many` accepts up to 1024 distinct Qi addresses. It groups by zone and
uses explicit batches of up to 32 addresses when the transport supports them,
with chain-ID checks included in each batch. Unsupported transports use at most
four concurrent individual reads. Failed batches are not replayed automatically.
The total result remains capped at 100,000 outpoints. Each response is a
latest-state observation; batching does not establish an atomic or historical
snapshot. Native SDK wallet refresh retains its surrounding head checks.

## Evidence and tests

The pinned candidate protocol reference is [go-quai f3f345c877300c044e3e0081a48bf3cf786fb9cc](https://github.com/dominant-strategies/go-quai/tree/f3f345c877300c044e3e0081a48bf3cf786fb9cc). Method/field review used:

- [Account simulation arguments](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/internal/quaiapi/transaction_args.go#L36).
- [Transaction RPC model](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/internal/quaiapi/api.go#L999), [nonce and transaction lookup](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/internal/quaiapi/api.go#L1442).
- [Code/storage/call/estimate methods](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/internal/quaiapi/quai_api.go#L827), [receipts](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/internal/quaiapi/quai_api.go#L1799), [indexed outpoints](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/internal/quaiapi/quai_api.go#L200), [bloom size](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/core/types/bloom9.go#L33).

Normal tests combine mock transports, malformed/adversarial responses and [captured public responses](https://github.com/mpoletiek/quai-rust-sdk/blob/main/crates/quai-provider/tests/fixtures/README.md). They neither contact a live node nor contain private keys. Run `cargo test -p quai-provider --locked`. No HTTP transport is needed for the mock/fixture suite; `--no-default-features` is supported.

An ignored `live_reads` test is opt-in and requires `QUAI_RPC_URL` plus decimal `QUAI_EXPECTED_CHAIN_ID`. It uses the exact supplied endpoint, reads public zero-address state and nonexistent transaction lookups, and simulates/estimates a zero-value account call. It does not sign or submit. Invoke with `cargo test -p quai-provider --test live_reads -- --ignored` after explicitly configuring those variables. This smoke test passed on both the authorized direct LAN node (chain 9) and Orchard's resolved Cyprus-1 gateway (chain 15000) on 2026-09-11. It does not establish funded transaction or Qi-signature acceptance.

## Access lists, pool inspection and current topology

`create_access_list` simulates the exact account call at an explicit selector,
retains ordered access entries and gas used, and rejects VM-level execution
errors. It never inserts a generated access list into a previously signed call.
`protocol_expansion` reads the reported expansion number at an explicit shard.
`running_regions` derives regions from actual advertised running zones; it avoids
the pinned JS active-region threshold inconsistency. `pending_header_bytes`
returns bounded protobuf bytes, retaining the distinction from header JSON.

`pool_status` reports account pending/queued and Qi counts. `pool_content` and
`pool_inspect` accept explicit entry budgets up to 4096. Account content validates
sender/nonce map keys, chain, zone, hashes and pending inclusion state. The pinned
node's account content/inspection RPCs exclude Qi, even though status counts it.
Inspection text is bounded and omitted from Debug. Pool absence cannot release
signed claims. The node must explicitly enable the `txpool` RPC module; missing
methods remain visible RPC errors and are never reported as empty pools.

Run the read-only `quai-sdk` example `inspect_pool` with `QUAI_RPC_URL` and
`QUAI_EXPECTED_CHAIN_ID`. It reports topology, pool counts and pending-header size
and performs a zero-value access-list simulation; it never signs or submits.

`MinedBlock::{Latest,Number,Hash}` selects a positive-height mined zone block.
`mined_block` returns full executed transactions with validated block identity,
location, ordered inclusions, uniqueness and chain IDs. `block_hashes` requests
only the ordered unique nonzero transaction hashes, saving response size; it
cannot validate transaction inclusion fields omitted by that response.
`header_by_hash` checks the exact requested hash and zone. Hash lookup can return
an orphan; compare with `header_at` when canonicality matters. Budgets are
explicit (1..4096 transactions); genesis uses the root genesis API, and pending
work retains its separate protobuf API. Null remains unavailable and node errors
propagate. These APIs do not treat emitted outbound ETXs as executions.
`quai-sdk --example inspect_blocks` exercises all three forms without submission.

Receipt status `2` is preserved as `ReceiptOutcome::Locked`. Conversion tracking
returns `ConversionEffect::Locked`; neither state establishes current maturity.
Qi credit observation also inspects canonical failed conversions, since the
pinned node may create some outputs before gas exhaustion produces status `0`.
Attribution remains bound to the signed beneficiary/refund and final ETX hash;
missing value is reported as unobserved. Unknown receipt statuses remain errors.

## Deployment observations

`DeploymentReference::from_signed` binds direct creation to its exact signed
hash, sender, nonce, full init code, expected zone and trusted genesis. It computes
the Quai CREATE address locally. `observe_deployment` checks receipt identity,
predicted contract and canonical inclusion, reads runtime code at that numeric
block, and rechecks both inclusion and the sampled head. An optional expected
runtime Keccak hash produces an explicit comparison; init code is not runtime
code. Empty code, failed/locked/legacy outcomes, missing receipts and noncanonical
inclusion remain distinct. Unknown or pruned state errors are propagated.

Native `wait_for_deployment` uses the existing explicit `WaitConfig` limits.
It tolerates missing/reorganized observations until the deadline and returns the
observed execution outcome at the requested depth, including failures. Source
errors stop the wait. Dropping the future stops polling without affecting the
signed transaction. This API handles known direct signed creations; existing
contract addresses can still be read through `code`. Neither a receipt, runtime
hash match nor a confirmation count proves contract safety or consensus finality.

`Log::address` and `LogFilter::addresses` use the general `Address` type. Native
redemption and lockup receipts can name Qi beneficiaries; these are protocol
logs, not EVM contracts. Parsing still rejects unsupported zones and inconsistent
receipt/log associations. Exact address/topic/range filters apply to both ledgers.
Contract event adapters additionally require their configured Quai emitter.

`wrapped_qi_deposit_optional` maps only the pinned node's exact absent-deposit
error (`-32000`, `no wrapped Qi balance`, no data) to `None`; successful zero stays
`Some(0)`. Other failures propagate. `wrapped_qi_deposit` retains its raw error
behavior. Neither API establishes historical indexing or contract verification.

### Verifying an account transaction lookup

`Transaction::verified_quai` reconstructs canonical signed type-0 protobuf from
reported fields and verifies the recovered sender and locally computed hash.
It enforces transaction/access-list budgets before cloning RPC metadata and
accepts the pinned Quai recovery values 0/1. The default RPC parser still only
validates structure; verification is explicit. A valid signature/hash does not
establish canonical inclusion, confirmation or success. Tests reconstruct a
retained mainnet transaction and reject changed fields, signatures and oversized
caller-built metadata.


`HeadTracker::export_state` and `HeadTracker::from_state` preserve the bounded
public ancestry across application restarts. The canonical QHEAD001 encoding
binds zone/genesis and replay bounds and rejects trailing bytes, oversized counts,
noncontiguous heights and repeated/zero hashes. It is at most 163,887 bytes.
Restoring checks the expected network; polling still revalidates canonical node
history. Persist the cursor atomically with application updates, or use the SDK's
native `reconcile_persisted_head_replay` integration. Saved headers are trusted-node
observations, not proof of finality or historical Qi outpoints.


## Portable receipt confirmation observations

`observe_receipt_confirmation(zone, hash, positive_depth)` performs one bounded
observation with any configured native or non-Send browser transport. It returns
`ReceiptConfirmation::Pending` with the last observed inclusion (or none), or a
boxed `ConfirmedReceipt`. The observation rechecks canonical block association,
the receipt and the observed head before success. It performs at most five typed
reads, each with a chain-ID check. Missing/reorged data stays pending, and RPC
errors return immediately. A confirmed execution failure retains its explicit
receipt outcome. Separate node reads and confirmation depth do not prove finality.

The native `wait_for_receipt` uses this same internal observation implementation
and retains its existing Tokio deadline, cancellation and error semantics.
Without `polling`, callers can schedule individual portable observations. Wasm
applications can use `quai_browser::wait_for_receipt` with bounded browser timers,
an overall observable monotonic deadline and an explicit maximum poll count.
That adapter does not require Tokio, submit transactions or release wallet claims.

Account nonce competitors can be observed with `observe_account_replacements`
from a trusted signed original and an explicit bounded block range. Matching RPC
transactions are signature/hash verified; canonical anchors and receipts are
rechecked. Native `wait_for_account_transaction` follows bounded pages under an
overall timeout. Results never adopt unknown candidates or release wallet claims.


`observe_transaction_confirmation` and native `wait_for_transaction` also handle
indexed Qi transactions without receipts. They recheck numbered block membership,
transaction position, refreshed fields and head under explicit depth/deadline
limits. `transaction_block` performs the inclusion/hash-position check directly.
These observations do not verify signatures. `Transaction::verified_qi` separately
reconstructs and verifies ordinary, conversion and wrapping signatures and IDs,
complementing `verified_quai`. No absence/timeout releases signed custody claims.
See [transaction responses](https://github.com/mpoletiek/quai-rust-sdk/blob/main/docs/TRANSACTION_RESPONSE_PARITY.md).

Network labels and caller-owned registries are explicit bounded metadata, separate from trusted genesis/endpoint identities. FeeData exposes exact optional gas-price metadata.
See the [utility guide](https://github.com/mpoletiek/quai-rust-sdk/blob/main/docs/UTILITY_PARITY.md).
