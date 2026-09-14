# General resource fetching parity

`quai_rpc::fetch` supplies the general resource operations exposed by the published
`quais@1.0.0-alpha.57` FetchRequest/FetchResponse APIs. It is separate from the
SDK's JSON-RPC transports and their submission/retry rules. Models, cancellation,
policy and hooks are always available; `NativeFetch` requires native `http`, and
`quai_browser::BrowserResourceFetch` runs in windows and dedicated workers.

```rust,no_run
use quai_rpc::fetch::{FetchCancellation, FetchClient, FetchConfig, FetchRequest, NativeFetch};
# async fn example() -> Result<(), quai_rpc::fetch::FetchError> {
let client = FetchClient::new(NativeFetch::new(3_000)?, FetchConfig::default())?;
let request = FetchRequest::new("https://example.org/public.json")?;
let result = client.send(&request, &FetchCancellation::default()).await?;
result.response.assert_ok()?;
let json = result.response.json()?;
# let _ = json;
# Ok(()) }
```

For Wasm, replace `NativeFetch::new(...)` with `BrowserResourceFetch`. Use
`FetchClient::with_hooks` to supply application middleware. Native futures are
Send; browser futures may hold thread-local handles. Applications own task and
concurrency policy; this client does not create an unbounded background queue.

| Published operation | Rust equivalent |
| --- | --- |
| Request URL, method and clone | `FetchRequest::new`, `url`, `set_url`, `method`, `set_method`, `Clone` |
| Body and intrinsic content type | `set_bytes`, `set_text`, `set_json`, `body`, `clear_body` |
| Header get/set/clear/iteration | `FetchHeaders`, `headers`, `headers_mut`, `get`, `set`, `clear`, `iter` |
| Credentials and insecure-auth opt-in | `set_credentials`, explicit Authorization header, `allow_insecure_authentication` |
| Gzip | Native `allow_gzip` negotiation/decoding; browser-managed compression |
| Timeout/throttle configuration | Immutable per-client `FetchConfig` |
| Preflight, process and retry functions | `FetchHooks`, `FetchAction::Return`/`Retry` |
| GetUrl override/create/register | Explicit `FetchBackend`, `NativeFetch`, `BrowserResourceFetch` |
| Gateway get/register/lock | Application-owned `FetchHooks::gateway` and immutable client construction |
| Data gateway | `data_resource`, also available through default client dispatch |
| IPFS gateway | `ipfs_resource` with a caller-selected HTTP(S) base URL |
| Send | `FetchClient::send`, returning `FetchedResource` |
| Cancel signal/listeners/check | `FetchCancellation`, registered task wakeup and future drop |
| Redirect | `FetchRequest::redirect`; bounded opt-in native client following |
| Status/message/body/headers | `FetchResponse` getters with absent versus empty body representation |
| Text/JSON/status assertion | `text`, `json`, `ok`, `assert_ok` |
| Original request | `FetchedResource::original`, plus actual final `request` and exchange count |
| Server/throttle errors | `server_error` with synthetic 599; typed `FetchAction::Retry` |
| Debug string | Redacted Rust Debug; explicit getters for intentional inspection |

## Request and response bounds

Bodies are at most 1 MiB. URLs use lowercase scheme spelling and are at most
2 MiB; HTTP(S) URLs have a smaller 16 KiB limit and reject userinfo, fragments and
control characters. Explicit methods are validated tokens, at most 32 bytes.
Without an override, an absent body means GET and a present body means POST,
including an empty body. GET/HEAD bodies reject for portable HTTP behavior.

Headers normalize names and preserve validated ASCII values. A header set contains
at most 128 fields and 16 KiB of name/value bytes; a failed mutation leaves it
unchanged. Content type and compression headers are derived when absent. Host,
content-length, connection and transfer-encoding overrides reject before execution.
Status text is bounded to 1,024 bytes and omitted from Debug/error output.

JSON body encoding preflights depth 64, 65,536 nodes and 1 MiB of aggregate keys
and strings before serialization. JSON escaping may need bounded temporary space
beyond the final 1 MiB body limit. Response JSON uses serde_json's bounded input
and recursion limit, with floating-point round-trip parsing enabled. This preserves
already-parsed binary floats; it does not make large JSON integers exact. Represent
chain amounts and other exact large numbers as strings, or retain raw body bytes
and use an application-selected parser.

Native execution bounds the compressed wire body and decoded gzip body separately.
Chunked/unknown-length streams are counted while reading. Original content length
and encoding headers are retained alongside decoded bytes. Other compressed
encodings are unsupported. Browser Fetch controls compression negotiation and
exposes a decoded stream: its decoded bytes and exposed Content-Length are bounded,
but the SDK cannot measure hidden compressed wire bytes. Browser forbidden-header
rules, CORS filtering and cookie policy still apply; automatic credentials and
referrers are omitted. Empty HTTP streams normalize to `Some(empty)`.

## Lifecycle and retry rules

The default performs one exchange, no redirects, and a ten-second overall timeout.
Configuration allows 1–12 exchanges and 0–10 redirects. The timeout covers gateway,
preflight, processing and retry hooks, I/O and delays. A cancellation token supports
one active operation; cancelling it is permanent and wakes the task. Dropping send
releases active I/O/timers and the token's active registration. Cancellation does
not prove a remote state-changing request was never executed.

Use `FetchClient::send` for these lifecycle guarantees. Direct calls to the public
backend's `execute` method are one bounded-body exchange; an application bypassing
the client must supply its own deadline and cancellation policy.

HTTP 429 and explicit process retry decisions can retry only within the configured
exchange/method budget and with retry-hook approval. GET/HEAD are eligible by
default; other methods require explicit opt-in. Transport failures stop and are
never retried automatically. Process hooks see 429 responses too. Locally resolved
gateway responses are processed once, without a network retry loop. Attempts count
actual completed exchanges, including redirects; exhaustion returns the last
response, which can still be inspected or rejected with `assert_ok`.

Retry-After delta values are seconds. HTTP-date forms use the configured fallback
delay, which is deterministic exponential backoff rather than hidden random jitter.
Long retry delays wait only until the overall deadline. No resource setting changes
the SDK's transaction submission behavior.

## Gateways, redirects and authentication

Data URIs support the source's simple MIME/base64 form and strict percent bytes.
Malformed percent escapes reject; the published helper passes malformed `%gg`
through literally. IPFS resolution requires an explicit gateway and rejects path
traversal, empty path components, and query/fragment injection. Resolving a CID
through a gateway does not verify returned content. Custom schemes use per-client
application hooks; there is no mutable process-global transport/gateway registry.
Immutable client construction replaces the source global configuration lock.

HTTP authentication headers require a per-request insecure-auth opt-in. Basic
auth stores a bounded encoded header without a separate plaintext credential
property. These models redact diagnostics; they are not zeroizing secret stores.

Redirects support relative and absolute locations for GET/HEAD and reject HTTPS
downgrade. Same-origin headers are retained; cross-origin redirects clear every
explicit header and authentication opt-in, including custom API-key headers.
The source forwards explicit authorization headers across origins. Browser manual
redirects are opaque even for same-origin fetches, so the adapter returns
`Unsupported` rather than following a chain it cannot inspect or bound. Callers
can supply a separately resolved explicit URL. This is a browser platform
constraint, not a promise of transparent native/browser redirect equivalence.

## Evidence and source differences

[Five source tests](../compatibility/scripts/fetch.test.mjs) exercise twelve body/
data fixtures, hook/retry behavior, credential-forwarding redirects, response
views/diagnostics and gateway dispatch. The source interprets numeric Retry-After
as milliseconds, forwards custom authorization headers on cross-origin redirects,
prints private request context in `toString`, and permits malformed percent text.
Rust documents its corrected or stricter behavior instead of copying those choices.

[Eight shared native tests](../crates/quai-sdk/tests/resources.rs) cover the fixtures,
limits, retry/method policy, gateways, redirect isolation, stalled hooks, deadlines,
active-token reuse, cancellation and drop. The dedicated worker runs those tests
plus an actual HTTP test for echo, gzip, oversized/error bodies, opaque redirects
and stalled reads. [Three native server tests](../crates/quai-rpc/tests/fetch.rs)
check exact POST bytes, non-success bodies, gzip expansion, chunked/declared lengths,
cross-origin header clearing and deadline enforcement.

The resource fuzz target retains an oversized-integer case that exposed a one-ULP
round-trip drift in serde_json's default float parser. Enabling `float_roundtrip`
fixes that regression while retaining explicit string quantities. ASAN exercises
URL/data/header/model parsing and body serialization; it does not fuzz TLS, HTTP
scheduling or browser internals. Reports distinguish that corrected assertion
failure from memory-safety findings and from production security qualification.
