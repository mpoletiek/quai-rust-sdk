# Bounded discovery and durable address allocation

`discovery::discover` scans a trusted account xpub on both receive and change
branches for coin type Quai 994 or Qi 969. It needs an `ObservationSource` adapter;
this crate does not supply a production node adapter or claim a complete recovery
service. HD private keys, seeds and mnemonic phrases never enter the observation
API. Native `Mnemonic::generate(language, word_count)` separately uses fresh OS
entropy through getrandom, zeroizes its temporary buffer, and accepts only the
five BIP39 word counts. Browser entropy generation is not enabled.

## History and coverage

Each source explicitly advertises `CurrentStateOnly` or `HistoricalEverUsed` for
the network and ledger. Historical coverage requires an `ever_used` value for
every response, including fully spent addresses. Quai responses require balance
and account nonce; Qi responses contain current outpoints. A zero balance or empty
outpoint list is not evidence that an address has never been used.

This follows the distinction in [BIP44 account discovery](https://github.com/bitcoin/bips/blob/master/bip-0044.mediawiki#account-discovery):
recovery depends on transaction history, and gap stopping assumes wallet usage
respected the gap. Quai zone/ledger grinding additionally skips raw child indexes.
The implementation records exact half-open skipped-index intervals and actual
child indexes, so counts of matching addresses cannot be mistaken for BIP32 paths.

Both branches have explicit raw `[start, end)` bounds. Optional gap stopping counts
consecutive matching addresses, using historical usage when available and only
current activity otherwise. `gap_limit: None` scans the whole explicit interval.
With a current-only source, a run of fully spent addresses can trigger the gap
before a later funded address. Use explicit ranges to inspect later addresses;
even those ranges cannot establish unobserved history or recover funds outside
the specified account/ranges. Historical usage also cannot prove no addresses
exist after an arbitrarily large unused gap. Reports intentionally have no
`complete recovery` flag.

Reports retain history capability, `SourceClaim` trust, branch bounds, stop reason,
continuation index, skipped intervals, observations and a canonical-view recheck.
They distinguish range end, gap heuristic, address limit and cancellation. Each
branch permits at most 1,000,000 raw indexes; global matching-address and current
UTXO limits are at most 100,000. Account enumeration is explicit: scan each desired
account xpub and do not infer unbounded account recovery from one account.

The source must return the exact scope, address and checkpoint requested. Every
response is checked, including UTXO ownership, nonzero creating hashes, destination zones and duplicate
outpoints. Remote `reserved` flags are discarded. `classify_coin` combines the
candidate block height with locally recovered reservation state to expose
independent locked, expired and reserved flags plus spendability.

A source must actually support checkpoint-consistent observations. It must not
label latest-only balance/outpoint RPC results as historical block responses.
Native observation futures require `Send`; wasm observation futures may retain
thread-local browser state. The facade supplies Fetch-backed browser/worker sources with actual Chromium
coverage; other browser engines and real-provider qualification remain separate. Transport adapters own response-size limits,
deadlines and in-flight cancellation.
Discovery checks cancellation during grinding and around awaited observations;
cancelling an already running source request requires that source's cooperation.
An initial cancellation returns an error before requesting a tip; later cancellation
returns a partial report with no canonical check. Source errors fail closed.

Addresses are derived and read in windows. A window holds at most the number of
addresses the gap rule guarantees the branch will examine whatever the answers
(`GapCounter::guaranteed_remaining`), capped by the address budget and
`MAX_SCAN_WINDOW` (64). The source receives each window through
`ObservationSource::observe_many`, whose default calls `observe` per address;
results are consumed in derivation order and a response of the wrong length is
rejected. A scan that completes therefore queries exactly the addresses a
one-at-a-time scan would, and nothing past the gap stop. A scan aborted partway
through a window, by an error, a budget or cancellation after the read, has
still disclosed the rest of that window: at most 63 addresses that a retry
would query anyway. The Qi scanners `scan_qi` and `discover_qi` follow the same
rule through `outpoints_many`.

After scanning, the source is asked for the canonical block at the same height.
A changed/missing block marks the report inconsistent. A matching block is still
only the source's view, not a consensus proof or finality guarantee.

## Atomic persistence and reorg invalidation

With `sqlite`, `SqliteStore::commit_discovery(expected_generation, reports)` commits
public metadata, current Qi coins and their shared checkpoint in one transaction.
Reports must agree on scope/checkpoint, have a matching canonical recheck and not
be cancelled. The batch must include an observation for **every address already
stored in that scope**, including other accounts/branches. Partial account scans
cannot silently advance the checkpoint of unobserved old coins. Batch bounds,
immutable key origins and exact xpub derivations are checked again. A generation
compare-and-swap rejects concurrent or stale snapshots. All reservations survive.

`reconcile_checkpoint` compares a caller-observed canonical block at the stored
checkpoint's height. A different hash or unavailable hash clears checkpoint and
UTXOs and increments generation atomically, preserving signed claims and nonce
cursors. It does not release signed/ambiguous operations. The caller must invoke
this when its source reports a reorg or cannot establish the stored checkpoint;
it is an explicit recovery primitive, not a background reorg monitor. Higher-height
ancestry verification and block undo/replay journals remain unimplemented.

## Fresh receive/change addresses

`SqliteStore::allocate_address(account, change, max_attempts, cancellation)` first
commits a raw derivation range under SQLite's writer lock. It then performs public
zone/ledger grinding outside the transaction and commits the selected public
metadata plus snapshot invalidation before returning the address. Competing
connections/processes cannot allocate the same raw range. Receive/change have
separate cursors, bound to the same account xpub; another xpub cannot replace an
already bound coin/account namespace.

`next_derivation_index` exposes durable burned-range bounds for recovery planning.
Cancellation, failure or process death before address exposure leaves the entire
reserved range consumed. A successful allocation gives back the range past its
address unless the store changed meanwhile, so consecutive allocations do
not skip matching addresses a gap-limited restore would need. A returned
`AllocatedAddress` includes both its exact origin and the burned interval. Bounds
are 1 through 100,000 attempts; exhausting a range returns an error and still burns
it. There is no cursor rewind/reuse API. Imported and discovery metadata is
conservatively treated as exposed when establishing/advancing these cursors.

This conservative range reservation can create large unused gaps. Persist the
public origin metadata and allocation state; a mnemonic-only restore using a
small gap heuristic is therefore insufficient. Recovery must scan explicit ranges
that include prior allocation bounds (and may need additional history). Encrypted
QUAISEED backups exclude these cursors. Native QUAIWALT backups preserve them
and public ownership metadata while deliberately invalidating scan checkpoints
and UTXO snapshots on restore; see [FULL_BACKUP_FORMAT.md](FULL_BACKUP_FORMAT.md).

Tests cover fully spent runs longer than the gap with later funded addresses,
explicit-range recovery, unavailable history, both ledgers/branches, exact skipped
indexes, cancellation, inconsistent checkpoints, locked/reserved/expired coins,
atomic discovery CAS, restart recovery, separate-process allocation collisions,
and conservative reorg invalidation preserving signed claims.

## Portable durable HD address allocation

`allocation::AddressAllocationBook` binds one trusted account xpub, coin/account
metadata, network and zone. It tracks caller-supplied 128-bit allocation IDs,
receive/change raw cursors and pending/completed/abandoned ranges. Reserve consumes
1–100,000 indexes (clamped at `2^31`); skipped and unexamined indexes stay consumed.
IDs cannot be reused, including after abandonment. Completion checks the exact
child, ledger/zone and range; the same ID/index is idempotent, a different index
is rejected. Completed addresses cannot be abandoned or released.

The book is an in-memory state machine. Native `SqliteStore::allocate_address`
continues to provide its existing synchronous durable workflow. Browser callers
use the facade's `browser_addresses::BrowserAddressBook` (`wallet,browser`):

1. Open the exact network/account namespace and explicitly initialize prior raw
   receive/change floors, or use `initialize_from_backup` with `backup` enabled.
2. Call `allocate(id, change, max_attempts, cancelled)`. The range commits through
   IndexedDB CAS before search, and the selected address commits before return.
3. After cancellation, exhaustion, a revision conflict or a lost response, inspect
   the retained ID and explicitly `resume` or `abandon` that request. A new request
   uses a new ID. No cursor or request ID is reclaimed.

Initialization only accepts a never-written namespace. Existing state and
tombstones cannot silently reset to zero. An authenticated backup can supply
floors above all matching stored cursors and previously exposed HD children;
the account must match a retained private origin. Earlier addresses remain in the
backup inventory; the new journal tracks subsequent requests. An empty current
UTXO scan is not evidence for zero floors. Each account/zone has its own namespace;
there is no value-based account selection or React Native crypto bridge.

The public `QADDRBK1` codec checks magic, exact scope/account identity, sorted
unique IDs, complete nonoverlapping coverage from each initial floor to its next
cursor, exact lengths and all completed child derivations. The header contains
magic (8), network (32-byte big-endian chain ID, 32-byte genesis, one-byte zone),
32-byte account identity, four big-endian u32 values (initial receive/change and
next receive/change), and a big-endian u16 record count. Each record contains a
16-byte ID, one-byte branch (0/1), start/end u32, status (0 pending, 1 completed,
2 abandoned), and a completed child's u32 index only for status 1.
Account identity is Keccak of UTF-8 `quai-rust/address-allocation/v1`, big-endian
u32 coin/account, and the canonical account-xpub ASCII bytes.

The fixed budget is 4,096 retained IDs and 123,003 encoded bytes per book. Capacity
exhaustion fails closed; no automatic compaction drops old IDs/ranges. Bytes are
public metadata, not encrypted or a proof of freshness; the selected backend's
revision checks prevent stale cooperating writers from replacing current state.
Actual worker tests cover two independent connections, restart, completed-request
idempotency, cancellation after write dispatch, malformed state, tombstones and
backup floors. Twenty-four independently encoded Node fixtures use pinned HD
account/address pairs; this Rust journal has no claimed quais.js format counterpart.
UTXO/nonce reservations and full browser wallet-state merge remain separate
integrations. Payment allocation is available as described below.


## Portable payment destination allocation

With `payments`, `payment_allocation::PaymentAllocationBook` retains an explicit
owner account, peer code and send/receive direction for one network/zone. The
Wasm facade `browser_payments::BrowserPaymentBook` adds IndexedDB atomic commits:
`initialize(owner, prior_raw_index)`, then `allocate(owner, id, max_attempts,
cancelled)`. The whole raw interval commits before secret-assisted search; the
verified destination commits before return. Retain the ID through cancelled
futures and lost responses, inspect `snapshot(owner)`, and explicitly `resume`
or `abandon`. Neither IDs nor ranges become reusable. Concurrent stale writes
return a conflict; they are not automatically retried.

The adapter stores only public context and borrows the guarded private owner for
each operation. Imports re-derive every completed destination and check the owner,
account, peer, direction, ledger and zone. Receive records can be passed to
`QiKeyring::import_payment_receive(owner, peer, index)` for explicit local signing.
Send records are owned by the recipient and must not enter the local receive key
inventory. Each direction/zone has an independent namespace. Run derivation and
large journal validation in an application worker.

A new namespace requires an explicit raw floor. `from_channel` preserves existing
direction/zone cursors but its caller must establish network and freshness.
With `backup`, `initialize_from_backup` checks the exact authenticated network and
channel and preserves its consumed floor. It initializes only a never-written
namespace, never replaces live records or tombstones, and does not copy historical
exposures or transaction claims. Empty latest UTXOs are not evidence for zero.

`QPAYABK1` is a Rust-specific public codec: 8-byte magic, 65-byte network scope
(chain ID u256 big endian, genesis hash, zone byte), 32-byte identity, initial and
next raw indexes (u32 big endian), and record count (u16 big endian). Identity is
Keccak of UTF-8 `quai-rust/payment-allocation/v1`, owner code (80 bytes), account
(u32 big endian), peer code (80 bytes), and direction (0 send, 1 receive).
Each ID-sorted record has a 16-byte ID, start/end u32, status (0 pending,
1 completed, 2 abandoned), and a u32 child index only for completed records.
Import checks exact length, sorted unique IDs, complete gap-free interval coverage,
limits and completed derivations. No claimed public point is trusted.

The bound is 1,024 retained IDs and 29,811 encoded bytes, with at most 100,000
candidates per range. `2^31` denotes exhaustion. Abandonment does not free capacity;
there is no silent compaction or rollover. The codec provides neither encryption
nor freshness; browser revisions fence cooperating writers. Actual Chromium
worker tests exercise contention, dropped writes, restart, corruption/tombstones,
backup floors and send/receive ownership. Twenty-four independent Node encodings
use six pinned quais.js payment address vectors. Full live wallet state merge,
UTXO/nonce custody and transaction orchestration remain separate work.

Native `SqliteStore::allocate_address_compact` derives under a bounded SQLite
write lock and commits only the examined range plus the returned address metadata.
`allocate_payment_address_compact` provides the same contract for registered BIP47
channels. No address is exposed before commit; cancellation before commit rolls
back that unexposed allocation. Returned allocations and all preexisting burned
ranges remain consumed. The original range-before-search APIs reserve their
whole range before searching, and give back the part past the returned address
unless the store changed meanwhile. Native SDK change pools and payment intents use compact allocation.

A Qi refund can have a creating ETX hash with the Quai ledger bit. The creating
hash must be nonzero and in the correct destination zone, but its ledger bit does
not identify the output's ledger. Verified ownership, fixed denominations, locks,
claims and current UTXO observations remain necessary.
