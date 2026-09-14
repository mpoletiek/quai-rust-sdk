//! Raw contract entrypoints, binding and lossless bounded event queries.
#![cfg(feature = "abi")]
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
use quai_sdk::abi::{AbiError, AbiInterface};
use quai_sdk::contracts::{Contract, ContractError, ContractLog};
use quai_sdk::primitives::{Hash32, hexlify};
use quai_sdk::provider::{BlockTag, LogRange, RpcData, TopicMatch};
use quai_sdk::rpc::{Endpoint, RpcError, Transport};
use quai_sdk::{Provider, Routing, U256, Zone};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
const ADDRESS: &str = "0x0011223344556677889900112233445566778899";
const OWNER: &str = "0x0000000000000000000000000000000000000001";
#[derive(Default)]
struct State {
    calls: Vec<(String, Value)>,
    logs: Vec<Value>,
}
#[derive(Clone, Default)]
struct Mock(Arc<Mutex<State>>);
impl Transport for Mock {
    async fn request(&self, _: &Endpoint, method: &str, args: Value) -> Result<Value, RpcError> {
        let mut state = self.0.lock().unwrap();
        state.calls.push((method.into(), args));
        Ok(match method {
            "quai_chainId" => json!("0x9"),
            "quai_call" => json!("0xbeef"),
            "quai_estimateGas" => json!("0x5208"),
            "quai_getLogs" => json!(state.logs),
            _ => panic!("unexpected RPC"),
        })
    }
}
fn provider(mock: Mock) -> Provider<Mock> {
    Provider::new(
        mock,
        Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
        U256::from(9),
    )
}
fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "fixtures/shared/compatibility/fixtures/contract-io.json"
    ))
    .unwrap()
}
fn iface(value: &Value) -> AbiInterface {
    AbiInterface::from_json(value.to_string().as_bytes()).unwrap()
}
fn data(text: &str) -> RpcData {
    RpcData::new(quai_sdk::primitives::get_bytes(text).unwrap()).unwrap()
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn fallback_receive_matrix_matches_published_population_without_io() {
    let mock = Mock::default();
    let provider = provider(mock.clone());
    let fixture = fixture();
    assert_eq!(fixture["vectors"].as_array().unwrap().len(), 24);
    for row in fixture["vectors"].as_array().unwrap() {
        let contract = Contract::new(ADDRESS.parse().unwrap(), iface(&row["abi"]), &provider);
        let prepared = contract.prepare_fallback(
            data(row["data"].as_str().unwrap()),
            U256::from_str_radix(row["value"].as_str().unwrap(), 10).unwrap(),
        );
        let mut reversed = row["abi"].as_array().unwrap().clone();
        reversed.reverse();
        let reverse_contract =
            Contract::new(ADDRESS.parse().unwrap(), iface(&json!(reversed)), &provider);
        assert_eq!(
            reverse_contract
                .prepare_fallback(
                    data(row["data"].as_str().unwrap()),
                    U256::from_str_radix(row["value"].as_str().unwrap(), 10).unwrap()
                )
                .is_ok(),
            prepared.is_ok()
        );
        if row["rustExpected"].is_null() {
            assert!(prepared.is_err(), "case {row}");
            continue;
        }
        let call = prepared.unwrap();
        assert_eq!(call.destination().to_string(), row["rustExpected"]["to"]);
        assert_eq!(call.value().to_string(), row["rustExpected"]["value"]);
        assert_eq!(call.data().to_hex(), row["rustExpected"]["data"]);
        assert!(call.access_list().is_empty());
        #[cfg(feature = "wallet")]
        {
            let intent = call.clone().into_account_intent();
            assert_eq!(intent.to, call.destination());
            assert_eq!(intent.value, call.value());
            assert_eq!(intent.data, call.data().clone());
            assert!(intent.access_list.is_empty());
        }
    }
    assert!(mock.0.lock().unwrap().calls.is_empty());
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn exact_fallback_simulation_estimation_access_and_binding_checks() {
    let mock = Mock::default();
    let p = provider(mock.clone());
    let contract = Contract::new(
        ADDRESS.parse().unwrap(),
        iface(&json!([{"type":"fallback","stateMutability":"payable"}])),
        &p,
    );
    let call = contract
        .prepare_fallback(data("0x1234"), U256::from(1))
        .unwrap()
        .with_access_list(vec![quai_sdk::consensus::AccessTuple {
            address: ADDRESS.parse().unwrap(),
            storage_keys: vec![Hash32::from_bytes([8; 32])],
        }])
        .unwrap();
    assert_eq!(
        contract
            .simulate_fallback(OWNER.parse().unwrap(), &call, BlockTag::Latest, Some(30000))
            .await
            .unwrap()
            .to_hex(),
        "0xbeef"
    );
    assert_eq!(
        contract
            .estimate_fallback(OWNER.parse().unwrap(), &call, BlockTag::Latest)
            .await
            .unwrap(),
        21000
    );
    {
        let state = mock.0.lock().unwrap();
        assert_eq!(state.calls.len(), 4);
        assert_eq!(state.calls[1].1[0]["input"], "0x1234");
        assert_eq!(state.calls[1].1[0]["value"], "0x1");
        assert_eq!(state.calls[1].1[0]["gas"], "0x7530");
        assert_eq!(
            state.calls[1].1[0]["accessList"][0]["storageKeys"],
            json!([Hash32::from_bytes([8; 32]).to_string()])
        );
    }
    let attached = contract.attach(OWNER.parse().unwrap());
    assert_eq!(attached.address().to_string(), OWNER);
    assert!(matches!(
        attached
            .simulate_fallback(OWNER.parse().unwrap(), &call, BlockTag::Latest, None)
            .await,
        Err(ContractError::CallMismatch)
    ));
    assert!(matches!(
        contract
            .simulate_fallback(
                "0x0100000000000000000000000000000000000001"
                    .parse()
                    .unwrap(),
                &call,
                BlockTag::Latest,
                None
            )
            .await,
        Err(ContractError::ZoneMismatch)
    ));
    let nonpayable = Contract::new(
        ADDRESS.parse().unwrap(),
        iface(&json!([{"type":"fallback","stateMutability":"nonpayable"}])),
        &p,
    );
    assert!(matches!(
        nonpayable
            .simulate_fallback(OWNER.parse().unwrap(), &call, BlockTag::Latest, None)
            .await,
        Err(ContractError::Nonpayable)
    ));
    assert_eq!(mock.0.lock().unwrap().calls.len(), 4);
    let second = Mock::default();
    let other = provider(second.clone());
    let connected = contract.connect(&other);
    assert!(std::ptr::eq(connected.provider(), &other));
    connected
        .simulate_fallback(OWNER.parse().unwrap(), &call, BlockTag::Latest, None)
        .await
        .unwrap();
    assert_eq!(second.0.lock().unwrap().calls.len(), 2);
    assert_eq!(mock.0.lock().unwrap().calls.len(), 4);
}
fn log(topics: &[Hash32], data: &[u8], index: usize) -> Value {
    json!({"address":ADDRESS,"topics":topics.iter().map(ToString::to_string).collect::<Vec<_>>(),"data":hexlify(data).unwrap(),"blockHash":Hash32::from_bytes([1;32]).to_string(),"blockNumber":"0xa","transactionHash":Hash32::from_bytes([2;32]).to_string(),"transactionIndex":"0x0","logIndex":format!("0x{index:x}"),"removed":true})
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn wildcard_queries_preserve_unknown_malformed_and_removed_logs_and_share_declarations() {
    let mock = Mock::default();
    let provider = provider(mock.clone());
    let abi = AbiInterface::from_human_readable(&["event Ping(uint n)"]).unwrap();
    let (topics, bytes) = abi.event("Ping").unwrap().encode_log(&[json!(7)]).unwrap();
    mock.0.lock().unwrap().logs = vec![
        log(&topics, &bytes, 0),
        log(&[], &[], 1),
        log(&topics, &[1], 2),
        log(&topics, &bytes, 3),
    ];
    let contract = Contract::new(ADDRESS.parse().unwrap(), abi, &provider);
    let range = LogRange::Inclusive { from: 10, to: 20 };
    let logs = contract.query_logs(range, &[], 4).await.unwrap();
    assert_eq!(logs.len(), 4);
    let ContractLog::Decoded { event, values, .. } = &logs[0] else {
        panic!("decoded")
    };
    assert_eq!(event.signature(), "Ping(uint256)");
    assert_eq!(values[0], quai_sdk::abi::AbiEventValue::Value(json!("7")));
    let ContractLog::Decoded { event: again, .. } = &logs[3] else {
        panic!("decoded")
    };
    assert!(Arc::ptr_eq(event, again));
    assert!(matches!(logs[1], ContractLog::Unrecognized(_)));
    assert!(matches!(logs[2], ContractLog::Undecoded { .. }));
    assert!(logs.iter().all(|l| l.log().removed));
    assert_eq!(logs[2].log().data.bytes(), &[1]);
    let mut foreign = logs[0].log().clone();
    foreign.address = OWNER.parse().unwrap();
    assert!(matches!(
        contract.decode_log(foreign),
        ContractLog::Unrecognized(_)
    ));
    {
        let state = mock.0.lock().unwrap();
        assert_eq!(state.calls[1].1[0]["topics"], json!([]));
        assert_eq!(
            state.calls[1].1[0]["address"],
            json!([contract.address().to_string()])
        );
    }

    assert!(matches!(
        contract.query_logs(range, &[], 3).await,
        Err(ContractError::Abi(AbiError::Limit))
    ));
    let count = mock.0.lock().unwrap().calls.len();
    for (topics, max) in [
        (vec![], 0),
        (vec![TopicMatch::Any; 5], 1),
        (vec![TopicMatch::AnyOf(vec![])], 1),
        (vec![TopicMatch::AnyOf(vec![Hash32::ZERO; 129])], 1),
    ] {
        assert!(contract.query_logs(range, &topics, max).await.is_err());
    }
    assert_eq!(mock.0.lock().unwrap().calls.len(), count);
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn aggregate_decoded_event_budget_bounds_repeated_value_expansion() {
    let mock = Mock::default();
    let provider = provider(mock.clone());
    let abi = AbiInterface::from_human_readable(&["event Large(uint256[1024] n)"]).unwrap();
    let (topics, bytes) = abi
        .event("Large")
        .unwrap()
        .encode_log(&[json!(vec!["0"; 1024])])
        .unwrap();
    mock.0.lock().unwrap().logs = (0..64).map(|i| log(&topics, &bytes, i)).collect();
    let contract = Contract::new(ADDRESS.parse().unwrap(), abi, &provider);
    assert!(matches!(
        contract
            .query_logs(LogRange::Inclusive { from: 10, to: 10 }, &[], 64)
            .await,
        Err(ContractError::Abi(AbiError::Limit))
    ));
    let abi = AbiInterface::from_human_readable(&["event Text(string value)"]).unwrap();
    let (topics, bytes) = abi
        .event("Text")
        .unwrap()
        .encode_log(&[json!("x".repeat(900_000))])
        .unwrap();
    mock.0.lock().unwrap().logs = (0..5).map(|i| log(&topics, &bytes, i)).collect();
    let contract = Contract::new(ADDRESS.parse().unwrap(), abi, &provider);
    assert!(matches!(
        contract
            .query_logs(LogRange::Inclusive { from: 10, to: 10 }, &[], 5)
            .await,
        Err(ContractError::Abi(AbiError::Limit))
    ));
}
