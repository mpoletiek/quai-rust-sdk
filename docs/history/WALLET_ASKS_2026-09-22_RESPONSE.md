# Response to the Quai Terminal asks of 2026-09-22

This answers [the asks](WALLET_ASKS_2026-09-22.md) from Quai Terminal. Each
claim was checked against `0.1.0-alpha.10` and against the wallet's
`sdk-alpha10` working tree. Both asks are valid, and both ship in
`0.1.0-alpha.11`. Ask 1 ships as the fallback the ask offered, a cargo
feature, rather than as stable constructors. `Log` is left out because it
already has a way in.

## Outcome

| # | Ask | Verdict | Shipped as |
|---|---|---|---|
| 1 | Constructors for observation types | Valid for six of the seven types | `test-fixtures` feature with `new` on six types; `Log` already has `try_from` |
| 2 | SOCKS proxies in `HttpProxy` | Valid | `socks5h://` and `socks5://` accepted by `HttpProxy::parse` |

Tested against the wallet, from a scratch copy of its `sdk-alpha10` working
tree that points at the alpha.11 SDK by path:

- The six parked items compile after switching struct literals to `new`,
  with no assertion changed: the four tests and the `credit()` and
  `executed_destination()` fixtures. All four tests pass.
- The whole wallet workspace passes: 545 tests, which is the 541 reported on
  alpha.10 plus the four restored.
- `quai_chainId` against `https://rpc.quai.network/cyprus1`, through a local
  Tor SOCKS port with `socks5h://`, returned mainnet's `0x9` with TLS verified
  end to end.

## 1. Constructors for the observation types

**Verified.** `QiCreditObservation`, `AddressOutpoint`,
`ConversionObservation`, `ExternalObservation`, `EtxScanResult` and
`EtxExecutionObservation` are `#[non_exhaustive]`, with no constructor,
`Default` or `Deserialize`. The changelog's "need no change" was wrong for a
consumer that builds outputs in tests.

Constructors on these six are enough, because every type in their fields can
already be built from outside the SDK:

- `BlockReference` and the enums (`ScanCoverage`, `ConversionEffect`,
  `ConversionOriginObservation`, `ConversionSpendability`, `ReceiptOutcome`)
  are written as literals.
- `Extensions` has `Default`, and `OutPoint` is a literal.
- `Transaction` and `Receipt` are built from node JSON with `try_from`, as
  your `executed_destination()` already does.

**Not needed: `Log`.** `Log::try_from(serde_json::Value)` is public, so a log
can be built directly from its node JSON. That is simpler than decoding a
receipt that carries one log. `ZoneHeader` has the same.

**What shipped.** A `test-fixtures` feature on `quai-sdk`, forwarded to
`quai-provider`, adds a `new` taking every field, in declaration order, with no
defaults:

```rust
QiCreditObservation::new(beneficiary, transaction_hash, creating_hash,
    execution, head, outputs, locked_qits, unlocked_qits, unobserved_qits)
AddressOutpoint::new(outpoint, denomination, lock, extensions)
ConversionObservation::new(origin, scan, effect, spendability)
ExternalObservation::new(origin, scan, outcome)
EtxScanResult::new(coverage, last_block, transactions_examined, execution)
EtxExecutionObservation::new(transaction, receipt)
```

Why a feature rather than a stable `new`:

- These types carry facts that the observers establish: outputs ordered by
  index, `unobserved_qits` as the ETX value minus the observed outputs, and a
  receipt associated with its transaction. The constructors check none of
  them, because a fixture has to be able to state partial or inconsistent
  observations. On the default surface they would let production code make
  up observations too.
- The feature **is covered by semver**. If one of these types gains a field,
  its constructor gains a parameter, and the changelog lists that under
  `Breaking:`. Your fixture then stops compiling and you add the new field on
  purpose, which fits the ask's "no defaults" reasoning.

**Wallet migration.** Enable the feature in dev-dependencies only:

```toml
# crates/wallet-core/Cargo.toml
[dev-dependencies]
quai-sdk = { workspace = true, features = ["test-fixtures"] }
```

Then replace the struct literals with `new`, for example:

```rust
EtxScanResult::new(
    ScanCoverage::Complete,
    Some(block(25, 5)),
    1,
    Some(EtxExecutionObservation::new(
        Transaction::try_from(transaction).unwrap(),
        Some(Receipt::try_from(receipt(2, 1)).unwrap()),
    )),
)
```

The one struct-update expression, `ConversionObservation { effect: …,
..conversion.clone() }`, is also refused for a non-exhaustive type. Clone and
assign instead, since the fields stay public:

```rust
let mut locked = conversion.clone();
locked.effect = Some(ConversionEffect::Locked { etx_type: 2 });
```

**Changelog.** The alpha.10 entry now says that code which reads outputs needs
no change but a test that builds them does. It points to `test-fixtures` and
to `try_from`.

## 2. SOCKS proxies in `HttpProxy`

**Verified.** `HttpProxy::parse` accepted only `http` and `https`.

**What shipped.** `HttpProxy::parse` also accepts `socks5h://` and
`socks5://`, with optional `user:password@` credentials.

- `socks5h` sends the endpoint's host name to the proxy, so no lookup reaches
  the local resolver. That is the Tor setting: `socks5h://127.0.0.1:9050`.
- `socks5` resolves the name locally and sends the proxy an address, so the
  local resolver still learns which node is contacted. Prefer `socks5h`.
- Credentials are sent with SOCKS5 username/password authentication and never
  appear in `Debug` output. Tor uses distinct credentials to keep traffic on
  separate circuits, if you want to split node RPC from third-party lookups.
- The existing guarantees stand: proxies are opt-in, no environment variable
  is read, and an `https` endpoint's TLS is verified end to end.
- It adds no crates to the dependency graph. reqwest's `socks` support uses a
  `hyper-util` feature that was already enabled.

Loopback tests run a small SOCKS5 server and check the bytes it receives:
`socks5h` sends the name (address type 3, `rpc.invalid`, which cannot resolve
locally), `socks5` never sends the name, and credentials arrive intact while
staying redacted. `socks4` and `socks4a` are still rejected.

**Limits to know:**

- **IPv6 destinations fail through SOCKS5.** With reqwest 0.12.28 and
  hyper-util 0.1.20, an IPv6 destination reaches the proxy as the bracketed
  text `[::1]` under the host-name type, which no proxy resolves. That covers
  a literal IPv6 endpoint, and a `socks5://` name that resolves to IPv6 first
  (`localhost` does on many hosts). The request fails rather than leaking. Use
  a host name with `socks5h`, or an IPv4 address.
- **Only the HTTP transport takes a proxy.** `WsTransport` still connects
  directly. Your `ws_url` is unused today, so all your node traffic is HTTP
  and this covers it. If you enable WebSockets later, those connections would
  bypass the proxy until the WebSocket transport gains one. Ask if you need it.
- A local or LAN node is still reached directly when you leave `proxy` unset
  for that endpoint's transport, which matches your IPFS gateway handling.
  Each `HttpTransport` has one proxy setting, so use one transport for proxied
  endpoints and another for local ones.
