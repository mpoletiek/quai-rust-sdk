//! Contract event association, strict ABI decoding and reorg metadata.
#![cfg(feature = "abi")]
use quai_sdk::abi::{AbiEventValue, AbiInterface};
use quai_sdk::contracts::{Contract, ContractError};
use quai_sdk::primitives::Hash32;
use quai_sdk::provider::{LogRange, TopicMatch};
use quai_sdk::rpc::{RpcError, Transport};
use quai_sdk::{Endpoint, Provider, Routing, U256, Zone};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
const ADDRESS: &str = "0x0011223344556677889900112233445566778899";
const ABI: &[u8] = br#"[
{"type":"event","name":"Tagged","inputs":[{"name":"tag","type":"string","indexed":true},{"name":"amount","type":"uint256","indexed":false}]},
{"type":"event","name":"Anonymous","anonymous":true,"inputs":[{"name":"tag","type":"bytes32","indexed":true},{"name":"amount","type":"uint256","indexed":false}]}
]"#;
#[derive(Clone)]
struct Mock {
    calls: Arc<Mutex<Vec<(String, Value)>>>,
    row: Value,
}
impl Transport for Mock {
    async fn request(&self, _: &Endpoint, method: &str, params: Value) -> Result<Value, RpcError> {
        self.calls.lock().unwrap().push((method.to_owned(), params));
        match method {
            "quai_chainId" => Ok(json!("0x9")),
            "quai_getLogs" => Ok(json!([self.row])),
            _ => panic!("unexpected method"),
        }
    }
}
fn fixture() -> (Mock, Vec<Hash32>) {
    let abi = AbiInterface::from_json(ABI).unwrap();
    let (topics, data) = abi
        .event("Tagged")
        .unwrap()
        .encode_log(&[json!("private tag"), json!("7")])
        .unwrap();
    let data: String = data.iter().map(|b| format!("{b:02x}")).collect();
    (
        Mock {
            calls: Default::default(),
            row: json!({"address":ADDRESS,"topics":topics.iter().map(ToString::to_string).collect::<Vec<_>>(),"data":format!("0x{data}"),"transactionHash":Hash32::from_bytes([5;32]).to_string(),"blockHash":Hash32::from_bytes([6;32]).to_string(),"blockNumber":"0xf","transactionIndex":"0x2","logIndex":"0x3","removed":true}),
        },
        topics,
    )
}
fn provider(mock: Mock) -> Provider<Mock> {
    Provider::new(
        mock,
        Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
        U256::from(9),
    )
}
#[tokio::test]
async fn query_binds_signature_and_emitter_and_preserves_hashed_values_and_removal() {
    let (mock, topics) = fixture();
    let p = provider(mock.clone());
    let contract = Contract::new(
        ADDRESS.parse().unwrap(),
        AbiInterface::from_json(ABI).unwrap(),
        &p,
    );
    let events = contract
        .events(
            "Tagged",
            LogRange::Inclusive { from: 10, to: 20 },
            &[TopicMatch::Exact(topics[1])],
        )
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert!(event.log.removed);
    assert_eq!(event.log.inclusion.block_number, 15);
    assert_eq!(
        event.values,
        [
            AbiEventValue::IndexedHash(topics[1]),
            AbiEventValue::Value(json!("7"))
        ]
    );
    let calls = mock.calls.lock().unwrap();
    assert_eq!(
        calls[1].1[0]["topics"],
        json!(topics.iter().map(ToString::to_string).collect::<Vec<_>>())
    );
    assert_eq!(
        calls[1].1[0]["address"],
        json!([ADDRESS
            .parse::<quai_sdk::QuaiAddress>()
            .unwrap()
            .to_string()])
    );
    drop(calls);
    let mut foreign = event.log.clone();
    foreign.address = "0x0000000000000000000000000000000000000001"
        .parse()
        .unwrap();
    assert!(matches!(
        contract.decode_event("Tagged", foreign),
        Err(ContractError::CallMismatch)
    ));
    let mut malformed = event.log.clone();
    malformed.data = quai_sdk::provider::RpcData::new(vec![0; 31]).unwrap();
    assert!(matches!(
        contract.decode_event("Tagged", malformed),
        Err(ContractError::Abi(_))
    ));
    let mut anonymous = event.log.clone();
    anonymous.topics.remove(0);
    assert!(contract.decode_event("Anonymous", anonymous).is_ok());
}
#[tokio::test]
async fn ambiguous_anonymous_queries_and_excess_filters_fail_before_io() {
    let (mock, _) = fixture();
    let p = provider(mock.clone());
    let contract = Contract::new(
        ADDRESS.parse().unwrap(),
        AbiInterface::from_json(ABI).unwrap(),
        &p,
    );
    let range = LogRange::Inclusive { from: 10, to: 20 };
    assert!(matches!(
        contract.events("Anonymous", range, &[]).await,
        Err(ContractError::AnonymousEvent)
    ));
    assert!(matches!(
        contract
            .events("Tagged", range, &[TopicMatch::Any, TopicMatch::Any])
            .await,
        Err(ContractError::InvalidEventFilter)
    ));
    assert!(mock.calls.lock().unwrap().is_empty());
}
