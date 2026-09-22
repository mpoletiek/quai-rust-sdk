# Quai Rust SDK

The SDK combines typed Quai RPC, exact amounts and addresses, Quai/Qi transaction
signing, HD wallets, payment codes, ABI contracts and durable native wallet
workflows. It follows the pinned `quais@1.0.0-alpha.57` reference with explicit
Rust differences and node-version boundaries.

```rust
use quai_sdk::{QuaiAddress, Zone};

let address: QuaiAddress = "0x002b2596EcF05C93a31ff916E8b456DF6C77c750".parse()?;
assert_eq!(address.zone(), Zone::Cyprus1);
# Ok::<(), Box<dyn std::error::Error>>(())
```

| Feature | Enables |
| --- | --- |
| Default | Native HTTP plus offline wallet/signing APIs |
| `http` | Native HTTP provider transport |
| `ws` | Native WebSocket subscriptions and head following |
| `wallet` | BIP39/BIP32, exact Qi selection, consensus and local/watch-only signing |
| `backup` | Portable authenticated full-wallet decryption/encryption and read-only custody/cursor inspection |
| `sqlite` | Native account/Qi sessions, gap-50 current discovery, durable candidates and recovery |
| `abi` | ABI/EIP-712 values, artifacts, deployments and WQI/WQUAI adapters |
| `payments` | BIP47 payment codes; native registered-channel workflows also require `sqlite` |
| `keystore` | Bounded legacy JSON-keystore import/export |
| `browser` | Wasm Fetch/injected-provider adapters and IndexedDB snapshots; with `wallet`, durable HD allocation; add `payments` for durable payment destinations |

Browser consumers use `default-features = false` and choose the portable features
they need. SQLite sessions and native HTTP/WebSocket transports are target gated.
The native examples include `offline_wallet`, `qi_scan`, `payment_codes`,
`wrapper_intents`, `artifact_deployment`, `read_network`, `inspect_pool` and
`inspect_blocks` and the account-xpub `watch_qi`; check each example's feature and endpoint requirements.

Qi amounts use fixed denominations. Current discovery scans receive and change
branches with a default gap of 50 matching addresses, with explicit deeper ranges.
A latest-only outpoint RPC cannot establish fully spent historical address use.
Recovery preserves signed candidates and nonce/input claims across ambiguous
broadcasts and reorg observations. Confirmation counts do not prove finality or
cross-zone destination settlement.

This is an alpha SDK; pin the exact version (`quai-sdk = "=0.1.0-alpha.10"`)
because later pre-releases may break the API. The repository contains compatibility vectors,
platform CI, extracted-package consumer tests and qualified node evidence;
the complete reference mapping, deliberate Rust differences and remaining
funded-node/security qualification are tracked in the current parity analysis.

- [Workflow guide and runnable examples](https://github.com/mpoletiek/quai-rust-sdk/blob/main/docs/WALLET_WORKFLOWS.md)
- [Implementation status and remaining work](https://github.com/mpoletiek/quai-rust-sdk/blob/main/IMPLEMENTATION_STATUS.md)
- [Reference parity tracker](https://github.com/mpoletiek/quai-rust-sdk/blob/main/compatibility/parity.json)

License texts and third-party provenance accompany each crate. Test keys and
mnemonics in examples are public fixtures and must never receive real funds.

Native account sessions can opt into `AccountAccessListPolicy::Discover` with
`with_access_list_policy`. Ordinary calls and deployments then discover access
at the same nonce/block as fee preparation, retaining mandatory caller entries.
A nonce advanced by durable allocation triggers discovery again before review.
`Preserve` is the default. Generated entries are part of the frozen prepared and
signed payload; replacements and restart broadcast never rediscover access.

Contract interfaces accept `abi::AbiInterface::from_human_readable` for bounded
named Solidity ABI declarations. Full/minimal formatting and lossless JSON
metadata export preserve declaration order; malformed entries fail the import.

Browser consumers can call `browser::wait_for_receipt` with explicit monotonic
millisecond and poll-count limits. `Provider::observe_receipt_confirmation` also
exposes one portable observation for application-controlled scheduling. Both
check canonical receipt/head associations and preserve execution-failure outcomes.
