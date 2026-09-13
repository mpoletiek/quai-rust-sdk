# Browser account transaction custody

Enable SDK features `backup,browser` for `browser_accounts::BrowserAccountBook`.
The portable `wallet::account_custody::AccountOperationBook` uses `backup` and
requires neither SQLite nor Tokio. SDK `backup` now also enables the public
`payments` facade, matching the wallet backup feature's existing dependency.

One journal belongs to an exact chain ID, genesis, zone and compressed account
public key. Its domain-separated identity is independent of private-key origin;
HD and imported signers for the same address must share this namespace. Use one
database name consistently across tabs and workers. Different names do not share
nonce claims. The caller must establish trusted network/account identity and
prior custody; a remote pending nonce is not a recovery snapshot.

1. Open and explicitly initialize a never-written namespace with the retained
   nonce floor, or use `initialize_from_backup` with an authenticated backup.
   Existing data and tombstones reject initialization.
2. Generate and retain a unique `ReservationId`. `reserve_nonce` commits
   `max(remote_pending, retained_next)` before returning. Both the nonce and ID
   remain consumed after an unsigned release; `reopen_unsigned` explicitly repairs
   that same nonce gap under the same ID.
3. Prepare and review the exact account transaction through the portable
   transaction/contract APIs. `sign` checks the retained nonce, sender and chain,
   invokes the synchronous local signer, then commits canonical signed bytes
   before returning them. `commit_signed` also accepts independently signed bytes;
   its caller is responsible for keeping those bytes private until custody commits.
4. Retain explicitly approved fee-only replacements with `commit_replacement`.
   All fields except strictly increased gas price must match the selected parent.
   Original bytes and every candidate remain held. Node fee-bump acceptance is a
   separate check.
5. Call `mark_submitted` before an explicit typed provider broadcast. Submission
   failures, timeouts, confirmation and reorg never free signed nonce custody.
   This adapter neither sends transactions nor retries RPC calls.
6. Capture `snapshot().revision` before asynchronous canonical receipt/header
   checks. Supply it to `observe_inclusion`; a concurrent mutation or invalidation
   rejects stale results. Reorg invalidation removes only the exact observed
   candidate/block pair. Confirmation is not a finality proof.

After cancellation or any lost write response, reopen and inspect the **same ID**.
An IndexedDB transaction already dispatched may have committed. A revision
conflict is returned to the caller without an automatic allocation/sign/send retry.
Snapshot books are detached values; editing one does not persist a change.

`QACCTBK1` is a deterministic public custody encoding, not a quais.js wallet
serialization format or an encrypted secret backup. It binds network and public
key, stores the next nonce and sorted IDs, and retains states, exact signed root
bytes, replacement edges and optional inclusion. Import revalidates canonical
signatures, sender/chain/nonce, unique nonces, family edges and strict lengths.
The bounds are 256 retained IDs, 32 replacement edges per operation, 1 MiB per
signed transaction and 16 MiB for the entire journal. Exhaustion fails without
discarding existing custody. Released IDs still count toward capacity.

Authenticated native v4 backup import preserves root/candidate bytes and nonce
floors, including hash-only claims, and invalidates historical inclusion.
`merge_backup` also unions account custody into an existing live journal under
one CAS. It retains absent live IDs and all signed candidates, takes the maximum
nonce floor, and rejects conflicting ID/nonce/root assignments or bounds without
changing live state. An unsigned backup never releases or reopens a live operation;
authenticated signed evidence preserves the claim even if an old live record was
unsigned. Matching hash-only custody can gain canonical signed bytes. Every
successful merge discards live inclusion for fresh canonical reconciliation.

`WalletBackup::capture_account_custody` captures a detached journal snapshot with
explicit owned public metadata and secret origins, after proving their exact
derivation. Its existing encryption API produces an authenticated envelope which
can initialize or merge browser account custody and restore native SQLite custody.
This is an **account-only backup**: HD/payment allocation journals, other accounts,
Qi outpoints and other browser state are not captured. Inclusion is deliberately
omitted. Keep those other journals/backups independently; this method does not
claim to capture a complete browser wallet. A browser snapshot revision identifies
when the account capture was read; later writes may require a newer backup.

See [Qi custody](BROWSER_QI_CUSTODY.md) for durable browser input claims. Complete
browser prepare/fee/recovery orchestration remains separate work. IndexedDB records are public application state, not authenticated
against malicious replacement or rollback by code with the same browser origin.

Tests use public toy identities which must never be funded. Eight independent
Node encodings use pinned quais.js signatures; native and actual worker tests
cover transitions, import bounds, candidate changes and authenticated backup
initialization. Worker tests additionally cover competing connections, cancellation
after nonce/signature write dispatch, restart, stale observations and tombstones.
The aggregate 16 MiB boundary is tested natively; the fuzz campaign uses the
existing 65,536-byte input cap and makes no full-size fuzzing claim.

For combined HD/account/Qi/payment recovery, use [portable wallet capture](PORTABLE_WALLET_CAPTURE.md) with every relevant frozen journal and retained address inventory. Its recovery-format and snapshot-consistency boundaries are explicit.
