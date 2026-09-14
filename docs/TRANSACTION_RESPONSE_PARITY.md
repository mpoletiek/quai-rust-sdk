# Transaction response and Qi confirmation parity

The published `quais@1.0.0-alpha.57` Quai/Qi/contract transaction response classes
map to typed provider observations, explicit provider calls and bounded waits.
RPC responses remain node claims. Parsing, signature verification and canonical
inclusion observation are separate operations.

| Published response operation | Rust equivalent |
| --- | --- |
| Constructor, fields and `isMined` | `Transaction`, `TransactionDetails`, complete optional `Inclusion` |
| `hash`, `blockHash`, `blockNumber`, `index` | `hash`, `inclusion.block_hash`, `block_number`, `transaction_index` |
| `chainId`, `nonce`, gas/value/access list, signature | Exact typed Quai/Qi fields; ETX fields use a separate variant |
| `data`, `txInputs`, `txOutputs` | `input`, ordered Qi `inputs`/`outputs`, fixed denominations and optional locks |
| `getTransaction`, `provider` | Explicit `Provider::transaction(zone, hash)`; application owns the provider |
| `getBlock` | `Provider::transaction_block` verifies reported hash and transaction position against a numbered block |
| `confirmations` | `observe_transaction_confirmation`, with positive depth and rechecked block/head association |
| Quai/contract `wait` | Existing receipt/account-replacement waits, then optional `Contract::receipt_logs` decoding |
| Qi `wait` | Native/browser `wait_for_transaction`; no receipt is required |
| `replaceableTransaction` | Account replacement tracker and durable candidate families; Qi families retain explicitly reviewed signed alternatives |
| `removedEvent`, `reorderedEvent` | Explicit inclusion records, `HeadUpdate` removed/added blocks, block transaction positions and bounded `EventHub` delivery |
| `toJSON` | `Transaction::to_rpc_json` emits exact normalized node JSON with typed quantities and retained top-level extensions |

## Receipt-independent confirmations

`observe_transaction_confirmation(zone, hash, depth)` accepts a positive depth
and nonzero hash. It performs at most five typed reads: transaction, current head,
numbered inclusion block, transaction again, and numbered head again. Reads check
the provider's configured chain ID. The inclusion block must have the claimed
hash and the transaction hash at the exact reported index; at most 4,096 executed
hashes are accepted. Unknown metadata extensions may change, but transaction
fields, input bytes and inclusion must agree between reads.

Missing/pending transactions, insufficient depth, changed blocks/positions,
changed transaction fields and changed heads return `Pending` with the last
reported inclusion. Malformed data and RPC failures return errors. This is a
sampled node view, not authenticated finality or a guarantee against a later
reorg. It does not require an account-style receipt and does not verify signatures.
Execution outcome, input spendability, cross-zone settlement and replacement
adoption remain separate observations or wallet policies.

Native `Provider::wait_for_transaction` uses `WaitConfig` and an overall deadline
covering RPCs and delays. Browser `wait_for_transaction` uses the existing
`BrowserWaitConfig` monotonic deadline, positive interval and independent
completed-poll budget. It works in windows and workers without a Send requirement.
Browser suspension may delay timeout delivery, but observed expiry forbids success.
Dropping either future stops polling and drops the active read; no signed
transaction is cancelled, retried, adopted or released. Source errors stop the
wait. A zero-confirmation one-shot lookup is expressed directly with
`Provider::transaction`; wait configurations require positive confirmation depth.

## Explicit Qi RPC verification

`Transaction::verified_qi` complements `verified_quai`. It reconstructs exact
consensus fields, validates public keys, retains input order/aggregation
participants and checks the Schnorr signature and locally computed transaction ID.
Data length selects the supported transfer/wrapping/conversion builder (0/20/22).
Invalid denominations, nonzero user-created output locks, unsupported data,
changed inputs/chain identity and invalid signatures reject. Null/zero output
locks normalize to the canonical empty user-transaction lock.

The result is an immutable `SignedQiOperation`. Its signature does not establish
that inputs exist, their values, their maturity, adequate fees, historical inclusion
or node acceptance. RPC inclusion remains a separate claim. Native and worker
tests verify fourteen supported Qi fixture signatures across ordinary, multi-input,
conversion and wrapping cases, including repeated ordered signing keys.

## Differences and source defects

The response types are detached data; a provider is passed explicitly rather
than stored in each response. Rust exports normalized node JSON, not the source's
display object with decimal fields and the mistaken `_type: TransactionReceipt`
marker. ETX sender/origin fields are retained in their typed variant instead of
pretending an ETX carries a locally verifiable account signature. The published
Quai response constructor does not assign its declared `sender` and
`originatingTxHash` properties.

Both published response `getTransaction` methods compare an unresolved Promise
with a class using `instanceof`; normal async providers therefore produce null.
Stored inclusion is used for confirmation arithmetic without canonical checks,
and can produce negative counts after a head change. Rust awaits the provider and
rechecks inclusion, block position, transaction fields and head. The Qi class is
present in the published implementation/declarations but is missing from the root
runtime export; source tests import it from the published provider implementation.

Published Qi `replaceableTransaction` clones and stores a starting height; its
wait path does not use that height to discover competitors. Rust does not claim
automatic unregistered Qi replacement discovery. Explicit reviewed candidate
families remain available. Orphan/reorder filter objects are bookkeeping,
not finality signals: Rust applications apply bounded head updates and retain
transaction positions, then deliver their chosen typed notifications. Local event
registration does not implicitly start RPC/subscription tasks or release claims.

## Evidence

[Four source checks](../compatibility/scripts/transaction-responses.test.mjs)
exercise refresh, arithmetic, Qi waiting, metadata exports, inherited contract
response fields and listener removal. [Six shared native/worker tests](../crates/quai-sdk/tests/transaction_confirmation.rs)
cover missing/pending/deep inclusion, changed block/position/payload/head, actual
read cancellation, deadlines, provider errors, browser poll exhaustion and
fourteen independent signing vectors. Four tests also run without SDK default
features. Existing receipt, replacement, block and response-export tests remain
regression evidence. No funded transaction or production finality is claimed.
