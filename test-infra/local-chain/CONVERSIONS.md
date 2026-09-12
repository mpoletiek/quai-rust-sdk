# Conversion qualification rules and boundary

Rules below were read from clean go-quai commit `f3f345c877300c044e3e0081a48bf3cf786fb9cc`. The ordinary development profile's accepted transfers do not qualify conversions: it only accelerates execution activation, while the pinned controller activates at prime height 262,000 and the conversion lock lasts 241,920 zone blocks.

## Typed transaction rules

- Qi → Quai: exactly 22 data bytes, with the first two bytes an unsigned big-endian slippage value and the remaining 20 bytes a Qi refund address. All Quai output addresses must be in the input zone and equal one destination; repeated outputs to that destination are aggregated. Ordinary Qi change outputs remain unique and cannot reuse an input address. Transaction-pool validation permits different conversion destinations, but block processing rejects them: an SDK builder must enforce the stricter block rule.
- Slippage uses a 10,000-unit range; the node clamps to 30–9,000 (0.3%–90%). A typed builder should reject values outside the range rather than silently changing user intent. Absence of an explicit two-byte value defaults to 90% in prime processing, which should not become an implicit SDK default. A same-zone typed Qi refund address is the safe supported boundary; refund processing resolves it in the originating zone.
- Quai → Qi: a type-0 account transaction to a same-zone Qi address, explicit value and two-byte slippage. Its numeric minimum is **10^19 base units**, derived from `10000000000 * GWei`; the adjacent upstream comment describes a different amount and must not be copied as authoritative. Cross-zone Qi destinations fail. The EVM rejects conversions before `ControllerKickInBlock`, and both directions reject the specified post-Kawpow/post-SHA-equivalence hold intervals.
- Qi input ownership, unlocked outpoints, chain identity, ordered Schnorr/MuSig verification and denomination conservation still apply. Conversion and wrapping cannot be mixed. Twenty-byte wrapping data must remain a separate feature.

Primary pinned sources: [input/output validation and settlement](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/core/state_processor.go), [Quai conversion creation](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/core/vm/evm.go), [slippage and prime processing](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/core/slice.go), [numeric parameters](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/params/protocol_params.go).

## Estimator mismatch

`quai_estimateFeeForQi` calls `TransactionArgs.CalculateQiTxGas`, which constructs an internal `QiTx` containing only inputs and outputs; request data is omitted. The calculator adds a per-Quai-output ETX/Tx/storage formula. Actual block processing instead aggregates conversion outputs into one ETX and requires an additional 100,000 gas conversion fee. Consequently a successful quote is not proof that a 22-byte conversion's fee is sufficient, and multiplying an ordinary quote by an undocumented constant would not fix the contract.

The existing provider deliberately rejects nonempty data in its ordinary Qi estimator. An explicit conversion estimator must either implement and qualify the pinned processing calculation against current state or report that the remote quote is unqualified. Acceptance tests should capture both the exact request/quote and actual accepted fee/settlement, including a fee-boundary failure. Sources: [RPC estimator](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/internal/quaiapi/quai_api.go), [request gas construction](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/internal/quaiapi/transaction_args.go), [gas helpers](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/core/types/qi_tx.go).

## Acceptance strategy

A separate conversion development profile is needed; the clean reference and ordinary development snapshot must remain unchanged. Give it a distinct allocation/genesis identity, accelerate controller activation and the conversion lock explicitly, and record every additional change. Lowering controller activation exercises exchange-rate/history logic earlier than production, so successful block processing must be measured before any conversion result is claimed.

For each direction capture the originating signed transaction, canonical origin receipt and emitted ETX, then follow prime routing, destination receipt and actual locked/unlocked value. Qi conversion destination receipts may report `Locked`; origin success is not settlement. Quai → Qi emits locked UTXOs; Qi → Quai adds a locked account balance and later releases it at the configured conversion depth. A valid refund case should preserve its destination and value evidence. If the accelerated hierarchy cannot progress or settle, report the precise blocker and retain origin-only success as a narrower result.

Current status: typed conversions, origin inclusion, Qi-to-Quai release, Quai-to-Qi output creation/mature spend, and an account refund have been observed on the separate accelerated profile described below. No public-network conversion was submitted.

## Observed account-estimate preflight mismatch

On the ordinary profile at height 3, a read-only `quai_estimateGas` request converting 10^19 base units with slippage bytes `0x2328` returned `0x9858` (39,000). Executing the identical request through read-only `quai_call` returned RPC -32000, `CreateETX conversion error: Quai->Qi conversion is not allowed before block 262000`. `conversion-preflight-observation.json` preserves the exact request, state and both responses. Thus an account gas quote also does not prove controller eligibility. No transaction was sent for this check.


## Profile initialization experiments

`prepare_conversion.py` builds a fresh pinned source archive and the ordinary public-fixture patches, then changes only controller activation, conversion lock period and allocation identity. The Quai allocation is `10^27 + 1`, producing genesis `0xff38a93744ee5aae738addc88da4f6b171528244e81d34aa4b25579fa3f44ed2`; see `conversion-profile.json` for binary/allocation hashes. Use `conversion_harness.py` for this profile; the ordinary Rust acceptance client intentionally refuses its different genesis.

The first attempt used controller activation at prime height zero. Startup exposed genesis through RPC, but prime pending-header initialization failed because parent inbound ETX metadata did not exist; the zone pending-header handler then dereferenced a missing work object. No block or transaction was accepted. `conversion-controller-zero-attempt.json` records this failed attempt. The revised profile activates at prime block one, allowing the normal first-header initialization branch.

## Actual acceptance and settlement

The controller-1/lock-4 profile mined to zone height 7 and passed the same read-only conversion call that the ordinary profile rejected. A stopped-node funded snapshot occupies 850,160 bytes. Typed Rust wrappers then produced both origin transactions at zone height 8. These runs retained **all other fork thresholds**, including `ConversionSlipChangeBlock=285000`; there was no extra patch to force settlement success.

| Attempt | Origin | Observed destination outcome |
| --- | --- | --- |
| Qi → Quai, 100 Qits, explicit 22-byte data | `0x00e000911b5008cbb7c357d87ce8674f96ac7af3bba91666baedef928487c48d`, height 8 | Conversion ETX at height 16; value `0x242e3c60dd8ad812e` base units; account release at height 20 verified exactly |
| Quai → Qi, 10 Quai | `0x000c0046459c5090dde40e0f48e41ba7ae9d08ca846d7ae6f60f56db5f835865`, height 8 | Refund ETX type 5 at height 16; historical account balance increased by exactly `10^19`, the original value |
| Quai → Qi, 10,000 Quai | `0x00670063df6bcf8e2342acecfa409506e93191ca3581f16cda8d86057cf8fd55`, height 21 | Conversion ETX at height 25; ten indexed outputs totaling 2,390 Qits, locked until height 29 |

The Qi conversion's data was `2328` (explicit 90% slippage) followed by public Qi refund address `00d7bfbbdd71a5ac547956d138a72ce7f9527369`. It spent the synthetic denomination-14 bootstrap and converted denomination 4 (100 Qits), leaving a deliberately excessive 999,999,900-Qit fixture fee. These small, explicit fixtures establish wire/signature/processing behavior, not economic suitability, minimum fee sufficiency or a production exchange-rate quote.

The larger account conversion exercised a different amount under the same pre-slip-fork flow rule and succeeded; the earlier refund remains part of the record. Returned value and price impact are profile-specific. The test must not be described as a recommended conversion amount or slippage.

The ten converted outputs had denominations `[6,6,4,4,4,3,2,2,2,2]`, indices 0 through 9 and lock 29. An explicit spend attempt at height 26 returned remote code -32000, preserved as an ambiguous broadcast with its expected ID; its receipt remained absent. The remote message was intentionally not logged, so that message's exact wording is not claimed. At height 29 a newly signed ten-input transaction, with all inputs owned by public scalar 300, was accepted; it was included at height 30/status 1/gas 29,000. All ten source outpoints disappeared, with precisely 1,000 Qits at recipient scalar 1332 and 500 Qits change at scalar 2209; the fixture fee was 890 Qits.

Evidence:

- `conversion-origin-fixtures.json`: exact origin receipts and initially emitted ETXs.
- `conversion-settlement-fixtures.json`: executed conversion and refund ETXs at height 16, correlated by origin.
- `conversion-unlock-observation.json`: balance delta at 19→20 equals the exact sum of the Qi conversion plus all coinbases maturing in that block; separate 15→16 delta equals the full account refund.
- `conversion-large-height26-fixtures.json`: successful larger conversion, changed final ETX ID and all locked outputs before maturity.
- `conversion-spend-observation.json`: pre-maturity error code/ID, post-maturity receipt and ten-input transaction, exact remaining/recipient/change inventory assertions.

### Lifecycle findings required in the SDK

Prime processing mutates conversion ETX value, and refund processing changes its subtype. **The final ETX hash changes.** Polling only the hash originally emitted in the origin receipt continued returning null after actual settlement. The stable correlation is the originating transaction hash plus ETX index, with destination/direction/data validation, searching executed transactions in canonical destination blocks. Initial emitted and final included IDs must both be retained.

The pinned receipt codec also maps every nonfailed status to success in `Receipt.statusEncoding` (`core/types/receipt.go:194`), including in-memory `Locked=2`. Both the successful locked conversion and the refund therefore had RPC status 1. A successful receipt cannot establish maturity; validate the profile's lock rules and actual balance/outpoint state. The SDK's lifecycle scanner should report observed inclusion/refund separately from spendability.

### Reproduce this lane

First stop the ordinary-profile supervisor so the fixed loopback ports are free. Preparation refuses an existing work directory and preserves the clean source checkout. The helper argument points at the already-built ordinary-profile helper solely to compute the public allocation digest.

```sh
python3 test-infra/local-chain/prepare_conversion.py \
  --source /tmp/quai-sdk-research-go --work /tmp/quai-sdk-conversion-chain \
  --helper /tmp/quai-sdk-local-chain/sdk-local-tools
python3 test-infra/local-chain/conversion_harness.py start
```

Mine in bounded batches until the real prime terminus is controller eligible (this run required seven zone blocks), then stop/snapshot/restart using `conversion_harness.py`. With the node running:

```sh
python3 test-infra/local-chain/conversion_harness.py client --mode quai-to-qi
python3 test-infra/local-chain/conversion_harness.py client --mode qi-to-quai
python3 test-infra/local-chain/conversion_harness.py mine --count 1
```

Use `capture_conversions.py --quai ORIGIN_HASH --qi ORIGIN_HASH --output REPORT.json` between further bounded mining batches. It scans at most 128 blocks, captures executed ETXs by origin, and rejects a head change during capture. Once the first outcomes settle, `client --mode quai-to-qi-large` exercises the larger conversion. Inspect its returned output lock before using `client --mode qi-spend-converted`; that mode deliberately permits the explicit negative pre-lock test and should not be confused with a wallet's spendability preflight. The normal wallet must reject locked inputs before signing. Mining speed, prime block occurrence, exchange values, output counts and hashes vary; the checked-in records describe the exact observed run.

This lane does not qualify unmodified mature consensus, production forks/rates, fee boundary sufficiency, cross-zone conversion, wrapping, refund-to-Qi execution, wallet persistence/reconciliation, crash during import or reorganizations. It supplies real acceptance evidence and executable fixtures for those follow-up gates.
