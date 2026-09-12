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
response is checked, including UTXO ownership, zone/ledger hashes and duplicate
outpoints. Remote `reserved` flags are discarded. `classify_coin` combines the
candidate block height with locally recovered reservation state to expose
independent locked, expired and reserved flags plus spendability.

A source must actually support checkpoint-consistent observations. It must not
label latest-only balance/outpoint RPC results as historical block responses.
Native observation futures require `Send`; wasm observation futures may retain
thread-local browser state. A browser source implementation and browser runtime
qualification remain separate work. Transport adapters own response-size limits,
deadlines and in-flight cancellation.
Discovery checks cancellation during grinding and around awaited observations;
cancelling an already running source request requires that source's cooperation.
An initial cancellation returns an error before requesting a tip; later cancellation
returns a partial report with no canonical check. Source errors fail closed.

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
The entire reserved range is consumed, including skipped indexes, unused trailing
indexes, cancellations and process death before address exposure. A returned
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
