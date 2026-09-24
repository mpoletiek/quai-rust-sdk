# Asks from Quai Terminal, 2026-09-23

Quai Terminal (`~/Devspace/quai-rust-cli-wallet`) runs on `quai-sdk =0.1.0-alpha.11`. This ask is about speed on the wallet's review path. Nothing is broken.

## Summary

| # | Ask | Priority | Why it matters |
|---|---|---|---|
| 1 | Fewer sequential round trips in `observe_contract_code` / `verify_deployment` | Medium | Every contract a review touches is re-verified afresh, and each check is five RPCs in a row. At 250 ms per call to `rpc.quai.network`, that is about 1.25 s before a swap review can start quoting. |

## 1. Fewer sequential round trips in contract code observation

**What happens.** `QuaiProvider::observe_contract_code` (`quai-provider/src/contract_code.rs`) makes five calls, each waiting on the last:

1. `genesis_hash(zone)`
2. `latest_header(zone)` (or `header_at(n)`)
3. `code(address, n)`
4. `header_at(zone, n)`, to check the block did not change underneath
5. `genesis_hash(zone)` again

Each wallet review verifies its pinned contracts first-hand, never from a cache, and that is deliberate. It runs the checks concurrently across contracts, but each check is still a five-deep chain.

**Evidence (2026-09-23, from the wallet's RPC trace).**
- A swap review against `rpc.quai.network` runs at about 250 ms per call. It spent 1.8 s verifying pins: four venues' routers and factories, plus Multicall3. The checks ran in parallel, but each was a five-call chain.
- Verifying one HartiiLabs curve for a review needs three pin checks. Even with all of them parallel, and every other read folded into the same round, the floor was 1.75 s. That floor is the five-deep chain.
- Through a LAN node (1 ms per call) the same work is negligible, so this only shows on a public RPC. Most users are on one.

**What would help, either or both:**
- **Cache the genesis hash per provider and zone.** It cannot change, and the provider already checks it against the expected genesis when it connects. That makes calls 1 and 5 free after the first time, which takes two levels off every check.
- **Observe several contracts at one block:** for example `observe_contract_codes(addresses, block)`. It would read one header, then every address's code at that height concurrently, then re-check the header once. That is three levels in total however many contracts there are, instead of five per contract.

The wallet will keep verifying first-hand for reviews either way. This is only about how many round trips that costs.
