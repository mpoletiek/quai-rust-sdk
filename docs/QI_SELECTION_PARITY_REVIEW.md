# Qi fixed-denomination selection review

The reference is the published `quais@1.0.0-alpha.57` artifact. Rust's
`Denomination::VALUES` contains the same fifteen Qit values; `Denomination::new`
validates the consensus index. Amounts are not arbitrary-valued UTXOs.

| Published symbol/behavior | Rust mapping and difference |
| --- | --- |
| `denominations` | Exact fixed table `Denomination::VALUES`; values are u64 Qits and sums use checked U256 arithmetic |
| `UTXO` constructor, `from`, address/denomination/hash/index/lock fields | `CandidateCoin` with typed `QiAddress`, `Denomination`, `OutPoint` and `unlock_height`, plus explicit reservation and optional expiry information. Provider `AddressOutpoint` parses bounded current index responses. There is no nullable partially initialized object or unvalidated hash/index setter |
| `UTXO.toJSON` | Public typed fields are explicit; there is no legacy JS DTO serializer. Node lock metadata is retained in observations and selection inputs. The reference's from/toJSON drop lock information; Rust does not use that lossy round-trip for recovery. Wallet backups retain authorization history and invalidate current UTXO observations |
| `FewestCoinSelector` constructor / `availableUTXOs` / `target` | Pure `select_fewest(&coins, &SelectionRequest)` rather than a mutable class. Request includes zone, candidate height, target, fee and explicit resource/fee caps |
| `performSelection` | Same smallest-covering-single-input choice, or descending greedy fallback with reference tie ordering. Spend/change outputs preserve the complete input denomination inventory; ordinary transfers cannot merge small inputs into larger outputs |
| `selectedUTXOs`, `spendOutputs`, `changeOutputs`, `totalInputValue` | Ordered `CoinSelection.inputs`, `spend_outputs`, `change_outputs`, `input_value`, plus an explicit exact `fee` |
| `increaseFee` / `decreaseFee` | Re-run pure selection with the final requested fee and same target. Changes produce a new reviewable result, not mutation of an authorized transaction. `select_with_fee` and SDK preflight implement bounded fee convergence; increases never report uncovered fees and decreases preserve the target and full remaining change |

[Sixty-nine published-reference cases](../crates/quai-wallet/tests/selection.rs)
compare input identities/order and spend/change denominations. Other tests cover
lock equality, expiry, reservations, duplicates, value conservation, fee limits,
convergence, sweep and ordinary denomination capacity. Current indexed outputs
are still node claims; selection does not prove ownership or spendability.

The [executable JS regressions](../compatibility/scripts/selection-fee-regressions.test.mjs)
show why fee adjustment is an explicit deviation. With 11 available Qits, a target
of 5 and an existing fee of 1, the reference `increaseFee(6)` returns a selection
paying only 6 instead of the requested 7. Rust rejects insufficient funds. With
10 Qits, target 5 and fee 3, the reference `decreaseFee(1)` replaces existing change
and raises the paid fee to 4. Rust re-selection pays exactly 2 and retains 3 as
change. Failed planning cannot mutate a previously returned result.

Selecting again may change input or output denominations. Assign new or durably
retained output addresses through the wallet workflow and review the resulting
transaction before signing. These helpers do not replace the stricter
[same-input signed replacement workflow](BROWSER_REPLACEMENTS.md). They also do
not qualify aggregation block placement or funded node acceptance.
