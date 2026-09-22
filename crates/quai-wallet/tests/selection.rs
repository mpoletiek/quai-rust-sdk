//! Fixed-fee reference parity, spendability boundaries and bounded fee planning.
use quai_consensus::{Denomination, OutPoint, U256};
use quai_primitives::Zone;
use quai_wallet::{
    CandidateCoin, SelectionError, SelectionRequest, preserves_denominations, select_fewest,
    select_fewest_converting, select_with_fee,
};
use serde_json::Value;
fn coin(index: u16, denomination: u8) -> CandidateCoin {
    CandidateCoin::new(
        OutPoint {
            transaction_hash: "0x0080008011111111111111111111111111111111111111111111111111111111"
                .parse()
                .unwrap(),
            index,
        },
        "0x0088223344556677889900112233445566778899"
            .parse()
            .unwrap(),
        Denomination::new(denomination).unwrap(),
    )
}
fn request(target: u64, fee: u64) -> SelectionRequest {
    SelectionRequest::new(
        Zone::Cyprus1,
        U256::from(100),
        U256::from(target),
        100,
        1000,
    )
    .with_fee(U256::from(fee), U256::from(10000))
}
#[test]
fn fee_reselection_preserves_target_and_never_reports_uncovered_or_reversed_adjustments() {
    let coins = [coin(0, 2), coin(1, 0)]; // Ten plus one Qits.
    let before = select_fewest(&coins, &request(5, 1)).unwrap();
    // Pinned JS increaseFee(6) returns a six-Qit fee despite requiring seven.
    assert_eq!(
        select_fewest(&coins, &request(5, 7)),
        Err(SelectionError::InsufficientFunds)
    );
    assert_eq!(before.fee, U256::from(1));
    let high = select_fewest(&coins, &request(5, 3)).unwrap();
    let low = select_fewest(&coins, &request(5, 2)).unwrap();
    assert_eq!(high.fee, U256::from(3));
    assert_eq!(low.fee, U256::from(2));
    for result in [before, high, low] {
        let spend: u64 = result.spend_outputs.iter().map(|d| d.value()).sum();
        let change: u64 = result.change_outputs.iter().map(|d| d.value()).sum();
        assert_eq!(spend, 5);
        assert_eq!(U256::from(spend + change) + result.fee, result.input_value);
    }
}
#[test]
fn every_fixed_fee_vector_matches_the_reference_or_documents_its_deviation() {
    let file: Value = serde_json::from_str(include_str!(
        "fixtures/shared/compatibility/fixtures/selection.json"
    ))
    .unwrap();
    let vectors = file["vectors"].as_array().unwrap();
    assert_eq!(vectors.len(), 75);
    let indexes = |denominations: &[Denomination]| {
        serde_json::json!(denominations.iter().map(|d| d.index()).collect::<Vec<_>>())
    };
    let mut deviations = 0;
    for vector in vectors {
        let coins: Vec<_> = vector["denominations"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(i, d)| coin(i as u16, d.as_u64().unwrap() as u8))
            .collect();
        let req = request(
            vector["target"].as_str().unwrap().parse().unwrap(),
            vector["fee"].as_str().unwrap().parse().unwrap(),
        );
        // `expected` is the reference's FewestCoinSelector, for ordinary
        // transfers; `expectedConversion` is its ConversionCoinSelector, for
        // Qi->Quai conversions and wraps.
        for (key, result) in [
            ("expected", select_fewest(&coins, &req)),
            ("expectedConversion", select_fewest_converting(&coins, &req)),
        ] {
            let expected = &vector[key];
            let id = format!("{} {key}", vector["id"]);
            if expected["error"] == true {
                assert!(result.is_err(), "{id}");
                continue;
            }
            let selected = result.unwrap_or_else(|e| panic!("{id}: {e:?}"));
            let inputs: Vec<_> = selected.inputs.iter().map(|c| c.denomination).collect();
            let outputs = selected
                .spend_outputs
                .iter()
                .chain(&selected.change_outputs)
                .fold(U256::ZERO, |sum, d| sum + U256::from(d.value()));
            assert_eq!(outputs + selected.fee, selected.input_value, "{id}");
            // A conversion's spend outputs are aggregated by the node, so only
            // its change is held to the denomination rule.
            let checked: Vec<_> = if key == "expected" {
                selected
                    .spend_outputs
                    .iter()
                    .chain(&selected.change_outputs)
                    .copied()
                    .collect()
            } else {
                selected.change_outputs.clone()
            };
            preserves_denominations(&inputs, &checked).unwrap_or_else(|e| panic!("{id}: {e:?}"));
            assert!(check_denominations(&inputs, &checked).is_ok(), "{id}");
            if expected["combinesDenominations"] == true {
                // The reference builds a shape go-quai rejects unless the
                // transaction is first in its block. Ours deviates on purpose.
                deviations += 1;
                assert_ne!(indexes(&selected.spend_outputs), expected["spend"], "{id}");
                continue;
            }
            assert_eq!(
                serde_json::json!(
                    selected
                        .inputs
                        .iter()
                        .map(|c| c.outpoint.index)
                        .collect::<Vec<_>>()
                ),
                expected["inputs"],
                "{id}"
            );
            assert_eq!(indexes(&selected.spend_outputs), expected["spend"], "{id}");
            assert_eq!(
                indexes(&selected.change_outputs),
                expected["change"],
                "{id}"
            );
        }
    }
    // selection-69 and selection-70: the reference's ordinary selector combines
    // smaller inputs into larger outputs, which the node refuses.
    assert_eq!(deviations, 2);
}

#[test]
fn lock_equality_expiry_and_reservations_are_enforced() {
    let mut c = coin(0, 4);
    c.unlock_height = U256::from(100);
    assert!(select_fewest(&[c.clone()], &request(10, 1)).is_ok());
    c.unlock_height = U256::from(101);
    assert_eq!(
        select_fewest(&[c.clone()], &request(10, 1)),
        Err(SelectionError::InsufficientFunds)
    );
    c.unlock_height = U256::ZERO;
    c.expires_at = Some(U256::from(100));
    assert!(select_fewest(&[c.clone()], &request(10, 1)).is_err());
    c.expires_at = Some(U256::from(101));
    assert!(select_fewest(&[c.clone()], &request(10, 1)).is_ok());
    c.reserved = true;
    assert!(select_fewest(&[c], &request(10, 1)).is_err());
}
#[test]
fn duplicate_snapshots_overflow_and_unbounded_shapes_fail() {
    let c = coin(0, 3);
    assert_eq!(
        select_fewest(&[c.clone(), c], &request(1, 0)),
        Err(SelectionError::InvalidCoin)
    );
    let mut r = request(1, 1);
    r.target = U256::MAX;
    assert_eq!(
        select_fewest(&[coin(0, 14)], &r),
        Err(SelectionError::Overflow)
    );
    let mut r = request(4, 0);
    r.max_outputs = 2;
    assert_eq!(
        select_fewest(&[coin(0, 1)], &r),
        Err(SelectionError::LimitExceeded)
    );
    let mut r = request(12, 0);
    r.max_inputs = 1;
    assert_eq!(
        select_fewest(&[coin(0, 1), coin(1, 1), coin(2, 1)], &r),
        Err(SelectionError::LimitExceeded)
    );
}
#[test]
fn fee_convergence_uses_final_shape_and_respects_authorized_budget() {
    let coins = [coin(0, 3), coin(1, 4)];
    let mut r = request(49, 0);
    r.max_fee = U256::from(10);
    let mut rounds = 0;
    let selected = select_with_fee(&coins, &r, 5, |selection| {
        rounds += 1;
        assert!(!selection.inputs.is_empty());
        Ok(U256::from(3))
    })
    .unwrap();
    assert_eq!(selected.fee, U256::from(3));
    assert_eq!(rounds, 2);
    assert_eq!(
        select_with_fee(&coins, &r, 5, |_| Ok(U256::from(11))),
        Err(SelectionError::FeeBudgetExceeded)
    );
    assert_eq!(
        select_with_fee(&coins, &r, 2, |s| Ok(s.fee + U256::from(1))),
        Err(SelectionError::FeeDidNotConverge)
    );
}

#[test]
fn output_capacity_cannot_combine_small_coins_in_ordinary_transfers() {
    let selected = select_fewest(&[coin(0, 2), coin(1, 1), coin(2, 1)], &request(20, 0)).unwrap();
    assert_eq!(
        selected
            .spend_outputs
            .iter()
            .map(|d| d.value())
            .collect::<Vec<_>>(),
        [10, 5, 5]
    );
    for target in 1..20 {
        let selected =
            select_fewest(&[coin(0, 2), coin(1, 1), coin(2, 1)], &request(target, 1)).unwrap();
        let input: u64 = selected
            .inputs
            .iter()
            .filter(|c| c.denomination.value() >= 10)
            .map(|c| c.denomination.value())
            .sum();
        let output: u64 = selected
            .spend_outputs
            .iter()
            .chain(&selected.change_outputs)
            .filter(|d| d.value() >= 10)
            .map(|d| d.value())
            .sum();
        assert!(output <= input);
        assert_eq!(
            selected
                .spend_outputs
                .iter()
                .map(|d| d.value())
                .sum::<u64>(),
            target
        );
    }
}
#[test]
fn sweep_limits_locks_fee_and_aggregation_are_explicit() {
    use quai_wallet::{SweepMode, select_sweep};
    let mut req = request(0, 0);
    let mut locked = coin(2, 6);
    locked.unlock_height = U256::from(101);
    let coins = [coin(0, 1), coin(1, 1), locked];
    let preserved = select_sweep(&coins, &req, SweepMode::PreserveDenominations).unwrap();
    assert_eq!(preserved.inputs.len(), 2);
    assert_eq!(preserved.spend_outputs.len(), 2);
    let aggregate = select_sweep(
        &coins,
        &req,
        SweepMode::Aggregate {
            maximum: Denomination::new(14).unwrap(),
        },
    )
    .unwrap();
    assert_eq!(aggregate.spend_outputs[0].value(), 10);
    req.max_inputs = 1;
    assert!(select_sweep(&coins, &req, SweepMode::PreserveDenominations).is_err());
    req.max_inputs = 100;
    req.fee = U256::from(10);
    assert!(select_sweep(&coins, &req, SweepMode::PreserveDenominations).is_err());
}

/// go-quai's `CheckDenominations` (core/state_processor.go at the pinned
/// f3f345c): outputs at each denomination must come from inputs of that
/// denomination or larger ones carried down, never from smaller ones combined.
/// Only the first Qi transaction in a block skips it, so a wallet cannot rely on
/// being first. Conversion and wrapping destination outputs are removed from the
/// tally before it runs, which is why they are exempt here too.
fn check_denominations(inputs: &[Denomination], outputs: &[Denomination]) -> Result<(), usize> {
    let mut counts = [0u64; 15];
    let mut out = [0u64; 15];
    for input in inputs {
        counts[input.index() as usize] += 1;
    }
    for output in outputs {
        out[output.index() as usize] += 1;
    }
    let mut carry = [0u64; 15];
    for i in (1..15).rev() {
        let total = counts[i] + carry[i];
        if out[i] > total {
            return Err(i);
        }
        let step = Denomination::new(i as u8).unwrap().value()
            / Denomination::new(i as u8 - 1).unwrap().value();
        carry[i - 1] += (total - out[i]) * step;
    }
    Ok(())
}

fn inventory_coins(counts: &[(u8, usize)]) -> Vec<CandidateCoin> {
    let mut out = Vec::new();
    for (denomination, n) in counts {
        for _ in 0..*n {
            out.push(coin(out.len() as u16, *denomination));
        }
    }
    out
}
fn values(denominations: &[Denomination]) -> Vec<u64> {
    denominations.iter().map(|d| d.value()).collect()
}

#[test]
fn conversion_spend_outputs_are_not_capped_by_the_input_denominations() {
    // The mainnet wrap of 15 Qi that exposed this: one 5000, nine 1000 and two
    // 500 Qit coins. Built as an ordinary transfer it needs twelve outputs to
    // the destination, and it sat unmined while a reference-built wrap
    // confirmed. Qi block inclusion is scarce, so the size matters.
    let inventory = [(7u8, 1), (6, 9), (5, 2)];
    let coins = inventory_coins(&inventory);
    let inputs: Vec<_> = coins.iter().map(|c| c.denomination).collect();
    let ordinary = select_fewest(&coins, &request(15000, 0)).unwrap();
    assert_eq!(
        values(&ordinary.spend_outputs),
        [
            5000, 1000, 1000, 1000, 1000, 1000, 1000, 1000, 1000, 1000, 500, 500
        ]
    );
    let converting = select_fewest_converting(&coins, &request(15000, 0)).unwrap();
    // The reference's ConversionCoinSelector: denominate(15000) = 10000 + 5000.
    assert_eq!(values(&converting.spend_outputs), [10000, 5000]);
    assert!(converting.change_outputs.is_empty());
    assert_eq!(converting.input_value, U256::from(15000));

    // The node's rule rejects those aggregated outputs for an ordinary transfer,
    // and exempts them for a conversion, which is the whole distinction.
    assert_eq!(
        check_denominations(&inputs, &converting.spend_outputs),
        Err(8)
    );
    assert!(check_denominations(&inputs, &ordinary.spend_outputs).is_ok());
    assert!(check_denominations(&inputs, &converting.change_outputs).is_ok());
}

#[test]
fn converting_change_still_preserves_the_input_inventory() {
    // Change stays in the Qi ledger, so the node still checks it against the
    // inputs: it may split a larger coin down, never combine smaller ones up.
    let coins = inventory_coins(&[(7u8, 1), (6, 9)]); // 5000 + 9000 = 14000 Qits.
    let selection = select_fewest_converting(&coins, &request(3000, 0)).unwrap();
    // Fewest inputs: the single 5000 covers it, and 2000 comes back as change.
    assert_eq!(values(&selection.spend_outputs), [1000, 1000, 1000]);
    assert_eq!(values(&selection.change_outputs), [1000, 1000]);
    let inputs: Vec<_> = selection.inputs.iter().map(|c| c.denomination).collect();
    assert_eq!(values(&inputs), [5000]);
    assert!(check_denominations(&inputs, &selection.change_outputs).is_ok());
    preserves_denominations(&inputs, &selection.change_outputs).unwrap();

    // Spending every coin leaves no change, and the destination takes the
    // largest-first shape whatever the inputs were.
    let all = select_fewest_converting(&coins, &request(14000, 0)).unwrap();
    assert_eq!(values(&all.spend_outputs), [10000, 1000, 1000, 1000, 1000]);
    assert!(all.change_outputs.is_empty());

    // Inputs of only small coins still aggregate upward on the spend side,
    // while their change stays within what those coins can split into.
    let small = inventory_coins(&[(6u8, 15)]);
    let selection = select_fewest_converting(&small, &request(9500, 0)).unwrap();
    assert_eq!(
        values(&selection.spend_outputs),
        [5000, 1000, 1000, 1000, 1000, 500]
    );
    assert_eq!(values(&selection.change_outputs), [500]);
    let inputs: Vec<_> = selection.inputs.iter().map(|c| c.denomination).collect();
    assert_eq!(inputs.len(), 10);
    assert!(check_denominations(&inputs, &selection.change_outputs).is_ok());
    // The same value as an ordinary transfer cannot reach a 5000 output.
    let ordinary = select_fewest(&small, &request(9500, 0)).unwrap();
    assert_eq!(values(&ordinary.spend_outputs).iter().max(), Some(&1000));
}

#[test]
fn converting_selection_respects_the_output_bound_and_the_fee() {
    let coins = inventory_coins(&[(6u8, 15)]); // Fifteen 1000-Qit coins.
    let whole = select_fewest_converting(&coins, &request(15000, 0)).unwrap();
    assert_eq!(values(&whole.spend_outputs), [10000, 5000]);
    assert!(whole.change_outputs.is_empty());
    let mut bounded = request(15000, 0);
    bounded.max_outputs = 1; // The two-output destination shape no longer fits.
    assert_eq!(
        select_fewest_converting(&coins, &bounded),
        Err(SelectionError::LimitExceeded)
    );
    let with_fee = select_fewest_converting(&coins, &request(14000, 1000)).unwrap();
    assert_eq!(
        values(&with_fee.spend_outputs),
        [10000, 1000, 1000, 1000, 1000]
    );
    assert!(with_fee.change_outputs.is_empty());
    assert_eq!(with_fee.fee, U256::from(1000));
    assert_eq!(with_fee.input_value, U256::from(15000));
}
