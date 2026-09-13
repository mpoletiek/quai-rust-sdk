# Qi address inventory and cached gap views

`quai_wallet::qi_addresses::QiAddressBook` supplies scoped, public lookup and usage
views on native and Wasm targets. It holds at most 4,096 addresses for one explicit
chain, genesis and zone, with no eviction. It stores no private keys, coins,
allocation cursors or spend claims. Its contents are an in-memory cache; rebuild
from verified origins and durable journals after restart.

Register known addresses with `import_hd(&account, change, raw_index)`,
`import_public(&public_key)` or `import_payment_receive(&owner, &peer, raw_index)`.
The latter verifies the BIP47 receiving public key using the owner, retaining only
public metadata. It requires the wallet's `payments` feature. Imported metadata
must match the book's Qi ledger and zone. Identical registration preserves prior
status; conflicting ancestry rejects. These calls do not allocate or authorize
exposing an address anew: use the durable address/payment journals for that.

| Lookup | Contents |
| --- | --- |
| `addresses()` / `address(qi_address)` | All known public records / exact typed address |
| `branch(false)` / `branch(true)` | External / change BIP44 records |
| `account(account)` | Exact HD and payment account records; direct imports have no invented account |
| `imported()` | Direct public-key imports, excluding verified channel origins |
| `payment_channel(&peer)` | Verified channel receive records, across explicit accounts in this zone |
| `gap_branch(change)` / `gap_payment_channel(&peer)` | The corresponding records whose cached status is `Unused` |

Each immutable `QiAddressRecord` exposes `public()`, `origin()`, `status()` and
`checkpoint()`. These views cover the reference's `getAddressInfo`, origin getters
and three cached gap getters, with typed origins and explicit scope.

## Refresh and status

`quai_sdk::discovery::refresh_qi_address_book(&provider, &mut book,
max_outpoints, cancelled).await` queries all registered addresses, validates fixed
denominations and rejects duplicate outputs across the whole refresh. The output
budget must be 1 through 100,000. It checks chain/genesis and the latest and
numbered head before applying a complete batch. A failed query, changed head,
callback error, budget error, cancellation or dropped future leaves the previous
view intact. Use an outer deadline to bound stalled RPCs.

`refresh_qi_address_book_with_use_checker` accepts a caller-owned async usage hint
and invokes it only for addresses with empty current outputs. No `Send` bound is
required for that callback, including in browser workers. Callers must bound its
resources. A positive hint marks usage without creating coins or certifying
historical coverage. Latest-only RPC reads remain separate source observations,
even when the final head agrees.

| `QiAddressStatus` | Meaning |
| --- | --- |
| `Unknown` | No retained current observation; the initial state for every origin |
| `Unused` | Explicitly observed empty with no positive hint or retained usage |
| `Used` | Positive output/use-hint evidence was recorded |
| `AttemptedUse` | Explicit local `mark_attempted` call; no execution or inclusion is inferred |

Empty refreshes preserve prior `Used` or `AttemptedUse`; `mark_attempted` does not
downgrade `Used`. `invalidate_observations()` clears statuses and checkpoints to
`Unknown` while keeping public origins. Use it after reorg/restart before accepting
a lower or conflicting checkpoint. It cannot access or rewind durable allocation
floors or claims. Empty current outputs never prove that an address was never used.

For another trusted observer, `record_observations(scope, checkpoint, batch)`
updates usage atomically. It rejects unknown or duplicate addresses, foreign scope,
zero block hashes, oversized batches, backward heights and conflicting hashes at
the same height. This method checks consistency, not chain canonicality; the
caller supplies that evidence.

The pinned JavaScript SDK initially calls imported private-key addresses unused
without a query. Rust initially uses `Unknown` for all origins. Rust's cache also
keeps attempted use until explicit invalidation. Direct imports remain separate
from BIP47 account origins and contain no secret `derivationPath` strings.

[Native and Chromium-worker tests](../crates/quai-sdk/tests/qi_address_book.rs)
cover origin filters, exact payment ownership, idempotence, status transitions,
atomic rejection, use hints, duplicate/budget errors, reorgs, partial failures and
cancellation. These are public-toy mock RPC observations, not funded node tests.
