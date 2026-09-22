//! Threshold aggregation, exact fee conservation and wallet spendability policy.
#![cfg(feature = "wallet")]
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
use quai_sdk::consensus::{Denomination, OutPoint};
use quai_sdk::wallet::{
    AggregationPolicy, CandidateCoin, CoinSelection, SelectionError, SelectionRequest, SweepMode,
    select_aggregate, select_sweep,
};
use quai_sdk::{U256, Zone};
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
fn request(fee: u64) -> SelectionRequest {
    SelectionRequest::new(Zone::Cyprus1, U256::from(100), U256::ZERO, 100, 1000)
        .with_fee(U256::from(fee), U256::from(10000))
}
fn conservation(s: &CoinSelection) {
    let inputs: U256 = s
        .inputs
        .iter()
        .map(|c| U256::from(c.denomination.value()))
        .sum();
    let outputs: U256 = s
        .spend_outputs
        .iter()
        .chain(&s.change_outputs)
        .map(|d| U256::from(d.value()))
        .sum();
    assert_eq!(s.input_value, inputs);
    assert_eq!(inputs, outputs + s.fee);
    assert!(s.change_outputs.is_empty());
    let ids: std::collections::BTreeSet<_> = s.inputs.iter().map(|c| c.outpoint).collect();
    assert_eq!(ids.len(), s.inputs.len());
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn reference_order_and_exact_fee_corrections() {
    let fixture: Value = serde_json::from_str(include_str!(
        "fixtures/shared/compatibility/fixtures/aggregation.json"
    ))
    .unwrap();
    let mut matched = 0;
    let mut corrected = 0;
    for v in fixture["vectors"].as_array().unwrap() {
        let coins: Vec<_> = v["denominations"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(i, d)| coin(i as u16, d.as_u64().unwrap() as u8))
            .collect();
        let req = request(v["fee"].as_str().unwrap().parse().unwrap());
        let policy = AggregationPolicy::new(
            Denomination::new(v["maximumInput"].as_u64().unwrap() as u8).unwrap(),
            Denomination::new(v["maximumOutput"].as_u64().unwrap() as u8).unwrap(),
            false,
        );
        let result = select_aggregate(&coins, &req, policy);
        let e = &v["expected"];
        if e["error"] == true {
            assert!(result.is_err(), "source error case {}", v["id"]);
            continue;
        }
        let s = result.unwrap_or_else(|e| panic!("case {}: {e}", v["id"]));
        conservation(&s);
        assert_eq!(s.fee, req.fee);
        if e["actualFee"] != v["fee"] {
            corrected += 1;
            continue;
        }
        assert_eq!(
            serde_json::to_value(
                s.inputs
                    .iter()
                    .map(|c| c.outpoint.index)
                    .collect::<Vec<_>>()
            )
            .unwrap(),
            e["inputs"],
            "input order case {}",
            v["id"]
        );
        assert_eq!(
            serde_json::to_value(
                s.spend_outputs
                    .iter()
                    .map(|d| d.index())
                    .collect::<Vec<_>>()
            )
            .unwrap(),
            e["spend"],
            "output order case {}",
            v["id"]
        );
        matched += 1;
    }
    assert!(matched > 50);
    assert_eq!(corrected, 1);
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn fee_shortfall_is_funded_and_refund_stays_separate() {
    let coins = [coin(0, 1), coin(1, 1), coin(2, 1)];
    let policy = {
        let mut updated = AggregationPolicy::default();
        updated.require_reduction = false;
        updated
    };
    let s = select_aggregate(&coins, &request(6), policy).unwrap();
    // Reference pays only 5 of the requested 6 Qits on this exact snapshot.
    assert_eq!(s.fee, U256::from(6));
    assert_eq!(s.input_value, U256::from(15));
    assert_eq!(
        s.inputs
            .iter()
            .map(|c| c.outpoint.index)
            .collect::<Vec<_>>(),
        [0, 2, 1]
    );
    assert_eq!(
        s.spend_outputs
            .iter()
            .map(|d| d.index())
            .collect::<Vec<_>>(),
        [1, 0, 0, 0, 0]
    );
    conservation(&s);
    assert_eq!(
        select_aggregate(&coins, &request(6), AggregationPolicy::default()),
        Err(SelectionError::InvalidRequest)
    );
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn threshold_preserves_large_coins_and_dispatches_through_sweep() {
    let coins = [coin(0, 1), coin(1, 1), coin(2, 1), coin(3, 7), coin(4, 8)];
    let mut p = {
        let mut updated = AggregationPolicy::default();
        updated.maximum_input = Denomination::new(1).unwrap();
        updated
    };
    let s = select_aggregate(&coins, &request(0), p).unwrap();
    assert_eq!(s.inputs.len(), 3);
    assert_eq!(s.spend_outputs.len(), 2);
    conservation(&s);
    assert_eq!(
        select_sweep(&coins, &request(0), SweepMode::AggregateThreshold(p)).unwrap(),
        s
    );
    p.require_reduction = false;
    let paid = select_aggregate(&coins, &request(1), p).unwrap();
    assert_eq!(paid.inputs[0].outpoint.index, 3);
    assert!(!paid.inputs.iter().any(|c| c.outpoint.index == 4));
    conservation(&paid);
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn spendability_and_snapshot_integrity_precede_selection() {
    let mut coins = vec![
        coin(0, 1),
        coin(1, 1),
        coin(2, 1),
        coin(3, 1),
        coin(4, 1),
        coin(5, 1),
    ];
    coins[3].reserved = true;
    coins[4].unlock_height = U256::from(101);
    coins[5].expires_at = Some(U256::from(100));
    let p = AggregationPolicy::default();
    let s = select_aggregate(&coins, &request(0), p).unwrap();
    assert_eq!(s.inputs.len(), 3);
    let original = coins.clone();
    coins.push(coins[3].clone());
    assert_eq!(
        select_aggregate(&coins, &request(0), p),
        Err(SelectionError::InvalidCoin)
    );
    let mut invalid = original;
    invalid[3].outpoint.transaction_hash = quai_sdk::primitives::Hash32::from_bytes([0; 32]);
    assert_eq!(
        select_aggregate(&invalid, &request(0), p),
        Err(SelectionError::InvalidCoin)
    );
    let mut foreign = coin(20, 1);
    foreign.address = "0x0188223344556677889900112233445566778899"
        .parse()
        .unwrap();
    foreign.outpoint.transaction_hash =
        "0x0080018011111111111111111111111111111111111111111111111111111111"
            .parse()
            .unwrap();
    assert_eq!(
        select_aggregate(&[foreign], &request(0), p),
        Err(SelectionError::InsufficientFunds)
    );
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn resource_and_fee_limits_fail_before_returning_a_plan() {
    let coins = [coin(0, 1), coin(1, 1), coin(2, 1)];
    let p = AggregationPolicy::default();
    let mut r = request(0);
    r.max_inputs = 2;
    assert_eq!(
        select_aggregate(&coins, &r, p),
        Err(SelectionError::LimitExceeded)
    );
    r = request(0);
    r.max_outputs = 1;
    assert_eq!(
        select_aggregate(&coins, &r, p),
        Err(SelectionError::LimitExceeded)
    );
    r = request(1);
    r.max_fee = U256::ZERO;
    assert_eq!(
        select_aggregate(&coins, &r, p),
        Err(SelectionError::InvalidRequest)
    );
    r = request(0);
    r.target = U256::from(1);
    assert_eq!(
        select_aggregate(&coins, &r, p),
        Err(SelectionError::InvalidRequest)
    );
    assert_eq!(
        select_aggregate(&coins, &request(15), p),
        Err(SelectionError::InsufficientFunds)
    );
    let too_many = vec![coin(0, 0); 100001];
    assert_eq!(
        select_aggregate(&too_many, &request(0), p),
        Err(SelectionError::LimitExceeded)
    );
}
