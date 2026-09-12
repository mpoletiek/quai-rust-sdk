# Node qualification infrastructure

This directory starts the Phase 0 node qualification work. It contains a safe read-only HTTP probe, its tests, and explicit endpoint profiles. **It does not yet contain a funded local chain, snapshot, miner, WebSocket harness, or transaction acceptance evidence.**

## Run the probe

Python 3.10 or newer, standard library only. From the repository root:

```sh
python3 test-infra/readiness.py --url http://127.0.0.1:9200 --expected-chain-id 1337
python3 test-infra/readiness.py --url https://orchard.rpc.quai.network/cyprus1 --expected-chain-id 15000
python3 -m unittest discover -s test-infra/tests -v
```

The URL is required and used exactly as supplied. Nothing appends `/cyprus1` or changes a port. This implements the connection behavior required by local `use_pathing: false` (JavaScript boolean `usePathing: false`). The local profile uses HTTP `9200` and WS `8200`; **8200 is a WebSocket endpoint, not a second HTTP endpoint**. The Orchard profile records gateway routing separately; pass its already resolved HTTP URL to the probe. The JSON profiles are declarative qualification records, not automatically loaded CLI configuration.

The probe validates `quai_chainId`, obtains `quai_blockNumber`, then requests `quai_getHeaderByNumber` at that explicit height. It checks the pinned node's header shape, Cyprus-1 location, header number/hash, Prime-terminus number, expansion number and gas/state quantities. It reports whether gas/state limits are nonzero. Exit 0 means only that the requested reads were validated; zero limits, unknown forks, and unproven transaction acceptance remain visible in the report. Exit 1 indicates invalid configuration, network failure or a failed requested check.

Optional `--quai-address PUBLIC_TEST_ADDRESS` reads balance at the captured height. Optional `--qi-address PUBLIC_TEST_ADDRESS` validates the current-head outpoint response; adding `--expected-outpoint HASH:DECIMAL_INDEX` requires a known outpoint to appear. An empty list does not establish index readiness. Observing an outpoint proves that one index entry is queryable; it does not prove index completeness, lock maturity, ownership of its key, spendability, or an atomic view shared with the captured header. These optional addresses must belong to the expected Cyprus-1 ledger; use test addresses only. No private-key option exists.

Every RPC runs in a child process with a wall deadline (default 10 seconds, configurable maximum 30). This bounds DNS, TLS, response reads and parsing; cleanup can add up to roughly 2 seconds per request. Bodies are capped at 1 MiB. Only five hardcoded read methods are allowed. The probe rejects redirects, URL userinfo/fragments, encoded HTTP bodies, malformed quantities, duplicate JSON keys, mismatched IDs and malformed JSON-RPC envelopes. TLS verification stays enabled. Environment proxies are disabled deliberately. Reports redact URL paths/queries and server error text/data; URLs passed on the command line remain visible to local process inspection and shell history, so use endpoints without embedded secrets.

## Verified observations, 2026-09-11

| Check | Observed result |
|---|---|
| Toolchain | `go version go1.26.5-X:nodwarf5 linux/amd64`; candidate go-quai declares Go 1.24 |
| Container/node tools | `docker` and `quai` not found on PATH; no node binary build was attempted |
| Pinned source checkout | `/tmp/quai-sdk-research-go`, HEAD `f3f345c877300c044e3e0081a48bf3cf786fb9cc`; this temporary checkout is not a retained build dependency |
| Local loopback outside sandbox | TCP connections to `127.0.0.1:9200` and `127.0.0.1:8200` both returned `ConnectionRefusedError` at approximately 20:22 UTC |
| Orchard probe at 20:22:21 UTC | Chain ID 15000; zone height 7,782,199; Prime-terminus height 1,708,503; expansion 0; gas/state limits both 50,000,000 |
| Orchard header hash | `0x5b57b5b0865aba642e12c0aa505ed6ddb1386cab7481b2ab7f292b712d8c045d` |
| Probe tests | 14 passed, including local HTTP fixtures for exact path preservation, redirect refusal, oversized responses and wall deadlines |

The initial sandbox prevented loopback connections. The outside-sandbox observations above distinguish that restriction from an actual refused connection. Orchard rejected Python's default user-agent with HTTP 403; the explicit `quai-rust-sdk-readiness/0.1` user-agent used by the final probe succeeded. These are point-in-time observations, not an uptime claim or a deployed-version attestation. No funds, keys, node data, node configuration, mining state or remote application state were changed.

## Remaining gate and next action

### User-supplied LAN mainnet node

`10.0.0.12` was verified on 2026-09-11: direct HTTP port 9200 returned chain ID
9 and Cyprus-1 height 10,047,036 through the Rust SDK. Header gas/state limits
were both 50,000,000 and genesis matched the pinned mainnet configuration.
Port 9001 returned chain ID 9 and running locations `[[0,0]]`; direct WS port
8200 passed Rust chain-ID, real head-subscription, unsubscribe and shutdown tests.
`quai_clientVersion` reports `go-quai/v0.56.0-f3f345c8`; deployed build attestation,
sync status, reconnect/backfill and historical index completeness remain unqualified.

```sh
cargo run --locked -p quai-sdk --example read_network -- http://10.0.0.12:9200 false 9
python3 test-infra/readiness.py --url http://10.0.0.12:9200 --expected-chain-id 9
```

The [LAN profile](profiles/mainnet-lan-direct.json) records this evidence. Use it
for read-only mainnet compatibility checks. See [wallet gaps](../docs/WALLET_GAPS.md)
for full findings. No mainnet transaction or wallet mutation was performed.

### Disposable funded chain

Follow [the node harness specification](../docs/node-harness.md) to provision an owned mature, funded chain from the pinned source. First obtain or build a reproducible mature snapshot with known public test-key provenance and verified genesis/allocation identity. Record exact node build/configuration and patches; enable and prove address indexing using a known surviving Qi outpoint. Retain snapshot digest, preparation/reset times, head/fork context and expected account/UTXO inventory. Then qualify HTTP and WebSocket routes, sign and obtain receipts for account transfers and deployments, and obtain node acceptance for single/multiple-input Qi transactions and conversion vectors. This gate is still open; a read-only success cannot close it.
