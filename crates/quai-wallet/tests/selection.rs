//! Fixed-fee reference parity, spendability boundaries and bounded fee planning.
use quai_consensus::{Denomination, OutPoint, U256};
use quai_primitives::Zone;
use quai_wallet::{
    CandidateCoin, SelectionError, SelectionRequest, select_fewest, select_with_fee,
};
use serde_json::Value;
fn coin(index: u16, denomination: u8) -> CandidateCoin {
    CandidateCoin {
        outpoint: OutPoint {
            transaction_hash: "0x0080008011111111111111111111111111111111111111111111111111111111"
                .parse()
                .unwrap(),
            index,
        },
        address: "0x0088223344556677889900112233445566778899"
            .parse()
            .unwrap(),
        denomination: Denomination::new(denomination).unwrap(),
        unlock_height: U256::ZERO,
        expires_at: None,
        reserved: false,
    }
}
fn request(target: u64, fee: u64) -> SelectionRequest {
    SelectionRequest {
        zone: Zone::Cyprus1,
        candidate_height: U256::from(100),
        target: U256::from(target),
        fee: U256::from(fee),
        max_fee: U256::from(10000),
        max_inputs: 100,
        max_outputs: 1000,
    }
}
#[test]
fn all_69_fixed_fee_vectors_match_reference_input_and_output_order() {
    let file: Value = serde_json::from_str(include_str!(
        "../../../compatibility/fixtures/selection.json"
    ))
    .unwrap();
    assert_eq!(file["vectors"].as_array().unwrap().len(), 69);
    for vector in file["vectors"].as_array().unwrap() {
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
        let result = select_fewest(&coins, &req);
        if vector["expected"]["error"] == true {
            assert!(result.is_err(), "{}", vector["id"]);
            continue;
        }
        let selected = result.unwrap();
        assert_eq!(
            serde_json::json!(
                selected
                    .inputs
                    .iter()
                    .map(|c| c.outpoint.index)
                    .collect::<Vec<_>>()
            ),
            vector["expected"]["inputs"],
            "{}",
            vector["id"]
        );
        assert_eq!(
            serde_json::json!(
                selected
                    .spend_outputs
                    .iter()
                    .map(|d| d.index())
                    .collect::<Vec<_>>()
            ),
            vector["expected"]["spend"]
        );
        assert_eq!(
            serde_json::json!(
                selected
                    .change_outputs
                    .iter()
                    .map(|d| d.index())
                    .collect::<Vec<_>>()
            ),
            vector["expected"]["change"]
        );
        let outputs = selected
            .spend_outputs
            .iter()
            .chain(&selected.change_outputs)
            .fold(U256::ZERO, |sum, d| sum + U256::from(d.value()));
        assert_eq!(outputs + selected.fee, selected.input_value);
    }
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
