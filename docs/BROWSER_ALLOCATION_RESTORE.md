# Live HD and payment allocation recovery

The portable address and payment allocation books now have `merge_backup` methods
with the `backup` feature. Browser address/payment adapters expose the same merge
under an IndexedDB compare-and-exchange. Existing journal IDs and completed
addresses/exposures remain retained. The method returns the number of pending
requests it abandoned; it never performs a search or exposes a new address.

A merge proves the backup's exact HD account or payment owner/peer/network before
changing state. It takes the maximum old/imported burned cursor for each relevant
branch or direction. Exhaustion at `2^31` remains exhausted. Every pending request
is abandoned: restoring older allocation history must not allow an in-flight
search to expose an address whose earlier use is ambiguous. Completed requests
remain idempotently inspectable, and all consumed IDs still reject reuse.

New allocations begin above the maximum cursor. Failed ownership checks leave the
journal unchanged. Browser writes use the snapshot revision; a racing allocation
or restore causes one write to fail without automatic retry. After cancellation,
inspect the same IDs and current state because a dispatched storage write may
already have committed. These merges do not release account/Qi transaction claims.

A merged address journal uses `QADDRBK2`; a merged payment journal uses `QPAYABK2`.
The framing and size limits are unchanged. The floor now marks a fully consumed
prefix containing retained historical requests. Below it, completed/abandoned
ranges may have gaps representing other restored history; they cannot overlap,
straddle the floor, or remain pending. New ranges above the floor must form complete
contiguous coverage through the next cursor. Every completed address/point is still
re-derived during import. Version 1 decoding retains its original strict coverage
rules; existing v1 states continue to round-trip unchanged. Older SDK versions
which only understand v1 cannot reopen a v2 journal.

The original limits still apply: 4,096 HD IDs or 1,024 payment IDs, including
abandoned IDs, and bounded raw ranges of at most 100,000 indexes. Merge does not
prune records or reclaim capacity. This is a Rust persistence format, not quais.js
wallet JSON compatibility. Recovery backups retain burned cursors and inventory;
keep public journals when exact allocation-ID history must also survive export.

Retain earlier owned addresses and payment exposures using the combined capture's
`previous_inventory` field. Merging a cursor into a live allocator does not copy
all earlier inventory into that allocator. See [portable wallet capture](PORTABLE_WALLET_CAPTURE.md).

These adapters make each individual journal restore atomic. Coordinated restoration
of several journals still requires an atomic batch integration; do not interpret
several separate successful calls as one database-wide transaction.

Independent Node fixtures cover 48 merged states using pinned quais.js derivations.
Native and actual Chromium worker tests verify exact framing, preserved IDs,
exhaustion, malformed history, reopening and allocation-versus-restore CAS races.
Existing v1 allocation tests remain enabled. [Retained validation](../test-infra/reports/browser-allocation-restore-2026-09-13.json).
