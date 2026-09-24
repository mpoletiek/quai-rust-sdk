# Response to the Quai Terminal ask of 2026-09-23

This answers [the ask](WALLET_ASKS_2026-09-23.md) about the round trips in
contract code observation. The problem is real and ships fixed in
`0.1.0-alpha.12`, but not the way the ask proposed. While checking it we also
found a bug in the wallet's pin verification, described in the
[last section](#a-bug-in-the-wallets-pin-verification).

## Outcome

| Proposal | Verdict | What shipped |
|---|---|---|
| Cache the genesis hash per provider and zone | Declined | Batching removes the same round trips and keeps the checks |
| `observe_contract_codes(addresses, block)` | Shipped, with a caveat | Same-block observation of up to 125 contracts; not always faster |

Measured against `https://rpc.quai.network/cyprus1`, where a round trip on a
reused connection takes about 245 ms:

| Case | alpha.11 | alpha.12 |
|---|---|---|
| One observation (`verify_deployment`, `observe_contract_code`) | 1.23 s, 5 requests | 0.49 s, 2 requests |
| 100 observations in a row | 124 s | 51 s |
| Five contracts, concurrent single observations | 1.26 s | 0.75 s |

Your 1.8 s for five pins matches what we measured for five concurrent
observations from a cold connection pool: 1.86 s. Each concurrent check opens
its own HTTP/1.1 connection, and a cold TLS handshake costs about 0.6 s more.

## Why not cache the genesis

- **The premise does not hold.** The provider does not check the genesis when it
  connects. `Provider::new` makes no network call and knows only a chain ID.
  Your wallet checks the genesis in `require_identity` when a session opens,
  which may be what the ask had in mind.
- **Caching would remove a check.** The second genesis read catches an endpoint
  that switched networks between the first read and the code read, for example
  behind a load balancer or after it was re-pointed.
- **Batching gets the same saving.** An observation is now two rounds:
  1. The genesis and the header, in one batch. No address is in this round.
  2. The code, read by that header's block hash, plus the header and genesis
     rechecks, in one batch.

  Both genesis reads stay, and each costs nothing extra because it travels in a
  batch. Reading by hash (`{"blockHash": ...}`, which go-quai honors) also pins
  the bytes to exactly the block that is rechecked.

Your `WalletTransport` caches identity answers only on the single-request path.
Its `request_batch` passes straight through, so this change reaches you with no
code change. Upgrading is enough.

## `observe_contract_codes`: use it for consistency, not speed

`Provider::observe_contract_codes(genesis, targets, block)` observes up to
`MAX_CONTRACT_CODE_TARGETS` (125) contracts of one zone at the same block. Each
target is an address with an optional expected runtime hash. It requires the
trusted genesis, and it returns `GenesisMismatch` before any address is sent.

We expected it to be the fastest option. It is not, on a high-latency link:

| Five contracts of 10–15 KB each | Median |
|---|---|
| One `observe_contract_codes` call | 1.0 s |
| Five concurrent `observe_contract_code` calls | 0.75 s |

The response size is the cause. Every runtime arrives in one response, about
58 KB here. An idle connection's TCP window delivers roughly 14 KB per round
trip, so that response took 2–4 round trips. A batch of five empty-code
addresses (197 bytes), or of one large contract, took one.

What we suggest:

- **For speed, as in `verify_pinned_all`:** keep the concurrent
  `verify_deployment` calls. After the upgrade each one is two rounds.
- **When contracts must agree on a block,** such as a router and its factory:
  use `observe_contract_codes`, then check each observation as
  `verify_deployment` does. Empty code means missing, and
  `matches_expected == Some(false)` means a runtime mismatch.
- **`discover_contract`** calls `observe_contract_code` without a trusted
  genesis, so it sends the address before the network is confirmed for that
  call. It is not unchecked: the session confirmed the identity when it opened.
  Its doc comment says "genesis checked", though. To make that true per call,
  use `observe_contract_codes(network.genesis_hash()?, &[(address, None)],
  BlockTag::Latest)`.

**A possible next step.** The node serves `quai_getProof`, whose account proof
carries the code hash. A pin check needs only that hash, not the bytecode, and
it can be checked against the block's state root. That would shrink the
response and could make the result verifiable. It is not in this release; ask
if you want it.

## A bug in the wallet's pin verification

`verify_pinned` in `crates/wallet-core/src/data.rs` retries **every** error:

```rust
// A new block during the observation is transient.
Err(_) if attempt < 4 => tokio::time::sleep(std::time::Duration::from_millis(200)).await,
Err(e) => return Err(CoreError::Network(format!("could not verify {what}: {e}"))),
```

The comment names the one transient case, a new block or reorg during the
observation (`ObservationChanged`). The pattern `Err(_)` also catches failures
that repeat on every attempt:

- **`GenesisMismatch`:** the node is on another network.
- **`Provider(ChainMismatch)`, `Provider(InvalidRequest)` and
  `InvalidDeployment`:** configuration errors.

For those, the wallet:

1. **Makes four attempts instead of one**, waiting 600 ms before it reports.
2. **On alpha.11 and earlier, sends the pinned contract's address to the
   wrong-network node four times.** alpha.12 stops each attempt before the
   address is sent, but the retries still happen.
3. **Reports a trust failure as a connectivity problem.** A node on the wrong
   network ends as `CoreError::Network("could not verify …")` rather than a
   rejection, so the user is told to check their connection.

alpha.12 adds `ContractError::class()`, matching the other SDK error types, so
the retry can be exact:

```rust
Err(quai_sdk::contracts::ContractError::GenesisMismatch) => {
    return Err(CoreError::Rejected(format!("{what}: the node is not on network `{}`", network.id)));
}
// Only a new block or reorg during the observation, or a network blip, is transient.
Err(e)
    if attempt < 4
        && matches!(e.class(), quai_sdk::primitives::ErrorClass::Stale | quai_sdk::primitives::ErrorClass::Transient) =>
{
    tokio::time::sleep(std::time::Duration::from_millis(200)).await
}
Err(e) => return Err(CoreError::Network(format!("could not verify {what}: {e}"))),
```

We applied this to a scratch copy of your current tree, built against
alpha.12. It passes `cargo clippy -p wallet-core --all-targets` with no
warnings, and your workspace tests pass: 655. The one failure,
`subgraph::tests::open_candles_expire_before_the_bucket_closes`, is in your
uncommitted `subgraph.rs` edit and does not touch the SDK. `candle_ttl(bucket,
1_800_000_001)` no longer returns 5.

## Verification

- New tests use a batching mock transport. They check:
  - two rounds, and that the first contains no address;
  - code read by the header's block hash;
  - `GenesisMismatch` after one round;
  - a changed header or genesis outranks a failed code read, and a failed code
    read alone surfaces as the RPC error;
  - the 125-target limit fills one batch exactly, and invalid requests send
    nothing.
- The existing non-batching tests still see the same five reads in order.
  `verify_deployment` and `ContractCodeTarget` now send no `getCode` on a
  genesis mismatch.
- Tip reorganizations: over 100 live observations each, the old code saw one
  `ObservationChanged` and the new code none. It stays the rare, retryable case.
- SDK: all tests, clippy, docs, the semver gate against alpha.11, and the wasm
  checks pass.
