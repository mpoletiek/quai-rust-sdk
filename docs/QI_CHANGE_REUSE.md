# Reusing Qi change addresses that were never signed

Design review for ask 2 of [Quai Terminal's asks](WALLET_ASKS_2026-09-19.md),
phase 3 of [the response](WALLET_ASKS_2026-09-19_RESPONSE.md). It relaxes the
rule that an allocated address is never handed out again, so it was reviewed
for funds safety and privacy before any code was written.

## Problem

A seed-only restore, like Pelagus, scans the change branch with a gap of 50
matching addresses. Before this change, every allocated change address stayed
consumed:

- a wallet that allocates a pool per send and drops what it did not use burns
  the rest of the pool on every send;
- a rejected review burns the change its prepared transaction used;
- a process restart burns every live pool.

Enough of these leave later change outside the restore's window. The funds are
not lost, but they stay invisible until a deep scan.

## Rule

A change address may be handed out again only if it never appeared in a signed
payload the store holds. Before this, it may have appeared in a fee estimate
sent to the node. That is not a new leak: `refresh_qi` already sends every
stored address to the same node in one batch.

## Design

- **Durable released set.** A new `released_change` table (schema v6) holds
  change indexes per scope and Qi account. It is filled in three ways:
  - `QiChangePool::release(self, &mut SqliteStore)` returns a pool's unused
    addresses;
  - `QiChangePool::reclaim(&mut self, store, prepared)` releases an unsigned
    prepared transaction's claim and returns its change to the pool, and
    `reclaim_operation` does the same for a prepared special operation;
  - `SqliteStore::release_change` is the store-level primitive both use.
- **Lowest first.** `QiChangePool::allocate` takes released indexes lowest
  first, and only then derives fresh ones. Change therefore fills the gaps
  inside the window a restore scans.
- **No invalidation.** A released address is already stored metadata whose
  coins every refresh reads, so taking one does not move the scope generation
  or require a new refresh. Only fresh addresses invalidate the snapshot.
- **Guards.** The released set is protected in four places:
  1. **Store API.** `release_change` refuses an address that is not this
     account's Bip44 change address below the branch cursor.
  2. **Signed payloads.** It also refuses an address whose 20 bytes appear in
     any stored signed payload or replacement. Qi and Quai payloads carry
     output addresses as raw bytes, so this SQLite `instr` check can report a
     false positive, which only keeps an address burned, but never a false
     negative.
  3. **Signing.** Committing a signed payload or replacement removes each of
     its output addresses from the released set in the same transaction, so
     an address signed by any path is never handed out again.
  4. **Reclaims.** A reclaim consumes the prepared object and releases the
     claim with the existing `Reserved`-only transition. A transaction that
     was signed, or marked signed, cannot be reclaimed.
  Taking a released address deletes its row under the write lock, so two
  handles or processes never get the same one.
- **Backups.** The released set is not exported, and a restore or merge clears
  it for each scope it writes. Released addresses are therefore burned after a
  restore: another device may have signed with them. This can widen a gap, but
  it can never reissue an address that appeared in a signed payload elsewhere.
- **Cursor.** Nothing rewinds the derivation cursor. The released indexes all
  lie below it, so the non-compact allocator's give-back of an unexamined tail,
  which is guarded by the scope generation, is unaffected.

## Payment destinations: deferred

The response proposed releasing a payment's destinations too. Review found a
hazard the change-address design does not have:

- A `QiIntent` is plain, cloneable data. After a release, the SDK cannot tell
  whether a copy of the intent was signed somewhere, or will be.
- If the destination was reissued, both payments would land on one of the
  peer's addresses. That is on-chain address reuse, visible to the peer and to
  every observer.

Change addresses never leave a non-cloneable pool or prepared object, so for
them the SDK can prove the rule. The cost of burned destinations falls on the
receiver's scan, which can continue past a gap. Until a design binds
destinations to a reservation, a wallet should:

- build one `QiIntent` per payment and reuse it across retries, since
  `prepare` never consumes destinations;
- allocate destinations after the user settles on the amount.

## Remaining ways to burn change

- A crash, or a pool dropped without `release`. The addresses stay burned, as
  before.
- A restore, which burns the released set.
- A signed transaction that is later dropped. Its change was signed, so it
  stays burned.

Recover with a deep scan: `QiScanOptions` with `gap_limit: None` over an
explicit change range.

## Tests

- Release, then reallocate: the same addresses come back, lowest first, and the
  snapshot is not invalidated.
- An address in a signed payload is refused by `release_change`, and signing
  removes a released address from the set.
- A reclaimed prepared transaction returns its change; a signed one cannot be
  reclaimed.
- Two handles releasing and allocating concurrently never share an address.
- Restoring a backup clears the released set.
- Repeated `InsufficientChange` retries and rejected reviews keep change
  inside a gap-50 restore.
