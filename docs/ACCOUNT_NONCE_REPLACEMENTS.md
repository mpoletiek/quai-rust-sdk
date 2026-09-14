# Discovering mined account replacements

`Provider::observe_account_replacements(&signed_original, trusted_genesis, request)`
finds mined transactions with the original sender and nonce, including candidates
that were never registered in a wallet journal. It works with native and browser
transports. The original is a validated `SignedQuaiTransaction`; chain, origin zone
and trusted genesis are explicit. No keys are needed.

`AccountReplacementScanRequest` bounds a page to 256 positive block heights,
4,096 executed transactions per block and 65,536 total. It specifies an inclusive
range at or below the observed head and an optional preceding page anchor. The
observer verifies full-block inclusion, parent links and every matching signature,
recovered sender and local transaction hash. It re-reads indexed receipts and
rechecks all scanned canonical anchors, the preceding anchor, head and genesis.
Two claimed canonical occupants reject. This is evidence from an RPC source,
not authenticated finality or an atomic chain snapshot.

`AccountReplacementScan` returns a verified candidate, contiguous coverage end,
first unavailable block and observed head. A missing block stops coverage.
No candidate only describes the scanned prefix; it never means dropped, cancelled
or safe to reuse. A candidate can have a missing receipt. Its confirmation count
includes the origin block; a failed receipt still consumes the account nonce.

Classification follows the published quais.js account waiter, in this order:

| Result | Meaning |
| --- | --- |
| No replacement reason | Exact original transaction identity |
| `Repriced` | Same recipient, value and input data; gas/access-list differences are not compared |
| `Cancelled` | Otherwise, empty-data zero-value transfer to the sender |
| `Replaced` | Any other transaction with the same sender and nonce |

These labels describe signed transactions. They do not authorize their adoption,
confirm an application's intent, or permit releasing custody claims. Registered
fee replacements remain the stricter workflow in [replacement preparation](BROWSER_REPLACEMENTS.md).

With native `polling`, `wait_for_account_transaction(original, genesis,
start_block, WaitConfig)` scans successive bounded pages until an indexed original
or competitor reaches the required observed depth. The overall timeout includes
RPC calls and delays. Missing history does not advance coverage. Dropping the
future stops polling; errors and changed anchors return immediately. Restart from
an explicitly chosen trusted height after a reorg. The cursor exists only within
that future; no claim is released and no candidate is added to wallet storage.

On Wasm, `quai_browser::wait_for_account_transaction(&provider, original,
genesis, start_block, BrowserWaitConfig)` follows the same pages using
window/worker monotonic timers. `max_polls` counts completed pages and idle
observations; available pages drain without a polling delay. The total deadline
also covers stalled reads. Dropping the future clears its timers and active read.
Browser suspension can delay delivery, but observed expiry prevents success.
`BrowserAccountWaitError` aliases the shared structured browser wait error.

Both waiters use the portable `AccountReplacementTracker`. Applications with
another executor can construct this tracker and call `poll(&provider)` under
their own deadline. Each poll returns `Pending { inclusion, more_available }`
or `Confirmed(candidate)`, checks genesis and the prior page even if the head
has retreated, and advances only after a complete successful page. Failed or
cancelled polls retain the cursor. It is an in-memory cursor, not a serializable
chain snapshot: after a reorg, reconstruct it from an explicitly trusted start.
Hash-only receipt polling and registered-family reconciliation remain separate APIs.

## Combined wallet reconciliation

`AccountSession::observe_nonce(id, request)` reconciles a durable family first and,
only when no registered candidate is canonical, scans one bounded page. It returns
`Registered(hash)`, `Unregistered(candidate)` or `Unresolved { scanned_through,
missing_block }`. A scan occupant that is itself a durable candidate contradicts the
family read and returns `ObservationChanged`. The claim stays held in every case.
On mainnet (2026-09-14) a same-key cancellation signed outside the store at nonce 13
was reported as `Unregistered` with reason `Cancelled`; see the
[mainnet checks](../test-infra/orchard/mainnet-checks-2026-09-14.json).

## Published-reference comparison

The executable [reference tests](../compatibility/scripts/account-replacement.test.mjs)
exercise `quais@1.0.0-alpha.57` offline. `QuaiTransactionResponse` is a type-only
root export; its runtime implementation is the superclass of the public
`ContractTransactionResponse`. Its waiter scans full blocks for a sender/nonce
competitor and reports `TRANSACTION_REPLACED` with the three classifications.
Its zero-confirmation wait returns an absent receipt without a scan.

Rust returns a typed candidate, preserves failed execution explicitly and requires
positive depth for waits. A one-shot receipt lookup supplies the zero-depth use
case. Bounds, signature verification, genesis and canonical rechecks are stronger
than the reference waiter. Rust does not reproduce the pinned `getTransaction`
Promise/instanceof bug: the reference checks a Promise before awaiting it, so an
ordinary asynchronous provider result becomes null. Rust's typed transaction read
awaits the response and validates the requested identity.

[Native tests](../crates/quai-provider/tests/account_replacement.rs) cover all
classifications, original inclusion, failed/missing receipts, incomplete history,
forged signatures, receipt mismatch, reorgs, competing occupants, resource limits,
multi-page waiting, timeout and cancellation. These are signed public-toy fixtures,
not funded node replacement-policy acceptance.

[Chromium worker tests](../crates/quai-browser/tests/account_wait.rs) exercise
unknown classifications, failed execution, successive pages, poll budgets,
forgery/reorg rejection, stalled reads, cancellation and no-I/O configuration
rejection. The shared timer loop also retains the existing receipt-wait tests.
