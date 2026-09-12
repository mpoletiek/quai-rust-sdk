# quai-browser

Concrete wasm32 adapters for browser Fetch and an explicitly supplied Quai wallet provider. This unpublished alpha adds browser transport and a small permission API; it does not make the native wallet/storage stack browser compatible.

The crate exposes `BrowserConfig` and sanitized `BrowserError` on all targets. `BrowserFetchTransport`, `InjectedProvider` and `fill_random` exist only on wasm32. There is no native browser stub silently substituting another transport. Both adapters implement the existing non-`Send` wasm `quai_rpc::Transport` contract and can be used with `Provider` and explicit `Routing` without enabling native HTTP or wallet features.

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

## Injected wallet

An application passes its chosen JS provider object into `InjectedProvider::new(provider, endpoint_identity, shard, expected_chain_id, config)`. The adapter does not search global wallet objects, auto-select a wallet or call wallet methods during construction. The endpoint serves as an exact routing identity; requests go to the injected provider with the pinned quais.js `{method, params, shard}` shape, where the shard is encoded as `0x00`, `0x10`, etc.

Every high-level operation checks `quai_chainId` afresh. Generic `Transport`/`read` uses a narrow read-method allowlist and rejects wallet permission, signing and sending methods. A chain check detects configuration changes but cannot authenticate a malicious provider or make several requests atomic.

The explicitly invoked wallet APIs are:

- `accounts()`: read already exposed Quai accounts, without requesting access.
- `request_accounts()`: call `quai_requestAccounts` after checking the chain. The application must invoke this from its approved connection flow.
- `personal_sign(address, bytes)`: check chain, account exposure and the configured sender zone, then call `personal_sign([hex_message, lowercase_address])` once. The returned value must be a 65-byte hex signature with a supported recovery byte. The adapter validates low-S scalars and recovers the requested account from the exact personal-message hash. The application still owns message semantics.

- `sign_typed_data(address, data, policy)`: validate a bounded immutable EIP-712 document, require a matching domain chain (or explicit unbound policy), check account exposure, call `quai_signTypedData_v4` once, and recover the returned signature against the exact cached document hash. RPC serialization includes the canonical domain schema and uses strings for integer values.

Provider errors retain only a safe numeric code, including EIP-1193 user rejection 4001 and unsupported method 4200. Provider messages, data and request payloads never enter error strings. Injected results use a bounded JSON-only serializer that rejects cycles, accessors, custom prototypes, BigInt and unsafe integer numbers; chain/amount quantities remain hex strings. Memory already allocated by an injected provider is outside this adapter's control.

Dropping or timing out an injected request ends this adapter's wait. EIP-1193 has no cancellation primitive that retracts a wallet approval dialog or operation. No account/signature request is retried automatically. Generic `quai_sendTransaction`/`quai_sendRawTransaction`, transaction signing, chain switching, permission enumeration/revocation, wallet discovery, and accounts/chain event listeners remain unimplemented in this slice.

## Browser boundaries

| Responsibility | Current behavior |
| --- | --- |
| Entropy | `fill_random` explicitly calls secure-context Web Crypto, at most 65,536 bytes, and fails without it. No fallback PRNG or automatic key creation. |
| Timers/cancellation | Native browser timers plus AbortController; no Tokio runtime. Suspended pages can delay timers. |
| CPU work | No mining, address grinding, mnemonic derivation or heavy signing runs automatically. Applications must place expensive work in a dedicated worker with explicit progress/cancellation. The bridge uses `globalThis`; dedicated-worker tests verify Fetch, timeouts and entropy without Window or Tokio. Additional worker tests execute HD Qi grinding, BIP340 signing/verification, ECDSA recovery and OS-random key generation. This is a bounded runtime slice, not a general worker job/persistence API. |
| Persistence | No localStorage, IndexedDB or browser key persistence. Native SQLite is not linked. Browser durable reservation semantics and encrypted IndexedDB storage require a separate implementation and review. |
| Trust | The application chooses its origin, CSP and injected provider. Same-origin JS and wallet extensions remain privileged; this adapter cannot protect against a compromised application origin. |

## Verified tests and reproduction

On 2026-09-11 the crate compiled with Rust 1.97.1 for `wasm32-unknown-unknown` and ran in actual headless Chromium through wasm-bindgen-test 0.3.78. Tests use a separate loopback HTTP server with CORS plus a synthetic injected wallet object; they do not load a real wallet extension. Signing uses already-public fixture scalars; entropy tests use ephemeral unfunded keys.

Nine browser tests cover real Fetch path/query and ID validation, refused redirects, streamed oversize data, deadlines and future cancellation, concurrency capacity, exact injected shard and account/signing arguments, no automatic prompts, chain mismatch, user-rejection redaction, malformed/accessor results, unauthorized or wrong-zone signing, non-ASCII signatures, and secure entropy. Five dedicated-worker tests verify Fetch/provider composition, timeout, Web Crypto, HD Qi grinding, verified legacy-keystore decryption and native Rust crypto running inside WebAssembly without Window or Tokio. Two additional native tests exercise limits, permission allowlists, bounded JSON serialization and envelope validation.

With Rust's wasm target, matching wasm-bindgen CLI 0.2.128 and Chromium/chromedriver installed:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.128 --locked
CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
  python3 crates/quai-browser/tests/run_browser.py
# Repeat with --suite worker for the dedicated-worker runtime tests.
```

The runner creates and removes its own server, chooses an unused loopback port, and uses an isolated browser profile through chromedriver. `CHROMEDRIVER` and `WASM_BINDGEN_TEST_WEBDRIVER_JSON` can override browser setup. In this development environment, the matching official wasm standard library and prebuilt wasm-bindgen release tools were installed under `/tmp`; system Rust and browser profiles were untouched. Native tests and wasm clippy are separate commands; ordinary native `cargo test` does not execute browser tests.

The browser bridge is packaged at `src/bridge.js` and imported through wasm-bindgen's module mechanism. Downstream wasm-bindgen packaging must copy its generated JS snippets alongside the wasm artifact. Browser-specific dependency code is target gated. Browser WS, real injected-wallet interoperability, remote-node browser CORS, general worker orchestration, Firefox/Safari, background-tab behavior and browser storage remain unverified.

Sources: pinned quais.js `src/providers/provider-browser.ts` and `provider-jsonrpc.ts` define the Quai method names and shard extension; [EIP-1193](https://eips.ethereum.org/EIPS/eip-1193) defines the provider request/permission/error model. [wasm-bindgen browser testing](https://wasm-bindgen.github.io/wasm-bindgen/wasm-bindgen-test/browsers.html) describes the runtime harness, and [Fetch RequestInit](https://developer.mozilla.org/en-US/docs/Web/API/RequestInit) documents browser credential, redirect and cancellation controls.
