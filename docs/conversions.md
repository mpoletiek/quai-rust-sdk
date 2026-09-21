# Explicit conversion transaction types

**September 12 update:** See [current wallet workflows](WALLET_WORKFLOWS.md) for
the added node-backed Qi scanner, mixed-origin sessions, conversion/wrapping
preparation and recovery APIs. Historical evidence and limitations below retain
their original scope; the newer guide describes the additional implementations.

`quai-consensus` exposes separate conversion types. They validate the static wire and signature rules of pinned go-quai `f3f345c877300c044e3e0081a48bf3cf786fb9cc`. They do not prove input existence, value conservation, conversion activation, current exchange rates, fee sufficiency, refund execution or destination settlement. Ordinary Qi signing and submission continue to accept only ordinary empty-data transfers.

## Qi to Quai

Construct `QiConversionTransaction::new(chain_id, inputs, converted_denominations, change_outputs, intent)`, with:

```rust,ignore
let intent = QiConversionIntent {
    destination: same_zone_quai_address,
    refund: same_zone_qi_refund_address,
    slippage: ConversionSlippage::new(100)?, // 1%, explicit
};
let transaction = QiConversionTransaction::new(
    chain_id,
    ordered_inputs,
    converted_denominations,
    unique_qi_change_outputs,
    intent,
)?;
let signed = transaction.sign_local(&ordered_local_key_references)?;
// Persist canonical signed bytes and input claims durably before submission.
let result = provider.broadcast_qi_conversion(&signed).await?;
```

The builder writes exactly 22 data bytes: two-byte unsigned big-endian slippage, then the 20-byte Qi refund address. `ConversionSlippage` rejects values outside 30–9000 ten-thousandths (0.3%–90%) instead of silently accepting the node's clamp. No default slippage is supplied.

Selection for a conversion or a wrap uses `select_fewest_converting`, so the
account outputs are decomposed largest-first without regard to the input
denominations: the node credits their total as one value and exempts them from
the rule that forbids combining smaller inputs into larger outputs. Change stays
in the Qi ledger and is still bound by the input inventory.

Every account output must target one same-zone Quai address. Repeated conversion outputs to that address are valid: Go block processing aggregates their denomination values and removes the address from its reuse set. Different account destinations are rejected even though the transaction-pool validator is less strict. Change outputs must be unique same-zone Qi addresses and cannot reuse any input address. This intentionally supports a narrower shape than combining conversion with cross-zone Qi outputs. A conversion must contain at least one account output; 20-byte wrapping payloads are a separate operation and are rejected.

`from_transaction` explicitly classifies an existing unsigned payload without changing its bytes. `decode_unsigned`, `SignedQiConversionTransaction::decode`, signing digests and transaction IDs preserve order and reject noncanonical or unknown wire fields. Single-input signing, ordered multi-input signing, reversed input ordering and duplicate input keys are supported; duplicate **outpoints** are rejected. Local signing is not a distributed MuSig protocol.

## Quai to Qi

`QuaiToQiTransaction::new(QuaiTransaction)` validates an explicit type-0 request with a Qi destination, exactly two slippage bytes and value at least `MIN_QUAI_CONVERSION_VALUE` (`10_000_000_000_000_000_000` base units). It preserves the destination, value, data, nonce, gas and access list exactly. The sender is established at signing/recovery, where the ordinary account signer enforces a same-zone Qi destination. The returned `SignedQuaiTransaction` can use the existing account broadcast method.

The minimum comes from the pinned numeric rule `10000000000 * GWei`; the adjacent upstream comment gives a conflicting description, so it is not used as the source of this constant. A valid static envelope can still fail controller activation, fork hold intervals, fees or current chain-state rules.

## Quai-to-Qi gas budgeting

The raw `estimate_quai_conversion_gas` RPC result is retained for compatibility.
The pinned node reports destination gas based on the nominal quote's greedy
output decomposition. It omits origin intrinsic gas and ETX creation, and a
controller discount can increase the denomination count. A percentage margin
alone can therefore leave the destination underfunded.

`estimate_quai_conversion_gas_budget` (used by account preparation on native and
browser paths) combines the greater of that RPC estimate and a denomination-count
bound with origin costs. For nominal Qits `v`, every lower denomination `d[i]`
needs at most `min(v / d[i], d[i+1] / d[i] - 1)` outputs; the largest denomination
needs at most `v / d[max]`. Summing those bounds covers every amount below the
quote. Destination gas is 21,000 + 9,000 per bounded output; origin gas is 42,000
plus exact calldata cost (4 for zero bytes, 16 otherwise). Caller margins and
fee caps apply afterward. This budget requires an empty access list, a positive
quote, checked arithmetic and a bound within the node's output limit. It remains
advisory for a later larger quote or a different node gas schedule.

The [Orchard record](../test-infra/orchard/README.md) captures the original failure,
which produced partial locked outputs despite a successful origin receipt.

## Slippage and batch-wide discounts

The pinned node prices conversions per prime-block batch, not per transaction.
It processes a block's conversions in descending slippage order and applies the
cubic flow discount to their combined pre-discount Quai value. A conversion whose
discount exceeds its slippage is refunded as a `ConversionRevertType` ETX; only
its origin fee is lost. Other users' concurrent conversions therefore change the
outcome, and neither `quai_quaiToQi` nor `quai_calculateConversionAmount`, which
price one transaction, can predict it.

`conversion_batch_discount_bps(batch_total, flow_amount)` reproduces the pinned
formula for a hypothetical batch total and the block's `conversionFlowAmount`: 20
basis points at or below the flow amount, `10 + ceil(10·(T/F)³)` up to ten times it,
and the node's 9000 floor beyond. Size slippage against the flow you are willing to
share a batch with. Any k-Quai discount is applied separately.

The k-Quai discount applies to exactly **one direction at a time**, chosen by
whether the exchange rate rose over the last `MinerDifficultyWindow` prime
blocks: Quai-to-Qi takes it while the rate is increasing, Qi-to-Quai while it is
not. On mainnet on 2026-09-20 the header's `kQuaiDiscount` was `0x0`, so it
currently removes nothing; because the k-Quai controller is frozen the rate is
not increasing, so Qi-to-Quai is the side that takes it whenever it becomes
nonzero. Whichever discounts apply, the node floors the result at one tenth of
the original value, which is the same boundary as the 9000-basis-point cap.

## When conversions are held entirely

go-quai v0.56.0 hard-codes **two** windows in which it refuses conversions — one
after each of the two k-Quai controller changes it knows about, each
`KQuaiChangeHoldInterval` (20,000 prime blocks, about six to seven days) long,
enforced as literal height comparisons.

**Both directions are held, and the failure differs sharply.** Wrapping is
exempt either way: the Qi path tracks `conversion` and `wrapping` as separate
flags and holds only the former.

| Direction | Where it is refused | What it costs |
| --- | --- | --- |
| Qi → Quai | Pool admission, in `ValidateQiTxOutputsAndSignature` via `addQiTxs` | Nothing. The submit errors, the transaction never enters the mempool, and the same signed bytes work once the window passes. |
| Quai → Qi | **EVM execution**, in `CreateETX` and `opConvert` | **Nonce consumed and gas burned.** The transaction is admitted and mined; only the conversion fails. A retry needs a fresh transaction. |

**This is not a rule that controller changes halt conversions.** Qi is designed
as energy money whose purchasing power external markets price against production
cost, and those markets are still being bootstrapped; whether a future
controller change carries a hold at all is the protocol team's decision and is
not something this SDK predicts. What is modelled here is two specific
historical windows, re-pinned whenever the go-quai constants are.

There is also nothing to query instead. No header field reports it, and
`quai_getKQuaiAndUpdateBit` returns a hardcoded constant in v0.56.0 with its
state lookup unreachable, so mirroring the node's constants is the only faithful
source.

`quai_consensus::conversion_held(prime_terminus)` answers this, and
the fork heights and the interval are public beside it.

**What actually happens to a held conversion.** The check runs inside
`ValidateQiTxOutputsAndSignature`, which the pool calls from `addQiTxs`, so the
node rejects the transaction at **admission**: `quai_sendRawTransaction` returns
an error and the transaction never enters the mempool. It does not sit pending
and it is not permanently dead either — the hold is height-based, so the same
signed bytes become acceptable once the window passes. Resubmitting the retained
payload afterwards is the remedy, and no funds are at risk in the meantime.

Checking first therefore avoids a wasted cycle, and for the Quai-to-Qi direction
it avoids burning gas on a conversion that cannot happen. **The SDK does not
check for you**: `Provider::estimate_qi_special_fee` deliberately does not
consult the windows, because they cannot recur on mainnet and a chain still
below one runs parameters a matching chain ID does not attest.

Note that a fork block is the *first* block of its own window, so a profile that
activates at a controller change cannot convert during its own opening 20,000
prime blocks.

Both windows are behind **mainnet**, which was at prime terminus 2,256,896 on
2026-09-20, so nothing is held there and conversions are being accepted. They
are not behind every chain: **Orchard was at 1,728,920** the same day, about
26,000 prime blocks below the window that opens at 1,755,000. When it crosses
that height the profile becomes applicable and the window opens in the same
block, so a conversion becomes priceable and unacceptable simultaneously. Note
that Orchard's fork parameters are not independently attested; see
[node-harness.md](node-harness.md).

## Two different slippage numbers

A conversion involves two quantities that are easy to confuse, and a wallet
needs both:

- **What you will lose now.** `Provider::estimate_conversion` returns the
  undiscounted `rate_amount`, the node's discounted `expected_amount`, and
  `implied_slippage_bps` between them, all read at the current head. Show this.
- **What you will tolerate.** `ConversionSlippage`, encoded into the
  transaction's first two bytes. Send this. The node clamps it into 30..=9000
  and treats missing slip as 9000; this SDK rejects out-of-range values instead
  of silently rewriting the intent. If the realized discount exceeds it, the
  conversion is refunded rather than settled.

`estimate_conversion` prices one transaction alone, exactly as the node's own
`quai_calculateConversionAmount` does, so it is a **lower bound** on the
realized slippage rather than a prediction: the batch above is what decides the
outcome. It also carries a small rate-basis difference, because the rate RPCs
evaluate against the zone header while `quai_calculateConversionAmount`
evaluates against the prime terminus block, and the rate includes that block's
`2 x AvgTxFees` after the conversion-lock fork. No RPC exposes the
prime-terminus basis alone, and the reference wallet computes its displayed
figure the same way. The cubic discount is at least 20 basis points for any
value, so this never reads zero for a real conversion. Use `conversion_batch_discount_bps` to size the tolerance, and
`estimate_conversion` to tell the user what the discounts cost at this instant.
Both quote at the current head deliberately — `quai_quaiToQi` resolves its rate
from the current head whatever block selector it is given, so pairing it with an
older selector would report a gap between two different rates as slippage.

On mainnet on 2026-09-14, a recurring third-party Qi-to-Quai conversion of about
255–280 QUAI at 1500 basis points shared every observed batch. It refunded a
100 QUAI conversion at 100 basis points and another at 500 (two competitors), and
reduced successful conversions by 2.8–5.5%. A 200 QUAI size with an 800 basis-point
bound accepted single-competitor batches and refunded a two-competitor batch.
See the [mainnet record](../test-infra/orchard/mainnet-2026-09-14.json).

## Explicit fee profiles

`Provider::estimate_qi_conversion_fee` returns `ProviderError::ConversionFeeEstimationUnavailable` without network I/O. The provider does not quietly route this request through its ordinary Qi estimator.

The pinned `TransactionArgs.CalculateQiTxGas` builds its internal Qi transaction without request data. More specifically, the RPC gas helper charges `ETXGas + TxGas + 2*ColdSload + 2*SstoreSet` per account output, while block processing aggregates account conversion outputs and adds `QiToQuaiConversionGas` (100,000 gas) once. Intrinsic Qi gas itself ignores data, so omission of the 22-byte payload alone is not the complete cause of the estimator mismatch. Those are the *execution* charges. Inclusion is priced separately: the miner's filter and block validation use `CalculateBlockQiTxGas`, which charges `ETXGas` (21,000) for **each** output that creates an ETX, so every Quai-ledger destination output counts. That figure overtakes the flat 100,000 past four destination outputs, and a fee priced only on the execution charge is valid but silently unmineable: the miner skips it and leaves it pooled. `qi_special_gas` therefore bounds the larger of the two, and `QiFeeQuote::floor_qits` reports the un-margined threshold. `prepare_special` refuses an explicit fee below it where the profile applies. `estimate_qi_special_fee` implements a separately selected `V056ShaAnchored` profile at/after prime 1,755,000. Its conservative gas bound and the equivalence of public and internal fee conversion rates are checked against pinned Go functions in `test-infra/go-oracle/special_fee_test.go`. It rechecks the sampled head and rejects incompatible fork state. The high-level `prepare_special_estimated` iterates exact selected shapes within caller fee and round limits before reserving inputs. Quotes remain advisory and do not guarantee future inclusion.

`broadcast_qi_conversion` still permits explicit submission of a previously signed envelope with an independently authorized fee. It verifies expected chain identity and returns the exact locally computed transaction ID on ambiguous outcomes. It sends once and does not own a wallet store. Callers must preserve canonical bytes and claims before exposure/submission and reconcile ambiguous acceptance without releasing signed inputs. `QiSession::prepare_special` / `prepare_special_estimated` and `sign_special` provide durable specialized custody and restart recovery.

## Verification and runtime qualification

Eight pinned JavaScript conversion fixtures cover six Qi single/ordered/reversed/repeated/all-identical key lists and two Quai conversions at the slippage bounds. Rust compares unsigned protobuf, digest, signed bytes, transaction ID and recovered sender/signature. Additional tests reject malformed lengths, clamp overflows, wrong ledgers/zones, missing or different conversion destinations, repeated change, input reuse, altered signatures and the account minimum boundary.

The independent Go oracle verifies both [JavaScript fixtures](../crates/quai-consensus/tests/conversion-vectors.json) and [fresh Rust signatures](../crates/quai-consensus/tests/conversion-rust-vectors.json). Captured reports are [CONVERSION-RESULTS.json](../test-infra/go-oracle/CONVERSION-RESULTS.json) and [CONVERSION-RUST-RESULTS.json](../test-infra/go-oracle/CONVERSION-RUST-RESULTS.json). These are wire/signature compatibility results, not chain acceptance. Reproduce them with:

```sh
node compatibility/scripts/generate-conversions.mjs
cargo test -p quai-consensus
cargo test -p quai-provider --test conversions
cargo run -p quai-consensus --example conversion_oracle > /tmp/quai-rust-conversions.json
python3 test-infra/go-oracle/run.py --fixtures crates/quai-consensus/tests/conversion-vectors.json
python3 test-infra/go-oracle/run.py --fixtures /tmp/quai-rust-conversions.json
```

The [conversion development profile record](../test-infra/local-chain/CONVERSIONS.md) tracks its separate controller/lock modifications and runtime evidence. Origin-block acceptance, prime routing, destination execution, lock release and refunds are distinct milestones. Results from an accelerated isolated profile must not be described as production fork/settlement qualification.

Primary pinned sources: [Go input/output and conversion processing](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/core/state_processor.go), [account conversion creation](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/core/vm/evm.go), [slippage settlement](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/core/slice.go), [protocol numeric parameters](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/params/protocol_params.go), [RPC argument gas calculation](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/internal/quaiapi/transaction_args.go), and [Qi gas helpers](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/core/types/qi_tx.go).

## Bounded lifecycle observations

`ConversionReference::from_qi(trusted_genesis, &signed_conversion)` and `from_quai(trusted_genesis, &signed_account_conversion)` bind a locally signed direct conversion to its chain, zone, original amount, sender, destination, refund address and exact data. The reference fixes the direct-conversion ETX index to zero. It does not infer references for contract-created conversions.

`Provider::observe_conversion(&reference, EtxScanRequest)` first checks chain/genesis and the origin receipt's canonical-by-number block. It validates the emitted ETX against the signed intent, then scans only **executed** destination block transactions using `originatingTxHash` plus `etxIndex`. An emitted `outboundEtxs` entry is not a destination execution. The final ETX hash and converted value may change at prime; comparing only the original emitted hash misses real settlement.

```rust,ignore
let reference = ConversionReference::from_qi(trusted_genesis, &signed)?;
let observed = provider.observe_conversion(&reference, EtxScanRequest {
    zone: reference.zone(),
    from: first_destination_block_to_check,
    to: last_destination_block_to_check,
    max_transactions_per_block: 1_024,
    max_total_transactions: 8_192,
    preceding_block: previous_page_anchor,
}).await?;
```

Each scan covers an explicit inclusive span of at most 256 blocks, with at most 4096 executed transactions per block and 65,536 total. Per-block budgets are reduced to the remaining total budget before typed transaction allocation. Block hashes, heights, locations, transaction indexes, duplicate hashes, local transaction chain IDs, parent links and matched receipt associations are checked. A supplied preceding page anchor must be contiguous. The ending canonical block anchor and the origin anchor are rechecked after the relevant reads; changes return `ObservationChanged`. These remain observations from a trusted node, not atomic RPC snapshots, consensus proofs or finality guarantees.

Unavailable blocks yield `ScanCoverage::Unavailable` with the first missing height and preserve any earlier observed execution. `Complete` describes only the explicitly requested range. A missing origin receipt, missing emitted ETX, missing execution receipt and noncanonical origin have distinct representations. No absence is treated as transaction rejection, successful recovery or proof of complete historical coverage. Generic applications can use `block_with_transactions` and `scan_external_transactions` directly with their own explicit `EtxCorrelation` key and pagination policy.

Known final subtype 2 with a successful receipt yields `ConversionReported`; subtype 5 yields `RefundReported`. Refund ETXs retain the **original** destination, sender and data. The actual beneficiary is the original Quai sender for Quai-to-Qi refunds, or the signed Qi refund address for Qi-to-Quai refunds. The observer checks that a subtype-5 refund preserves the original amount. Unknown subtypes remain `UnknownSubtype`; unavailable receipts, failed receipts and legacy post-state outcomes remain separate observations. The full observed transaction and receipt remain available for inspection.

`ConversionSpendability::Unverified` is always returned. The pinned receipt encoder maps any non-failed status—including internal locked status 2—to persisted success status 1. Consequently success cannot establish unlock height or spendability. Conversion/refund subtype interpretation does not qualify a network's fork schedule, lock period, output index completeness or current spend eligibility. There is no durable conversion state machine or automatic release/rebroadcast policy in this observer.

`locked_quai_balance(address)` exposes `quai_getLockedBalance` as a latest-only **aggregate account** observation with headers sampled before and after. It takes no historical block parameter because the RPC does not support one. Even equal headers do not make the read atomic; differing headers are preserved. Neither zero locked balance nor a balance increase attributes value to a particular conversion or proves its funds spendable. Qualified maturity tracking still needs the relevant fork profile and independent state/outpoint evidence.

Validation includes captured block 16 from the isolated conversion profile at genesis `ff38a93744ee5aae738addc88da4f6b171528244e81d34aa4b25579fa3f44ed2`, the original and final ETX receipts, and adversarial mutations. The explicitly ignored `live_disposable_conversion_correlation_and_locked_balance` test was separately run successfully against that verified localhost profile using read-only RPC. It observed changed final hashes, a Qi-to-Quai conversion, the small Quai-to-Qi refund, and aggregate locked balance. This test never signs, funds, mines or submits a transaction. Its successful observation is not a production-network or maturity qualification.

The block fixture is [`conversion-block16.json`](../crates/quai-provider/tests/fixtures/conversion-block16.json); receipt captures remain under `test-infra/local-chain`. The provider tests can be reproduced offline with `cargo test -p quai-provider --test conversion_tracking`. Run the ignored live test only when the distinct disposable conversion profile is explicitly active on localhost port 19200; it rejects a different genesis.
