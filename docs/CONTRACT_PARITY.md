# Contract bindings, events and deployment parity

The published `quais@1.0.0-alpha.57` contract/factory APIs map to immutable ABI
bindings, explicit provider calls and native/browser account workflows. Rust
uses named methods and typed values instead of JavaScript Proxy properties,
optional runner methods, dynamically generated subclasses and trailing overrides.

| Published capability | Rust equivalent |
| --- | --- |
| Construction, `from`, `buildClass`, target/address/interface | `Contract::new`, reusable `AbiInterface`, `address`, `interface` |
| `attach`, `connect`, runner | `Contract::attach`, `connect`, explicit `provider` reference |
| Method/fragment lookup, population | `AbiInterface::function`, canonical signatures, `Contract::prepare`, `ContractCall` |
| Static call, result, estimation, send | `call`/`simulate`, ABI positional/named decoding, `Provider::estimate_gas` or account preflight, then explicit signing/submission |
| Fallback/receive population, simulation, estimation | `prepare_fallback`, `simulate_fallback`, `estimate_fallback`, `FallbackCall` |
| Event lookup and deferred/value filters | `AbiInterface::event`, `AbiEvent::encode_filter_topics`, `events_by_values` |
| Wildcard/filter queries | `Contract::query_logs` retains known, unknown and malformed logs |
| Event payload metadata and listener removal | `ContractLog`, `AbiEventValue`, caller-held emitter/filter context and `EventHub` listener IDs |
| Listener registration, once, emit, list/count, removal | Bounded local `EventHub`, composed with explicit provider subscriptions/queries and ABI decoding |
| Receipt event interpretation | `Contract::receipt_logs` or free `decode_receipt_logs` |
| Factory/artifact/constructor population | `SolidityArtifact`, ABI constructor encoding, `prepare_deployment`, `PreparedDeployment` |
| Address calculation/grinding | `contract_address`, bounded cancellable `prepare_deployment` |
| Deploy | Native `AccountSession` and browser `BrowserAccountSession::prepare_deployment`, then explicit sign/submit |
| Deployment transaction | Explicit `DeploymentReference` and durable signed-candidate records |
| Deployed code and known-transaction wait | `Provider::code`, `Contract::verify_deployment`, existing deployment observation/waits |
| Wait for code without a deployment hash | `ContractCodeTarget`, native/browser `wait_for_contract_code` |

## Raw fallback and receive calls

`prepare_fallback(RpcData, U256)` retains exact data and native value. Empty data
selects a declared receive entry point when present; otherwise a fallback is
required. Nonempty data requires a fallback. A selected nonpayable declaration
rejects nonzero value before I/O. ABI metadata does not prove the runtime path:
nonempty calldata can still select a function in the actual deployed contract.

`FallbackCall` keeps a validated recipient and ordered access list. Access-list
attachment validates consensus bounds without sorting, deduplicating or inserting
an implicit callee. Simulation and estimation revalidate the bound address,
sender zone and current ABI before I/O. Simulation returns raw bytes. With the
`wallet` feature, `into_account_intent` carries those reviewed fields into durable
native/browser preparation and signing; the contract helper itself does not send.

## Lossless event queries and explicit delivery

`ContractLog` owns its original log and represents `Decoded`, `Unrecognized` or
`Undecoded` results. Decoded values include a shared `Arc<AbiEvent>` declaration;
indexed compound/dynamic parameters remain topic hashes. Unknown, ambiguous and
malformed logs retain their exact data, association and removed status. A foreign
emitter is unrecognized by a bound decoder. Anonymous events require an explicitly
selected declaration through the existing `decode_event` API.

`query_logs` requires a numeric block range or block hash and an explicit result
limit in 1–65,536. Topics have at most four positions, each OR set has 1–128 values,
and numeric ranges use the provider's 10,000-block span limit. Invalid local
limits reject before I/O. The complete RPC response is still subject to transport
and provider limits: `max_logs` is not a streaming response-allocation limit.

A query shares event declarations across results and caps aggregate decoded JSON
at 65,536 nodes and 4 MiB of string/key bytes, with depth 64. Per-log decoding also
uses ABI bounds. Budget exhaustion returns an error instead of a partial result.
The existing strict `events` helper remains useful when a malformed matching event
should fail the operation.

Applications own event tasks and context: register bounded `EventHub` listeners,
feed decoded subscription/query results, apply removed logs, and remove listeners
by ID. Local registration does not automatically start RPC or reconnect work.
Explicit provider calls retrieve blocks, transactions and receipts from retained
identities; detached event metadata is not evidence of current canonical inclusion.

## Address-only code appearance waits

`ContractCodeTarget` requires a valid account address and nonzero trusted genesis;
an expected runtime-code hash is optional. Each observation uses the existing
rechecked genesis/head/code read. Empty code and changed sampled views remain
pending. Stable wrong genesis or a nonempty wrong runtime hash fails the wait.

Native and browser/worker `wait_for_contract_code` accept `CodeWaitConfig` with a
positive timeout up to `i32::MAX` milliseconds, a positive interval no greater
than timeout, and 1–100,000 completed polls. The overall deadline includes RPCs
and delays. Provider failures stop immediately; dropping the future drops active
reads. Browser suspension can delay timeout delivery, but observed expiry cannot
return success. No deployment transaction hash, receipt or broadcast is required.
Code presence is a sampled node observation, not confirmation depth, finality,
proxy implementation verification or permission to release wallet claims.

## Documented source differences

The pinned Interface parser lets a later nonpayable fallback overwrite an earlier
receive flag. Rust keeps receive behavior independent of declaration order. The
24-case fixture records one intentional difference and the native/worker tests
check reversed ABI order as well.

Published `queryTransaction` unconditionally throws `@TODO`. Rust provides receipt
lookup and lossless ABI decoding as explicit operations. The factory's `IPFSHash`
length check does not publish an artifact or include its value in deployment
bytes; Rust does not reproduce that unused gate. Published factory `connect`
also drops that metadata. Rust accepts immutable caller-supplied artifacts.

Published grinding can return its last unsuccessful address candidate after
10,000 attempts. Rust rejects exhaustion and supports cancellation. Exact init
bytes preserve leading zeroes; the existing CREATE fixtures document the source
helper's byte-coercion defect. Deployment preparation retains required access
lists, exact nonce and fees. Submission acknowledgment and subsequent code/receipt
observations remain separate phases.

## Evidence

[Five source tests](../compatibility/scripts/contract-io.test.mjs) cover the
24-case receive/fallback matrix, raw simulation/estimation and bindings, wildcard
logs, source ordering defect, TODO query and code waiting without a deployment
hash. [Four shared native/worker tests](../crates/quai-sdk/tests/contract_io.rs)
cover exact fields, pre-I/O rejections, lossless logs, shared declarations and
aggregate decoded-node/string limits. [Six shared code tests](../crates/quai-sdk/tests/contract_code.rs)
include stable genesis/runtime identity, empty/changed observations, deadline,
poll exhaustion, provider errors and actual read cancellation. One funded/live
code qualification test remains ignored in the offline suite.

Existing contract, event, deployment, receipt and local-event tests provide
regression evidence. The sanitizer ABI target now exercises contract fallback
preparation and owned log decoding directly with public seeds. It does not claim
to fuzz asynchronous RPC scheduling or query aggregation, which have deterministic
tests. No new funded-network acceptance is claimed by this batch.
