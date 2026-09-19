# Response to the Quai Terminal asks of 2026-09-19

This answers [the asks](WALLET_ASKS_2026-09-19.md) from Quai Terminal. Each was
checked against `main` at `0.1.0-alpha.5`. It records what we will do, what we
changed from the proposal, and the plan for `0.1.0-alpha.6`.

## Verdicts

| Ask | Verdict | Effort |
|---|---|---|
| 3. A lost observation race is Stale | Implement, covering one more race site | Small |
| 1. Qi stores tolerate a second writer | Implement, with a safer retry | Medium |
| 6. Refresh does not rewrite unchanged coins | Implement, together with ask 1 | Small–medium |
| 2. Unused change and payment addresses are not lost | Implement, after a design review | Large |
| 8. One RustCrypto generation | Implement the part in the SDK's graph | Small |
| 5. Proxy support | Owner's decision | Small |
| 7. Chain codes are wiped on drop | Raise upstream | Upstream |
| 4. Domain-separated key between payment codes | Deferred | — |

**Ask 3.** Both `StorageError::Conflict` returns in `storage/observations.rs`
are races, not the account-xpub mismatch that `Conflict` also covers:

- at line 199, the cache revision moved;
- at line 168, another writer added a replacement candidate between the read and
  the write. The ask did not mention this one.

**Ask 1.** The problem is as described. One part of the proposal is unsafe:
committing the earlier reads against a re-read generation. The generation also
moves when an address is imported or the snapshot is invalidated. Committing
reads taken before an import would restore a snapshot missing the new address's
coins, which is exactly what the generation check prevents. On a lost race we
redo the refresh instead. Both stale errors already report class `Stale`, so
retrying on `class()` works today; this is now documented.

**Ask 8.** The RustCrypto split is ours: `quai-keystore` uses sha2 0.11,
hmac 0.13, pbkdf2 0.13 and scrypt 0.12, while k256 and bip32 use sha2 0.10 and
hmac 0.12. Together with a few unrelated pairs, a downstream build of the SDK
duplicates about 10 crates (`cargo tree -d`). **ark-ff is not in the SDK's
dependency graph.** It comes from the wallet's own dependencies (alloy/ruint
features there).

**Ask 7.** Confirmed: bip32 0.5.3's `ExtendedKeyAttrs` derives `Clone`, and has
no `Zeroize` or `Drop`. Our wrapper cannot wipe it, because `XPrv` exposes its
attributes read-only; doing it here would take `unsafe`.

**Ask 4.** Deferred. The reasons, and guidance for any builder using payment
codes beyond payments, are in
[using payment codes for anything but payments](PAYMENT_CODES_BEYOND_PAYMENTS.md).
In short: the notification-to-notification ECDH is the pair's index-0 payment
secret, and a static key has no forward secrecy on a permanent public chain.
Messaging should use separate keys that the payment code vouches for, with a
ratchet.

## Plan for 0.1.0-alpha.6

The work is ordered so the smallest, most certain changes land first. The two
storage changes that touch the same code (asks 1 and 6) land together, and the
invariant-changing work (ask 2) comes last, behind a design review.

### Phase 1: the race error (ask 3)

1. Add `StorageError::ObservationRaced`, class `Stale`. `StorageError` is
   non-exhaustive, so adding it does not break callers.
2. Return it at both race sites in `storage/observations.rs`, lines 168 and 199.
   `Conflict` keeps its other meanings.
3. Check every caller that matches `Conflict` (settlement tracking, candidate
   observation, and the browser equivalents), so none treats a race as terminal.
4. Tests: two store handles race on one observation, and on one candidate
   family. Each loser gets `ObservationRaced`; rerunning the observation
   succeeds.
5. Changelog, under Changed: callers that matched `Conflict` for a race now get
   `ObservationRaced`.

**Done when:** two trackers following one settlement never report "invalid"
because the other one committed first.

### Phase 2: concurrent Qi writers and quiet refreshes (asks 1 and 6)

**Store (ask 6).** When a refresh's coin set is identical to the stored one
(same outpoints, denominations, owners and lock heights), update only the
checkpoint. That update is still under the generation check, and it does not
bump the generation. Heights only move forward without an explicit
invalidation, so any lock that expires at the new height makes spending more
permissive, never less. Reservations fenced on the old generation remain valid,
because nothing they selected changed.

**Refresh (ask 1).**

1. `refresh_qi` catches a lost generation race at commit time.
2. It re-reads the store. If the stored snapshot is valid, and its checkpoint is
   at or after the tip this refresh read, it returns that checkpoint: someone did
   the same or newer work.
3. Otherwise it redoes the whole refresh, including the reads, up to a bounded
   number of attempts with jitter.
4. It never commits reads taken under an older generation.
5. A new `RefreshOptions` (non-exhaustive, with `attempts` and a jitter bound) is
   taken by a new `refresh_qi_with`. `refresh_qi` keeps its signature and its
   three attempts.

**Reservations (ask 1).** `QiSession::prepare`, `prepare_special` and
`prepare_sweep` retry once when `reserve_qi` loses a generation race: they
re-read the snapshot and select again. A claim conflict (an input already
reserved) is not retried.

**Docs.** "Retry on `class() == Stale`, never on a specific variant." Add this to
the sync-order section of `WALLET_WORKFLOWS.md` and to the error-class
documentation.

**Tests.**

- A second store handle commits between the reads and the commit. Covers both
  the "same or newer, return" case and the "older, redo" case.
- An address import in the same window must lead to a full redo, never a commit
  of the stale reads.
- N threads refreshing one store: all succeed within the attempt budget.
- An unchanged refresh leaves the generation and the coin rows untouched, and
  advances the checkpoint.

**Mainnet check (read-only).** Rerun the wallet's experiment against a copy of
the custody store: three concurrent refreshes, 36 runs. Record the failure rate
before and after, and the WAL growth of an idle wallet before and after.

**Done when:** two processes sharing one Qi store never fail each other's
refresh or send, and an idle refresh writes no coin rows.

### Phase 3: reusing addresses that were never used (ask 2)

This relaxes the rule that an allocated address is never handed out again. It
starts with a short design document, reviewed for funds safety and privacy
before any code.

**Proposed design.**

- **Rule.** An address may be handed out again only if it never appeared in any
  signed payload the store holds. It may have been shown in a review, but that
  exposure is local and acceptable.
- **Change.**
  - `QiChangePool::release(self, &mut SqliteStore)` returns the pool's unused
    addresses to a durable released set, scoped to the store and account.
  - `QiChangePool::allocate` takes from that set first, lowest index first, so
    change stays inside the gap-50 window a seed-only restore scans.
  - A pool that is dropped without `release` stays burned, as today.
- **Payment destinations.** `release_unsigned` returns the destinations its
  reservation allocated to the peer's released set. Moving allocation to signing
  time is the alternative; we will pick one in the design.
- **Concurrency.** Adding to or taking from the released set bumps the scope
  generation, like every other cursor writer.
- **Backups.** A restored store treats released addresses as burned. That is the
  conservative choice: it may widen a gap, but it can never reissue an address
  that appeared in a signed payload on another device.
- **Gap.** Document the remaining ways to burn addresses (process crash, a pool
  dropped without release), and how to recover with a deep scan.

**Tests.**

- Release, then reallocate: the same addresses come back, lowest first.
- A signed-then-released reservation's addresses are never reissued.
- Two processes releasing and allocating concurrently.
- Restoring a backup does not reissue released addresses.
- Repeated `InsufficientChange` retries with doubling no longer push change past
  gap 50.

**Qualification.** A funded Orchard run: retries and rejected reviews, then a
seed-only restore that finds every coin at the default gap.

**Done when:** a wallet can retry and reject reviews indefinitely without
pushing funds beyond the default restore gap.

### Phase 4: dependency hygiene (ask 8)

1. Move `quai-keystore` to pbkdf2 0.12, scrypt 0.11, sha2 0.10 and hmac 0.12,
   the generation k256 and bip32 use.
2. Keystore test vectors, including the JavaScript-generated ones, must be
   byte-identical.
3. Run `cargo tree -d` before and after, and record both.
4. Run `cargo audit` on the new versions.
5. Tell the wallet that ark-ff comes from its own dependencies.

**Done when:** the SDK contributes no RustCrypto duplicates downstream.

### Decisions and upstream work, alongside the phases

- **Ask 5 (proxy).** If approved: add `HttpConfig::with_proxy(url)`, opt-in,
  for HTTP and SOCKS5 URLs. SOCKS5 goes behind a feature, because it adds a
  dependency. System proxy variables stay ignored. Each transport carries its
  own URL and credentials, so each wallet can get its own Tor circuit.
  WebSocket proxying is a separate follow-up.
- **Ask 7.** Open an issue, or a pull request, on `iqlusioninc/crates` for
  `ZeroizeOnDrop` on `ExtendedKeyAttrs` and `XPrv`. Revisit if the SDK replaces
  bip32's private derivation, as it already has for public derivation.

### Release

- Phases 1, 2 and 4 can ship as `0.1.0-alpha.6` without phase 3 if phase 3's
  review runs long.
- Each phase follows the alpha.5 process:
  - the extended verification matrix, including the test-infra clients and the
    browser suites;
  - reviews for correctness, security, funds safety and documentation;
  - fixes verified by tests that fail without them;
  - signed commits;
  - a PR, a green CI run on `main`, a signed tag and an approved release.
