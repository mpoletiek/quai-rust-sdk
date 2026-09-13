# Atomic browser wallet restore

`quai_sdk::browser_backups::merge_wallet_backup` merges an authenticated
`WalletBackup` into explicitly selected, initialized browser journals. Enable
`backup,browser` with native default features disabled on `wasm32-unknown-unknown`.

Construct `BrowserWalletRestoreTargets` with HD allocation stores, account
custody stores, Qi custody stores and payment stores paired with their private
owners. All stores must use the same database name. Different open connections,
tabs and workers are supported. The caller enumerates every relevant journal;
there is no implicit database discovery or claim that omitted state was restored.

The operation reads and validates each current journal, applies its portable
ownership-checked merge to a private candidate, then commits all candidate bytes
in one IndexedDB read/write transaction. Every observed revision must still
match. Invalid ownership, incompatible operations, duplicate namespaces, missing
or tombstoned stores, cross-database targets, bounds and concurrent changes fail
without a partial update. A conflict requires explicit re-reading and review;
there is no retry loop.

HD/payment merges retain request IDs, completed exposures and maximum burned
floors; pending ranges become abandoned. Account/Qi merges retain live operations,
held claims and compatible signed replacement candidates. They discard chain
observations for fresh reconciliation. No transaction is sent and no signed
claim is released. `BrowserWalletRestore` reports new revisions in HD, account,
Qi, payment input order, abandoned allocation count and account/Qi merge effects.

At most 128 journals and 16 MiB of combined candidate public state are accepted.
Each journal's own format, ownership and capacity limits also apply. Restore
requires a guarded, already validated backup, typically obtained by decrypting
`EncryptedWalletBackup`; raw IndexedDB bytes are not authenticated recovery
origins. Retain its address/payment inventory for `previous_inventory` when
capturing successor backups. Recovery backups preserve allocation floors and
known exposures but do not recreate missing request-ID history.

Cancellation before the final transaction dispatch is read-only. Cancellation
after dispatch may commit **all** updates, even if Rust receives no result.
Read every target revision and operation ID before deciding whether to retry.
IndexedDB atomicity does not prevent a malicious same-origin script from
rewriting a database or rolling back external backup files.

The lower-level `quai_browser::compare_exchange_snapshots` accepts
`BrowserSnapshotUpdate` entries and returns revisions in input order. It supports
atomic initialization and tombstones as well as updates. `None` expected revision
means never written, which excludes tombstones. The SDK merge intentionally
requires live initialized journals. Payloads must be public or caller-encrypted;
this storage layer does not encrypt secrets.

Validation lives in [worker storage tests](../crates/quai-browser/tests/worker.rs)
and [combined wallet tests](../crates/quai-sdk/tests/portable_capture.rs):
concurrent writers, conflict rollback, cancellation, duplicate/cross-database
rejection, tombstones, payload/count bounds, six-journal recovery and retention
of signed custody and burned IDs. These exercise actual Chromium IndexedDB;
they do not qualify other engines or OS-level power-loss durability.
