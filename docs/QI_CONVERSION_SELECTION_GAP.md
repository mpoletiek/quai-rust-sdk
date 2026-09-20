# Conversion and wrapping spend outputs are capped by input denominations

**Status:** fixed on 2026-09-20, after independent verification of every claim
below. See [what verification added](#what-verification-added) for the two
things it changed. Originally reported as an open bug, reproduced on mainnet
2026-09-19.
**Affects:** `quai-wallet` selection, reached through `quai-sdk`'s `prepare_special_*`.
**Impact:** Qi wrapping and Qi→QUAI conversion transactions are built several times larger
than the reference implementation builds them, and lose the scarce Qi block slot as a result.

## Summary

`select_fewest` decomposes **both** the spend outputs and the change outputs with
`denominate_available`, which can only emit denominations the selected input coins already
cover. That rule is correct for ordinary Qi→Qi transfers and is stated correctly in
[QI_SELECTION_PARITY_REVIEW.md](QI_SELECTION_PARITY_REVIEW.md) — *"**ordinary transfers**
cannot merge small inputs into larger outputs"*.

The reference does not apply it to conversions or wraps. `quais` has a **second coin
selector** for that path, and the parity review only ever modelled the first one.

Since `prepare_special_once` routes `QiSpecialIntent::Conversion` and
`QiSpecialIntent::Wrapping` through `select_fewest`, both inherit the ordinary-transfer cap.

## The reference

`quais` (`dominant-strategies/quais-6.js`, `main`):

- `src/transaction/coinselector-fewest.ts:135` — ordinary sends:
  ```ts
  const spendDenominations = denominate(amount, maxInputDenomination);
  ```
- `src/transaction/coinselector-conversion.ts:21-23` — conversions and wraps:
  ```ts
  protected override createSpendOutputs(amount: bigint): UTXO[] {
      // Spend outpoints are not limited to max input denomination
      const spendDenominations = denominate(amount);
  ```
  `ConversionCoinSelector` extends `FewestCoinSelector` and overrides **only**
  `createSpendOutputs`. Change keeps the parent's capped decomposition
  (`coinselector-fewest.ts:159`), which is correct — change is Qi the sender keeps.
- `src/wallet/qi-hdwallet.ts:590` — `convertToQuai` selects `new ConversionCoinSelector(utxos)`;
  `:673` — ordinary `sendTransaction` selects `new FewestCoinSelector(utxos)`.

Wrapping reaches the conversion selector in the reference wallet: Pelagus's `wrapQi`
(`PelagusWallet/pelagus-extension`, `background/services/transactions/index.ts:669-712`) calls

```ts
qiWallet.convertToQuai(to, amount, { data: WRAPPED_QI_CONTRACT_ADDRESS_BYTES })
```

The `data` field carrying the owner contract is what makes the node treat it as a wrap rather
than a protocol conversion — the same distinction this repo encodes in
`QiWrappingTransaction` vs `QiConversionTransaction`. The selection path is shared.

## Why the cap does not belong there

The spend outputs of a conversion or a wrap are addressed to a **Quai-ledger destination**,
and the node aggregates them into one credit. `quai-consensus` says so itself:

- `crates/quai-consensus/src/wrapping.rs:12` — *"Single same-zone account destination; repeated
  denominations aggregate here."*
- `crates/quai-consensus/src/qi.rs` `validate_for` — the Quai-ledger output is exempted from the
  reused-address rule precisely because *"Go block processing aggregates equal conversion
  addresses"*.

There is no Qi denomination inventory on the far side to preserve: the value becomes an
18-decimal ERC-20 balance (WQI) or a QUAI account credit. Nothing in `validate_for` constrains
output denominations against input denominations either — that constraint exists only in
`denominate_available`.

## Observed on mainnet

Wrap of 15 Qi, Cyprus-1, tx `0x00a100d1754d77c14f013d24a0dd9fe8e75cb27dd0f422a5b2e555f62110fea5`,
built by `prepare_special_estimated(..., QiFeeProfile::V056ShaAnchored, ...)`:

| | outputs to the WQI destination |
| --- | --- |
| built by this SDK | `5000 + 9×1000 + 2×500` = **12 outputs** |
| `denominate(15000)` (reference) | `10000 + 5000` = **2 outputs** |

13 inputs, 16 outputs total. The sending wallet's largest coin was 5000 Qits, so the capped
decomposition could not do better; the uncapped one does not care.

This matters because Qi block inclusion is scarce. Sampling 150 consecutive Cyprus-1 blocks the
same day: 2104 transactions mined, of which **1** was a Qi transaction. A transaction six times
larger than it needs to be loses that slot repeatedly. The transaction above sat unmined for
over 20 minutes while a Pelagus-built wrap from another wallet confirmed normally.

## Suggested fix

In `crates/quai-wallet/src/selection.rs`:

1. Add an uncapped largest-first decomposition (the analogue of `denominate(value)` with no
   `maxDenomination`), used for spend outputs only.
2. Add a conversion/wrapping entry point — e.g. `select_fewest_converting` — identical to
   `select_fewest` (`:199`) except that line `:271` uses the uncapped decomposition for
   `request.target`. Line `:272`, change, keeps `denominate_available` and the capacity
   inventory it has consumed.
3. In `crates/quai-sdk/src/qi/special.rs:210`, use that entry point, leaving ordinary sends
   (`qi.rs:528`, `qi_preflight.rs:362`) on `select_fewest`.

The `max_outputs` bound and the `MAX_TRANSACTION_MESSAGES` check at `:273` still apply, and
both get easier to satisfy, not harder.

### Worth deciding explicitly

- **Fee estimation.** `prepare_special_estimated` converges a fee against the selected shape.
  A smaller shape should converge to a smaller fee; confirm the profile is driven by the final
  output count rather than a pre-decomposition estimate.
- **Aggregation is a different thing.** `SweepMode::Aggregate` documents needing *"the node's
  first-Qi-transaction block exception"*. That is about combining Qi **change** upward. The
  change side of this fix stays capped, so nothing here should need that exception — but it is
  the assumption most worth checking against a node before release.

### Test to pin it

A conversion/wrap selection whose inputs are all small coins must still produce the minimal
largest-first spend decomposition. The 15 Qi case above is a ready fixture: inputs of
`5000 + 1000×9 + 500×2 + …`, target 15000, expected spend outputs `[10000, 5000]`, expected
change unchanged in behaviour. The existing
[published-reference cases](../crates/quai-wallet/tests/selection.rs) cover `FewestCoinSelector`
only; `ConversionCoinSelector` has no counterpart there yet, which is how this was missed.

---

Reported from `quai-rust-cli-wallet`, which consumes `quai-sdk =0.1.0-alpha.6`. That wallet
cannot work around it locally: `prepare_special_*` takes a `QiPolicy` but no selector hook, so
the decomposition is not reachable from the caller.

## What verification added

Every claim above was checked independently: the pinned `quais@1.0.0-alpha.57`
package (not only upstream `main`), both selectors run side by side on the
reported coin set, the on-chain transaction, and the pinned go-quai source. Two
things came out of it.

**The open question is answered, and the fix needs no block-position
exception.** In `ProcessQiTx` (pinned `f3f345c`, `core/state_processor.go`) each
conversion or wrapping destination output is removed from the denomination tally
before the check — `outputs[uint(txOut.Denomination)] -= 1 // This output no
longer exists because it has been aggregated` — and `CheckDenominations` runs
only `if !isFirstQiTx`. `core/worker.go:3037` repeats both at mining time, and
the transaction pool has no such check. So aggregated spend outputs are valid in
any Qi transaction, while change must stay capped. That is what shipped.

**The cap is right for ordinary transfers, and the reference's is not.**
`CheckDenominations` rejects combining smaller inputs into larger outputs, which
is exactly what `denominate_available` models, so this repo's ordinary path and
the wording in [QI_SELECTION_PARITY_REVIEW.md](QI_SELECTION_PARITY_REVIEW.md)
are node-correct. The reference's `FewestCoinSelector` caps at the *largest*
input denomination without counting how many it holds: from one 5000 and five
1000 coins, a target of 10000 returns `[5000, 5000]`, which the node refuses
unless the transaction happens to be first in its block. The generated fixtures
now carry the node's rule, and `selection-69` and `selection-70` are such cases.
That is a separate defect in the reference, worth reporting upstream.

Two smaller observations: `ConversionCoinSelector` is not re-exported from any
`quais` entry point, so only its own HD wallet can reach it, and a fee
replacement of a conversion or a wrap had to stop checking the aggregated
destination outputs, or aggregating them would have made replacements
unbuildable.

## The fix as shipped

- `quai_wallet::select_fewest_converting`, with `denominate_largest` for the
  spend side. Change keeps `denominate_available` and its inventory.
- Used by `QiSession::prepare_special*` (`qi/special.rs`) and by the portable
  and browser preflight (`qi_preflight.rs`), which had the same cap.
- Replacement validation (`qi/replacements.rs`, `qi_replacement.rs`) checks only
  Qi-ledger outputs against the input denominations.
- Tests: the 15 Qi case as a unit fixture, converting change against a port of
  the node's `CheckDenominations`, output-bound and fee cases, the 75 generated
  reference vectors across both selectors, and an SDK wrap that asserts one
  aggregated output and a working fee replacement. Each fails without the fix.

## Follow-up: why the stuck wrap never mined

Fixing the shape was not the whole story. The wallet's 15 Qi wrap stayed pooled
because of a second defect, in this SDK's fee model.

The node prices a conversion or wrap twice, and a fee must clear both:

- **Execution** (`ProcessQiTx`) charges the intrinsic gas plus
  `QiToQuaiConversionGas`, 100,000, once.
- **Inclusion** (`CalculateBlockQiTxGas`, used by the miner's filter at
  `worker.go:3037` and by block validation) charges the intrinsic gas plus
  `ETXGas`, 21,000, for *each* output that creates an ETX. Every Quai-ledger
  destination output is one.

`qi_special_gas` modelled only the execution charge, as a flat 100,000. At twelve
destination outputs the node wanted 409,400 gas and the SDK priced 257,400, so
the quoted 78 Qits came to 2.43e13 wei per gas against a 3.04e13 base fee. The
miner's filter classes that as "not an invalid tx": it skips the transaction and
leaves it in the pool, reporting nothing. The Qi pool is an LRU keyed by hash
with no time expiry, so it sits there. Our own 10 Qi wrap, with one destination
output, came to 6.93e13 per gas and mined in under a minute.

Both are fixed: the gas bound now takes the larger of the two floors,
`QiFeeQuote::floor_qits` reports the un-margined threshold, and
`prepare_special` refuses an explicit fee below it.

For a parent that is already stuck,
`QiReplacementIntent::aggregate_destination` re-decomposes the destination
outputs largest-first while keeping the same inputs, address and total. The
storage layer already allowed it: a Qi family requires the same inputs, the same
data and a strictly lower output total, not identical outputs. Consensus allows
it because the node aggregates that destination into one credit.
