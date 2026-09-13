# Remote account signing

`quai_sdk::rpc_signer::RpcAccountSigner` (feature `wallet`) binds an already
exposed remote Quai account to an immutable chain ID, trusted genesis and zone.
It works with native HTTP/WebSocket and Wasm Fetch transports. Construction is
offline; no account permission request or signing occurs implicitly. An injected
browser extension uses the separate `InjectedProvider`, which tracks wallet events.

```rust,no_run
use quai_sdk::{rpc_signer::RpcAccountSigner, wallet::discovery::NetworkScope};
use quai_sdk::{HttpTransport, HttpConfig, Routing, Zone, U256};
# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let scope = NetworkScope {
    chain_id: U256::from(9),
    genesis: "0xac81c28f1a72591b87b5f16c9793cdc0e87c45c6d426d1a364c3b8f6386b5b8b".parse()?,
    zone: Zone::Cyprus1,
};
// Application-selected signing service; public read RPCs need not expose accounts.
let transport = HttpTransport::new(HttpConfig::default())?;
let signer = RpcAccountSigner::new(
    transport,
    Routing::direct("https://wallet.example/cyprus1", Zone::Cyprus1.into())?,
    scope,
    "0x0011223344556677889900112233445566778899".parse()?,
)?;
let signature = signer.sign_message(b"Exact message bytes").await?;
# let _ = signature;
# Ok(()) }
```

Every authorization request is bracketed by chain/genesis and passive
`quai_accounts` checks. Accounts are concrete typed addresses; duplicate,
malformed, Qi-ledger and oversized account lists reject. These checks detect
inconsistent observations, not a server that lies consistently or changes away
and back between reads. Transports must enforce response bounds, correlated RPC
IDs and deadlines. An application can wrap the entire future in an overall deadline.
No state-changing request is automatically retried.

| API | RPC and result validation |
| --- | --- |
| `sign_message` | `personal_sign [hexBytes, lowerAddress]`; recover the Quai personal-message digest and require the bound account |
| `sign_typed_data` | `quai_signTypedData_v4 [lowerAddress, jsonDocument]`; explicit `DomainPolicy` and recovered EIP-712 digest |
| `legacy_sign` | `quai_sign [lowerAddress, hexBytes]`; caller supplies the expected digest because remote prefix/hash semantics may differ |
| `sign_transaction` | `quai_signTransaction` with exact populated type-0 fields; canonical protobuf decode, sender recovery and equality of every unsigned field, including nonce zero and ordered access lists |
| `send_transaction` | `quai_sendTransaction`; returns an unverified `RemoteSendAcknowledgement`, never assumes the wallet preserved the requested fields |
| `unlock` | `personal_unlockAccount [lowerAddress, password, durationSeconds]`; explicit finite 1–3,600 seconds and a boolean result |
| `provider`, `address`, `scope`, `check_account` | Explicit read/simulation composition and immutable identity; no mutable reconnect or implicit permission request |

Requests have a 1 MiB serialized parameter limit. Messages are capped before hex
allocation, transaction data before protobuf encoding, access lists at 1,024
entries and 8,192 total storage keys, and passive accounts at 1,024 entries.
Unlock passwords are at most 1,024 UTF-8 bytes. A password is sent to the selected
service and may be copied by its transport; this API does not promise zeroization
of arbitrary transport-owned JSON. Signer/error diagnostics omit payloads and
passwords; explicit remote error fields remain available for deliberate inspection.

## Durable signing and recovery

For local custody, prepare and review through a native `AccountSession` using a
`WatchOnlySigner`, or a `PreparedBrowserAccountTransaction`. Pass its immutable
`transaction()` to the remote signer. Commit the returned verified bytes with
`commit_external_signature`, then broadcast from the durable account journal.
The commit rechecks exact reviewed fields and the current reservation; native
preparation is bound to its original opened SQLite handle, and browser writes use
revision compare-and-swap. Retain remote signed bytes until durable commit succeeds.

`RpcSignerError::dispatched` distinguishes preflight failure from an operation
that might already have signed, unlocked or submitted. Remote rejection, timeout
and failed post-request checks remain dispatched. A dropped future returns no
error, and cannot dismiss a remote approval. Never release a nonce solely because
the remote call or durable commit failed.

For wallet-mediated send, persist `RemoteSendIdentity::new(scope, address, tx)`
and the complete requested transaction before awaiting. A digest is not a signed
transaction ID. The wallet may alter fees, value or nonce; this flow does not
commit an unknown signed payload to the SDK's local account journal. A successful
acknowledgement or `reported_hash` from a failed post-check is still unverified.
`RemoteSendAcknowledgement::from_reported_hash` supports explicit restart recovery.
Its `observe(provider)` performs one lookup, verifies canonical signed identity,
checks the trusted network before and after, and reports `matches_request`.
`None` only means not indexed. Receipt canonicality and confirmation are separate
provider operations. No unbounded transaction polling is hidden in this API.

## quais.js comparison

The published `JsonRpcSigner` returns remote signatures without recovery checks,
uses an unspecified (`null`) unlock duration, and its `sendTransaction` waits for
indexing internally. Rust makes verification, finite unlock duration and bounded
observation explicit. It provides both offline signing and wallet-mediated sends.
Neither published `JsonRpcSigner` nor this adapter delegates Qi input signing;
Qi HD/imported/payment signing uses the dedicated Qi workflow.

Inherited provider conveniences compose through `signer.provider()`:
`transaction_count` for a nonce; typed `CallRequest` plus `call`, `estimate_gas` or
`create_access_list` for simulation; `quote_account` / `quote_account_with_access`
for an exact transaction with explicit nonce, observation and fee policies.
Concrete `QuaiAddress` and `.zone()` replace Promise/Addressable coercion.
The remote signer does not silently populate or reserve a transaction before signing.

Evidence: [published-source tests](../compatibility/scripts/rpc-signer.test.mjs),
[shared native/worker protocol tests](../crates/quai-sdk/tests/rpc_signer.rs),
[native custody tests](../crates/quai-sdk/tests/accounts.rs) and
[browser custody tests](../crates/quai-sdk/tests/account_preflight.rs).
These use public toy keys and mock services. No funded-node signing, actual remote
wallet interoperability or crates.io publication is implied.
