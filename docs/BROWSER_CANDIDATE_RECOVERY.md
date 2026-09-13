# Browser signed-candidate recovery

Enable `backup,browser` with native defaults disabled. Create
`BrowserRecoverySession::for_account(&provider, &account_book)` or
`BrowserRecoverySession::for_qi(&provider, &qi_book)`. The session reads only public
custody and needs no unlocked key, signer or wallet prompt. It supports account
calls/conversions/deployments and ordinary/conversion/wrapping Qi operations.

`signed_candidates(id)` returns validated canonical bytes in root-first order,
followed by retained replacements. `broadcast_root(id)` submits the original
persisted transaction; `broadcast_candidate(id, hash)` selects an exact member.
Neither selects a higher fee automatically. An unknown hash rejects without a
send. Both check the configured network and commit Submitted with the revision
captured before RPC, then make one typed provider send. Errors and cancellation
retain the entire family. After an ambiguous send, reopen the same book and
explicitly replay the same root or candidate hash. Confirmed families require
reconciliation of lost inclusion before submission is available again.

`reconcile(id)` checks every candidate receipt, origin ledger/sender/destination,
canonical block, sampled head, and surrounding chain/genesis. More than one
canonical member rejects. Failed execution still consumes a nonce; Locked is a
reported origin outcome, not proof of destination spendability. Confirmation
counts include the origin block and do not establish finality.

The returned `BrowserFamilyUpdate` contains the source-reported candidate view,
committed revision and persisted inclusion. These can differ when an earlier
inclusion remains canonical but receipt indexing is unavailable: absence alone
never clears a previously observed inclusion. A changed/unavailable canonical
header clears that observation and returns the family to Submitted. Exact signed
bytes, account nonce floors and Qi input claims remain held. The SDK does not infer
a dropped transaction or release claims from absence, errors or reorgs.

All dependent reads precede one compare-and-exchange against the original journal
revision. A concurrent tab, backup merge or other writer rejects the attempt.
There is no automatic retry. Cancellation before write dispatch cannot mutate
state; after dispatch, inspect the same ID to determine whether the write committed.
These separate node reads cannot provide an atomic chain snapshot.

`candidate_observation::observe_signed_candidates` exposes the portable read-only
observer independently of storage. It accepts one caller-validated mutually
exclusive family, 1–33 canonical signed payloads totaling at most 16 MiB, and
rejects duplicate identities. Journals validate ownership and replacement
relationships; this standalone observer does not replace those checks. Native
`recovery::track_family` now uses this same observer with SQLite cache fences.

[Tests](../crates/quai-sdk/tests/browser_recovery.rs) cover actual Chromium
IndexedDB concurrency, cancellation, restart, ambiguous sends, retained inclusion,
reorgs, competing candidates and all three Qi wire forms. RPC acknowledgements are
synthetic test results, not funded node acceptance. [Browser Qi preparation](BROWSER_QI_WORKFLOW.md) supplies the selection/fee/signing
workflow. [Reviewed replacement preparation](BROWSER_REPLACEMENTS.md) creates
explicitly approved candidate edges.

## Destination settlement

`observe_settlement(id, candidate_hash, kind, request, max_outputs)` reconstructs
intent from the selected persisted signed bytes. `SettlementKind` selects either
conversion direction, native Qi wrapping, WQI redemption with an explicit
contract/index, one cross-zone Qi output, or cross-zone Quai execution. Invalid
signed scope, operation interpretation or candidate selection rejects. No keys
are required and no transaction is sent.

The bounded `EtxScanRequest` specifies the destination zone, inclusive block range
and external-transaction limits. For subsequent pages supply the preceding block
anchor. Provider observers check network identity, canonical origin, signed
sender/recipient, emitted external transactions, destination execution and current
attributed Qi credit. Missing origins and failed origins remain distinct; origin
inclusion alone does not establish execution or maturity. Qi credit is a current
node observation, not historical or atomic UTXO recovery.

The returned `BrowserSettlementUpdate` contains an advisory observation and a
revision fenced against the journal read before RPC. A concurrent writer causes
a storage conflict; no stale view is returned as a successful journal update.
The same custody bytes are committed: nonce floors, claims and candidate families
are preserved. Destination observations and page cursors are **not persisted in
the browser custody frame**. After restart reconstruct the same signed reference
and recheck explicit ranges. Native settlement resume additionally persists
bounded, revalidated destination scan cursors.

`settlement_observation::observe_signed_settlement` exposes the same portable
read-only operation independently of storage; native settlement delegates to it.
The browser tests cover reconstruction of all six signed intents, unavailable and
failed origins, unchanged custody and a concurrent-update conflict. Successful
and mature destination execution requires separate provider/node qualification;
these browser fixtures do not establish funded settlement acceptance.
