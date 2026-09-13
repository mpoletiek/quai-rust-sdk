//! Log filter response validation and bounded range semantics.
use quai_primitives::{Hash32, Zone};
use quai_provider::{LogFilter, LogRange, Provider, TopicMatch};
use quai_rpc::{Endpoint, Routing, RpcError, Transport, U256};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
const ADDRESS: &str = "0x0011223344556677889900112233445566778899";
#[derive(Clone, Default)]
struct Mock {
    calls: Arc<Mutex<Vec<(String, Value)>>>,
    logs: Vec<Value>,
}
impl Transport for Mock {
    async fn request(&self, _: &Endpoint, method: &str, params: Value) -> Result<Value, RpcError> {
        self.calls.lock().unwrap().push((method.to_owned(), params));
        match method {
            "quai_chainId" => Ok(json!("0x9")),
            "quai_getLogs" => Ok(json!(self.logs)),
            _ => panic!("unexpected method"),
        }
    }
}
fn provider(mock: Mock) -> Provider<Mock> {
    Provider::new(
        mock,
        Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
        U256::from(9),
    )
}
fn filter() -> LogFilter {
    LogFilter {
        zone: Zone::Cyprus1,
        range: LogRange::Inclusive { from: 10, to: 20 },
        addresses: vec![ADDRESS.parse().unwrap()],
        topics: vec![
            TopicMatch::Exact(Hash32::from_bytes([3; 32])),
            TopicMatch::AnyOf(vec![Hash32::from_bytes([4; 32])]),
        ],
    }
}
fn log() -> Value {
    json!({"address":ADDRESS,"topics":[Hash32::from_bytes([3;32]).to_string(),Hash32::from_bytes([4;32]).to_string()],"data":"0x0012","transactionHash":Hash32::from_bytes([5;32]).to_string(),"blockHash":Hash32::from_bytes([6;32]).to_string(),"blockNumber":"0xf","transactionIndex":"0x2","logIndex":"0x3","removed":false})
}
#[tokio::test]
async fn explicit_range_topic_or_and_removal_are_preserved() {
    let mut removed = log();
    removed["removed"] = json!(true);
    let mock = Mock {
        logs: vec![removed],
        ..Default::default()
    };
    let p = provider(mock.clone());
    let logs = p.logs(&filter()).await.unwrap();
    assert_eq!(logs.len(), 1);
    assert!(logs[0].removed);
    assert_eq!(logs[0].data.bytes(), [0, 0x12]);
    let calls = mock.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].1[0]["fromBlock"], "0xa");
    assert_eq!(calls[1].1[0]["toBlock"], "0x14");
    assert!(calls[1].1[0]["topics"][1].is_array());
}
#[tokio::test]
async fn foreign_out_of_range_wrong_topic_and_duplicate_logs_fail() {
    let mut cases = vec![vec![log(), log()]];
    for (field, value) in [
        (
            "address",
            json!("0x1000000000000000000000000000000000000000"),
        ),
        ("blockNumber", json!("0x15")),
        ("topics", json!([Hash32::ZERO.to_string()])),
    ] {
        let mut row = log();
        row[field] = value;
        cases.push(vec![row]);
    }
    for logs in cases {
        assert!(
            provider(Mock {
                logs,
                ..Default::default()
            })
            .logs(&filter())
            .await
            .is_err()
        );
    }
    let mut by_hash = filter();
    by_hash.range = LogRange::BlockHash(Hash32::from_bytes([9; 32]));
    assert!(
        provider(Mock {
            logs: vec![log()],
            ..Default::default()
        })
        .logs(&by_hash)
        .await
        .is_err()
    );
}
#[tokio::test]
async fn invalid_log_bounds_fail_without_network_calls() {
    let mock = Mock::default();
    let p = provider(mock.clone());
    for range in [
        LogRange::Inclusive { from: 1, to: 0 },
        LogRange::Inclusive {
            from: 0,
            to: 10_000,
        },
        LogRange::Inclusive {
            from: 0,
            to: u64::MAX,
        },
    ] {
        let mut f = filter();
        f.range = range;
        assert!(p.logs(&f).await.is_err());
    }
    let mut f = filter();
    f.topics = vec![TopicMatch::AnyOf(vec![])];
    assert!(p.logs(&f).await.is_err());
    assert!(mock.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn qi_protocol_logs_are_distinct_from_contract_events_and_preserve_zone_checks() {
    let qi = "0x00a8c6cf826b72080fa6f838a3329ec0b906b408";
    let mut row = log();
    row["address"] = json!(qi);
    row["data"] = json!("0x03e8");
    let p = provider(Mock {
        logs: vec![row.clone()],
        ..Default::default()
    });
    let mut f = filter();
    assert!(p.logs(&f).await.is_err()); // An exact Quai filter must not match Qi.
    f.addresses = vec![qi.parse().unwrap()];
    let logs = p.logs(&f).await.unwrap();
    assert_eq!(logs[0].address.ledger(), quai_primitives::Ledger::Qi);
    assert_eq!(logs[0].data.bytes(), [3, 232]);
    f.addresses.clear();
    assert_eq!(p.logs(&f).await.unwrap().len(), 1);
    for foreign in [
        "0x0180000000000000000000000000000000000001",
        "0xff80000000000000000000000000000000000001",
    ] {
        row["address"] = json!(foreign);
        assert!(
            provider(Mock {
                logs: vec![row.clone()],
                ..Default::default()
            })
            .logs(&f)
            .await
            .is_err()
        );
        let mut invalid = f.clone();
        invalid.addresses = vec![foreign.parse().unwrap()];
        assert!(p.logs(&invalid).await.is_err());
    }
}

#[test]
fn captured_redemption_receipt_accepts_qi_beneficiary_and_checks_log_association() {
    let value: Value = serde_json::from_str(include_str!(
        "fixtures/shared/test-infra/local-chain/wqi-evidence/wqi-redemption-receipt.json"
    ))
    .unwrap();
    let receipt = quai_provider::Receipt::try_from(value.clone()).unwrap();
    assert_eq!(receipt.outcome, quai_provider::ReceiptOutcome::Succeeded);
    assert_eq!(receipt.logs.len(), 1);
    assert_eq!(
        receipt.logs[0].address.ledger(),
        quai_primitives::Ledger::Qi
    );
    assert_eq!(receipt.logs[0].data.bytes(), [3, 232]);
    assert_eq!(receipt.inclusion.block_number, 25);
    for (field, replacement) in [
        ("transactionHash", json!(Hash32::ZERO.to_string())),
        ("blockHash", json!(Hash32::ZERO.to_string())),
        ("blockNumber", json!("0x18")),
    ] {
        let mut wrong = value.clone();
        wrong["logs"][0][field] = replacement;
        assert!(quai_provider::Receipt::try_from(wrong).is_err());
    }
}
