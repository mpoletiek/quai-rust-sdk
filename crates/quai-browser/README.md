# quai-browser

Concrete wasm32 adapters for browser Fetch, WebSocket and an explicitly supplied Quai wallet provider. This alpha adds browser transport and a small permission API; it composes with the portable wallet sessions in `quai-sdk` and supplies IndexedDB custody and recovery adapters. Native SQLite storage is a separate backend.

The crate exposes `BrowserConfig`, `BrowserSocketConfig` and sanitized `BrowserError` on all targets. `BrowserFetchTransport`, `BrowserWebSocketTransport`, `BrowserSubscription`, `InjectedProvider`, `InjectedSubmissionTransport` and `fill_random` exist only on wasm32. There is no native browser stub silently substituting another transport. The transport adapters implement the existing non-`Send` wasm `quai_rpc::Transport` contract and can be used with `Provider` and explicit `Routing` without enabling native HTTP or wallet features.

## Fetch

```rust,ignore
use quai_browser::{BrowserConfig, BrowserFetchTransport};
use quai_provider::Provider;
use quai_primitives::Zone;
use quai_rpc::{Routing, U256};

let transport = BrowserFetchTransport::new(BrowserConfig::default())?;
let provider = Provider::new(
    transport,
    Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into())?,
    U256::from(9), // Explicit expected network, chosen by the application.
);
let genesis = provider.genesis_hash(Zone::Cyprus1).await?;
```

Direct routing preserves the URL and port, corresponding to `use_pathing: false`. For a remote gateway, resolve its shard path with the existing routing API, such as Orchard `/cyprus1`. Fetch obeys browser CORS, mixed-content and private-network access policies; endpoint owners must permit the application's origin. Native HTTP reachability does not establish browser reachability.

Requests use POST JSON, `credentials: omit`, no referrer, `cache: no-store`, and `redirect: error`. Response bodies are streamed into a byte budget before UTF-8 decoding and strict JSON-RPC envelope validation. Duplicate envelope fields, wrong IDs, conflicting result/error and malformed UTF-8 fail closed. Request serialization also has a byte limit. Each request owns an AbortController; timeout, returned-future drop and completion abort it and release local capacity. No automatic retries occur. Aborting a submitted RPC does not prove that the node did not execute it.

Default limits are 10 seconds per RPC, 1 MiB outgoing JSON, 2 MiB response data and eight concurrent operations shared across clones. Excess concurrency fails immediately with `AtCapacity`/`Busy`. Limits describe encoded payloads; JS strings, parsed objects and temporary buffers occupy additional memory. Browser timer throttling or page suspension can delay deadlines; these are event-loop deadlines, not guarantees while execution is suspended.

## WebSocket

```rust,ignore
use quai_browser::{BrowserSocketConfig, BrowserWebSocketTransport};
use quai_rpc::Endpoint;
use serde_json::json;

let socket = BrowserWebSocketTransport::connect(
    Endpoint::parse("ws://127.0.0.1:8200")?,
    BrowserSocketConfig::default(),
).await?;
let mut heads = socket.subscribe(json!(["newHeads"])).await?;
let head = heads.next().await?;
heads.unsubscribe().await?;
socket.close();
```

The non-`Send` transport works in windows and dedicated workers without Tokio.
Compose it with `Provider` and routing to the exact connected endpoint; a different
endpoint is rejected. Raw subscribe/unsubscribe RPCs are rejected by `Transport`:
use the subscription API so remote registrations have bounded local ownership.
`close()` closes all clones and subscriptions; last-owner drop closes the session.
`is_open()` reports local connection state, not node liveness or chain identity.

Requests share the Fetch configuration defaults: eight in flight, ten-second
opening/request/notification deadlines, 1 MiB outgoing and 2 MiB incoming messages.
Socket defaults add 16 subscriptions, 64 queued notifications per subscription
and 8 MiB of total queued notification bytes. Configuration bounds outgoing socket
buffers and completed response/notification delivery budgets. A completed request
retains its Rust capacity permit until its future completes or is dropped.
The browser allocates complete WebSocket messages before the SDK can inspect them;
these limits cannot bound the browser's frame assembly or native networking memory.

Registration happens before delivering the subscribe response, preserving an
immediate notification. Queue overflow poisons that stream with `SubscriptionLagged`.
Cancelling after a notification has been delivered but before Rust consumes it also
marks the stream lagged; cancelling a still-waiting read releases its waiter.
A quiet read timeout leaves the subscription usable. A cancelled subscribe closes
the session because its remote ID may be unknown. Dropping a subscription sends one
bounded cleanup request; failed, cancelled, oversized or capacity-blocked cleanup
closes the shared session. Explicit unsubscribe requires a successful node response.

Malformed envelopes, wrong/future IDs, duplicate known JSON fields and binary or
oversized messages fail closed. Cancelled request replies are never reassigned.
No reconnect or write replay occurs. On disconnection/lag, applications must open a
new session and reconcile canonical history through the provider before treating
new notifications as continuous. Browser WebSocket controls cookies, Origin, TLS
and handshake policy; it does not expose Fetch's credential/redirect controls or
arbitrary handshake headers. Page suspension can delay timers and notifications.

## Injected wallet

An application passes its chosen JS provider object into `InjectedProvider::new(provider, endpoint_identity, shard, expected_chain_id, config)`. The adapter does not search global wallet objects, auto-select a wallet or call wallet methods during construction. The endpoint serves as an exact routing identity; requests go to the injected provider with the pinned quais.js `{method, params, shard}` shape, where the shard is encoded as `0x00`, `0x10`, etc.

Every high-level operation checks `quai_chainId` afresh. Generic `Transport`/`read` uses a narrow read-method allowlist and rejects wallet permission, signing and sending methods. A chain check detects configuration changes but cannot authenticate a malicious provider or make several requests atomic.

The explicitly invoked wallet APIs are:

- `accounts()`: read already exposed Quai accounts, without requesting access.
- `request_accounts()`: call `quai_requestAccounts` after checking the chain. The application must invoke this from its approved connection flow.
- `personal_sign(address, bytes)`: check chain, account exposure and the configured sender zone, then call `personal_sign([hex_message, lowercase_address])` once. The returned value must be a 65-byte hex signature with a supported recovery byte. The adapter validates low-S scalars and recovers the requested account from the exact personal-message hash. The application still owns message semantics.

- `sign_typed_data(address, data, policy)`: validate a bounded immutable EIP-712 document, require a matching domain chain (or explicit unbound policy), check account exposure, call `quai_signTypedData_v4` once, and recover the returned signature against the exact cached document hash. RPC serialization includes the canonical domain schema and uses strings for integer values.

Provider errors retain only a safe numeric code, including EIP-1193 user rejection 4001 and unsupported method 4200. Provider messages, data and request payloads never enter error strings. Injected results use a bounded JSON-only serializer that rejects cycles, accessors, custom prototypes, BigInt and unsafe integer numbers; chain/amount quantities remain hex strings. Memory already allocated by an injected provider is outside this adapter's control.

Dropping or timing out an injected request ends this adapter's wait. EIP-1193 has no cancellation primitive that retracts a wallet approval dialog or operation. No account/signature request is retried automatically. Generic transport calls remain restricted. Direct wallet-mediated sending, exact transaction signing and opt-in signed-payload submission are described below; chain switching, permission enumeration/revocation and wallet discovery remain unimplemented. Passive account/chain/disconnect
monitoring is available through `context_revision()` and
`monitors_context_events()`. A changed context rejects an in-flight signature;
providers without removable event listeners are rechecked explicitly.

## Injected transaction signing and submission

`InjectedProvider::sign_quai_transaction(address, &transaction)` requests
`quai_signTransaction` for an already-populated type-0 transaction. It checks the
chain, configured sender zone, exposed account and complete request budget before
signing. The returned canonical signed protobuf must recover the requested sender
and match every unsigned field: chain, nonce, destination, value, gas, gas price,
data and ordered access-list entries/storage keys. The adapter rejects wallet
changes to those fields and any changed account/chain context. There is no implicit
nonce allocation, fee estimation, signing retry or broadcast. Unsupported method
4200 and user rejection 4001 remain sanitized numeric errors.

For explicit submission, create `injected.signed_submission_transport()` and
compose it with the ordinary `quai_provider::Provider` and exact endpoint routing.
This capability forwards `quai_sendRawTransaction` only after validating canonical
signed Quai or Qi bytes, chain and origin zone. The original injected transport
keeps its read-only allowlist. Ordinary Qi, Qi conversion and Qi wrapping variants
use the same high-level broadcast APIs as native transports:

```rust,ignore
let signed = injected.sign_quai_transaction(address, &transaction).await?;
// Persist the exact signed bytes and claims in the application's wallet state.
let provider = quai_provider::Provider::new(
    injected.signed_submission_transport(), routing, expected_chain_id,
);
let acknowledgement = provider.broadcast(&signed).await?;
```

The provider checks the acknowledged hash against the locally computed ID. A
remote error, timeout, malformed/wrong acknowledgement or context change after
submission begins preserves that expected ID as an ambiguous outcome. No write is
retried. Keep the ID and signed bytes before awaiting: dropping a future cancels
only the local wait and does not retract submission or an extension's approval UI.

For exact signing, Rust separates signing from verified raw submission. The direct
wallet-mediated alternative below uses `quai_sendTransaction`. Applications own
preparation, durable reservations and confirmation/recovery. A real extension may
not implement signing or raw submission: these methods are tested with an explicit
synthetic provider in Chromium, not qualified against a live wallet extension.
Injected Qi signing remains unsupported by the pinned reference; local verified Qi
signatures can be submitted through the explicit transport.

## Wallet-mediated sending

Some extensions expose `quai_sendTransaction` while declining offline signing.
`InjectedProvider::send_quai_transaction(address, &transaction)` supports this
flow with a fully populated request and explicit chain/account checks. It does
not request additional accounts, populate omitted fields or retry the send.
Retain `WalletSendIdentity::new(address, &transaction)` before awaiting: it stores
the requested account, chain, nonce and unsigned signing digest. **That digest is
not a transaction ID.** Keep the full request in application state as well.

A successful call returns `WalletSendAcknowledgement`, containing the original
request and a structurally valid wallet-reported hash. It is not proof of exact
signing or inclusion. `acknowledgement.observe(&provider)` makes one readonly
lookup and reconstructs canonical signed bytes from the RPC transaction fields.
The recovered sender and computed hash must match the node's claimed identity.
`matches_request()` then explicitly reports whether every field and sender match
the original request. The actual verified signed transaction remains available
when the wallet changed fields; changes are never silently treated as exact signing.
Node inclusion metadata still needs separate canonical receipt/confirmation checks.

```rust,ignore
let identity = quai_browser::WalletSendIdentity::new(address, &transaction)?;
// Persist identity and the complete request before dispatch.
let acknowledgement = injected.send_quai_transaction(address, &transaction).await?;
// Persist the reported hash for later readonly reconciliation.
if let Some(observed) = acknowledgement.observe(&provider).await? {
    let exact = observed.matches_request();
    let actual = observed.signed();
    // Present any wallet changes and track this actual transaction's receipt.
}
```

`WalletSendError::Preflight` means no transaction request was dispatched. Once
dispatch begins, failures—including wallet rejection, timeout and context change—
are conservatively `Ambiguous`, retaining the original request identity and any
parseable reply hash. Dropping the future does not retract an approval or send.
Without a returned hash, consult the wallet's activity and the account's node
history; the requested nonce/digest alone cannot prove what the wallet submitted.
Do not automatically retry or release claims based on an absent transaction.
A lookup returning `None` means not indexed at that node, not definitively dropped.
`WalletSendAcknowledgement::from_reported_hash` reconstructs the unverified handle
from the saved request and reply hash after restart or an ambiguous error; it still
requires readonly signed-transaction verification.

The pinned Pelagus source exposes wallet-mediated sending but does not forward
all request fields (including nonce/access list) in that path. This is why exact
matching is an observation result, not a promise made at request time. Current
runtime tests use a synthetic selected provider; real extension interoperability
remains unqualified. The request method and quantity/access-list shapes follow
pinned quais.js; automatic unbounded post-send polling is replaced with explicit
bounded reads. Chain switching/discovery and full browser wallet state remain open.

## Browser boundaries

| Responsibility | Current behavior |
| --- | --- |
| Entropy | `fill_random` explicitly calls secure-context Web Crypto, at most 65,536 bytes, and fails without it. No fallback PRNG or automatic key creation. |
| Timers/cancellation | Native browser timers plus AbortController; no Tokio runtime. Suspended pages can delay timers. |
| CPU work | No mining, address grinding, mnemonic derivation or heavy signing runs automatically. Applications must place expensive work in a dedicated worker with explicit progress/cancellation. The bridge uses `globalThis`; dedicated-worker tests verify Fetch, timeouts and entropy without Window or Tokio. Additional worker tests execute HD Qi grinding, BIP340 signing/verification, ECDSA recovery and OS-random key generation. This is a bounded runtime slice, not a general worker job/persistence API. |
| Persistence | `BrowserSnapshotStore` stores opaque bytes using scoped, revision-checked IndexedDB transactions. Native SQLite is not linked. Applications own encryption, serialization and wallet reservation/state integration. |
| Trust | The application chooses its origin, CSP and injected provider. Same-origin JS and wallet extensions remain privileged; this adapter cannot protect against a compromised application origin. |

## Verified tests and reproduction

On 2026-09-13 the crate compiled with Rust 1.97.1 for `wasm32-unknown-unknown` and ran in actual headless Chromium through wasm-bindgen-test 0.3.78. Tests use a separate loopback HTTP server with CORS plus a synthetic injected wallet object; they do not load a real wallet extension. Signing uses already-public fixture scalars; entropy tests use ephemeral unfunded keys.

Browser tests cover real Fetch path/query and ID validation, refused redirects, streamed oversize data, deadlines and future cancellation, concurrency capacity, exact injected shard and account/signing arguments, no automatic prompts, chain mismatch, user-rejection redaction, malformed/accessor results, unauthorized or wrong-zone signing, non-ASCII signatures, and secure entropy. Twenty-two dedicated-worker tests include five WebSocket tests and twelve injected transaction tests and verify Fetch/provider composition, timeout, Web Crypto, HD Qi grinding, verified legacy-keystore decryption and native Rust crypto running inside WebAssembly without Window or Tokio. The window suite passes 28 tests, including five real WebSocket tests and twelve transaction signing/submission tests shared with the worker suite. Twelve Node bridge tests additionally cover queue budgets and cancellation between JS delivery and Rust resumption. Additional native tests exercise limits, permission allowlists, bounded JSON serialization and envelope validation.

With Rust's wasm target, matching wasm-bindgen CLI 0.2.128 and Chromium/chromedriver installed:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.128 --locked
CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
  python3 crates/quai-browser/tests/run_browser.py
# Repeat with --suite worker for the dedicated-worker runtime tests.
# --suite sdk-worker exercises portable account/Qi discovery and Qi message signing.
# --suite sdk-contracts and --suite sdk-events exercise portable dapp workflows.
```

The runner creates and removes its own server, chooses an unused loopback port, and uses an isolated browser profile through chromedriver. `CHROMEDRIVER` and `WASM_BINDGEN_TEST_WEBDRIVER_JSON` can override browser setup. In this development environment, the matching official wasm standard library and prebuilt wasm-bindgen release tools were installed under `/tmp`; system Rust and browser profiles were untouched. Native tests and wasm clippy are separate commands; ordinary native `cargo test` does not execute browser tests.

The browser bridges are packaged at `src/bridge.js` and `src/socket.js` and imported through wasm-bindgen's module mechanism. Downstream wasm-bindgen packaging must copy its generated JS snippets alongside the wasm artifact. Browser-specific dependency code is target gated. Real injected-wallet interoperability, remote-node browser CORS, general worker orchestration, Firefox/Safari, and background-tab behavior remain unverified. Scoped IndexedDB CAS,
independent-tab conflicts and tombstones are tested; these checks do not establish
complete wallet restore/reservation semantics.

Sources: pinned quais.js `src/providers/provider-browser.ts` and `provider-jsonrpc.ts` define the Quai method names and shard extension; [EIP-1193](https://eips.ethereum.org/EIPS/eip-1193) defines the provider request/permission/error model. [wasm-bindgen browser testing](https://wasm-bindgen.github.io/wasm-bindgen/wasm-bindgen-test/browsers.html) describes the runtime harness, and [Fetch RequestInit](https://developer.mozilla.org/en-US/docs/Web/API/RequestInit) documents browser credential, redirect and cancellation controls.

## Scoped snapshots

`BrowserSnapshotStore::open` requires an explicit database and scope (chain,
genesis, zone and wallet identity). `read` returns opaque bytes plus their
revision; `compare_exchange` writes only if the expected revision still matches.
`None` bytes preserves a tombstone revision so stale tabs cannot recreate
deleted state. Clones share a handle, and dropping the last handle closes it.

Records are bounded to 16 MiB and the database to 2,048 scope slots. The adapter
does not encrypt bytes or interpret wallet state. Encrypt sensitive payloads
before persistence and reconcile wallet claims in the application's state layer.
Run `python3 crates/quai-browser/tests/run_storage.py` for real Chromium storage
checks in addition to the wasm transport and worker suites.


## Bounded receipt waiting in windows and workers

`wait_for_receipt(&provider, zone, transaction_hash, BrowserWaitConfig)` supports
non-Send Fetch, socket and injected read transports. Configure positive
`confirmations`, `timeout_ms`, `poll_interval_ms` and `max_polls`; there are no
implicit forever-wait defaults. Milliseconds must fit a browser timer
(1 through 2,147,483,647), the interval cannot exceed the timeout, and at most
100,000 complete observations are permitted. Each observation uses the provider's
canonical receipt/head checks and never submits a transaction.

An owned deadline timer races the complete polling operation, including stalled
RPC futures. An additional timer exists only during the interval between polls.
Dropping the outer future clears both timers and drops the active read future;
Fetch then aborts and releases its concurrency permit. No transaction is cancelled
by cancelling its observer. Node errors return immediately without retry.
`PollLimit` and `Timeout` retain the transaction identity, completed-poll count and
last completed poll's inclusion. A partially cancelled poll contributes no
inclusion to those fields. Confirmed execution failures retain their explicit
outcome rather than becoming successful transfers.

The adapter also checks `performance.now()` before polls and before accepting
results, so delayed timer callbacks cannot return success after observable clock
expiry. Background suspension can delay resumption; this is not a real-time
scheduler or proof of chain finality. Missing clock/timer facilities fail without
a wall-clock fallback. Same-origin application code remains trusted.

Nine actual Chromium worker tests cover portable observations, vanished/changed
receipts and canonical heads, delayed inclusion, explicit limits, provider errors,
stalled reads, dropped futures and cancellation of actual Fetch with immediate
reuse of its sole request permit. Node tests check timer bounds and cleanup. The
native waiter retains seven canonicality/timeout/cancellation regressions.

## Atomic multi-journal updates

`compare_exchange_snapshots` accepts 1–128 `BrowserSnapshotUpdate` entries in one
named database, including separate open connections. It rejects duplicate scopes,
cross-database targets, stale revisions, oversized combined payloads and new
slots beyond the shared 2,048-record limit. Tombstones consume slots. Existing
records can update at capacity. Any failure aborts the whole transaction; total
payload is bounded to 16 MiB. The single-record API uses the same implementation.
Cancellation after dispatch may commit all entries; re-read before retrying.

The facade's `browser_backups::merge_wallet_backup` validates owned recovery
merges across initialized HD, account, Qi and payment books before this commit.
See [atomic restore](https://github.com/mpoletiek/quai-rust-sdk/blob/main/docs/BROWSER_ATOMIC_RESTORE.md) and the
[SDK guide](https://github.com/mpoletiek/quai-rust-sdk/blob/main/SDK_DOCUMENTATION.md).


`wait_for_transaction` waits for indexed Quai/Qi/ETX inclusion without requiring
a receipt. It uses the same `BrowserWaitConfig` deadline and completed-poll
budget as receipt waiting, checks exact block position and refreshed transaction
fields/head, and stops active reads on drop. It does not infer finality, verify
signatures, discover unregistered Qi competitors or release custody claims.
Use `Transaction::verified_qi` separately when verifying RPC signature/hash
claims. See [transaction responses](https://github.com/mpoletiek/quai-rust-sdk/blob/main/docs/TRANSACTION_RESPONSE_PARITY.md).

General resource requests are available through `quai_rpc::fetch::FetchClient`.
Use `NativeFetch` on native HTTP or `quai_browser::BrowserResourceFetch` in Wasm.
Models, hooks, gateways, cancellation and bounded policy are documented in the
[resource guide](https://github.com/mpoletiek/quai-rust-sdk/blob/main/docs/FETCH_PARITY.md).
These operations are separate from JSON-RPC transport and submission policy.
