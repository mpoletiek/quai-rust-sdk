# Isolated WQI acceptance — 2026-09-13

Durable SDK sessions deployed the observed mainnet WQI creation bytecode, wrapped
native Qi, claimed WQI tokens, and redeemed them into a locked native Qi output.
This run also exposed and fixed four SDK defects: absent backing handling,
specialized Qi recovery validation, missing WQI lockup access declarations, and
Qi-address receipt logs. The [aggregate report](../../reports/wqi-acceptance-2026-09-13.json)
hashes the retained public signed payloads, RPC transcripts and verification results.

This is the explicitly patched isolated go-quai v0.56.0 development profile,
chain 1337, genesis `0x654e7a894d57de62ec19b9c161cb1c647466278e0565d3e1ba5d806ae6af0aee`.
It uses public toy scalar 805 and public HD seed `[7;32]`, exclusively synthetic
funds. The explorer supplied bytecode without verified source. No mainnet writes,
source audit, unmodified-node acceptance or funded Orchard acceptance is implied.

## Actual flow

| Operation | Origin block | Observed result |
| --- | --- | --- |
| Deploy | 13 | Local contract `0x0007ba2A9A6e5559134Ff1153b8a7F271B44f827`; checked runtime and initial zero balances |
| Native Qi wrap | 14 | Matched type-4 ETX; 1000 Qits unclaimed backing observed at head 17; zero token atoms |
| Claim before fix | 18 | Failed; entire 60937 gas limit consumed; backing remained 1000 Qits |
| Larger-gas control | 19 | Failed again with 100% gas margin; backing unchanged |
| Claim with access fix | 20 | Succeeded with original 10% margin; 1e18 token atoms, zero unclaimed backing |
| Unwrap | 21 | Matched type-6 destination execution at block 25; tokens zero, 1000 native Qits locked |

The redemption output is index 0, denomination 6, at fresh durable receive address
`0x00a8c6cf826b72080fa6f838a3329ec0b906b408`, creating hash
`0x000e00bcfb19536767d398099f9efff614a39e1ea2b707a6ddea0cd2845f6469`.
Its reported lock is **241945**. The wallet refresh sees 1000 locked Qits plus
8710 spendable Qits of older change. Preparation requiring all 9710 Qits fails
with insufficient spendable funds before creating a reservation or signature.
This is locked-credit qualification; a mature redemption spend was not performed.
The old fixture prime height uses the historical conversion lock and legacy
wrapping behavior, not modern fork semantics.

Each prepare, broadcast and verify is a separate process using existing SQLite
state. Broadcast uses exact persisted signed bytes. The native wrap deliberately
spent a 10000-Qit coin with an explicit 9000-Qit synthetic fee, so this is not a
specialized automatic-fee efficiency result. Failed account operations retain
their nonce claims and signed bytes; the subsequent claims use new reservation IDs
and nonces. The successful claim alone is included in the successful flow.

## Why the failed claims mattered

`quai_call` and gas estimation succeeded because the node discovers accessed
accounts during simulation. Signed execution requires the lockup account in the
access list. The node-generated list in `wqi-claim-access-list-probe.json` names
Cyprus-1 lockup `0x000000000000000000000000000000000000000A`. `WrappedQi` now
includes the zone-local address in both claim and unwrap calls; `ContractCall`
preserves it through simulation and conversion to the durable account intent.
The gas-only retry is retained as a failed control, not a solution.

The exact node response `-32000: no wrapped Qi balance` with no error data now
maps to `None` in `Provider::wrapped_qi_deposit_optional` and zero in the wrapper;
the raw provider API and unrelated errors remain unchanged. Both recovery APIs
now dispatch origin-zone validation through `SignedQiOperation`, preserving the
specialized conversion/wrapping rules. `Log::address` and log filters accept
validated addresses on either ledger; exact contract event checks still require
the configured Quai contract address. The captured redemption receipt is also a
packaged parser regression with hostile transaction/block association variants.

## Bytecode identity and reproduction

Creation SHA-256: `8add7fd6b6a88b107b95eb04ede0872388f2a6634a00e58b7741f295a15b7cdc`.
Mainnet runtime SHA-256: `bc933c0661d3995cdcf53db5bf0424778a2b35b11a8c5a08999f53d33cf08df2`.
Deployment simulation and the mined code both match the reference runtime after
**only three pinned PUSH32 permit-cache operands** are relocated: self-address,
chain ID 9 → 1337, and the corresponding EIP-712 domain separator. The harness
checks each original operand and opcode before substitution; the remaining bytes
must match exactly. `mainnet-metadata-read.json` retains read-only name/symbol/domain
queries. This explicit adjustment is not byte-for-byte mainnet runtime equality.

```sh
python3 test-infra/local-chain/fetch_wrapper_bytecode.py --wrapper wqi
CARGO_HOME=/your/cargo/cache python3 test-infra/local-chain/highlevel_harness.py run --mode wqi-probe
CARGO_HOME=/your/cargo/cache python3 test-infra/local-chain/highlevel_harness.py run --mode wqi-deploy-prepare
CARGO_HOME=/your/cargo/cache python3 test-infra/local-chain/highlevel_harness.py run --mode wqi-deploy-broadcast
python3 test-infra/local-chain/harness.py mine --count 1
CARGO_HOME=/your/cargo/cache python3 test-infra/local-chain/highlevel_harness.py run --mode wqi-deploy-verify
```

Use the existing funded high-level fixture first, then repeat prepare/broadcast/
bounded mining/verify for `wrap`, `claim_access`, and `unwrap`. Wait for actual
ETX execution before claim or final unwrap verification. Historical blocks and
failed-attempt nonces above belong to this recorded run; fresh runs need not
repeat the old buggy claims. Each operation ID is one-use. The fixed loopback
provider validates the isolated genesis; the harness never resets wallets or
rebroadcasts as part of verification. Verify each intermediate balance before
executing the next stage. The final wallet inventory assertion intentionally
expects this fixture's older 8710-Qit change.
