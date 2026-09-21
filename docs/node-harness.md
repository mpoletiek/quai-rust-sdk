# Phase 0 node harness specification

Status: **provisional; mature funded harness not available**. The implementation must test against both an owned local hierarchy and Orchard. Readiness evidence belongs to an exact profile and run; accelerated-chain results must remain distinct from unmodified-node results.

## Source baseline and transport profile

The candidate node is [go-quai commit f3f345c877300c044e3e0081a48bf3cf786fb9cc](https://github.com/dominant-strategies/go-quai/tree/f3f345c877300c044e3e0081a48bf3cf786fb9cc), version v0.56.0. Readiness logic was checked against that source, not inferred from Ethereum conventions.

The hierarchy CLI defaults to HTTP base 9001 and WS base 8001. Zone ports add 199 plus `20 * region + zone`, yielding Cyprus-1 HTTP 9200 and WS 8200. Prime/region/zone endpoints are distinct. Local direct routing preserves the supplied URL and explicitly disables pathing; it never discovers or rewrites sibling ports implicitly. Orchard uses resolved `/cyprus1` and `/prime` gateway paths. [Pinned port configuration](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/cmd/utils/flags.go#L1056).

The probe uses `quai_getHeaderByNumber`, whose response has body-header fields at the top level and the work-object header under `woHeader`. Its `woHeader.number` is the requested zone height, and `woHeader.primeTerminusNumber` supplies the Prime context. Treat a response with an incompatible schema as a failed check; do not search arbitrary nested objects for plausible numbers. [Pinned header marshaling](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/core/types/wo.go#L1175).

## Why a fresh chain is insufficient

The pinned constants define `BlocksPerDay = 86400 / 5 = 17,280` and `TimeToStartTx = 15 * BlocksPerDay = 259,200`. Gas and state limit calculation both return zero while the **parent zone height** is below this threshold. The local duration target is one second, but that does not by itself rewrite these package-initialized block counts. A genesis-only health check therefore cannot establish executable transfers or contracts. Verify the actual resulting limits and parent-boundary behavior. [Activation constants](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/params/protocol_params.go#L169), [gas calculation](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/core/block_validator.go#L437), [state calculation](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/consensus/misc/statefee.go#L9).

Cyprus-1 initialization verifies the allocation file against a configured hash. Replacing its contents with arbitrary prefunds is not an established unmodified-node shortcut. A supported mature snapshot must preserve its genesis/allocation identity and include spendable test funds whose keys are explicitly public fixture material. Never assume addresses in a shipped allocation file have available keys. [Allocation selection](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/cmd/utils/flags.go#L1703), [allocation hash verification](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/params/genesis_alloc.go#L95).

The index setting is `node.index-address-utxos`. The backend for `GetOutpointsByAddress` reads its database index directly, and an empty successful RPC response can also describe missing/unpopulated index state. Qualification requires an independently known surviving outpoint, queried by address and verified against the chain/UTXO set; passing a random empty address proves only the method response shape. [Index flag](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/cmd/utils/flags.go#L587), [backend read](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/quai/api_backend.go#L230), [outpoint RPC](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/internal/quaiapi/quai_api.go#L200).

## Required profile/run manifest

Store public qualification metadata, never production keys or authenticated URL tokens:

- Exact node commit, binary/container digest, compiler, build flags and complete public configuration; explicitly list patches or record none.
- Chain ID, genesis hash, allocation digest, hierarchy topology, endpoint routes, pathing mode, HTTP/WS capability results and configured RPC version.
- Snapshot digest, trusted provenance, capture procedure, disk size, preparation/reset time, index settings and recovery/rebuild instructions.
- Zone head hash/number, Prime head/terminus context, relevant fork constants, and the exact active rule decisions at each tested boundary. Chain ID alone does not identify these rules.
- Public test-account addresses, public-fixture key identifiers, account balances/nonces, Qi outpoints, denominations, lock heights, expiry context and known spendable inputs. Keep secret testnet credentials outside source control if newly generated.
- Transaction vectors and acceptance/rejection/receipt evidence, traceable to the exact input snapshot, node profile and SDK/oracle version.

The following pinned values are minimum boundary inventory, not a claim that these names all use the same comparison or height context. Each test must follow the actual caller's rule and prove before/at/after behavior. [Pinned protocol parameters](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/params/protocol_params.go).

| Parameter | Pinned value | Qualification concern |
|---|---:|---|
| `TimeToStartTx` | 259,200 zone blocks | Parent-based gas/state activation |
| `ControllerKickInBlock` | 262,000 Prime blocks | Controller/conversion prerequisites |
| `QiActivationBlock` | 1,220,000 zone blocks | Approximation in the constant's comment; inspect actual caller |
| `ConversionSlipChangeBlock` | 285,000 | Conversion payload/rule boundary |
| `KawPowForkBlock` | 1,171,500 | Proof-of-work transition and resulting node profile |
| `SingularityForkBlock` | 1,530,500 | Lock/conversion behavior |
| `ShaEquivalentDifficultyForkBlock` | 1,755,000 | Controller/economic context |
| `MaxGrindIncreaseForkBlock` | 1,865,000 | Address grinding constraints |
| `ConversionStabilityForkBlock` | 1,872,600 | Conversion rules |
| `SelfDestructRefundForkBlock` | 1,919,500 | EVM behavior |
| `QiWrappingChangeBlock` | 1,570,000 | Whether a wrap destination also creates a local Qi UTXO |
| `ConversionLockChangeForkBlock` | 2,237,000 | Conversion lock duration/controller freeze |
| `MaxCodeSizeForkHeight` | 3,490,000 | Contract deployment size bound |

Two interval constants govern conversions rather than a single switch height.
`KQuaiChangeHoldInterval` is 20,000 prime blocks, roughly six to seven days.
v0.56.0 applies it to exactly two heights — `KawPowForkBlock` and
`ShaEquivalentDifficultyForkBlock` — rejecting every Qi-to-Quai conversion for
that many blocks after each, and leaving wraps alone. It is two hard-coded
windows rather than a general consequence of changing the controller.
`UnwrapQiLockPeriod` is 10 blocks, the short unwrap-only lock that replaces the
full conversion lock from `ConversionLockChangeForkBlock` onward.

Both hold windows are behind mainnet (prime terminus 2,256,896 on 2026-09-20),
but **Orchard was at 1,728,920** the same day and had not yet reached the
1,755,000 window. If Orchard shares these constants — which this document
otherwise warns must not be assumed from a matching chain ID — it enters a
20,000 prime-block window in which Qi-to-Quai conversions are refused. Worth
confirming with the protocol team before qualifying conversions there. Whether a
future controller change carries a hold at all is a protocol decision and a
re-pinning question, not something to extrapolate.

Do not apply this table to Orchard solely because the chain ID matches. The observed Orchard node binary/version is not attested; its actual parameters may differ. The read-only probe reports raw context and leaves fork qualification unresolved.

## Qualification sequence

1. Retain the source/build identity and choose a mature snapshot strategy. A fresh unmodified chain can be matured, or a valid owned snapshot restored; measure its cost. If a patched accelerated profile is necessary, label it explicitly and retain a second unmodified-node acceptance lane.
2. Prove deterministic reset and known account/Qi inventory. Validate genesis identity, nonzero gas/state limits, fork context, index witness, nonce/balance and input maturity. Capture configuration and public metadata before the first write test.
3. Test direct local HTTP and WS without path suffixes, Orchard gateway path resolution, explicit prepathed/custom-prefix endpoints and chain mismatch rejection. Use fixture proxies for HTTP/WS faults; actual WS subscription/reconnect evidence remains a separate requirement.
4. Submit public-fixture account transfers/deployments and Qi single/multiple-input spends; verify node acceptance and inclusion. Qualify the conversion payload mismatch identified by the audit against this exact node. Report failures as compatibility gaps, not automatic JS fallbacks.
5. Exercise fork boundaries, expired/locked Qi, spent/missing inputs, outpoint-index rebuild, reorg recovery, transaction ordering constraints and reset reproducibility. Public Orchard smoke tests supplement the controlled lane; they cannot cover inactive zones or provide deterministic historical state.

No existing miner, node configuration, chain data or wallet was altered to create this specification. Provisioning and actual transaction acceptance remain the next substantial Phase 0 dependency.
