# Provider construction, transport and event parity

This review covers `AbstractProvider`, `JsonRpcApiProvider`, `JsonRpcProvider`,
`BrowserProvider`, `SocketProvider` and `WebSocketProvider` in published
`quais@1.0.0-alpha.57`. Rust composes typed providers, transports, explicit routing,
remote signers and local event delivery. It does not reproduce their inheritance
or expose mutable JavaScript initialization promises and callback registries.

## Connection and request lifecycle

| Published API group | Rust behavior |
| --- | --- |
| Constructors, `initialize`, `_urlMap`, `_getConnection`, `connect` | `Endpoint`, `Routing::{direct, explicit, gateway, with_pathing}` and `Provider::new`; routes are fixed explicitly, with no default endpoint, port rewriting or discovery during construction |
| `_detectNetwork`, `_network`, `_getProvider` | Provider reads check the configured chain, and trusted workflows compare genesis. Creating a different network binding is explicit. The published `_getProvider` itself throws unsupported rather than switching networks |
| `_start`, `_waitUntilReady`, `initPromise`, `initResolvePromise`, `initRejectPromise`, `_initFailed`, `attemptConnect` | HTTP/Fetch configuration is offline; connect/read futures return typed results. Application control flow owns initialization success/failure. No mutable promise resolvers or persistent hidden retry flags |
| `createWebSocket`, `initWebSocket`, `waitShardReady`, `readyMap`, `ready` | Await native/browser `WsTransport::connect` or `BrowserWebSocketTransport::connect` per explicit endpoint. Handshakes have bounded deadlines. `is_open` reports local observed session state, not remote health or continuity; the application owns any per-shard map |
| `provider` | Explicit `Provider<T>` composition and borrowed provider access from contracts/signers, rather than a self-returning JS accessor |
| `_getOption`, `pollingInterval` | Typed `HttpConfig`, `WsConfig`, `BrowserConfig`, native/browser wait configurations and `HeadFollowPolicy`; construction validates bounds and callers retain chosen settings |
| `_setTimeout`, `_clearTimeout` | Scoped native futures/runtime timers and browser wait-owned timers; drop cancels the caller's wait and cleans local timer resources. No arbitrary global JS timer registry is part of the Rust provider API |
| `destroy`, `destroyed` | Explicit native WebSocket `shutdown`, browser socket `close`, local `EventHub::close`, and Rust ownership/drop. HTTP/Fetch lifetime is application-owned; dropping one clone does not destroy all clones |

The JS WebSocket implementation automatically reconnects. Rust permits explicit
reconnection and offers native `WsHeadFollower` with bounded attempts and numbered
canonical replay for read subscriptions. Browser applications reconnect explicitly
and compose the portable `HeadTracker`. No transport reconnect implicitly replays
a transaction submission. This difference is visible, not an assertion that every
backend can recover every missed event.

`Transport::request` is the extension boundary for `_send`, `_write`, `_perform`,
`send`, `getRpcRequest` and `_processMessage`. Native futures are Send; browser
implementations may use worker-local state. Built-in transports validate JSON-RPC
identity, limits and errors. Native HTTP and WebSocket transports use one correlated
request per call rather than the JS batching/stall queue. Applications can execute
independent calls concurrently within configured capacity. Injected browser
providers prohibit arbitrary signing through their read interface; signing and
submission have explicit methods/adapters.

Typed `CallRequest`, exact consensus transactions and Qi fee/operation requests
replace the JS `getRpcTransaction` / `_getTransactionRequest` normalization surface.
Provider operations validate and encode those requests; they do not coerce arbitrary
JS objects, Promises, decimal numbers or user-defined address objects. Explicit raw
transport calls use caller-owned JSON and do not acquire typed validation implicitly.
`LogFilter` and `LogRange` replace `_getFilter`; duplicate/invalid selectors and
out-of-range block numbers fail before network work.

`RpcError`, redacted `RemoteError`, `ProviderError` and explicit ABI revert decoding
replace `getRpcError` string heuristics. Numeric remote codes and deliberate access
to error data remain available, while arbitrary server payloads are omitted from
normal diagnostics. Failed submission/signing dispatch retains ambiguity; a timeout
or rejected reply does not imply a nonce can be reused.

`_wrapBlock`, `_wrapLog`, `_wrapTransactionReceipt` and `_wrapTransactionResponse`
map to detached typed parsing and the [response APIs](RESPONSE_PARITY.md). A response
has no hidden provider, no automatic retry and no implied signature/finality proof.
For transaction hashes, `Zone::from_byte(hash.bytes()[0])` and `Shard::from(zone)`
replace `zoneFromHash`/`shardFromHash`; arbitrary block hashes and storage words do
not acquire valid transaction routing merely because they are `Hash32` values.

## Reads and remote accounts

| Published read | Typed operation |
| --- | --- |
| `getBlock` | `mined_block` or `block_hashes` with explicit zone, selector and item budget; genesis and pending-header reads are separate |
| `getPendingHeader` | `pending_header_bytes`; the pinned node actually returns a bounded protobuf byte string rather than an ordinary execution-header JSON object |
| `getOutpointsByAddress` | `outpoints` for a typed Qi address; latest source claims do not establish historical completeness |
| `getOutpointDeltas` | `outpoint_deltas` with bounded Qi address list and explicit block anchors |
| `calculateConversionAmount` | `calculate_conversion_amount` with typed source/destination and exact amount; dedicated direction/rate functions are also available |
| `listAccounts`, `getSigner`, `hasSigner` | `Provider::accounts(zone, max_accounts)` or injected `accounts()`, then explicit address selection and `RpcAccountSigner::new/check_account` or injected signing methods |

Passive remote-account listing preserves reported order and accepts an explicit
1–1,024-item budget. Duplicate, oversized, malformed or Qi-ledger entries reject;
addresses can span zones, so signer construction requires a matching explicit scope.
Listing never requests additional wallet permission or proves private-key ownership.
The injected adapter's `request_accounts()` is separate and explicit. The published
browser `hasSigner(-1)` can return true; Rust uses a concrete typed address and does
not reproduce that index-coercion defect. Public read RPC services need not expose
remote wallet account/signing methods.

## Local events and transport subscriptions

`quai_provider::event_hub::EventHub<K, E>` provides runtime-independent, typed local
registration and fan-out. It owns no socket and spawns no tasks. Applications feed
bounded transport notifications or canonical `HeadUpdate` values explicitly, then
poll registrations in their chosen executor. Native WebSocket subscriptions expose
`recv`; browser subscriptions expose `next`. New-head, pending transaction, log and
explicit additional event subscriptions use their transport's subscription API.

```rust
use quai_provider::event_hub::{EventHub, EventHubConfig, EventPause, EventPoll};
# fn example() -> Result<(), Box<dyn std::error::Error>> {
let mut events = EventHub::<&str, u64>::new(EventHubConfig::default())?;
let listener = events.once("block");
let listener = listener?;
events.pause(EventPause::Buffer)?;
events.emit(&"block", &123)?;
events.resume()?;
assert_eq!(events.poll(listener)?, EventPoll::Event { value: 123, closed: true });
# Ok(()) }
```

| Published event operation | Rust local/transport composition |
| --- | --- |
| `on`, `addListener` | `EventHub::on` returns a monotonic local `ListenerId`; application explicitly starts the desired transport subscription |
| `once` | `EventHub::once` queues the first matching event only; the registration closes when that event is consumed |
| `emit` | Explicit `EventHub::emit(&key, &event)` returns the count queued; unknown keys and paused Drop mode return zero |
| `listeners`, `listenerCount`, `_forEachSubscriber` | `listeners` and `listener_count` report active local registrations; applications own their transport subscriptions separately |
| `off`, `removeListener`, `removeAllListeners` | `off(id)` or `remove_all(optional_key)` remove local queues; explicit unsubscribe/drop ends remote subscription ownership |
| `pause`, `paused`, `resume` | Buffer or Drop policy, pause-state inspection and resume; changing policy requires resume first |
| `_getSubscriber`, `_register`, `_recoverSubscriber`, `startZoneSubscriptions` | Explicit typed socket subscriptions, local registration and read-only reconnect/head-replay composition; no automatic cross-shard callback startup |

The hub defaults to 64 registrations, 64 queued items per listener and 4,096 total
queued items. Limits count items, **not dynamic heap bytes**; use bounded payloads
and `Arc`/`Rc` values when sharing large observations. Configured ceilings are 1,024
registrations, 1,024 items per listener and 65,536 total items. `emit` preflights all
matching queues before changing any queue; capacity failure leaves the supplied
event with the caller for explicit retry after draining or for gap recovery.
There is no silent eviction or partial delivery on a capacity error.

Buffered pause blocks polling while retaining existing/new events within limits.
Drop pause discards new events explicitly, preserving previously queued events.
A queued one-shot registration is inactive but still occupies a registration slot
until read or removed. IDs are not reused within a hub and are not interchangeable
between hubs. Closing a hub discards its local queues and cannot be undone.
Callback execution, arbitrary exception swallowing and JS `EventPayload` objects
are replaced by explicit polling and application control flow. Local pause does
not pause network I/O or prove that dropped source notifications are recoverable.
Published socket subscribers reject `pause(false)`; bounded local buffering is a
Rust convenience beyond that behavior.

## Qualification

[Four published-source tests](../compatibility/scripts/provider-lifecycle.test.mjs)
verify passive accounts, event order/once behavior, shared provider method bindings
and the socket buffering limitation. [Local event tests](../crates/quai-sdk/tests/provider_events.rs)
and [account/remote signer tests](../crates/quai-sdk/tests/rpc_signer.rs) run natively
and in Chromium workers. [Native WebSocket regressions](../crates/quai-rpc/tests/websocket.rs)
cover identity, lifecycle, ordering, immediate notifications, malformed messages,
cancellation, queue overflow and no replay. Earlier HTTP/browser/head-replay tests
remain evidence for their unchanged transport paths. One real-node WebSocket test
remains ignored in the local socket suite; mocks do not replace funded or external
wallet acceptance.
