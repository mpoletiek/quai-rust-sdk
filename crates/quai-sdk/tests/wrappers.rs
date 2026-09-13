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

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn unclaimed_backing_distinguishes_absence_zero_and_unrelated_errors() {
    use quai_sdk::rpc::RemoteError;
    struct Reply {
        code: Option<i64>,
        message: &'static str,
        data: Option<Value>,
        result: Value,
    }
    impl Transport for Reply {
        async fn request(&self, _: &Endpoint, method: &str, _: Value) -> Result<Value, RpcError> {
            match method {
                "quai_chainId" => Ok(serde_json::json!("0x3a98")),
                "quai_getWrappedQiDeposit" => match self.code {
                    Some(code) => Err(RpcError::Remote(RemoteError {
                        code,
                        message: self.message.into(),
                        data: self.data.clone(),
                    })),
                    None => Ok(self.result.clone()),
                },
                _ => panic!("unexpected method"),
            }
        }
    }
    let make = |reply| {
        Provider::new(
            reply,
            Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
            U256::from(15000),
        )
    };
    let owner = WQI_ADDRESS.parse().unwrap();
    let beneficiary = "0x0011223344556677889900112233445566778899"
        .parse()
        .unwrap();
    let tag = quai_sdk::BlockTag::Number(U256::from(13));
    let absent = make(Reply {
        code: Some(-32000),
        message: "no wrapped Qi balance",
        data: None,
        result: Value::Null,
    });
    assert!(
        absent
            .wrapped_qi_deposit(owner, beneficiary, tag)
            .await
            .is_err()
    );
    assert_eq!(
        absent
            .wrapped_qi_deposit_optional(owner, beneficiary, tag)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        WrappedQi::new(owner, &absent)
            .unwrap()
            .unclaimed(beneficiary, tag)
            .await
            .unwrap(),
        U256::ZERO
    );
    for (raw, expected) in [("0x0", 0), ("0x3e8", 1000)] {
        let provider = make(Reply {
            code: None,
            message: "",
            data: None,
            result: serde_json::json!(raw),
        });
        assert_eq!(
            provider
                .wrapped_qi_deposit_optional(owner, beneficiary, tag)
                .await
                .unwrap(),
            Some(U256::from(expected))
        );
        assert_eq!(
            WrappedQi::new(owner, &provider)
                .unwrap()
                .unclaimed(beneficiary, tag)
                .await
                .unwrap(),
            U256::from(expected)
        );
    }
    for (code, message, data) in [
        (-32601, "no wrapped Qi balance", None),
        (-32000, "state unavailable", None),
        (-32000, "no wrapped Qi balance (unavailable)", None),
        (
            -32000,
            "no wrapped Qi balance",
            Some(serde_json::json!({"reason":"unknown"})),
        ),
    ] {
        let provider = make(Reply {
            code: Some(code),
            message,
            data,
            result: Value::Null,
        });
        assert!(
            provider
                .wrapped_qi_deposit_optional(owner, beneficiary, tag)
                .await
                .is_err()
        );
        assert!(
            WrappedQi::new(owner, &provider)
                .unwrap()
                .unclaimed(beneficiary, tag)
                .await
                .is_err()
        );
    }
    let malformed = make(Reply {
        code: None,
        message: "",
        data: None,
        result: Value::Null,
    });
    assert!(
        malformed
            .wrapped_qi_deposit_optional(owner, beneficiary, tag)
            .await
            .is_err()
    );
}

#[test]
fn wqi_protocol_access_is_preserved_in_each_zone_and_account_intent() {
    for zone in [
        Zone::Cyprus1,
        Zone::Cyprus2,
        Zone::Cyprus3,
        Zone::Paxos1,
        Zone::Paxos2,
        Zone::Paxos3,
        Zone::Hydra1,
        Zone::Hydra2,
        Zone::Hydra3,
    ] {
        let mut contract_bytes = [0; 20];
        contract_bytes[0] = zone.byte();
        contract_bytes[19] = 0x42;
        let contract = quai_sdk::primitives::Address::from_bytes(contract_bytes)
            .try_into()
            .unwrap();
        let mut qi_bytes = contract_bytes;
        qi_bytes[1] = 0x80;
        let beneficiary = quai_sdk::primitives::Address::from_bytes(qi_bytes)
            .try_into()
            .unwrap();
        let provider = Provider::new(
            Offline,
            Routing::direct("http://127.0.0.1:9200", zone.into()).unwrap(),
            U256::from(15000),
        );
        let wrapper = WrappedQi::new(contract, &provider).unwrap();
        for call in [
            wrapper.claim_deposit().unwrap(),
            wrapper.unwrap(beneficiary, U256::from(1000), 9000).unwrap(),
        ] {
            assert_eq!(call.access_list().len(), 1);
            let entry = &call.access_list()[0];
            let mut expected = [0; 20];
            expected[0] = zone.byte();
            expected[19] = 0x0a;
            assert_eq!(entry.address.bytes(), &expected);
            assert!(entry.storage_keys.is_empty());
            #[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
            {
                let list = call.access_list().to_vec();
                let bytes = call.data().clone();
                let intent = call.into_account_intent();
                assert_eq!(intent.access_list, list);
                assert_eq!(intent.data, bytes);
            }
        }
    }
}
