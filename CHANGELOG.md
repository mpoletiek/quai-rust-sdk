# Changelog

## 0.1.0-alpha.10

Breaking:

- Public structs that the SDK may extend are `#[non_exhaustive]`, so a field
  added later no longer breaks a caller's struct literal or pattern. That is
  the change 0.1.0-alpha.9 began for three types; it now covers the rest of
  the beta surface.
  - Outputs, which callers read but do not build, need no change: `Receipt`,
    `ZoneHeader`, `Log`, `Transaction`, `BroadcastResult`, the observation,
    selection, reservation, allocation and snapshot views, and about sixty
    more.
  - Inputs gained constructors, and their fields stay public for reading and
    assignment. `FeePolicy::new(max_gas, max_gas_price, max_total_fee)` with
    `with_gas_margin_bps`; `QiPolicy::new(max_fee, max_inputs, max_outputs,
    max_snapshot_age)`, defaulting to a zero starting fee and eight rounds, with
    `with_initial_fee` and `with_max_fee_rounds`; `AccountIntent::new(to,
    value)` with `with_data` and `with_access_list`;
    `SelectionRequest::new(zone, candidate_height, target, max_inputs,
    max_outputs)` with `with_fee`; `CandidateCoin::new(outpoint, address,
    denomination)` with `with_unlock_height`, `with_expires_at` and
    `with_reserved`; `LogFilter::new(zone, range)` with `with_addresses` and
    `with_topics`; `PortableWalletCapture::new()` with a `with_*` method per
    source; `CallRequest::creation` and `CallRequest::for_transaction` beside
    `CallRequest::new`; and plain `new` for `QiIntent`, `QiSource`,
    `QiQuoteRequest`, `QuaiConversionIntent`, `ReplacementPolicy`,
    `DeploymentSearch`, `AggregationPolicy`, `Snapshot`, `AddressObservation`,
    `ContractCodeTarget`, `AccountNonceCandidate` and `FeeData`.
  - Types whose fields a protocol or standard fixes stay exhaustive and can
    still be built with a literal: the transaction, outpoint, input, output and
    access-list value types, the protobuf DTOs, `NetworkScope`, `Checkpoint`,
    `IndexRange`, `Search`, `PaymentSearch`, `ExtendedKeyMetadata`,
    `TypedDataField` and `RemoteError`.
- CI now fails a change that breaks the published API unless the changelog's
  top section lists it under `Breaking:`. `test-infra/semver_gate.py` runs
  `cargo semver-checks` forced to a minor release, because on an unchanged
  pre-release version the tool assumes a major bump and checks nothing.

Security:

- `SqliteStore::open` creates a new database, and with it the WAL and
  shared-memory files, readable by its owner only. The store holds account
  xpubs, which reveal every address a wallet owns, and SQLite otherwise created
  it at 0644 under the common umask. An existing file keeps its permissions.
- ECDSA signing recovers the signer before returning, as Schnorr signing
  already verified. RFC 6979 derives the nonce from the key and digest, so a
  faulted signature released beside a correct one for the same digest reveals
  the key. Transactions were already covered by the sender check; personal
  messages and typed data were not.

Changed:

- Batched reads carry one leading `quai_chainId` guard instead of one at each
  end. A batch is one request that one backend answers, so nothing changes
  chains between its elements, and a gateway that splits a batch could route
  the payload away from a guard at either end, so the trailing guard protected
  nothing while adding a third of every single read's calls. A custom
  `Transport` that inspects batches now sees `[quai_chainId, …]`.
- Account preparation takes three round trips and seven calls instead of five
  and fifteen: the genesis check and gas price travel together, then the
  sender's nonce and balance, then the estimate. Nothing carrying an address
  is sent before the genesis matches. `Provider::gas_price_on_network` is the
  new joined read.
- `RpcAccountSigner` brackets each state-changing request with four reads
  instead of six, keeping a network check immediately before and after it and
  the account-exposure check on both sides.
- Qi prepare, sign, broadcast, sweep, conversion, wrap and replacement read the
  metadata of the addresses they touch rather than the whole table. Every row
  read is validated at about 30 µs, and a Qi wallet only ever adds addresses.
  The new `SqliteStore::public_addresses` is the targeted read. Conversion,
  wrap and sweep preparation also stopped deriving a private key per input on
  every fee round just to read a stored public key.
- Payment-code derivation computes children from the code's own key and chain
  code with the precomputed generator table, and a receive child's point from
  one shared tweak. Receive grinding, the restore path, measured 2.7 to 2.9
  times faster against the previous `bip32` path in the same run; send
  grinding, dominated by its ECDH, 1.3 times. Results are bit-identical,
  asserted against `bip32` differentially.
- A WebSocket subscription that overflows its queue or the shared
  notification byte budget ends alone with `SubscriptionLagged` and is
  unsubscribed on the node; the session's other subscriptions and requests
  continue. Dropping or unsubscribing an ended subscription no longer closes
  the session.
- WebSocket frames and HTTP batch rows are parsed once instead of twice.
- `KdfLimits::default().max_memory_bytes` is 256 MiB plus 64 KiB. The standard
  N=2^18, r=8 documents need 2 KiB over 256 MiB and were refused.
- Full-wallet backup encoding sizes its plaintext buffer with a counting pass
  instead of reserving the 16 MiB format maximum for every backup.
- Workspace debug and test builds optimize third-party dependencies; the
  keystore suite went from 106 s to 5.3 s. A consumer's profile is its own.

Added:

- `WsConfig::ping_interval`, 30 seconds by default: an idle connection is
  pinged and closed as disconnected after two intervals with no inbound frame,
  so a half-open socket fails instead of going silent.
- `HttpConfig::with_proxy` and `HttpProxy` for an explicit forward proxy.
  System proxy variables are still never read, `https` endpoints keep
  end-to-end TLS through `CONNECT`, and proxy credentials are redacted.

Documentation:

- Dated reviews, the original plan and its audit moved to `docs/history/`, and
  the README drops pinning advice for yanked versions and a dated test count.
  Packaged `THIRD_PARTY_NOTICES.md` copies link to the license texts by URL,
  since a relative link cannot resolve inside a published crate.

## 0.1.0-alpha.9

Breaking:

- `QiFeeQuote`, `QiFeeProfile` and `QiReplacementIntent` are
  `#[non_exhaustive]`, joining the error enums. The first two are outputs and
  need no change at a call site; build the third with the new
  `QiReplacementIntent::new(parent, change_indexes, change_outputs)`, and opt
  into destination aggregation with `aggregating_destination()` rather than the
  field. Both of the fields added in 0.1.0-alpha.8 broke downstream struct
  literals, and a node release that changes the reward math needs a second fee
  profile; neither should break a caller again.
- Contract deployment and account sends are bounded by
  `MAX_POOL_TRANSACTION_BYTES` (128 KiB) instead of the 1 MiB codec ceiling, and
  the check runs before a nonce is reserved. A payload between the two limits
  used to reserve a nonce and be signed, then be refused at submission, which
  consumed the nonce locally for a transaction that never reached the chain.
  Anything the node's pool would have rejected now fails earlier and more
  cheaply; nothing that the node would have accepted is newly refused.
- `WsSubscriptionKind` is `#[non_exhaustive]`. It gained the `Accesses` variant
  in this release, and the node's subscription registry will gain more.

Fixed:

- An oversized Quai transaction is refused before submission rather than after a
  round trip. The node's pool rejects anything above its `txMaxSize` with
  `ErrOversizedData` before any other validation, which for a deployment means
  the nonce was already reserved and the transaction already signed. The new
  `quai_consensus::MAX_POOL_TRANSACTION_BYTES` (128 KiB) records that bound,
  distinct from the codec's `MAX_TRANSACTION_BYTES`, and contract deployment
  planning now bounds init code against it. Qi transactions take a pool path
  with no size check, so this is Quai-ledger only.
- The inclusion-floor guard added in 0.1.0-alpha.8 reached only one of the two
  paths that build a conversion or a wrap. `QiSession::prepare_special` refused
  an explicit fee below the node's inclusion floor; the portable `quote_qi`,
  which is what the browser and WASM consumers use, accepted it and built the
  unmineable transaction the guard exists to refuse. All three paths — session,
  portable and replacement — now share one `inclusion_floor` check, so they
  cannot disagree about the same operation. `quote_qi` reports
  `QiPreflightError::FeeBelowInclusionFloor`, which was previously constructed
  only on the replacement path. An ordinary transfer creates no conversion ETX,
  has no such floor, and is unaffected.
- Secret text guards are sized by measuring the normalized length rather than
  reserving a multiple of the input. NFKD expands U+FDFA from three bytes to
  thirty-three, so the previous fourfold reserve grew the buffer, and a growing
  `String` frees each earlier allocation unwiped — leaving progressively longer
  prefixes of a seed phrase or passphrase on the heap for a core dump, swap file
  or same-user read. `Mnemonic::to_seed` took no length bound at all. Also
  covers `Mnemonic::parse`, the Chinese re-spacing path, and the
  `CustomMnemonic` phrase, passphrase and `to_lowercase` paths in `wordlist`.

Added:

- `Provider::estimate_conversion` returns the undiscounted rate quote, the
  node's discounted estimate and the `implied_slippage_bps` between them, as the
  new `ConversionEstimate`. This is the pair of numbers a wallet shows before
  asking the user to pick a slippage tolerance: what the rate alone gives, and
  what the cubic flow discount, the k-Quai discount and the node's ten percent
  floor leave of it. All three RPCs were already wrapped; nothing assembled
  them. Both halves are read at the current head on purpose, because
  `quai_quaiToQi` resolves its rate from the current head whatever selector it
  is given. It prices one transaction, as the node's RPC does, so it is a lower
  bound on realized slippage and not a prediction — the node discounts a whole
  prime-block batch together. Size the tolerance with
  `conversion_batch_discount_bps`; see
  [docs/conversions.md](docs/conversions.md).
- `quai_consensus::conversion_held`, with `KAWPOW_FORK_BLOCK`,
  `SHA_EQUIVALENT_DIFFICULTY_FORK_BLOCK` and `KQUAI_CHANGE_HOLD_INTERVAL` beside
  it. go-quai v0.56.0 hard-codes two windows in which it refuses every
  conversion, one after each k-Quai controller change. **Both directions are
  held and they fail differently**: Qi-to-Quai is refused at pool admission, so
  nothing is spent and the same signed bytes work later, while Quai-to-Qi is
  refused inside the EVM in `CreateETX`/`opConvert`, so the transaction is mined
  and its nonce and gas are burned. Wrapping is exempt either way. Both windows
  are behind
  mainnet and cannot recur there, so the SDK records the rule rather than
  enforcing it: a chain still below one runs parameters a matching chain ID does
  not attest, and nothing is at risk either way, since the same signed bytes
  become acceptable once the window passes. Callers targeting such a chain can
  check before building. This is not a claim that controller changes halt
  conversions in general.
- `WsSubscriptionKind::Accesses { address }` subscribes to `["accesses", addr]`,
  the per-account notification the reference wallet drives its refresh from:
  every block touching that account on either ledger. Previously reachable only
  as an untyped `Raw` frame. The address is sent in the mixed-case checksum
  form, as the reference sends it. A disconnect stays terminal and accesses
  that happened while disconnected are not replayed, so reconcile before
  resubscribing; see [docs/WALLET_WORKFLOWS.md](docs/WALLET_WORKFLOWS.md).

Security:

- `RpcAccountSigner::unlock` refuses to put a keystore password on an
  unencrypted transport, with the new `RpcSignerFailure::InsecureTransport`,
  before dispatch. `Endpoint::parse` accepts `http` and `ws`, over which that
  password is readable on the path and unlocks every account in the node's
  keystore for the requested duration. `RpcAccountSigner::allow_insecure_unlock`
  opts back in for a loopback or private-network node, which is what the local
  harnesses use. No other call on the adapter is gated, because none carries a
  durable secret.

Changed:

- Coin selection deduplicates its snapshot in a hash set rather than an ordered
  one, and fee convergence validates and buckets that snapshot once instead of
  once per round. The ordered insert dominated selection at large snapshots, and
  a `select_with_fee` that re-validated every round made eight rounds cost eight
  selections. Neither changes which coins are chosen: ordering comes from the
  denomination buckets, not the dedupe set, and the 69 pinned JavaScript
  selection vectors are unchanged. `OutPoint` now derives `Hash`.
- The parallel address grind derives in batches, as the sequential grind always
  has. Both rayon paths called the single-index derivation inside their parallel
  loop, so the path the README advertises as roughly four times faster was the
  one paying a full point inversion per candidate instead of one per batch of
  thirty-two. Chunking the range restores the batch without changing which
  address is found; the existing sequential/parallel equivalence tests cover it.
  `window_scan` gives the parallel *window* path bench coverage it never had;
  the single-address `find_map_first` path is still unbenched.
- `compatibility` pins the reference package's compiled half as well as its
  source. `quais@1.0.0-alpha.57` ships a `lib/` build stamped `1.0.0-alpha.52`,
  and the export map resolves the package to `lib/`, so the oracle, every
  generated fixture and the declaration inventory observe alpha.52 while being
  labelled alpha.57. `npm run verify` now checks both `_version.js` files, an
  aggregate digest over the whole `lib/` tree, and the agreement between the two
  halves, so a future release that fixes the mismatch fails loudly instead of
  silently relocating the behavioral reference. Documented in
  `compatibility/README.md` and `SDK_PARITY_ANALYSIS.md`.
- `test-infra/go-oracle/SPECIAL-FEE-RESULTS.json` was regenerated against the
  pinned node. The committed artifact still recorded the 0.1.0-alpha.7 test file
  and omitted `TestSpecialFeeInclusionGasBound`, so 0.1.0-alpha.8's claim that
  its inclusion-gas bound "is checked against the pinned node" had no committed
  evidence behind it. The test passes; the transaction results are unchanged.
- A full protocol, security, performance and parity review against go-quai
  v0.56.0, `quais@1.0.0-alpha.57` and Pelagus 1.0 is recorded in
  [docs/PROTOCOL_ALIGNMENT_REVIEW_2026-09-20.md](docs/history/PROTOCOL_ALIGNMENT_REVIEW_2026-09-20.md).

## 0.1.0-alpha.8

Breaking:

- `qi_special_gas` takes a third argument, `destination_outputs`, between the
  output count and the UTXO set size: how many outputs create a conversion ETX,
  which is the count of Quai-ledger destination outputs. Two-argument calls no
  longer compile, and the count cannot be inferred from the other two.
- `QiFeeQuote` gains the public field `floor_qits`, and `QiReplacementIntent`
  gains the required public field `aggregate_destination`. Neither struct is
  `#[non_exhaustive]`, so any struct literal naming every field no longer
  compiles. Build a `QiFeeQuote` from `Provider::estimate_qi_special_fee` rather
  than by hand, and set `aggregate_destination: false` to keep the previous
  replacement behavior.

Fixed:

- `qi_special_gas` bounds both of the node's fee floors for a conversion or a
  wrap, not just one. Execution charges `QiToQuaiConversionGas` (100,000) once,
  which is what it modelled; inclusion charges `ETXGas` (21,000) for **each**
  Quai-ledger destination output, and that overtakes the flat charge past four of
  them. A quote priced on the smaller figure is valid but unmineable: the miner
  divides the fee by the inclusion gas, skips anything under the base fee, and
  neither reports nor evicts it. A mainnet wrap with twelve destination outputs
  needed 409,400 gas, was priced for 257,400, and sat pooled for a day. The bound
  is checked against the pinned node in `test-infra/go-oracle`. It takes a new
  `destination_outputs` argument, so callers must say how many outputs create a
  conversion ETX.

Added:

- `QiFeeQuote::floor_qits`: the same sample without the estimator's margin, which
  is the smallest fee that clears both floors for that exact shape. It is
  corrected like the quote itself, so converting it back never falls short of
  the fee it has to cover.
- `QiSession::prepare_special` refuses an explicit fee below that floor with the
  new `QiError::FeeBelowInclusionFloor`, rather than building something no miner
  takes. Only a chain state the profile does not cover leaves the fee unchecked;
  the explicit-fee path therefore now performs the estimator's reads and can
  surface a provider error, including head drift, where it previously performed
  none. Retry as with any other read in a prepare. The portable `quote_qi` does
  **not** carry this guard in this release; see 0.1.0-alpha.9.
- `quote_qi_replacement` and `QiSession::prepare_replacement` apply the same
  floor to a conversion's or wrap's replacement, with
  `QiPreflightError::FeeBelowInclusionFloor` on the portable path. Replacing a
  parent the miner skips must not produce a second candidate below the same
  threshold. A profiled quote already covers it.
- `QiReplacementIntent::aggregate_destination` re-decomposes a conversion's or
  wrap's Quai-ledger destination largest-first, keeping its address and total
  value, so a parent the miner keeps skipping becomes includable. Rejected for an
  ordinary transfer. `quai_wallet::denominate_largest` is now public for the same
  purpose.

## 0.1.0-alpha.7

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
([response](docs/history/WALLET_ASKS_2026-09-19_RESPONSE.md)).

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
