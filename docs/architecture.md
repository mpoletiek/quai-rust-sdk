# Architecture and current trust boundaries

The workspace implements parts of the [audited full plan](../QUAI_RUST_SDK_PLAN.md).
All crates remain unpublished. Implemented behavior and remaining acceptance
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
All crate publication flags remain disabled while these gates are open.

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
first and replacement headers oldest first. Missing history or a fork older than
the retained window returns `ReplayHistoryUnavailable` without changing the cursor.
A checkpoint at genesis uses the verified genesis hash; replay starts at block one.

With the `ws` feature, `WsHeadFollower` subscribes before replay, reconnects within
an explicit attempt budget, and polls after a bounded quiet interval. Notifications
only wake the numbered-header tracker; missed or duplicate notifications do not
skip blocks. No pending transaction or write request is replayed. Application
state must undo/apply each returned page and persist its checkpoint before asking
for another page. A cloned `HeadTracker` can be polled and adopted after a durable
application commit. The tracker itself is not a wallet database or consensus proof.
