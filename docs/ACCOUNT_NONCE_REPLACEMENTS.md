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

Browser callers use the portable observer with explicit page/timer policy. There
is no browser helper that automatically scans successive pages for an unknown
competitor. Hash-only receipt polling and registered-family reconciliation remain
separate APIs and must not be described as unknown-replacement discovery.

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
