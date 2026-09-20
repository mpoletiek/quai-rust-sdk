# Changelog

## Unreleased

Fixed:

- Qi conversions and wrapping decompose their destination outputs largest-first,
  through the new `select_fewest_converting`, instead of being capped by the
  input denominations. The node credits one aggregated value to the Quai-ledger
  destination and leaves those outputs out of its `CheckDenominations` rule, as
  the reference's `ConversionCoinSelector` already assumed. A mainnet wrap of
  15 Qi built twelve destination outputs where the reference builds two, which
  made it six times larger than it needed to be and cost it the scarce Qi block
  slot. Ordinary transfers keep the inventory-preserving decomposition, which is
  what the node enforces for them. Reported from a wallet in
  [docs/QI_CONVERSION_SELECTION_GAP.md](docs/QI_CONVERSION_SELECTION_GAP.md).
- A fee replacement of a conversion or a wrap checks only the outputs that stay
  in the Qi ledger against the input denominations, so an aggregated destination
  output no longer makes the replacement unbuildable.

## 0.1.0-alpha.6

Answers to Quai Terminal's asks of 2026-09-19
([response](docs/WALLET_ASKS_2026-09-19_RESPONSE.md)).

Changed:

- An observation that loses a race to another writer now returns the new
  `StorageError::ObservationRaced` (class `Stale`: observe again), not
  `Conflict` (class `Invalid`). This covers a moved cache revision, a stale
  `SettlementCursor`, and a replacement candidate that joined the family during
  `track_family`. It applies to `compare_exchange_observation[_scoped]`,
  `compare_exchange_family_observation[_scoped]`, `track_settlement`,
  `SettlementCursor::track`, `track_deployment` and `track_family`. `Conflict`
  keeps its other meanings. Callers that matched `Conflict` to detect a race
  should retry on `class() == Stale` instead.

- `SqliteStore::replace_snapshot` keeps the generation when the coins equal the
  stored ones and a checkpoint is stored: only the checkpoint advances, and an
  unchanged checkpoint writes nothing. It returns the generation after the
  commit, so use its return value rather than assuming a bump. An idle wallet's
  refresh no longer rewrites every coin row, grows the WAL, or fences
  concurrent reservations and observers.
- `refresh_qi` no longer fails when another handle on the same store commits
  first. It returns that handle's snapshot if it is labelled at or after its own
  block, and otherwise redoes its reads; reads are never committed under a
  generation they did not start from, because an import in between would drop
  the new address's coins. The three attempts now cover both a moving tip and a
  lost commit.
- `QiSession::prepare`, `prepare_cross_zone`, `prepare_special`,
  `prepare_special_estimated` and `prepare_sweep` select again once when
  another handle's commit makes their reservation stale. Nothing is claimed and
  no pool address is taken before that. A claim conflict is not retried. After
  an invalidation the retry reports `MissingSnapshot` (class `Stale`).

- **Schema v6.** Native stores migrate from v1 to v5 on open. An older SDK
  cannot open a v6 store.
- `QiChangePool::allocate` takes released change addresses first, lowest index
  first, and only then derives fresh ones. A pool made only of released
  addresses adds no metadata, so it needs no refresh.
- Committing a signed payload or a replacement removes any released change
  address it contains from the released set.
- A keystore whose scrypt cost breaks RFC 7914's `N < 2^(16r)` is refused with
  `KeystoreError::Format`. quais.js refuses it too; scrypt 0.12 had dropped the
  check, so such a file opened here but not in quais.js.

Added:

- `StorageError::ObservationRaced`.
- `QiChangePool::release`, `reclaim` and `reclaim_operation`, and
  `SqliteStore::release_change` and `take_released_change`. Change that never
  appeared in a signed payload is handed out again, so retries, rejected
  reviews and pools dropped after a send no longer push change past a
  gap-limited restore. Payment-code destinations are not released yet; see
  [reusing change that was never signed](docs/QI_CHANGE_REUSE.md).
- `qi_discovery::refresh_qi_with` and `RefreshOptions` (`max_addresses`, and
  `attempts` from 1 to 16).

## 0.1.0-alpha.5

Fixes from the first mainnet soak on 0.1.0-alpha.4, and the follow-ups the
alpha.4 reviews deferred.

Changed:

- `HeadTracker::poll` reports a page block missing above a base it read from
  the node as `ProviderError::ObservationChanged` (class `Stale`, retry), not
  `ReplayHistoryUnavailable` (class `Invalid`, re-anchor). The block goes
  missing when the chain reorganizes to a shorter branch between the two reads,
  or when a load-balanced read reaches a lagging node. A 98.7-hour mainnet soak
  through `rpc.quai.network` logged three `ReplayHistoryUnavailable` errors,
  with every reorg at most two blocks deep. The log does not show which read
  missed a block, so some may have been the still-unretried newest-anchor case.
  A missing tip, a missing retained anchor (including the newest), and a
  missing block one above a genesis base, which is never read, are still
  `ReplayHistoryUnavailable`. A wallet should cap how often it retries a
  `Stale` error before telling the user.
- Payment-channel writes return the new `StorageError::LimitReached`, not
  `Invalid`, when the store's 1,024 channels are full. That includes
  `import_payment_channel`, receive-index import, payment-address allocation
  and backup restore. Mailbox discovery reports `ChannelRegistration::Refused`
  only for that error, where a damaged row used to be reported the same way.
- `quai_sdk::qi::sign_message` returns the new `QiError::TransactionMessage`, not
  `MessageSigning`, for a message that is a Qi transaction.
- `MailboxSource` (new) is `Clone`, not `Copy`, because `Senders` carries a list.
- Invalid mailbox entries are cut to 128 characters.

Added:

- `PaymentMailbox::notifications_in_blocks` reads `NotificationSent` logs for a
  block range, in requests of at most `MAILBOX_LOG_BLOCKS` (10,000) blocks.
  - It halves a request that exceeds the transport's response limit, and grows
    back after one that succeeds.
  - A log that does not decode is listed in `invalid` and skipped; it no longer
    fails the read.
  - Announcement spam can slow the read but, unlike `getNotifications`, cannot
    disable it.
  - The range's end must be `MAILBOX_SETTLED_DEPTH` (16) blocks below the tip,
    and its hash must match before and after the read. Each log request is
    batched with the header of its last block, so a lagging backend cannot
    answer with missing logs. Any of these failing is `ObservationChanged`
    (retry), and an `Ok` result can be persisted as covered. The transport
    must batch.
  - More than `MAX_MAILBOX_NOTIFICATIONS` entries fail the read; narrow the range.
  - The event indexes nothing, so this read reveals nothing about which
    receiver is reading.
- `MailboxDiscovery::source` with `with_source`, and `MailboxSource`:
  - `Contract` (default): the one-call read.
  - `Logs { from, to }`: pages a block range. A reversed range is `InvalidPolicy`.
  - `Senders(codes)`: probes senders an earlier `Logs` pass left unregistered.
    A later range never sees those announcements again.
- `MailboxDiscovery::min_funded` with `with_min_funded`: `RegisterFunded`
  registers a sender only when its probe finds at least this many Qits. The
  default of zero keeps the old behavior.
- `QiError::TransactionMessage` and `StorageError::LimitReached`.
- `Provider::logs_served_through`: a range's logs, read in one guarded batch
  with the header of the range's last block, so the answering node is shown to
  have the whole range.
- `estimated_gas()` on `AccountReplacementQuote` and `PreparedAccountReplacement`:
  the gas a fee replacement needs now, so a wallet can see the headroom left
  under the parent's fixed gas limit.

## 0.1.0-alpha.4

Hardening and speed from a full-workspace review, and the SDK pieces a desktop
wallet needs first.

Security:

- `sign_qi_message` refuses bytes that a protobuf decoder, reading them the way
  the node does, would take as a transaction with inputs: oversized,
  non-canonical and mistyped encodings included
  (`SignerError::QiTransactionMessage`, and `has_transaction_inputs` to check
  first). The check walks the bytes without allocating. Qi messages and single-input spends
  are both BIP340 over Keccak of the raw bytes, so signing such a "message"
  produced a valid signature for that spend. **Every earlier release signs
  them; do not sign third-party bytes as a Qi message with 0.1.0-alpha.1
  through alpha.3.**
- Mailbox discovery no longer registers announced senders on its own.
  Announcements are unauthenticated, and dust makes any sender look funded, so
  by default (`MailboxRegistration::ReportOnly`) it probes each new sender over
  `MAILBOX_PROBE_GAP` (five) addresses, reports what it found, and persists
  nothing. `RegisterFunded` registers senders whose probe finds funds, for a
  restore. `MAX_MAILBOX_NOTIFICATIONS` rises from 4,096 to 32,768, so
  announcing past the old bound no longer disables discovery for good.
- Legacy `SqliteStore::allocate_address` gives back the unexamined part of its
  burned range on success, only if the scope generation is unchanged, so an
  address another writer recorded meanwhile is never issued again. A large
  `max_attempts` used to skip about `max_attempts / 512` matching addresses per
  allocation, so a gap-limited restore stopped before later addresses.
- A failed Qi prepare leaves a change pool lent by `&mut` whole, so failures no
  longer push later change beyond a restore's gap.
- JSON-RPC responses and WebSocket frames that are not valid UTF-8 are
  rejected, including bytes inside fields the SDK ignores (found by fuzzing).
- Zone headers with an all-zero hash are rejected.
- `discover_qi` rejects outpoints whose hash is from another zone.
- `refresh_qi` labels a snapshot only when the tip did not move across its
  reads, making up to three attempts and then failing with
  `QiError::StaleSnapshot`, so a coin orphaned mid-refresh is never
  offered for selection.
- Secret-handling fixes in mnemonic, keystore and fetch credential paths.
- With a gap limit set, mailbox probes read at most `2 * MAILBOX_PROBE_GAP`
  addresses, so a sender funding every few addresses cannot keep a probe
  running for a full page.
- Known limitation: the mailbox returns every announcement in one call, so
  enough announcements (about 5,400, or fewer long strings) push the response
  past the 2 MiB limit and `discover_mailbox_channels` fails for that receiver
  from then on. Funds are not at risk; register a known sender directly with
  `SqliteStore::import_payment_channel`.

Breaking:

- Every public error enum is `#[non_exhaustive]`; match with a wildcard arm.
  So are `ReplacementReason` and `RpcSignerFailure`. Outcome enums that gate a commit or signing decision (`ScanStop`,
  `WindowStop`, `CanonicalStatus`, `ActivityStatus`, `ChannelRegistration`,
  `ErrorClass`, candidate statuses) and
  spend-limit policies (`FeePolicy`, `QiPolicy`, `ReplacementPolicy`,
  `SelectionRequest`) stay exhaustive; see `docs/architecture.md`.
- The `*Config` structs (`HttpConfig`, `WsConfig`, `FetchConfig`,
  `EventHubConfig`, `WaitConfig`, `CodeWaitConfig`, `BrowserConfig`,
  `BrowserSocketConfig`, `BrowserWaitConfig`), `KdfLimits`, `DeriveLimits`,
  `HeadFollowPolicy`, and the scan options and requests (`QiDiscoveryOptions`,
  `QiScanOptions`, `PaymentScanOptions`, `DiscoveryRequest`, `EtxScanRequest`,
  `AccountReplacementScanRequest`) are `#[non_exhaustive]`. Build them with
  `Default::default()` or `new` and the `with_*` methods.
- Reports and updates are `#[non_exhaustive]`: `DiscoveryReport`,
  `BranchCoverage`, `QiScanReport`, `CurrentQiDiscovery`, `ObservedQiBalance`,
  `PaymentScanReport`, `MailboxDiscoveryReport`, `MailboxChannelScan`,
  `RestoreReport`, `AccountMergeReport`, `QiMergeReport`,
  `HeadUpdate`, `SettlementUpdate`, `FamilyUpdate`, `WalletReplayUpdate`,
  `PersistedWalletReplayUpdate`, `DeploymentUpdate`, `AccountOperation`,
  `QiOperation`, `ReorgInvalidation`, `HeadReplayState`, `HeadReplayCommit`,
  and `RpcSignerError`.
- `discover_mailbox_channels` takes a `&MailboxDiscovery` request (`start`,
  `max_channels`, `options`, `registration`) in place of three arguments.
  `MailboxChannelScan` reports `registration: ChannelRegistration` and the
  value `found`. An unreadable mailbox is `QiError::MailboxUnreadable`, not
  `IdentityMismatch`.
- `QiDiscoveryError::IdentityMismatch` is renamed `NetworkMismatch`.
- A failed network check returns a new `NetworkMismatch` variant of
  `AccountError`, `AccountPreflightError`, `QiError` and `DiscoveryError`
  instead of `IdentityMismatch`, `ObservationChanged` or `InvalidObservation`.
- A malformed `FeePolicy` (zero `max_gas`, margin over 100%) is
  `InvalidOperation` / `Invalid` on every path; some paths said `FeeLimit`.
- In a `quai_wallet` `DiscoveryError`, a reorg across the reads is the new
  `ObservationChanged` (class `Stale`) and a source's error keeps its class,
  instead of both surfacing as `InvalidObservation`.
- Internal `quai-*` dependencies are exact (`=`) requirements. A caret
  requirement on a pre-release also matches later pre-releases, so pinning
  `quai-sdk` exactly did not pin its siblings. Users of 0.1.0-alpha.1 through
  alpha.3 should pin every `quai-*` crate they depend on.
- `refresh_qi_address_book` reports a row missing from a batched read as
  `QiDiscoveryError::IncompleteObservation`, not `ObservationChanged`, and
  `scan_qi` and payment-channel scans report one as the new
  `QiError::IncompleteObservation` (class `Transient`).
- Legacy keystore import enforces minimum KDF strength by default
  (`KdfLimits`: scrypt N·r·p at least 2^20, PBKDF2 at least 100,000 rounds,
  salt at least 16 bytes). A weaker document fails with the new
  `KeystoreError::WeakParameters`; opt out with
  `KdfLimits::without_strength_floors()`. `DeriveError::WeakParameters`
  reports a `DeriveLimits` floor violation; those floors are off by default.
- Genesis mismatches in the head tracker, deployment and code waits, and
  conversion tracking are the new `ProviderError::GenesisMismatch` (class
  `NetworkMismatch`), not `InvalidResult`.

Added:

- `ErrorClass` (`Transient`, `Stale`, `NetworkMismatch`, `Ambiguous`,
  `Invalid`, `Cancelled`, `Storage`) and `class()` on the errors background
  sync meets, so a sync loop can decide to retry, re-observe, stop or
  reconcile. `RpcSignerError::class()` is `Ambiguous` once a request was sent.
- `SqliteStore::activity`: outgoing operations with status and decoded
  amounts, from the store's own records. Only outputs to the wallet's Qi BIP44
  addresses count as change.
- `DynTransport`, so one `Provider<DynTransport>` type can hold any native
  transport chosen at runtime.
- `Provider::account_states` (returning `AccountState`), `Provider::headers`,
  `Provider::headers_on_network` and `Provider::expected_chain_id`, with the
  bounds `MAX_ACCOUNT_STATES`, `MAX_OUTPOINT_ADDRESSES` and `MAX_BATCH_CALLS`.
- `AccountPublic::search_window` returning `SearchWindow` and `WindowStop`,
  `search_window_async` (yields between slices) and, with the `rayon` feature,
  `search_parallel` and `search_window_parallel`. `Grinding::Parallel` on scan
  options uses them: about 3.5x faster on six cores, with identical results.
  `quai-sdk` gains a `rayon` feature. `MAX_SCAN_WINDOW` bounds a window.
- `GapCounter`, the shared BIP44 gap rule.
- `MailboxDiscovery`, `MailboxRegistration`, `ChannelRegistration` and
  `MAILBOX_PROBE_GAP`.
- `new` and `with_*` builders for every non-exhaustive struct a caller builds.
- `PublicKey::add_tweaks`, several tweaks with one shared field inversion.
- `ObservationSource::observe_many` for batched sources. Its default calls
  `observe` per address, so existing sources compile unchanged.
- `ReservationId` and `ReservationState` at the `quai_wallet` root.
- Fuzz targets for WebSocket dispatch and the provider's response parsers.

Changed:

- Change pools lent to `prepare`, `prepare_sweep` and `prepare_special` by
  `&mut` lose only the addresses a prepared transaction uses; passing by value
  behaves as before.
- `AccountSession` and `QiSession` futures are `Send`.
- Scans grind in slices and yield between them instead of blocking an
  executor thread for about a second per window. That does not free a tokio
  current-thread runtime or a browser page; run scans on a multi-thread
  runtime, a dedicated thread or a Web Worker.
- `scan_qi`, `discover_qi`, the generic `discover` and `scan_payment_channel`
  read addresses in gap-bounded windows. A completed scan queries exactly what
  a one-at-a-time scan would; an aborted one may already have read the rest of
  its window, at most 63 addresses a retry would read anyway.
- Scan use-checks run up to four at a time, in order, where they used to run
  one at a time: a node sees up to four concurrent requests from a scan.
- `HeadTracker::poll` batches its reads: one round trip when idle, three per
  new page of up to 126 headers, instead of five plus one per block.
- Network identity checks and header reads that nothing separates travel in
  one round trip; identity checks compare the chain ID locally.
- `refresh_qi` and `include_known_qi_addresses` read up to 1,024 addresses per
  `outpoints_many` call.
- Replacement conversions check the parent's gas limit against the same
  origin-cost budget the original preparation used, and compare the raw
  estimate against the fixed limit without applying the margin a second time.
- Faster address derivation (generator tables, a scan-specific derivation
  path, batched normalization) and cached SQLite statements.

## 0.1.0-alpha.3

Security, stability and wallet-usability fixes from a full project review.

- **Breaking:** `payment_channels::discover_mailbox_channels` now takes a `start`
  index and reports `next_start`. Previously every call processed the first
  `max_channels` announced senders again, so senders beyond that page were never
  scanned; because mailbox announcements are unauthenticated, anyone could hide a
  real sender's channel by announcing 64 codes first.
- Add `QiKeyring::load_payment_channels`, which loads receive keys for every channel
  registered to an owner. A channel left unloaded let coin selection choose its
  outputs and then fail the whole spend.
- Funded mainnet WQI round trip after prime block 2,237,000: wrap, claim, unwrap,
  a 10-block redemption lock, and a spend of the redeemed Qi
  (`test-infra/orchard/mainnet-wqi-roundtrip-2026-09-15.json`).
- Qualification harness: Qi stages load all registered payment channels, and mainnet
  payment amounts use a single Qi denomination.

## 0.1.0-alpha.2

Documentation-only release; no API or behavior changes from `0.1.0-alpha.1`.

- Crate READMEs no longer describe the packages as unpublished, and install
  instructions use the crates.io release with an exact alpha pin.
- Releases are now published by the tag-triggered `release` GitHub Actions
  workflow using crates.io Trusted Publishing.

## 0.1.0-alpha.1 — 2026-09-14

Initial modular Quai/Qi SDK alpha, published to crates.io from commit `bf315ee`
(tag `v0.1.0-alpha.1`) and compared against the published
`quais@1.0.0-alpha.57` artifact. APIs may change during alpha development.

- Add `AccountSession::observe_nonce`, combining durable candidate reconciliation
  with discovery of unregistered same-nonce transactions.
- Add Pelagus-compatible `payment_mailbox` announcements and
  `discover_mailbox_channels`; the SDK discovered and paid a real Pelagus wallet.
- Add `conversion_batch_discount_bps` for the pinned batch-wide conversion discount.
- Accept Qi refund outpoints whose creating ETX hash carries the Quai ledger bit.
- Allocate native change and payment destinations compactly, committing only
  examined children; add `continue_payment_channel`.
- Add explicit mainnet/Orchard WQUAI addresses and correct the Orchard example
  and funded harness to use the verified testnet deployment.
- Correct Quai-to-Qi preparation gas: include origin costs and conservative
  denomination fragmentation after discounts; preserve the raw estimate API.
- Twelve crates cover typed addresses/amounts, crypto, consensus serialization,
  RPC/providers, ABI/contracts, signers, HD wallets, payment codes, keystores,
  browser adapters and the `quai-sdk` facade.
- Qi discovery defaults to 50 matching receive/change addresses, with fixed
  denominations, explicit deep ranges and mixed HD/imported/BIP47 ownership.
- Native and browser workflows support account/Qi preparation, exact fee checks,
  signing, submission, replacements, conversion/wrapping and settlement observations.
- Durable reservations, immutable signed candidates, authenticated full-wallet
  backups and coordinated IndexedDB restore support restart and recovery.
- Utility parity includes ten BIP39 wordlists, public derivation, bounded KDFs,
  curve helpers, threshold aggregation, fixed-point checked/wrapping arithmetic,
  transaction interchange, ABI reflection, typed data and resource fetching.
- Explicit Rust differences correct reference bugs and replace dynamic JavaScript
  lifecycle/coercion with typed values, bounded resources and caller-owned futures.

The [complete SDK guide](SDK_DOCUMENTATION.md) describes usage. The
[current comparison](SDK_PARITY_ANALYSIS.md) records the audited scope, deliberate
omissions, source corrections and useful Rust additions. The
[publishing guide](docs/PUBLISHING.md) describes the registry metadata and ordered
upload procedure. All twelve crates are on crates.io, with API documentation
built on docs.rs.

This alpha is not production-qualified. Unmodified funded-node execution, mature
redemption spend, broader engines/extensions, sustained reliability/fuzz campaigns
and independent security review remain outside the established evidence. Read the
[security policy](SECURITY.md) before integration.
