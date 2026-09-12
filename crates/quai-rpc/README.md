# quai-rpc

Explicit shard routing and bounded native HTTP and WebSocket JSON-RPC transports. This crate is an unpublished alpha; live checks establish the narrow compatibility described below.

Enable `ws` for `WsTransport`, `WsConfig`, `WsSubscriptionKind`, and `WsSubscription`. The feature uses native Tokio and rustls with WebPKI roots, and does not enable wallet functionality. Browser WebSocket support is not implemented.

```rust,ignore
use quai_rpc::{Endpoint, Transport, WsConfig, WsSubscriptionKind, WsTransport};
use serde_json::json;

// Direct local/LAN node: use the exact WS port, without an appended shard path.
let endpoint = Endpoint::parse("ws://127.0.0.1:8200")?;
// A gateway instead supplies its resolved path, e.g.
// Endpoint::parse("wss://orchard.rpc.quai.network/cyprus1")?;
let transport = WsTransport::connect(endpoint.clone(), WsConfig::default()).await?;
let chain = transport.request(&endpoint, "quai_chainId", json!([])).await?;
// Compare chain with your expected network before further application operations.
let mut heads = transport.subscribe(WsSubscriptionKind::NewHeads).await?;
let notification = heads.recv().await?;
heads.unsubscribe().await?;
transport.shutdown().await?;
```

`Endpoint` is already resolved when passed to `connect`; use `RoutingConfig` with `use_pathing: false` for a direct node, or explicit gateway routing before connecting. A transport accepts only its exact configured endpoint, including path and query. Connect separately to each shard endpoint. HTTP redirects and environment proxies are not followed. Endpoint debug output and transport errors omit sensitive paths, queries, response bodies and underlying network error strings.

One connection multiplexes concurrent requests with unique, never reused IDs. Limits cover the entire request deadline including permit/queue waiting, pending request count, outgoing JSON size, incoming frame/message sizes, subscription count, per-subscription notification count and aggregate queued notification bytes measured before JSON decoding. The decoded JSON representation can occupy more memory than its encoded byte budget; callers should choose conservative limits. Defaults are 10-second connect/request deadlines, 16 requests, 16 subscriptions, 64 notifications per subscription, 2 MiB messages/frames and 8 MiB aggregate queued notification bytes. Dropping a request future cancels its wait, with pending capacity reclaimed by a bounded cleanup interval. Cancellation or transport failure cannot establish whether a state-changing request was accepted remotely; transports never replay requests.

`NewHeads`, `NewPendingTransactions`, and `Logs` use exact Quai subscription names. Logs support typed address and topic filters; `Raw` permits explicit additional event arguments. The endpoint selects the shard: no extra zone parameter is appended. Registration installs the server subscription ID before delivering its acknowledgement, so immediate notifications can be received safely. Notifications currently return `serde_json::Value`; semantic event DTOs, automatic reconnect, resubscription, durable cursors, log backfill and reorg reconciliation remain separate work.

Call `unsubscribe` to remove an individual subscription while keeping the connection. Dropping an active subscription, cancelling an in-flight registration/unsubscribe, or overflowing a subscription terminates the shared session, making continuity loss explicit. The overflowing receiver yields `SubscriptionLagged`; other consumers receive terminal disconnect errors. After a terminal error, `recv` returns `None`. Graceful `shutdown` is bounded by the request deadline and aborts the session task if it cannot finish. Last-owner drop also aborts the task. To recover, create a new connection and reconcile application state before resubscribing; no transparent replay occurs.

## Validation

The real loopback tests in `tests/websocket.rs` cover path/query preservation, exact endpoint checks, reversed concurrent responses, immediate notifications, exact pending/log arguments, graceful unsubscribe, notification count/byte overflow, request cancellation and late responses, registration cancellation, pending-request shutdown, deadlines including permit waits, inbound/outbound size limits, unsolicited/duplicate malformed envelopes and refused redirects. The opt-in live test checks chain ID, subscribes to `newHeads`, receives a header notification, unsubscribes and closes; it never sends transactions.

Verified on 2026-09-11:

- Direct LAN `ws://10.0.0.12:8200`: chain 9, actual `newHeads` notification, unsubscribe and shutdown passed. This complements the authorized direct HTTP node at port 9200, with pathing disabled.
- Orchard `wss://orchard.rpc.quai.network/cyprus1`: chain 15000, TLS handshake, actual `newHeads` notification, unsubscribe and shutdown passed. Earlier HTTP checks identify this gateway as go-quai v0.34.0-pre, older than the pinned implementation baseline.

Reproduce explicitly (requires network access):

```sh
QUAI_WS_URL=ws://10.0.0.12:8200 QUAI_EXPECTED_CHAIN_ID=9 \
  cargo test -p quai-rpc --features ws --test websocket \
  live_chain_and_subscription_handshake -- --ignored
```

Live pending-transaction/log event streams, Qi events, TLS failure fixtures, disconnect recovery/backfill and sustained-load/soak testing remain unverified. These narrow read-only checks do not establish funded transaction or mature local consensus-harness readiness.

## Protocol and dependency sources

Subscription names and envelopes follow pinned `quais.js` `provider-socket.ts` and `subscriber-connection.ts`, plus go-quai `quai/filters/api.go` at commit `f3f345c877300c044e3e0081a48bf3cf786fb9cc`. The native dependency is tokio-tungstenite 0.30.0: [async connect API](https://docs.rs/tokio-tungstenite/0.30.0/tokio_tungstenite/fn.connect_async_with_config.html), [TLS feature definitions](https://docs.rs/crate/tokio-tungstenite/0.30.0/features), and [frame/message configuration](https://docs.rs/tungstenite/0.30.0/tungstenite/protocol/struct.WebSocketConfig.html). Dependencies are pinned by the workspace lockfile; updating them requires rerunning the protocol and lifecycle checks.
