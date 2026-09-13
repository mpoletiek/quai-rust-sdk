# Blocks, receipts and logs

The Rust provider returns detached typed observations. Application-owned providers
perform subsequent reads explicitly; no response object hides a connection, retry
loop or finality claim. Unknown top-level transaction, receipt and log fields remain
in `extensions`; remaining block fields remain in the block's metadata map.

## Blocks

`Provider::mined_block(zone, MinedBlock::{Latest, Number, Hash}, budget)` returns a
`TransactionBlock` with executed transactions in exact inclusion-index order.
`block_hashes` requests only executed hashes. The caller budget is 1–4,096 items.
These APIs require positive mined zone heights; genesis identity uses
`Provider::genesis_hash`, and pending blocks do not masquerade as mined blocks.
A hash lookup can return an orphan and does not prove canonicality.

`BlockTransactionId::{Index, Hash}` selects exact entries. Full blocks use
`transaction(id)` to borrow a prefetched transaction. Hash blocks provide
`transaction_hash(id)` and an explicit asynchronous `transaction(provider, id)`;
the latter checks the returned block hash, number and index. Missing entries or
unindexed transactions return `None`. They never return a neighboring transaction.

`metadata()` borrows a `BlockMetadata` view:

| Metadata | API and representation |
| --- | --- |
| Consensus and work object headers | `header()` and `work_header()` return optional raw maps; malformed objects return errors |
| Date | `timestamp_seconds()` returns exact U256 Unix seconds; application date libraries handle their own supported ranges |
| Size and entropy | `size()` and `total_entropy()` preserve U256 quantities |
| Interlinks and subordinate manifest | `interlink_hashes()` and `sub_manifest()` parse bounded hash arrays, preserving order |
| Uncles and work shares | `uncles()` and `work_shares()` return bounded raw values without fetching or claiming header verification; conflicting `workshares`/`workShares` aliases reject |
| Outbound ETXs | `outbound_etxs(budget)` returns distinct `Hash` or `Prefetched` external items, rejecting zero/duplicate identities, nonexternal objects and budget violations |

Outbound ETXs are not transactions executed in this block. Their source-reported
inclusion is retained without interpreting it as destination execution. Select an
outbound item by vector index or `.iter().find(|item| item.hash() == hash)`; a bare
hash remains unavailable as a full transaction until explicitly fetched from an
appropriate source. Missing metadata differs from an empty list or malformed data.
Metadata lists are bounded at 8,192 entries; outbound lists have the tighter caller
budget. The source may still omit history or supply inconsistent claims.

Both block representations export `to_rpc_json()`, preserving the block metadata
and rechecking identity/inclusion before returning normalized node JSON. No JS
property renaming, time conversion or automatic metadata verification occurs.

## Transactions, receipts and logs

`Transaction`, `Receipt` and `Log` have explicit `TryFrom<serde_json::Value>` parsing
and `to_rpc_json()` export. Exports preserve exact hexadecimal quantities and
leading-zero byte data, top-level extensions, ordered access lists, Qi input/output
fields, receipt logs and outbound ETXs. Public field mutations are revalidated.
They normalize optional fields and inactive ETX nonce to zero; they do not promise
identical original JSON spelling or preserve unknown nested fields discarded by
existing typed parsers. Exporting an observation is not transaction signing.
`Transaction::verified_quai` separately reconstructs and verifies account signatures.

| quais.js receipt/log field or helper | Rust equivalent |
| --- | --- |
| Hash, block hash/number/index | `Receipt::transaction_hash`, `Log::transaction_hash` and `inclusion` |
| Sender, recipient, contract address | Receipt `from`, `to`, `contract_address` with explicit optionality and ledger validation |
| Type/status | `TransactionKind` and `ReceiptOutcome::{Failed, Succeeded, Locked, PostState}` |
| Gas and fee | `gas_used`, `cumulative_gas_used`, `effective_gas_price`; `Receipt::fee()` uses checked U256 multiplication in Its |
| Logs/bloom | `logs`, `logs_bloom`; strict 10,240-byte Quai bloom and receipt/log association checks |
| ETX metadata | `outbound_etxs`, `originating_tx_hash`, `etx_type` with typed variant checks |
| Log address/topics/data/index/removal | `address`, `topics`, `data`, `log_index`, `removed`; removal remains a source claim |
| Iteration and length | Standard slice/vector `.iter()` and `.len()`; iteration preserves node order |
| `getBlock`, `getTransaction`, `getTransactionReceipt` | Explicit provider mined-block, transaction and receipt reads using retained identity and chosen zone |
| `confirmations` | `observe_receipt_confirmation` or native/browser receipt waiters; recheck canonical block association and observed head |
| Orphan, removed or reordered events | `HeadTracker`/`HeadUpdate` report removed blocks before added blocks; applications match stored inclusions and reobserve receipts/logs. JS-style event-filter descriptors are not emitted automatically |
| `toJSON` | Normalized `to_rpc_json()`, with full node metadata instead of the JS display-object shape |

Fee calculation is execution gas debit, excluding value transfers or downstream
ETX effects. An overflow is an error. Confirmed execution failures remain explicit
outcomes. Canonical observations and replay are not cryptographic finality proofs.
The published receipt's `getResult()` delegates to `getTransactionResult`, but the
published JSON-RPC backend has no mapping and throws `UNSUPPORTED_OPERATION`.
Rust does not fabricate archive execution results. A future trace backend would
need separate node/API qualification.

## ABI receipt log interpretation

With facade feature `abi`, `contracts::decode_receipt_logs(&interface, &receipt,
max_logs)` returns every original log in order as `ReceiptLog`:

- `Decoded { log, event, values }` retains the ABI declaration and positional
  values. Indexed dynamic and compound arguments stay hashes. The declaration
  exposes its name, signature, input types and indexed flags.
- `Unrecognized(log)` retains an unknown topic or a log with no identifiable
  nonanonymous event.
- `Undecoded { log, error }` retains malformed matching data or ambiguous ABI
  interpretation without losing the original log.

The free function considers every emitter, matching published contract receipt
behavior. `Contract::receipt_logs` restricts decoding to the bound address while
retaining foreign-emitter logs as unrecognized. Anonymous events require explicit
`Contract::decode_event`. The whole-receipt budget is 1–65,536 logs; over-budget
input rejects instead of silently truncating. The view borrows the receipt and ABI
rather than cloning provider-bearing class objects.

## Reference findings and qualification

The published `Block.getTransaction(hash)` and `getExtTransaction(hash)` invert
one equality condition for prefetched objects: they skip the matching item and
can return a different item, including for an unknown hash. Rust exact lookups do
not reproduce this bug. `getPrefetchedTransaction(hash)` in the same JS class
compares correctly. JS receipt confirmations subtract reported heights without
canonical block validation; Rust's existing observer rechecks association and head.

[Four published-source regressions](../compatibility/scripts/response-views.test.mjs)
record these behaviors and ABI receipt handling. [Shared response tests](../crates/quai-sdk/tests/response_views.rs)
use retained mainnet/Orchard records and explicitly synthetic Qi DTOs;
[event tests](../crates/quai-sdk/tests/events.rs) exercise decoded/unknown/malformed/
foreign logs. [Provider block tests](../crates/quai-provider/tests/blocks.rs),
[receipt tests](../crates/quai-provider/tests/wallet_reads.rs),
[confirmation tests](../crates/quai-provider/tests/confirmation.rs) and
[head replay tests](../crates/quai-provider/tests/head_tracker.rs) cover the composed
read/reorg behavior. These are deterministic tests and retained observations,
not new funded acceptance or independently verified consensus execution.
