# Native SQLite storage foundation

Enable `quai-wallet`'s optional `sqlite` feature and use `quai_wallet::storage`.
The implementation uses rusqlite 0.40.2 and bundled SQLite, only on native targets.
Browser storage remains a separate, unimplemented adapter. This is a public-state
store, not a complete wallet, verified reconciliation service, or reorg engine.
[DISCOVERY.md](DISCOVERY.md) describes the bounded watch-only scan contract, atomic
discovery commits, conservative checkpoint invalidation and fresh address allocation.

Each connection is permanently bound to `(chain_id: U256, genesis_hash, zone)`.
Applications must obtain and verify that identity against their trusted network
configuration. A nonzero genesis hash is required. Ordinary wallet tables and lookups include
the full scope, including nonce cursors and operation IDs. Payment-channel rows
share chain/genesis identity across their explicit nine-zone cursor arrays.

Schema v2 uses application ID `0x51574149`, `user_version=2`, STRICT tables, checked
fixed-width blobs and foreign keys. An unrecognized application ID/version or a
nonempty database with no application ID is rejected before persistent writes.
Schema creation and version assignment are one transaction. Existing v1 databases
are migrated atomically after validating their exact table/column inventory; unknown
fields or tables reject the migration without creating channel tables. Write transactions use `BEGIN IMMEDIATE`; reads of a
snapshot keep checkpoint, generation and coins in a single read transaction.
Connections use WAL, synchronous FULL, foreign keys and a five-second busy timeout.
Lock timeouts and I/O errors return an error rather than retrying a wallet action.
These choices follow [SQLite transaction semantics](https://www.sqlite.org/lang_transaction.html)
and [rusqlite transaction documentation](https://docs.rs/rusqlite/latest/rusqlite/struct.Transaction.html).

Store the database and its WAL/SHM files on a local filesystem with reliable
locking. Durability still depends on OS/filesystem/hardware honoring synchronization.
The database is not encrypted and exposes addresses, balances, derivation indexes
and transaction relationships. Use application-private directory permissions.
The API does not accept mnemonics, seeds, private keys, xprvs or arbitrary serialized
payloads. Its signed-payload table accepts only validated canonical Quai/Qi
transactions (up to 1 MiB), never an arbitrary byte import. Transaction calldata
is public transaction content and must not contain secrets. Database access/tampering by another
program is outside this integrity boundary; application IDs are format markers,
not authentication. A maliciously changed database or rolled-back database copy
cannot be recognized reliably without an external trusted checkpoint.

## Metadata and snapshots

`PublicAddress::derive` derives an actual public key/address from an account xpub
and records coin, account, change and actual child index. Xpub ancestry is trusted
from its caller, as with the HD API. `PublicAddress::imported` makes no ancestry
claim. Existing address/public-key/origin records are immutable; importing a
different origin for the same address fails the entire transaction.

Every `import_metadata` call, even an empty or idempotent import, increments the
snapshot generation, clears its checkpoint and deletes its UTXO snapshot. It
preserves all reservations, allocated nonce claims and nonce cursors. Thus importing
public keys cannot accidentally restore a scan checkpoint for keys never scanned.
`invalidate_snapshot` supplies the same primitive for a future recovery engine.

`replace_snapshot` compares scope and expected generation before atomically
replacing all coins and the checkpoint. It rejects duplicate outpoints, unknown
owners, invalid zone/ledger hashes, oversize batches, stale generations and rewinds.
A lower checkpoint or a different hash at the same height requires explicit
invalidation first. This cannot detect a reorg to a higher height: a scanner must
verify ancestry before calling it. Checkpoints are caller observations, not proofs.
Reservation flags on replacement are ignored; reads compute them from durable
claims. A stale snapshot cannot make a signed/confirmed claimed coin spendable.

## Reservations and recovery

Generate a fresh `ReservationId` per operation and persist intent before signing.
IDs remain consumed after release. `reservations` provides bounded pagination by
ID, and `reserved_nonce` / `reserved_outpoints` recover allocations after restart.

1. Reserve selected Qi inputs atomically with `reserve_qi`, specifying the current
   snapshot generation and candidate spend height. All inputs must be present,
   unlocked and unexpired. Competing connections/processes cannot claim the same
   outpoint. Reservation batches never partially succeed.
2. For an account transfer, `reserve_nonce` atomically allocates
   `max(remote_pending, persisted_next)` and persists the incremented cursor.
   Remote pending must be queried for the exact network/address. Arithmetic is
   checked; `u64::MAX` is not allocatable because its successor is unrepresentable.
3. Sign locally, then call `commit_signed_quai(id, &signed)` or
   `commit_signed_qi(id, &signed)` successfully **before exposing signed bytes or
   attempting broadcast**. This commits canonical signed bytes and the Signed
   transition atomically. It checks the chain and exact account sender/nonce or
   complete Qi input outpoint/owner set against the reserved allocation. Qi claim
   owners survive snapshot invalidation. `signed_payload` recovers bytes after
   restart and revalidates the signature, hash, scope and claims before returning.
   The lower-level `mark_signed(id, transaction_hash)` remains available for
   conservative hash-only tracking; it does not persist bytes for rebroadcast. If marking fails, do not expose
   or submit the signature. A crash between signing and this mark is safe only if
   no signature escaped. State is `Reserved → Signed → Submitted → Confirmed`.
4. `mark_submitted` records an attempted submission. A timeout, lost response,
   process exit or absent mempool entry does not release a signed claim.
5. `observe_inclusion` records caller-verified block inclusion for the exact signed
   transaction. It does not prove finality or release claims. Claims remain held
   after checkpoint invalidation and possible reorgs.

`release_unsigned` only accepts a Reserved operation with no transaction hash.
It releases Qi claims, retains the operation record, and never rewinds nonce
cursors or erases account allocations. Cancelled account transactions can therefore
leave nonce gaps; filling/replacing these requires a future orchestration layer.
There is deliberately no release API for signed/submitted/confirmed operations,
no reservation TTL, no opaque evidence-digest override, and no automatic reuse.
A future reconciliation layer must validate actual chain evidence before allowing
reuse. Conflicting spend observations, replacement transactions and reorg-driven
state transitions are not implemented. The current store favors stranded claims
over an unsafe double allocation after ambiguous broadcast.

Public snapshot limits are 100,000 coins/addresses per scope, 4,096 Qi claims per
operation and 1,000 operation records per enumeration page. Generation is checked
at signed 64-bit SQLite's maximum. Metadata and snapshots are application inputs;
this layer does not fetch remote balances, verify receipts, run background chain
rescans, keep block undo journals, or synchronize persistent Qi payment codes. The
separate bounded discovery engine requires an explicitly qualified observation source.

Validation includes reopen recovery, immutable-origin and snapshot rollback,
chain/genesis/zone isolation, stale generations, nonce overflow, foreign-database
byte preservation, schema field allowlists, and simultaneous reservations through
independent connections, threads and operating-system processes.


## Durable payment channels (schema v2)

`import_payment_channel(owner, channel, expected_generation)` explicitly registers
or CAS-imports an owner-validated `quai-payments::PaymentChannel`. `None` creates a
new channel; existing rows require their current generation and may never decrease
any cursor or revive exhaustion. Each row is scoped by chain/genesis, exact local
and peer payment payloads, and account, and owns all nine zones × send/receive cursors.
This avoids divergent copies when several zone-bound stores share a database.
Imports invalidate all existing snapshots on the affected network.

`allocate_payment_address(owner, peer, direction, max_attempts, cancelled)` commits
a disjoint raw range under `BEGIN IMMEDIATE` before doing bounded derivation outside
the transaction. Cancellation, exhausted searches, crashes, and later persistence
failure keep that whole range consumed. The first matching result is returned only
after a second transaction records its immutable public exposure. Trailing unused
candidates stay burned. This deliberately consumes more indices than an in-memory
search; back up cursors and use explicit recovery bounds.

Receive allocation additionally registers the derived owned public key and invalidates
the corresponding zone snapshot. Send allocation records a peer-owned destination and
does not insert it into local owned addresses. `payment_addresses` enumerates recorded
exposures and validates their exact public derivations against the private owner and
peer. It does not query transaction history or discover unknown counterparties.

QUAIWALT v2 captures registered channels, their cursors and exposures together with
all native scopes. It verifies seed-based payment ownership and merges cursors with
max/exhaustion semantics in the same transaction as ordinary state restoration. Old
v1 restores retain existing channels. Application-held channels must be explicitly
registered before capture; the store cannot detect another database or device.
Currently imported private payment nodes and master-xprv payment origins are not
supported by full-backup ownership verification. Ordinary seed/xprv/imported-key
backup support remains as documented in [FULL_BACKUP_FORMAT.md](FULL_BACKUP_FORMAT.md).

Tests cover process races, cancelled range retention, restart, shared zone counters,
network isolation, stale generation/cursor rejection, tampered exposure derivation,
atomic v1 schema migration, and ownership-checked stale backup restoration.


## Live workflow handle identity

`SqliteStore::instance()` returns an opaque, process-local `StoreInstance` for that
specific opened handle. Construction is private, there is no serialization API,
and allocation uses a checked monotonic counter that refuses exhaustion rather
than wrapping. This does not change SQLite schema or persisted backup identity.
The SDK binds live unsigned account/Qi preparations and one-use Qi change pools
to this identity. Separate stores with equal chain, metadata, cursors, and reservation
IDs cannot exchange those objects. Reopening the same file also creates a new
identity, so unsigned objects cannot be transferred to the reopened handle.

Complete preparation and signing with the same opened store. Once signed bytes are
committed, `broadcast(reservation_id)` can reopen and revalidate them for explicit
restart rebroadcast. Handle identity is not a proof against cloned databases,
independently operated copies of a seed, stale backups, or compromised application
memory; those still require coordinated allocation and recovery reconciliation.
