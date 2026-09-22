# Protocol alignment and full review, September 20, 2026

A deep review of `0.1.0-alpha.8` against go-quai, quais.js and the Pelagus
wallet, covering protocol correctness, security, performance, efficiency and
release evidence. Every finding below was re-verified against source after the
reviewing agent reported it; claims that could not be re-derived are marked.

Severity is calibrated to consequence for a funded consumer, not to how
surprising the finding is. "Blocking" means it should be resolved or explicitly
accepted in writing before the alpha.8 tag is promoted.

## Verdict

**Conditional no-go on promoting alpha.8 as tagged.** The fee arithmetic the
release is built around is correct, and the build is clean. Of the three items
originally filed as blocking, one is fixed, one is withdrawn, and one remains.

The code quality bar here is high and the review found no consensus-level
encoding, signing or denomination defect.

| ID | Status |
| --- | --- |
| B1 — inclusion-floor guard missing on the portable path | **Fixed.** All three explicit-fee paths now share one `inclusion_floor` helper, with a regression test that pins the refusal. |
| B2 — acceptance gate with no headroom | **Downgraded to LOW, no code change.** The stated mechanism was wrong. The floor does drift, but by a measured fraction of a percent per block that the existing Qit rounding already covers at this fee size. See the section. |
| B3 — committed go-oracle evidence predates the release | **Fixed.** The oracle was re-run against the pinned node; `TestSpecialFeeInclusionGasBound` passes and the artifact now records it. |

Also closed in the same pass: the missing `Breaking:` section, the
reference-evidence tooling gap (E1), both security findings (S1, S2), the
`accesses` subscription (W1), the conversion-estimate helper (C1), the
transaction-size preflight (P2) and the three performance findings (F1-F3).
Nothing is now known to block the tag.

**Two findings were withdrawn after checking their premises, and the record of
why is kept deliberately.** B2 and W2 both looked serious and neither survived
verification; P1 shrank from an enforced guard to a documented constant. The
pattern in all three was the same — a plausible failure mode that the node's
actual behaviour does not produce. See each section.

**These fixes are not in the published 0.1.0-alpha.8**, which went to crates.io
on 2026-09-20 before this review landed. They are staged for the next release;
the alpha.8 changelog entry records the inclusion-floor gap that shipped in it.

## Reference alignment: the SDK is pinned to the current tip of everything

| Reference | SDK pin | Upstream on 2026-09-20 | Status |
| --- | --- | --- | --- |
| go-quai | `v0.56.0` / `f3f345c877300c044e3e0081a48bf3cf786fb9cc` | `main` tip **is** that commit (2026-09-09); `dev` abandoned since 2022 | current |
| quais.js | `1.0.0-alpha.57` | npm `latest` = `1.0.0-alpha.57` | current |
| Pelagus | not pinned | `1.0` branch tip `8c8a044` (2026-09-10) | current |

There is no upstream protocol change queued that the SDK has not tracked. The
two local go-quai checkouts on this machine (`v0.52.0`, `v0.53.0`) are stale;
this review used a fresh clone of the pinned commit.

## Blocking findings

### B1 — The inclusion-floor guard was added to one of two twin paths (HIGH)

`QiSession::prepare_special` refuses an explicit fee below the inclusion floor
(`crates/quai-sdk/src/qi/special.rs:268-296`). The portable `quote_qi` does not:
`crates/quai-sdk/src/qi_preflight.rs:443` is still
`QiFeeMode::Explicit(_) => None`, and `qi_preflight.rs:280-282` rejects only
`Node` for a special transaction, so Explicit + conversion is expressly allowed.

`QiPreflightError::FeeBelowInclusionFloor` (`qi_preflight.rs:186`) is constructed
from exactly one site — `qi_replacement.rs:208`, the replacement path. The
original-build path never raises it.

The repository's own tests encode the contradiction. `tests/qi_preflight.rs:254-275`
drives `quote_qi` with a conversion and a wrap at `QiFeeMode::Explicit(U256::from(5))`
and asserts **success**; `tests/qi.rs:3383-3414` asserts the session path
**rejects** a low explicit fee against the same mock. Same operation, opposite
outcome, decided only by which API the caller picked.

Consequence: the browser/WASM consumer, which uses the portable path, can still
build exactly the unmineable mainnet wrap alpha.8 exists to prevent.

The mechanism is duplication: `qi_replacement.rs:136-215` and
`qi/replacements.rs:137-240` are near-line-for-line duplicates, down to a
verbatim three-line comment. A guard added to one will keep missing the other.

### B2 — DOWNGRADED to LOW: the floor is marginal by a measured fraction of a percent, not by the base fee

This was filed as a blocker on the reasoning that `floor_qits` carries no margin
(`crates/quai-provider/src/qi_special_fee.rs:203-206`, against the 20% margin on
`qits` at `:177-180`) while the gate accepts exactly it
(`crates/quai-sdk/src/qi/special.rs:285-287`), so a moving base fee would strand
a floor-priced conversion. **The stated mechanism was wrong, and a first
correction of it was also wrong.** The record of both, and the measurement that
settles it:

**What the base fee actually is.** `CalcBaseFee` is
`QiToQuai(MinBaseFeeInQits) / TxGas` (`core/headerchain.go:1907-1926`), so it is
not an independent EIP-1559-style quantity; it is a Qit constant restated through
the conversion rate. `QiToQuai` is linear in the Qit amount, and the node
compares `QiToQuai_B(fee) / gas` against `BaseFee_B`, so the rate very largely
divides out. That much holds.

**Where the first correction went wrong.** It concluded the rate therefore
cancels exactly, on the evidence that the `exchangeRate` header field is frozen
(see the kQuai section). That is the wrong quantity. The conversion rate is
`CalculateQuaiConversionReward / CalculateQiReward`
(`consensus/misc/rewards.go:82-93`), and both sides depend on the block's
fork-aware equivalent difficulty, while the Quai side additionally carries
`2 × AvgTxFees` at and after `ConversionLockChangeForkBlock`
(`CalculateQuaiRewardWithFees`). A frozen `exchangeRate` does not freeze that.

**Measured on mainnet, 2026-09-20.** `quai_qiToQuai` for a fixed 1,000 qits:

| Sample | Rate (its per 1,000 qits) |
| --- | ---: |
| zone 10,092,544 | 129,991,386,610,083,934,946 |
| zone 10,158,080 | 147,897,938,670,515,554,278 |
| zone 10,204,864 | 127,098,456,231,874,272,613 |

A ~16% spread. Over six *consecutive* blocks the per-block ratio `R_{B-1}/R_B`
was 1.0033, 1.0046, 1.0033, 1.0033, 1.0033 — a steady decline of roughly a third
of a percent per block.

**What that means for the gate.** A block's base fee is computed from its
*parent* (`core/worker.go:2251`, enforced at
`core/headerchain_validation.go:759`), so the requirement at inclusion is
`fee ≥ gas × 5 / 21000 × (R_{B-1}/R_B)`, and the SDK's floor sampled at block A
carries the same shape, `(R_{A-1}/R_A)`. Both therefore already include the
~1.003 factor, and the exposure is only the *difference between the two ratios* —
about 0.13 percentage points across the observed samples. `qits_covering`
already rounds up by a whole Qit, which on a floor of the size this release
deals with (98 Qits for the 409,400-gas mainnet example) is around 1% of
headroom, comfortably more than the residual.

So the conclusion stands — no margin was added, and the gate stays at the true
floor — but it stands on a measured sub-percent bound rather than on exact
cancellation. Two consequences worth keeping:

- A **large** conversion, where one Qit of rounding is a negligible fraction of
  the fee, has proportionally less protection. Callers pricing large conversions
  should use `Profile` mode and its 20% margin rather than paying `floor_qits`.
- The closed-form offline floor below is an approximation good to the same
  fraction of a percent, not an identity. Treat `× 5 / 21000` as the naive value
  and add a point or two if using it without a node read.

**2. Below the scaling threshold, Qi gas is fixed.**

```go
func CalculateQiGasWithUTXOSetSizeScalingFactor(scalingFactor float64, baseRate uint64) uint64 {
	if scalingFactor < 15 {
		return baseRate
	}
	return uint64(scalingFactor*float64(baseRate)) / 15
}
```

`scalingFactor = math.Log(utxoSetSize)` (`core/worker.go:960`), so scaling
engages only past **e¹⁵ ≈ 3,269,017 outputs**. Below that, gas is exactly
`inputs*SloadGas + outputs*CallValueTransferGas + EcrecoverGas` plus `ETXGas` per
ETX-creating output (`core/types/qi_tx.go:160-186`) — a pure function of the
transaction shape, with no chain state in it.

**The inclusion floor is therefore close to a closed form in Qits:**

```
floor_qits ≈ ceil(required_gas × MinBaseFeeInQits / TxGas)
           = ceil(required_gas × 5 / 21000)
```

with `MinBaseFeeInQits = 5` (`params/protocol_params.go:218`) and
`TxGas = 21000` (`:49`). For the release's own mainnet example, 409,400 gas gives
98 Qits. This is an approximation, not an identity: it drops the `R_{B-1}/R_B`
factor measured above, and integer truncation in the node's two divisions can
move the true threshold by a further unit.

**The opportunity this exposes** is the opposite of a margin. The *gas* half is
genuinely offline below the scaling threshold — it is a pure function of the
transaction shape — so a caller can forecast `required_gas` with no provider at
all, and only needs one cached UTXO-set-size read to know the threshold has not
been crossed. The *Qit* half still wants a node read, because of the
`R_{B-1}/R_B` factor, but the closed form bounds it to within a fraction of a
percent, which is enough to sanity-check a quote or to price offline with a
couple of points of margin. Either way the alpha.8 guard's current cost —
`quai_getLatestUTXOSetSize` plus two header reads and two rate conversions on
*every* explicit-fee conversion or wrap — is more than the problem needs, and it
is the new availability dependency recorded under S3, with the hostile-endpoint
vector alongside it.

### B3 — The changelog's only external-evidence claim is contradicted by the committed artifact (HIGH)

`CHANGELOG.md:14-15` states the bound "is checked against the pinned node in
`test-infra/go-oracle`". The committed artifact does not show that:

- `SPECIAL-FEE-RESULTS.json` records `testSourceSha256 = d95e1278…`, which is
  **byte-identical to the alpha.7 version** of `special_fee_test.go`. The current
  file hashes `2a883cb6…`.
- `git diff v0.1.0-alpha.7..HEAD` on that artifact is **empty**; it was never
  regenerated.
- Its recorded tests are `['TestSpecialFeeGasBound',
  'TestShaAnchoredFeeRatesIgnoreDifficultyArgument']`. The new
  `TestSpecialFeeInclusionGasBound` (`special_fee_test.go:40`), which validates
  the release's entire premise, is absent.
- go-oracle is not referenced by any CI workflow.

The 21,000-per-ETX inclusion model is, independently, correct: `ETXGas = 21000`
at `params/protocol_params.go:55` and `QiToQuaiConversionGas = 100000` at `:164`,
and the crossover claim holds (4 × 21,000 < 100,000 < 5 × 21,000). This finding
is about missing proof, not a wrong number. Regenerate the artifact, or soften
the claim.

## Post-review corrections, 2026-09-20 (second pass)

Three agents re-reviewed the alpha.9 change set against pinned go-quai v0.56.0.
They found ten defects, including four in claims this review itself made. Each
is recorded where it belongs; the ones that corrected a *protocol* statement are
collected here because the original statements were confident and wrong.

| ID | Correction |
| --- | --- |
| F1 | **"Quai-to-Qi is exempt from the hold windows" was false.** `core/vm/evm.go:705-723` sets `conversion = true` for an in-zone Qi destination and applies both windows; `core/vm/instructions.go:1056-1069` repeats it for `opConvert`. Both directions are held. Qi-to-Quai fails at pool admission and costs nothing; **Quai-to-Qi fails inside the EVM, so the transaction is mined and its nonce and gas are burned**. Wrapping is genuinely exempt — the Qi path tracks `conversion` and `wrapping` separately. |
| F2 | **`accesses` is Cyprus-1 only on the pinned node.** `Address::UnmarshalJSON` (`common/address.go:261-272`) bakes in `Location{0,0}` before `InternalAddress()` checks scope, unlike every other address RPC, so a non-Cyprus-1 address is rejected whatever endpoint it is sent to. Upstream-reportable. |
| F3 | **`estimate_conversion` pairs two different rate bases.** The rate RPCs evaluate against the *zone* header (`quai_api.go:1590`), `quai_calculateConversionAmount` against the *prime terminus* block (`:1721`). The rate carries that block's `2 x AvgTxFees` after the conversion-lock fork, so `implied_slippage_bps` is the discount plus a basis gap. No RPC exposes the prime-terminus basis alone; the reference wallet has the same mismatch. Documented rather than removed. |
| F5 | The pool-size guard compares protobuf wire length against a threshold the node applies to an RLP `tx.Size()`. A courtesy check; preparation keeps 512 bytes of headroom so SDK-built transactions never reach the boundary. |
| F6 | `qi_special_gas` samples the *current* UTXO set size; validation scales with the *parent's*. Immaterial below the scaling threshold, but the field no longer claims to be what the node used. |

A measurement in this document was also wrong and has been replaced: see the
performance section's note on the fee-rounds bench.

## The three conversion RPCs, in detail

`quai_qiToQuai`, `quai_quaiToQi` and `quai_calculateConversionAmount` are the
node's conversion surface, and they answer three *different* questions. Treating
any of them as "the conversion price" is a mistake.

### What each one actually computes

| RPC | Node entry | Rate source | Discounts | Answers |
| --- | --- | --- | --- | --- |
| `quai_qiToQuai` | `quai_api.go:1566` | prime terminus **of the requested block** | none | raw rate quote |
| `quai_quaiToQi` | `quai_api.go:1595` | prime terminus **of the current head** | none | raw rate quote |
| `quai_calculateConversionAmount` | `quai_api.go:1685` | prime terminus of the current head, no block argument | cubic flow **and** k-Quai, plus a 10% floor | single-transaction settlement estimate |

**The first two are not symmetric.** `QiToQuai` resolves the prime terminus from
the header you asked for; `QuaiToQi` resolves it from
`s.b.CurrentBlock().PrimeTerminusHash()` regardless of the block argument. So
`quai_quaiToQi` does not honour a historical selector for its rate. The SDK
already knows this and says so at
`crates/quai-provider/src/wallet_rpc.rs:134-136`, which deliberately declines to
promise a historical quote. Nothing to fix; worth not forgetting, because it
makes the `qits_covering` round trip (`quai_quaiToQi` then `quai_qiToQuai` at a
pinned block) asymmetric in principle — harmless while the two prime termini
agree, which is the normal case.

**`CalculateConversionAmount` is a different animal.** Its pipeline, for a
conversion of `value` from the origin ledger:

1. `tenPercentOriginalValue = value / 10`, held aside in origin-ledger units.
2. Convert to Quai if the origin is Qi, because the cubic discount is defined on
   Quai values only.
3. `ApplyCubicDiscount(quaiAmount, primeTerminus.ConversionFlowAmount())`.
4. Convert back to the origin ledger.
5. `× (100000 − kQuaiDiscount) / 100000`.
6. Apply the k-Quai discount **only in one direction**, decided by whether the
   exchange rate rose over the last `MinerDifficultyWindow` prime blocks:
   Quai→Qi takes it when the rate is *increasing*, Qi→Quai when it is *not*.
7. Floor the result at the 10% held aside in step 1.
8. Convert to the destination ledger and return.

### What this means today

- **`kQuaiDiscount` is `0x0` on mainnet** (read 2026-09-20), so step 5 is
  currently the identity and the directional asymmetry in step 6 has no effect.
  It is not dead code, though: it activates whenever the controller sets a
  discount, and because kQuai is frozen the rate is *not* increasing, so
  Qi→Quai is the side that would take it.
- **`conversionFlowAmount` is 100 QUAI.** Under `ApplyCubicDiscount`, a batch at
  or below that takes 20 basis points; above ten times it — 1,000 QUAI — the
  function returns zero and the 10% floor in step 7 is what you actually
  receive. That is the same 9000-basis-point boundary the SDK already models.
- **The rate itself moved ~16% across the samples in B2** and about a third of a
  percent per block, so any of these quotes is perishable.

### How the SDK stands against it

Largely well, and the conceptual work is already done. `docs/conversions.md:72-86`
states the part that matters most and that a naive integration gets wrong: the
node prices conversions **per prime-block batch**, in descending slippage order,
applying the cubic discount to the batch's combined value — so neither
`quai_quaiToQi` nor `quai_calculateConversionAmount`, both of which price one
transaction in isolation, can predict the outcome. `conversion_batch_discount_bps`
(`crates/quai-consensus/src/conversion.rs:262`) reproduces the node's formula for
a hypothetical batch, and the mainnet record of 2026-09-14 shows it being used
against real competing conversions. All three RPCs are wrapped, with accurate
caveats, at `crates/quai-provider/src/wallet_rpc.rs:114-177`.

Gaps found, none blocking:

| ID | Sev | Finding |
| --- | --- | --- |
| C1 | MEDIUM — **fixed** | No "expected slippage" helper: `calculate_conversion_amount` was wrapped but called from no workflow. Added `Provider::estimate_conversion`, returning the rate quote, the discounted estimate and `implied_slippage_bps` between them. See the Pelagus pattern below. |
| C2 | LOW | The k-Quai discount's **direction rule** is nowhere in the SDK. `docs/conversions.md:86` says only "Any k-Quai discount is applied separately", which is true but does not say that it applies to exactly one direction at a time, chosen by the exchange-rate trend. Harmless at `kQuaiDiscount = 0`; wrong-footing the day it is nonzero. |
| C3 | LOW | The protocol's **10% floor** on conversion output is not stated in the SDK docs. It is the reason the worst case is "receive 10%" rather than "receive nothing", and it is the other face of the 9000-bps cap the SDK already models. |
| C4 | INFO | `CalculateQuaiConversionReward` changed at `ConversionLockChangeForkBlock` (2,237,000, now past): the Quai side of the rate now includes `2 × AvgTxFees` (`CalculateQuaiRewardWithFees`). The SDK reads rates from the node so its numbers are right, but `QiFeeProfile::V056ShaAnchored` spans that boundary with a single lower bound of 1,755,000, describing two rate regimes as one. Reinforces P1's recommendation to bound the profile above. |
| C5 | LOW | The SDK **rejects** slippage outside `30..=9000` (`conversion.rs:16-21`); the node **clamps** into that range and defaults to `MaxSlip` (9000) when the data carries no slip at all (`core/slice.go:374-384`). The SDK's constants are exactly right — `MinSlip = 30`, `MaxSlip = 9000` (`params/protocol_params.go:242-243`) — and rejecting rather than silently rewriting user intent is the better choice. Worth recording that the reference wallet's custom field admits `0`, which the node then silently raises to 30, so a "0% slippage" setting there is really 0.3%. |
| C6 | LOW | `calculate_conversion_amount` needs only ledger scope and zone, not real accounts — the reference quotes with placeholder addresses (`0x0090…`/`0x0010…`, `convertAssets.ts:158-159`). The SDK's wrapper accepts any same-zone pair and so supports this, but does not say so, and a caller may believe a funded address is required. |

### The pattern the reference wallet implements, and what the SDK is missing

Pelagus separates two quantities that are easy to conflate, and the
[protocol documentation](https://docs.qu.ai/learn/tokenomics/token-dynamics/conversions)
confirms the model:

- **Expected slippage** — what you will lose to the discounts right now. Computed
  as the gap between the undiscounted rate and the discounted estimate:
  `getLatestQuaiToQiRate` for the raw rate, `calculateConversionAmount` for the
  discounted amount, then
  `slip = (rate × amount − expected) / (rate × amount)` (`convertAssets.ts:160-193`).
  This is **displayed**, not sent.
- **Max slippage** — the tolerance the user is willing to accept, default 100 bps
  with 1/3/5% presets and a custom field
  (`MaxSlippageSelector.tsx:6-10`). This is **sent**, encoded two bytes
  big-endian into the transaction data: alone for Quai→Qi
  (`transactions/index.ts:535-546`), and followed by the 20-byte refund address
  for Qi→Quai, making the 22 bytes the node's `MaxQiTxDataLength` allows
  (`:1164-1180`, `params/protocol_params.go:262`).

If the realised discount exceeds the sent tolerance, the node refunds the
conversion as a `ConversionRevertType` ETX rather than settling it — the
protocol docs put the floor at `finalValue ≥ 0.1 × originalValue`, and note that
Quai→Qi refunds arrive by ETX in the next zone block while Qi→Quai refunds stay
locked for the two-week conversion lock-up.

**The SDK had every ingredient and assembled none of them.** It wraps all three
RPCs, it models the slippage encoding exactly, and in
`conversion_batch_discount_bps` it models the *batch* effect that Pelagus does
not — the reference's displayed slippage is a single-transaction figure, and
`docs/conversions.md` is right that competing conversions in the same prime block
change the outcome. What was missing was the small helper on top.

**Added as `Provider::estimate_conversion`** (C1). Given a direction and an
amount it returns a `ConversionEstimate`: the undiscounted `rate_amount`, the
node's discounted `expected_amount`, and `implied_slippage_bps` between them,
rounded up so a wallet is never shown less slippage than the estimate implies.
Both halves are read at the current head deliberately, since `quai_quaiToQi`
resolves its rate from the current head whatever selector it is given — pairing
it with an older selector would report the gap between two different rates as
slippage. The doc comment is explicit that this is a lower bound on realized
slippage rather than a prediction, and points at
`conversion_batch_discount_bps` for sizing the tolerance. This leaves the SDK
strictly ahead of the reference here: it now offers the same displayed figure
*and* the batch model the reference lacks.

## Protocol alignment against go-quai v0.56.0

No CRITICAL findings. Encoding, signing digests, transaction IDs, denominations,
address/ledger/zone rules and MuSig aggregation match the pinned node exactly.
v0.56 has no `MinerTip` field (GasPrice only), and the v0.53→v0.56 `core/types`
diffs are nil-hardening only.

| ID | Sev | Finding |
| --- | --- | --- |
| P1 | MEDIUM — **recorded, not enforced** | The two k-Quai hold windows are exposed as `quai_consensus::qi_to_quai_conversion_held` with their constants and a boundary test. The SDK does not act on them; see [the kQuai section](#p1-in-full-the-kquai-freeze-and-the-conversion-hold-interval). |
| P2 | MEDIUM — **fixed** | `MAX_POOL_TRANSACTION_BYTES` (128 KiB) now records the node's `txMaxSize` (`core/tx_pool.go:55-61,:806-810`) beside the codec's 1 MiB `MAX_TRANSACTION_BYTES`. Broadcast refuses an oversized Quai transaction before the submit, and deployment planning bounds init code against it. Qi transactions take a pool path with no size check, so the bound stays Quai-only. |
| P3 | MEDIUM | `docs/QI_CONVERSION_SELECTION_GAP.md:187` attributes the inclusion gas floor to `core/worker.go:3037`; that line is the `!firstQiTx` `CheckDenominations` guard. Correct cites are `core/worker.go:2526` and `core/state_processor.go:389`. |
| P4 | MEDIUM | `docs/node-harness.md:44` omits `KQuaiChangeHoldInterval` (20 000) and `QiWrappingChangeBlock` (1 570 000); the latter decides whether a wrap destination output also creates a local Qi UTXO. |
| P5 | LOW | Unwrap rules unmodelled: denominations ≤ 500 Qits are dropped and outputs locked `UnwrapQiLockPeriodAt` (10 blocks post-2 237 000) — `core/state_processor.go:463-465,:479-483`. Both new since v0.53. |
| P6 | LOW | `denominate_available` checks denomination 0; go's `CheckDenominations` loop stops at 1. The SDK is stricter, which is safe. |
| P7 | LOW | The node does not enforce a single wrap destination (only conversions do, `core/state_processor.go:1990-1996` vs `:2004-2012`). The SDK is correctly stricter; worth reporting upstream. |

### P1 in full: the kQuai freeze and the conversion hold interval

**Where the chain is.** Mainnet Cyprus-1 read on 2026-09-20 reports prime
terminus **2,256,896**. The relevant constants
(`params/protocol_params.go`):

| Constant | Value | State today |
| --- | ---: | --- |
| `ShaEquivalentDifficultyForkBlock` | 1,755,000 | past |
| `ConversionLockChangeForkBlock` | 2,237,000 | past |
| `KQuaiFreezeForkBlock` = the above | 2,237,000 | **active for 19,580 prime blocks** |
| `KQuaiChangeHoldInterval` | 20,000 (≈6–7 days of prime blocks) | no window open |

**kQuai is currently frozen.** `CalculateBetaFromMiningChoiceAndConversions`
returns the parent exchange rate unchanged once prime height reaches
`KQuaiFreezeForkBlock` (`core/exchange_controller.go:14-19`), under the comment
"A future fork can explicitly resume or replace the controller without changing
the pre-fork exchange-rate history." Sampling the chain agrees: `exchangeRate`
is byte-identical (`0xb815924db8c94800`) at prime 2,212,000, 2,241,939 and
2,256,583.

**Impact today: none from the hold interval, and a quiet tailwind.** Both hold
windows — after the KawPow fork and after the Sha-equivalent fork — closed long
ago, so no conversion is being refused for this reason now. The freeze also
means the Qi↔Quai rate is constant by consensus, which is why every
rate-sensitive quantity in the fee path is currently stable. That is a property
of this moment, not of the protocol, and it is worth being explicit that the SDK
is presently being exercised in the easiest rate regime it will ever see.

**Why it matters when the freeze lifts.** Every controller change so far has
been paired with a 20,000-prime-block window in which the node rejects *every*
Qi→Quai conversion while leaving wraps alone
(`core/state_processor.go:1790-1797` in the pool path and `:2095-2102` in block
processing, mirrored by the miner at `core/worker.go:2998-3001`). If resuming
kQuai follows that pattern, unfreezing brings a roughly week-long conversion
blackout. The SDK models none of it, and two existing behaviors turn that into
stranded funds rather than a clean error:

1. It will build straight into the window. Selection reserves the inputs,
   custody marks them, the transaction is signed and submitted, and only then
   does the node refuse it.
2. The refusal is at **pool admission**, not a pending limbo. The check lives in
   `ValidateQiTxOutputsAndSignature`, which the pool calls from `addQiTxs`
   (`core/tx_pool.go:1231,:1302,:1374`), so `quai_sendRawTransaction` errors and
   the transaction never enters the mempool. Confirmed against a real Pelagus
   attempt during a window, which was rejected rather than left pending.
3. It is also not permanently dead. The hold is height-based, so the same signed
   bytes become acceptable once the window passes; resubmitting the retained
   payload is the remedy. **No funds are stranded**, which removes this case
   from W2's justification entirely — an earlier draft of this review had it the
   other way round.

**The sharper, structural problem: the profile is open-ended above.**
`QiFeeProfile::V056ShaAnchored` refuses chain states *below* 1,755,000 and
accepts everything above without limit (`crates/quai-provider/src/qi_special_fee.rs:163-168`):

```rust
QiFeeProfile::V056ShaAnchored if before.prime_terminus_number < 1_755_000 => {
    return Err(ProviderError::ConversionFeeEstimationUnavailable);
}
QiFeeProfile::V056ShaAnchored => (),
```

A profile named for one node version and one reward regime should not keep
claiming to apply after a fork that changes that regime. When kQuai resumes, the
rate stops being constant and the reward math the profile is anchored to may
change, yet this gate will still report the profile as applicable and the
estimator will keep quoting. The failure is silent and it is on the fee path.

**What was done, and what was deliberately not.**

The windows and the interval are exposed as
`quai_consensus::qi_to_quai_conversion_held` with a boundary test, and the
behaviour is documented. **The SDK does not enforce them.** Three reasons, in
order of weight:

1. Mainnet can never reach either window again — prime terminus only increases,
   and both are far behind. The check would be unreachable code on the chain
   that matters.
2. A chain still *below* a window runs its own parameters, which a matching
   chain ID does not attest (`docs/node-harness.md` says so explicitly). A
   hard-coded height check could refuse a dev chain the node would have
   accepted. Development chains are exactly where that risk lives.
3. Nothing is at stake either way. The node refuses a held conversion at pool
   admission, so the wallet learns immediately, and the same signed bytes become
   acceptable once the window passes.

An upper bound on the fee profile was also considered and rejected: no fork
above the current chain height in v0.56.0 changes conversion math, so any
ceiling would either do nothing or break the SDK on an arbitrary future date.
Designing against an unannounced fork is guesswork. The structural protection is
`#[non_exhaustive]` on `QiFeeProfile`, so a second profile can be added without
breaking callers, and re-pinning go-quai is what surfaces a new regime.

There is also nothing to query instead of the constants. No header field reports
a hold, and `quai_getKQuaiAndUpdateBit` returns a hardcoded value in v0.56.0
with its state lookup unreachable, so mirroring the node's own constants is the
only faithful source available.

## Reference-evidence integrity

### E1 — The pinned `quais@1.0.0-alpha.57` is a broken publish, and the tooling pins the half nothing runs (HIGH, evidence integrity)

Verified directly: `node_modules/quais/package.json` and `src/_version.ts` both
say `1.0.0-alpha.57`, but `lib/esm/_version.js` and `lib/commonjs/_version.js`
both say **`1.0.0-alpha.52`**. The published compiled JavaScript is five releases
behind its own published source.

The export map (`.` → `./lib/esm/index.js`) means every consumer path lands in
`lib/`:

- `compatibility/scripts/reference.mjs:1` imports `'quais'` → the alpha.52
  runtime. Every fixture and `*.test.mjs` regression pins alpha.52 behavior.
- `compatibility/scripts/inventory.mjs:13` derives declarations from that same
  export map (`.js` → `.d.ts`), so `api-inventory.json` and all 3,928
  `parity.json` rows describe **alpha.52's** surface.
- `compatibility/scripts/verify-reference.mjs:20-22` hashes **only**
  `node_modules/quais/src`, and line 18 asserts the version from `package.json`.
  Both pass. Nothing in the pipeline ever looks at `lib/`.

The coverage gap is real and bounded. `deepScan`, `getOutpointsByAddresses`,
`setAddressStatus`, `setGapLimit` and `getReusableAddress` all exist in `src/`,
appear in **zero** `lib/**/*.d.ts` files, and have **zero** rows in both
`api-inventory.json` and `parity.json`.

This is not a correctness defect in the Rust code and puts no funds at risk. It
means `SDK_PARITY_ANALYSIS.md`'s "0 pending / 0 partial against alpha.57" is in
fact a statement about alpha.52, and that alpha.53–57 members were never in
review scope. It is the systemic instance of the blind spot already noticed once
for `ConversionCoinSelector`.

Recommended: extend `verify-reference.mjs` to hash `lib/` and assert
`lib/esm/_version.js` against `package.json` so this fails loudly; state alpha.52
as the behavioral reference in `compatibility/README.md` and
`SDK_PARITY_ANALYSIS.md`; report the broken publish upstream.

### E2 — A documented migration snippet will throw on any correctly built quais (MEDIUM)

Six places assert both JS `xPub()` methods return xprv
(`docs/HD_WALLET_PARITY_REVIEW.md:13`, `docs/LEGACY_WALLET_MIGRATION.md:18`,
`parity.json:3797/30185/31959`, `IMPLEMENTATION_STATUS.md:819`). True of `lib/`;
false of the pinned `src/`, which neuters at `quai-hdwallet.ts:80`,
`qi-hdwallet.ts:288`, `bip44/bip44.ts:53`.

`docs/LEGACY_WALLET_MIGRATION.md:23` tells users to run
`HDNodeWallet.fromExtendedKey(wallet.xPub()).neuter().extendedKey`. Against a
fixed quais, `xPub()` returns an xpub, `fromExtendedKey` returns
`HDNodeVoidWallet`, which has no `neuter()`. That line becomes a `TypeError` the
moment a consumer upgrades.

### E3 — Stale review documents (LOW)

- `docs/dependency-audit.md` records 373/259 packages and hash `61808040…`;
  actual is 403/359 and `c9dde68b…`. `cargo audit 0.22.2` re-run on both locks:
  still 0 vulnerabilities, same 2 unmaintained warnings — the conclusion holds,
  the evidence binding does not.
- `docs/SECURITY_REVIEW_2026-09-11.md` still says "All crates remain
  `publish = false`"; `Cargo.toml:11` is `publish = ["crates-io"]`.
- `docs/QI_SELECTION_PARITY_REVIEW.md`'s `increaseFee`/`decreaseFee` "reference
  defects" are alpha.52 defects already fixed in the pinned `src/`
  (`coinselector-fewest.ts:258-266,:306-311`).

## Security

No CRITICAL or HIGH established. Two MEDIUM, four LOW.

### S1 — Mnemonic and passphrase guards grow, leaving unwiped heap copies (MEDIUM)

The 2026-09-11 review fixed this class for the keystore password; the
`quai-wallet` equivalents were not.

`crates/quai-wallet/src/mnemonic.rs:184-187` (`to_seed`) reserves
`passphrase.len() * 4` and, unlike `wordlist.rs:561`, applies **no length bound
at all**. NFKD plus case folding expands well past 4×; measured for U+FDFA:

```
input bytes      : 3
reserved (len*4) : 12
NFKD bytes       : 33
expansion factor : 11.0
```

Same under-reservation at `mnemonic.rs:110-113`, where the doc comment at
`:105-109` asserts it cannot happen. Also `wordlist.rs:565-566` and `:524,526`
(`collect::<String>()` from zero capacity, reallocating ~6 times for a 24-word
phrase before the length check at `:567`) and `:238-242`.

The regression test does not catch it: the test named
`secret_text_guards_are_sized_before_any_secret_is_copied` ends at
`mnemonic.rs:241` with `reparsed.to_seed("\u{fdfa}")` — the exact input that
makes the guard reallocate — and asserts only `seed.0.len() == 64`. Its two
capacity assertions cover `phrase()` only.

Impact: a core dump, swap file or same-user read recovers several successively
longer prefixes of the seed phrase instead of one wiped buffer. Fix: adopt the
keystore's pre-size-and-reject pattern and bound `Mnemonic::to_seed`'s
passphrase.

### S2 — Node keystore password sent over plaintext HTTP (MEDIUM)

`crates/quai-sdk/src/rpc_signer.rs:273-290` (`unlock`) validates only
`password.len() > 1024` and the duration, then dispatches
`personal_unlockAccount`. Neither `dispatch` (`:184-194`) nor
`RpcAccountSigner::new` (`:115-138`) checks the scheme, and `Endpoint::parse`
(`crates/quai-rpc/src/routing.rs:36-38`) accepts `http`/`ws`. Grepping
`rpc_signer.rs` for `https`/`scheme`/`tls` returns nothing.

The codebase already implements this control elsewhere —
`FetchError::InsecureAuthentication` at `crates/quai-rpc/src/fetch/model.rs:296-303`
refuses auth headers over `http` — so this is an inconsistency, not a missing
concept. With the `http://node:9001` shape used throughout
`test-infra/local-chain/`, the password crosses the wire in cleartext and unlocks
every account in that node's keystore. `no_proxy()` means no TLS-terminating
proxy saves it.

### S3 — Lower severity

- LOW: wallet SQLite created with process umask (`storage.rs:129`), no
  `set_permissions` anywhere; exposes `account_xpub` (`storage.rs:243`) → full
  lifetime address enumeration.
- LOW: `hd.rs:188-203` leaves the master chain code unwiped; bip32-0.5.3's `Drop
  for ExtendedKey` zeroizes only `key_bytes`.
- INFO: `WsConfig::validate` (`websocket.rs:107-128`) never requires
  `max_notification_bytes >= max_message_bytes`, so a mis-sized config kills the
  whole multiplexed session on the first oversized notification.
- INFO: the new explicit-fee guard makes **every** explicit-fee conversion/wrap
  depend on `quai_getLatestUTXOSetSize` plus two header reads, fatal on any error
  but `ConversionFeeEstimationUnavailable`. That is a new availability dependency
  on a previously offline-priced path, and a hostile endpoint can block all such
  sends by inflating `baseFeePerGas`.

### Verified clean

Entropy and zeroization wrappers, redacting `Debug` throughout, **zero logging
calls in any `crates/*/src`**, RFC 6979 + low-s + BIP340 sign-then-verify against
pinned vectors, MuSig local aggregation with no nonce-reuse exposure, chain-id
replay binding on both ledgers, keystore constant-time MAC-before-decrypt with
KDF floors *and* ceilings, RPC/ABI/backup bounds checking, gzip bomb protection,
rustls with no `danger_*`, no redirects/retries/proxies, `unsafe_code = "forbid"`,
and the selection/denomination arithmetic (all 14 adjacent denomination split
ratios are exact integers; no `capacity[] -= 1` underflow).

The alpha.8 diff introduces no new panic or arithmetic hazard:
`aggregate_destination_outputs` preserves the destination total with
`checked_add`, rejects a second distinct destination, and guards
`max_outputs.checked_sub(kept)`. `qi_special_gas` is fully checked.

Not checked: `cargo deny` is not installed on this host, so no license/ban/source
policy was evaluated. MuSig equivalence with `AggregateKeys(keys, false)`
unverified. wasm/browser and SQLite concurrency only spot-checked.

## Performance and efficiency

A premise this review started with was wrong and is worth recording: consecutive
BIP32 non-hardened children do **not** differ by a known scalar — the offset is
an HMAC-SHA512 output (`hd.rs:553-565`) — so there is no point-addition shortcut
for the grind. Measured per-candidate split: EC work 93.68%, HMAC 4.70%,
address/keccak 1.62%. The grind is essentially at its floor, and `ScanBranch`
already bypasses bip32's generic ladder, the fingerprint and the
compress/decompress round trip.

All figures below are same-run ratios; this host drifts ~16% run to run, so no
absolute timing is load-bearing.

| ID | Impact | Finding |
| --- | --- | --- |
| F1 | HIGH — **fixed** | Coin selection was ~94% `BTreeSet` dedupe. It now uses a pre-sized `HashSet`; ordering comes from the denomination buckets, so nothing about the chosen coins changed and the 69 pinned JS vectors still pass. `OutPoint` derives `Hash`. |
| F2 | HIGH — **fixed** | `select_with_fee` repeated the whole O(n log n) validation per round. Validation and bucketing are hoisted into `validate_and_bucket`, run once; each round only re-runs `choose`. Single-shot error precedence is preserved by keeping the overflow check ahead of the coin loop. |
| F3 | HIGH — **fixed** | Both rayon paths derived one index at a time while the sequential path batched. They now chunk the range by `SCAN_BATCH` through a shared `classify_run`, so each chunk pays one normalization. The existing sequential/parallel equivalence tests cover it, and `window_scan` gives it bench coverage it never had. |
| F4 | HIGH | `bip39` `all-languages` costs ~853 KiB and is default-on and unstrippable (`crates/quai-wallet/Cargo.toml:35`); language is a runtime parameter so LTO cannot remove it. Stripped binaries measured 1,417,736 B vs 544,168 B. Reaches wasm via `quai-sdk`'s `default = ["http", "wallet"]`. Make it a cargo feature. |
| F5 | MEDIUM | `insert_address` re-imports the xpub and re-derives per address inside a write transaction (`storage.rs:1225-1233`, called at `:219`, `:544`, `backup_state.rs:146`) — ~3.2 CKDpub-equivalents where 1 suffices, ×N on backup restore. |
| F6 | MEDIUM | `allocate_address_compact` holds an IMMEDIATE write lock across a full grind and across O(A) derivations (`storage.rs:299-301,373,316-349`). |
| F7 | MEDIUM | `decode_batch` parses each row twice plus a copy on the SDK's largest JSON (`quai-rpc/src/http.rs:245-267`); `http.rs:228` serializes the request body twice. |
| F8 | MEDIUM | Qi discovery never overlaps grinding with its RPC round trip (`quai-sdk/src/discovery/qi.rs:341-430`), and it is worse with `Grinding::Parallel`. |
| F9 | LOW | Un-cached statements in two per-row loops (`storage.rs:561`, `:1267`); the same file gets it right at `:679`. HMAC key schedule recomputed per candidate, worth ~2.3%. |

**Measured after the fixes, 2026-09-20.** Same-run ratios only; this host drifts
about 16% between runs, so no absolute figure is load-bearing.

| Measurement | Result |
| --- | --- |
| `select_with_fee` 8 rounds / 1 round at n=10,000 | **1.18x** |
| `select_with_fee` 8 rounds / 1 round at n=100,000 | **1.55x** |
| `window_scan` parallel / sequential | **3.65x** faster |
| `select_fewest` at n=100,000 (new point) | 7.94 ms |

**A first version of this table reported 1.02x and 1.00x for the fee rounds, and
those figures were wrong.** The bench's estimator quoted a fee below the
request's starting fee, so `select_with_fee` converged on the first call and
both arms measured a single round. The numbers above come from a corrected ramp
that keeps every quote but the last above the running fee.

The corrected ratio still carries the finding: the review measured eight rounds
at 14.08x one round before the hoist, and they now cost 1.55x. It is not 1.00x
because only the O(n) validation and bucketing were hoisted — `choose` still
scans the buckets and re-denominates on every round, which is real work at a
hundred thousand candidates.

Criterion's own comparison against its stored baseline put `select_fewest` down
66% at n=100, 82% at 1,000 and 85% at 10,000; those are cross-run, so the
magnitude is indicative rather than exact, but it is far outside host drift.

Also noted: `quai-sdk/src/qi.rs:527` runs a full SQLite snapshot and `:75` a grind
on the async executor with no `spawn_blocking`. This is documented for
`search_window_async` but not for the `QiWallet` facade.

Benchmark gaps: no coverage of the parallel grind (which is why F3 was invisible),
`add_tweaks`, `select_with_fee`/`select_sweep`/`select_aggregate`, the SQLite
store, `decode_batch`, `quai-consensus` encode/signing-hash, or `quai-abi`. The
`selection` bench stops at 10,000 while the cap is 100,000, and its "per-element
cost stays flat" claim does not hold — 1.36× at 100k.

Clean: `quai-abi` encode (measure-then-allocate-once), `hash.rs`,
`HeadTracker::poll` (3 batched round trips), `blocks.rs`, `outpoints_many`
batching, `SCAN_BATCH=32` sizing, k256 `precomputed-tables`.

## Wallet integration gaps, measured against Pelagus

Pelagus 1.0 (`8c8a044`) on `quais@1.0.0-alpha.57` is the closest thing to a
reference consumer. The SDK is materially **ahead** of it on nonce and
replacement tracking, reorg replay, Qi inclusion-gas fee floors and
denomination-correct selection — Pelagus has no speed-up/cancel, no DROPPED
state, and hardcoded 1M/1.1M gas limits. The gaps run the other way on
subscriptions and recovery.

| ID | Sev | Finding |
| --- | --- | --- |
| W1 | HIGH | The SDK cannot express the `accesses` subscription. Pelagus does not poll: it opens `quai_subscribe ['accesses', address]` per watched address and drives balance refresh, incoming-funds notification, pending-tx confirmation, Qi resync and conversion revert detection off it. `accesses` has **0 occurrences** anywhere in `crates/` or `docs/`; `WsSubscriptionKind` (`crates/quai-rpc/src/websocket.rs:137-156`) is `NewHeads \| NewPendingTransactions \| Logs \| Raw`, and `Raw` is documented as an untyped escape hatch with no typed payload, no resubscribe-on-reconnect and no missed-while-disconnected signal. `docs/WALLET_WORKFLOWS.md:622-639` prescribes per-head full sync instead. |
| W2 | **WITHDRAWN** | No chain state strands funds, so the release path solves nothing. See the section under this table. |
| W3 | MEDIUM | No escape hatch on `Provider`: `transport` and `routing` are private with no accessors and `read` is private (`crates/quai-provider/src/lib.rs:165-169,228`). Any untyped method needs a second parallel Transport+Routing that bypasses the chain-ID guard. 36 Pelagus-reachable RPC methods cannot be type-called; all but `accesses` and `qi_signAll` are dApp-bridge surface, which is a scope question rather than a defect. |
| W4 | MEDIUM | `FeeData` is a bare gas price — no tiers, floor or fallback — against Pelagus's full model with a 1 gwei floor. |
| W5 | MEDIUM | Conversion refund addresses are burned even when no refund occurs; Pelagus reclaims them. This is the same gap-pressure problem already fixed for change addresses. |
| W6 | MEDIUM | Rust default Qi gap is 50 (`discovery/qi.rs:16`); the reference's is 5 (`abstract-qi-wallet.ts:125`). No document states the difference, so addresses at indexes 5..50 past a used one are not recoverable by a default quais.js restore. `docs/QI_CHANGE_REUSE.md:10`'s "Pelagus scans with a gap of 50" could not be verified from this repository. |
| W7 | LOW | `account_states` omits locked balance. No multicall; Pelagus moved to Multicall3 per shard. |
| W8 | UNRESOLVED | `qi_signAll` is a real dApp method returning comma-joined **ECDSA/EIP-191** signatures over funded Qi addresses, which is incompatible with the SDK's BIP340-Schnorr `sign_qi_message`. Which is canonical was not determined; this needs an upstream answer. |

### W2 in full: WITHDRAWN — no chain state strands funds

The finding was that a signed reservation the node refuses keeps its outpoints
claimed forever, because `crates/quai-wallet/src/qi_custody.rs` exposes only
`release_unsigned` (line 230). Both routes into that state were checked and
neither strands anything.

**A held conversion is deferred, not dead.** The node refuses it at pool
admission, and the hold is a height comparison, so the same signed bytes become
acceptable once the window passes. Resubmitting the retained payload completes
the operation. Those coins were always going to be spent by that transaction.

**Inputs taken by a different confirmed spend are already gone.** Those outpoints
no longer exist on chain. `refresh_qi` rebuilds the snapshot from the node, so
they are not in it, and selection only draws from the snapshot minus claimed
outpoints. **The claim is on a coin that does not exist — it blocks nothing.**
The earlier draft of this finding mistook a stale bookkeeping row for a lock on
spendable value.

That leaves only "a signed transaction that can never be mined while its inputs
still exist." Underpricing does not qualify: a fee replacement is an exit, and a
falling base fee is another. No construction of that case survived scrutiny.

An `Abandoned` state was implemented far enough to price it before being
reverted: one exhaustive match, four integer decode tables, an `ActivityStatus`
variant — and, decisively, a rebuild of the `reservations` table, because
`CHECK(state BETWEEN 0 AND 4)` rejects a new value and the table is the foreign
key parent of `nonce_claims`, `qi_claims` and `signed_payloads`. A migration of
that shape for an empty set of failures is the wrong trade. Nothing was kept.

### The real custody gap: no operation lifecycle (NEW, MEDIUM)

Found while checking W2, and unrelated to it.

Both portable custody books cap lifetime operations at 256 —
`MAX_QI_OPERATIONS` (`qi_custody.rs:12`) and `MAX_ACCOUNT_OPERATIONS`
(`account_custody.rs:17`) — enforced on `reserve`, on import/merge, in
`validate` and on decode. **There is no removal path at all**: no prune, no
forget, no compaction in either file. The constant's own comment states the
design, "Retained IDs, including released unsigned requests; no automatic
pruning," but no manual pruning exists either, so released and confirmed
operations consume slots permanently.

These books are the durable custody behind `crates/quai-sdk/src/browser_qi.rs`
and the portable backup envelope, so a **browser or WASM wallet stops being able
to transact after 256 lifetime operations per ledger per scope** — roughly eight
months at one send a day — and the count survives backup and restore. The only
reset is a different custody `identity`, which abandons claim history, and the
book's fields are private with `merge` only adding, so a caller cannot filter
and re-import.

The native SQLite store has the opposite shape: neither a cap nor pruning, so it
grows without bound. Same missing lifecycle, different failure mode.

The fix is additive — prune terminal operations, `Released` and `Confirmed` past
a caller-chosen depth — with no schema change, no new state and no migration.
Deliberately not attempted in this pass.

## API consistency, surfaced by the alpha.8 diff

Three breaking changes are filed under `Fixed:`/`Added:` with no `Breaking:`
section, though the changelog uses one at `CHANGELOG.md:230`:

- `qi_special_gas` gained a required third parameter (`qi_special_fee.rs:59-63`).
- `QiFeeQuote` gained pub field `floor_qits`, and the struct
  (`qi_special_fee.rs:18-19`) is **not** `#[non_exhaustive]` — this breaks every
  downstream struct literal.
- `QiReplacementIntent` gained required pub field `aggregate_destination`
  (`qi_replacement.rs:34`); struct at `:16-17` also not `#[non_exhaustive]`. The
  repository's own tests needed eight edits.

`QiError` and `QiPreflightError` *are* `#[non_exhaustive]`, so the inconsistency
is within this release's own types. Both new guards also hardcode
`QiFeeProfile::V056ShaAnchored` (`special.rs:280`, `qi_replacement.rs:260`) while
every other call threads the caller's profile, and `QiFeeProfile` is not
`#[non_exhaustive]` either.

## Build and test results

All observed, not inferred:

| Command | Result |
| --- | --- |
| `cargo build --workspace --locked` | exit 0, 0 warnings |
| `cargo clippy --workspace --all-targets --locked` | exit 0, **0 warnings** |
| `cargo test --workspace --locked` | exit 0, 311 passed / 0 failed / 3 ignored |
| `cargo test --workspace --all-features --locked` | exit 0, **738 passed / 0 failed / 5 ignored** |
| `cargo doc --workspace --no-deps` | exit 0, 0 warnings, no broken intra-doc links |
| `cargo check -p quai-sdk --no-default-features --features <f>` × 6 | all exit 0 |

One trap worth documenting rather than fixing: plain `cargo test --workspace`
exercises **none** of alpha.8. `crates/quai-sdk/tests/qi.rs:2` is
`#![cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]` and `sqlite` is
not in `default = ["http", "wallet"]`, so 18 binaries run 0 tests. CI is correct —
`.github/workflows/ci.yml:27` and `release.yml:53` both use `--all-features` —
so the release gate is sound; the trap is local-only.

Also verified clean: all twelve crates use `version.workspace = true` with no
literal drift, all three lockfiles regenerated, every `qi_special_gas` call site
threaded, and the arithmetic — `qi_special_gas(13,16,12,0) = 409,400` and
execution `257,400` match the changelog's mainnet numbers exactly.

## Suggested sequencing

1. **Before cutting alpha.9**: done. B1, B3, P2, C1, W1, E1, S1, S2, F1-F3, the
   `Breaking:` section, `#[non_exhaustive]` and the version bump are all in and
   verified. B2, W2 and P1's enforcement were withdrawn after checking their
   premises.
2. **Next**: the custody operation lifecycle recorded above — 256 lifetime
   operations with no pruning on the portable books, unbounded growth on the
   SQLite store. Additive, no schema change, and the only finding in this review
   with a concrete failure date attached to it for a shipping browser wallet.
3. **Then**: F4's one-line `bip39` feature gate (2.6x stripped-binary win for
   wasm), the offline inclusion floor from the B2 section, E2's broken migration
   snippet, and the remaining perf items.
4. **Upstream**: report the broken `quais@1.0.0-alpha.57` publish and P7 (the
   node not enforcing a single wrap destination). Get an answer on W8, and on
   whether Orchard shares the pinned fork constants.

## Method

Six parallel reviews: go-quai protocol alignment, quais.js parity, Pelagus
integration, security, performance, and release readiness. Reference trees used
were a fresh clone of go-quai `f3f345c8` (not the stale local checkouts),
`compatibility/node_modules/quais/src` at alpha.57, and Pelagus at `8c8a044`.
The repository was not modified during the review (`git status --porcelain`
empty throughout); this document is the only addition.

Every finding above was re-derived from source before inclusion. Where a
reviewing agent's framing was wider than the evidence — notably the post-submit
error taxonomy in W2 and the severity of E1 — the narrower, verified claim is the
one recorded here.
