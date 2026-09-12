//! Offline wrapper ABI compatibility and exact amount regressions.
#![cfg(feature = "abi")]
use quai_sdk::rpc::{Endpoint, RpcError, Transport};
use quai_sdk::wrappers::*;
use quai_sdk::{Provider, Routing, U256, Zone};
use serde_json::Value;
struct Offline;
impl Transport for Offline {
    async fn request(&self, _: &Endpoint, _: &str, _: Value) -> Result<Value, RpcError> {
        panic!("offline intent unexpectedly requested network access")
    }
}
#[test]
fn wrapped_intents_match_js_and_keep_native_and_token_amounts_distinct() {
    let provider = Provider::new(
        Offline,
        Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
        U256::from(15000),
    );
    let quai = WrappedQuai::new(WQUAI_ADDRESS.parse().unwrap(), &provider).unwrap();
    let qi = WrappedQi::new(WQI_ADDRESS.parse().unwrap(), &provider).unwrap();
    let amount = U256::from(1_000_000_000_000_000_000u64);
    let calls = [
        quai.deposit(amount).unwrap(),
        quai.withdraw(amount).unwrap(),
        qi.claim_deposit().unwrap(),
        qi.unwrap(
            "0x0080000000000000000000000000000000000001"
                .parse()
                .unwrap(),
            U256::from(1000),
            1_000_000,
        )
        .unwrap(),
    ];
    let fixture: Value = serde_json::from_str(include_str!("wrapper-calls.json")).unwrap();
    for (call, reference) in calls.iter().zip(fixture["calls"].as_array().unwrap()) {
        assert_eq!(call.data().to_hex(), reference["data"]);
    }
    assert_eq!(calls[0].value(), amount);
    assert!(calls[1..].iter().all(|call| call.value() == U256::ZERO));
    assert_eq!(wqi_atoms_to_qits(amount).unwrap(), U256::from(1000));
    assert!(wqi_atoms_to_qits(amount + U256::from(1)).is_err());
    assert!(qits_to_wqi_atoms(U256::MAX).is_err());
    assert!(quai.deposit(U256::ZERO).is_err());
    assert!(
        qi.unwrap(
            "0x1080000000000000000000000000000000000001"
                .parse()
                .unwrap(),
            U256::from(1),
            1
        )
        .is_err()
    );
}

#[test]
fn redemption_rejects_dust_loss_and_insufficient_destination_gas() {
    let provider = Provider::new(
        Offline,
        Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
        U256::from(15000),
    );
    let wrapper = WrappedQi::new(WQI_ADDRESS.parse().unwrap(), &provider).unwrap();
    let address = "0x0080000000000000000000000000000000000001"
        .parse()
        .unwrap();
    let plan = QiRedemptionPlan::go_quai_v056(U256::from(1001)).unwrap();
    assert_eq!(plan.discarded_qits, U256::from(1));
    assert_eq!(plan.minimum_etx_gas, 9000);
    assert!(wrapper.unwrap(address, U256::from(1001), 9000).is_err());
    assert!(wrapper.unwrap(address, U256::from(1000), 8999).is_err());
    assert!(wrapper.unwrap(address, U256::from(1000), 9000).is_ok());
}
