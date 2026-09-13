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
synthetic test results, not funded node acceptance. Replacement preparation and
full browser Qi selection/fee workflows remain separate integration work.
