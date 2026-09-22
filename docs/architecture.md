# Architecture and current trust boundaries

The workspace implements parts of the [audited full plan](history/QUAI_RUST_SDK_PLAN.md).
All crates are published on crates.io as `0.1` alphas. Implemented behavior and remaining acceptance
requirements are listed in [status](../IMPLEMENTATION_STATUS.md) and
[wallet gaps](WALLET_GAPS.md); this document does not imply a stable API.

## Dependency boundaries

- `quai-primitives`: addresses, shard/ledger values, fixed hashes, exact decimal
  conversion and deterministic deployment prediction. No keys or network.
- `quai-crypto`: secret-conscious ECDSA/Schnorr/local ordered aggregation and
  hash/HMAC operations using maintained Rust backends. No RPC or wallet state.
- `quai-consensus`: canonical bounded protobuf, unsigned/signing/signed hashes
  and verified immutable transactions. No inferred UTXO or chain-state validity.
- `quai-abi`: bounded EIP-712 schemas/documents; general ABI work remains active.
- `quai-wallet`: mnemonic/HD identity, authenticated seed backup, Qi selection,
  optional native SQLite public state and ongoing discovery/recovery work.
- `quai-signer`: local and watch-only authorization, chain binding and explicit
  typed-data domain policy. Remote/injected flows require separate adapters.
- `quai-rpc`: exact endpoint routing, transport contracts, quantities, optional
  bounded native HTTP/WS. Raw methods are an application-owned escape hatch.
- `quai-provider`: typed methods, identity observations, exact signed submission
  and canonicality-aware receipt polling. It does not authenticate remote nodes.
- `quai-browser`: browser Fetch/injected-provider implementation and execution
  qualification in progress. Native feature checks do not prove browser support.
- `quai-sdk`: facade and native account orchestration, without duplicating codecs.

## API evolution

Four categories, so the chain and the SDK can grow without a breaking release
where that is safe, and cannot where it is not:

- **Non-exhaustive:** error enums, resource `*Config` and follow policies, scan
  options and requests, and reports and updates. Errors and limits keep growing
  as the chain does, and a new report field is harmless to a reader.
- **Exhaustive, deliberately:** outcome enums that gate a decision to commit,
  release, sign or broadcast, such as `ScanStop`, `WindowStop`,
  `CanonicalStatus`, `ActivityStatus`, `ChannelRegistration`, `ErrorClass` and
  the candidate statuses. A new variant must fail to
  compile rather than fall into a wildcard arm in a wallet.
- **Exhaustive, deliberately:** caller-authorized spend limits, such as
  `FeePolicy`, `QiPolicy`, `ReplacementPolicy` and `SelectionRequest`. They have
  no safe default, so a new limit must fail to compile at every caller rather
  than take one silently.
- **Exhaustive:** protocol-fixed wire types, and structs a caller builds as input
  to a trait it implements, such as `AddressObservation`.

A non-exhaustive struct cannot be built with a struct literal or
`..Default::default()` outside its crate. So every one a caller builds ships a
`with_*` method for each field, plus `Default` or a `new` that takes the
required fields.

The public traits `Transport` and `ObservationSource` are unsealed, so a method
added to either must have a default.

## Protocol values

Raw `Address` accepts any 20 bytes with validated checksum syntax. Typed
`QuaiAddress`/`QiAddress` additionally require a known zone and proper ledger bit.
Known zones do not imply activation or ownership. `Hash32` is exactly 32 bytes.
Amounts use checked integer arithmetic; unsigned wallet parsers cannot accept a
negative payment. Decimal display compatibility is separate from chain ranges.

Quai/Qi use protobuf, not Ethereum transaction envelopes. Canonical signed
objects cannot be mutated without constructing/signing a new payload. A
nonallocating wire preflight caps message count before decoder allocation.
Signature recovery and ledger/location hash rules are tested against JS and Go;
spendability, maturity, fees, forks and acceptance require chain-state evidence.
Ordinary Qi rejects nonempty data and arbitrary output locks pending specialized
qualified builders. Ordered multi-input signing preserves duplicate public keys.

## Network and transport

Direct routing (`use_pathing = false`) retains the exact URL, port, path and query.
Text configuration accepts literal `true`/`false`, never string truthiness.
Gateway routes derive each suffix from an immutable base; explicit tables fail
on missing shards. Discovery cannot silently add routes or switch endpoints.

Providers check expected chain IDs at their selected endpoints. `genesis_hash`
validates the special root-location height-zero shape independently from zone
execution headers. A stored wallet scope binds chain ID, genesis and zone.
These checks catch configuration errors; a malicious node or changing load
balancer can still lie or produce inconsistent observations. Fork/capability
profiles and trusted historical indexing remain qualification work.

HTTP uses pooled rustls connections, total deadlines and bounded responses/
concurrency. Redirects, automatic retries and environment proxies are disabled.
WS multiplexes a bounded number of requests/subscriptions per explicit endpoint,
registers subscriptions before accepting notifications, and reports disconnect/
lag explicitly. It does not silently replay or backfill. See the RPC crate README
for conservative cancellation/drop behavior and remaining lifecycle gaps.

Remote messages/data and credential-bearing URLs are accessible only explicitly;
ordinary diagnostics omit their contents. Raw JSON-RPC methods can change node
state if the application chooses them. A cancelled future only stops waiting;
it cannot retract a request the server already accepted.

## Authorization and persistence

Native account preparation freezes the exact destination, value, data, access
list, fee parameters and allocated nonce. Signing validates that the result
matches the reviewed payload and stores verified signed bytes before returning
it. Submission reads those bytes, records Submitted before I/O and never retries
automatically. Failures preserve the expected transaction ID and durable claim.
See [account workflow](account-workflow.md) for balance and nonce-gap limitations.

SQLite scopes public metadata by chain/genesis/zone, validates schema identity,
uses immediate transactions, generation checks and atomic checkpoint/UTXO
snapshots. A metadata-only import invalidates cached scan state. Reservations
survive restart; only unsigned claims can be released. Signed payload recovery
revalidates canonical bytes, signature, scope and exact reserved nonce/input set.
Inclusion is an observation, not a storage-layer finality proof. Reorg recovery
must preserve ambiguous claims until verified reconciliation can resolve them.

Private keys are non-cloneable and redacted; explicit exports return guarded
secret types. The authenticated native seed backup uses Argon2id and XChaCha20-
Poly1305, fresh OS salt/nonce and bounded hostile KDF parameters. The large KDF
arena is explicitly zeroized. It is currently a seed backup, not a complete
wallet export: imported keys, channels and exposed-index bounds need full-state
backup semantics. The public SQLite database is not encrypted or rollback-proof.

## Qualification

Exact fixtures, deterministic loopback tests, real read/subscription tests and
full-cost backup tests cover implemented boundaries. They do not replace funded
acceptance, reorg/crash/fault testing, fuzzing, performance measurement, real
browser/macOS/Windows execution or independent protocol/security review.
The crates are published as alphas while these gates remain open.

### Injected wallet context changes

`InjectedProvider` passively monitors `chainChanged`, `accountsChanged` and
`disconnect` when the selected provider supports removable EIP-1193 listeners.
`context_revision()` lets applications invalidate their cached display and account
state. Adapter clones share one listener set; the last drop removes it. Reads and
signatures that span a context change return `ContextChanged`. No signature request
is retried. Providers exposing only `request` instead receive post-response chain
and account checks, with `monitors_context_events()` explicitly reporting false;
transient changes cannot be detected without event support. Account-access requests
allow the expected permission-driven account change and recheck the chain afterward.

### Canonical head replay and WebSocket reconnection

`HeadTracker` polls a configured provider from an explicit trusted checkpoint. It
retains 2–4096 block anchors and returns at most 256 new headers per page, checking
parent links and both page anchors. Reorganizations return removed blocks newest
first and replacement headers oldest first. A missing tip, a missing retained
anchor (including the newest) or a fork older than the retained window returns
`ReplayHistoryUnavailable` without changing the cursor. A block missing from a page
above a base this poll read from the node returns `ObservationChanged` (retry): it
lies at or below the tip just read, so the chain moved between the reads. Block one
missing above a genesis base, which is the trusted hash and never read, stays
`ReplayHistoryUnavailable`. A caller should cap retries of a repeated `Stale`
error before telling the user.
A checkpoint at genesis uses the verified genesis hash; replay starts at block one.

With the `ws` feature, `WsHeadFollower` subscribes before replay, reconnects within
an explicit attempt budget, and polls after a bounded quiet interval. Notifications
only wake the numbered-header tracker; missed or duplicate notifications do not
skip blocks. No pending transaction or write request is replayed. Application
state must undo/apply each returned page and persist its checkpoint before asking
for another page. A cloned `HeadTracker` can be polled and adopted after a durable
application commit. The tracker itself is not a wallet database or consensus proof.

### Browser persistence

`BrowserSnapshotStore` commits opaque public state or caller-encrypted envelopes
in IndexedDB. `BrowserStorageScope` separates chain, genesis, zone and wallet;
applications should bind the same scope into encrypted associated data. Whole
snapshots use atomic compare-and-exchange revisions, with committed completion,
size limits, quota/schema errors and tombstones that preserve revision history.
Concurrent tabs/workers must reload and merge after `StorageConflict`; the adapter
never silently overwrites another writer. Dropping an in-flight write does not
prove rollback, so read its revision before retrying. Clones share the database
handle, which closes after the last drop or a database version change. This is a
persistence boundary for application snapshots, not the native SQLite wallet's
schema or an implicit plaintext key store.
