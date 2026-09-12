//! Deterministic canonical-header, reorganization, timeout and cancellation tests.
#![cfg(all(feature = "polling", not(target_arch = "wasm32")))]
use quai_primitives::{Hash32, Zone};
use quai_provider::{Provider, WaitConfig, WaitError};
use quai_rpc::{Endpoint, Routing, RpcError, Transport, U256};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

const URL: &str = "http://127.0.0.1:9200";
const TX: &str = "0x00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
fn hash(byte: u8) -> String {
    Hash32::from_bytes([byte; 32]).to_string()
}
enum Action {
    Result(Value),
    Pending,
}
type Step = (String, Value, Action);
#[derive(Clone, Default)]
struct Mock {
    steps: Arc<Mutex<VecDeque<Step>>>,
    in_flight: Arc<AtomicBool>,
}
impl Mock {
    fn read(&self, method: &str, params: Value, response: Value) {
        let mut steps = self.steps.lock().unwrap();
        steps.push_back((
            "quai_chainId".into(),
            json!([]),
            Action::Result(json!("0x9")),
        ));
        steps.push_back((method.into(), params, Action::Result(response)));
    }
    fn provider(&self) -> Provider<Self> {
        Provider::new(
            self.clone(),
            Routing::direct(URL, Zone::Cyprus1.into()).unwrap(),
            U256::from(9),
        )
    }
    fn drained(&self) {
        assert!(self.steps.lock().unwrap().is_empty());
    }
}
struct Flight(Arc<AtomicBool>);
impl Drop for Flight {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}
impl Transport for Mock {
    async fn request(
        &self,
        endpoint: &Endpoint,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        assert_eq!(endpoint.as_str(), URL);
        let (expected, args, action) = self
            .steps
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected poll/request");
        assert_eq!(method, expected);
        assert_eq!(params, args);
        match action {
            Action::Result(value) => Ok(value),
            Action::Pending => {
                self.in_flight.store(true, Ordering::SeqCst);
                let _flight = Flight(self.in_flight.clone());
                std::future::pending().await
            }
        }
    }
}
fn config() -> WaitConfig {
    WaitConfig {
        confirmations: 2,
        timeout: Duration::from_secs(1),
        poll_interval: Duration::from_millis(1),
    }
}
fn receipt(block_hash: u8) -> Value {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/lan-mainnet.json")).unwrap();
    let mut value = fixture["records"][0]["receipt"].clone();
    value["transactionHash"] = json!(TX);
    value["blockHash"] = json!(hash(block_hash));
    value["blockNumber"] = json!("0x10");
    value
}
fn header(number: u64, block_hash: u8) -> Value {
    json!({"gasLimit":"0x10000","stateLimit":"0x10000","woHeader":{
        "hash":hash(block_hash),"parentHash":hash(0),"number":format!("0x{number:x}"),"primeTerminusNumber":"0x4","location":"0x0000"}})
}
fn queue_success(mock: &Mock, receipt_hash: u8, head_hash: u8, head_number: u64) {
    mock.read(
        "quai_getTransactionReceipt",
        json!([TX]),
        receipt(receipt_hash),
    );
    mock.read(
        "quai_getHeaderByNumber",
        json!(["latest"]),
        header(head_number, head_hash),
    );
    mock.read(
        "quai_getHeaderByNumber",
        json!(["0x10"]),
        header(16, receipt_hash),
    );
    mock.read(
        "quai_getTransactionReceipt",
        json!([TX]),
        receipt(receipt_hash),
    );
    mock.read(
        "quai_getHeaderByNumber",
        json!([format!("0x{head_number:x}")]),
        header(head_number, head_hash),
    );
}
#[tokio::test]
async fn waits_for_inclusion_and_checks_receipt_and_head_canonical_hashes() {
    let mock = Mock::default();
    mock.read("quai_getTransactionReceipt", json!([TX]), Value::Null);
    queue_success(&mock, 1, 2, 17);
    let result = mock
        .provider()
        .wait_for_receipt(Zone::Cyprus1, TX.parse().unwrap(), config())
        .await
        .unwrap();
    assert_eq!(result.confirmations, 2);
    assert_eq!(result.receipt.inclusion.block_hash.to_string(), hash(1));
    assert_eq!(result.observed_head.number, 17);
    mock.drained();
}
#[tokio::test]
async fn reorged_candidate_is_not_returned_and_new_canonical_receipt_is_used() {
    let mock = Mock::default();
    mock.read("quai_getTransactionReceipt", json!([TX]), receipt(1));
    mock.read("quai_getHeaderByNumber", json!(["latest"]), header(17, 2));
    mock.read("quai_getHeaderByNumber", json!(["0x10"]), header(16, 3));
    queue_success(&mock, 3, 4, 18);
    let result = mock
        .provider()
        .wait_for_receipt(Zone::Cyprus1, TX.parse().unwrap(), config())
        .await
        .unwrap();
    assert_eq!(result.confirmations, 3);
    assert_eq!(result.receipt.inclusion.block_hash.to_string(), hash(3));
    mock.drained();
}
#[tokio::test]
async fn disappearing_receipt_between_canonical_check_and_return_is_repolled() {
    let mock = Mock::default();
    mock.read("quai_getTransactionReceipt", json!([TX]), receipt(1));
    mock.read("quai_getHeaderByNumber", json!(["latest"]), header(17, 2));
    mock.read("quai_getHeaderByNumber", json!(["0x10"]), header(16, 1));
    mock.read("quai_getTransactionReceipt", json!([TX]), Value::Null);
    queue_success(&mock, 3, 4, 18);
    let result = mock
        .provider()
        .wait_for_receipt(Zone::Cyprus1, TX.parse().unwrap(), config())
        .await
        .unwrap();
    assert_eq!(result.receipt.inclusion.block_hash.to_string(), hash(3));
    mock.drained();
}
#[tokio::test]
async fn invalid_limits_are_rejected_before_io() {
    let mock = Mock::default();
    for config in [
        WaitConfig {
            confirmations: 0,
            ..config()
        },
        WaitConfig {
            timeout: Duration::ZERO,
            ..config()
        },
        WaitConfig {
            poll_interval: Duration::ZERO,
            ..config()
        },
        WaitConfig {
            poll_interval: Duration::from_secs(2),
            ..config()
        },
    ] {
        assert!(matches!(
            mock.provider()
                .wait_for_receipt(Zone::Cyprus1, TX.parse().unwrap(), config)
                .await,
            Err(WaitError::InvalidConfig)
        ));
    }
    mock.drained();
}
#[tokio::test]
async fn total_deadline_cancels_even_a_stalled_transport_future() {
    let mock = Mock::default();
    mock.steps
        .lock()
        .unwrap()
        .push_back(("quai_chainId".into(), json!([]), Action::Pending));
    let result = mock
        .provider()
        .wait_for_receipt(
            Zone::Cyprus1,
            TX.parse().unwrap(),
            WaitConfig {
                timeout: Duration::from_millis(15),
                ..config()
            },
        )
        .await;
    assert!(matches!(result, Err(WaitError::Timeout { .. })));
    assert!(!mock.in_flight.load(Ordering::SeqCst));
    mock.drained();
}
#[tokio::test]
async fn dropping_wait_future_releases_in_flight_rpc_and_stops_polling() {
    let mock = Mock::default();
    mock.steps
        .lock()
        .unwrap()
        .push_back(("quai_chainId".into(), json!([]), Action::Pending));
    let provider = mock.provider();
    let task = tokio::spawn(async move {
        provider
            .wait_for_receipt(Zone::Cyprus1, TX.parse().unwrap(), config())
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while !mock.in_flight.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(!mock.in_flight.load(Ordering::SeqCst));
    mock.drained();
}
#[tokio::test]
async fn header_lookup_rejects_wrong_zone_height_and_selector_overflow() {
    for mut value in [header(17, 1), header(16, 1)] {
        if value["woHeader"]["number"] == "0x10" {
            value["woHeader"]["location"] = json!("0x0100");
        }
        let mock = Mock::default();
        mock.read("quai_getHeaderByNumber", json!(["0x10"]), value);
        assert!(mock.provider().header_at(Zone::Cyprus1, 16).await.is_err());
        mock.drained();
    }
    let mock = Mock::default();
    assert!(
        mock.provider()
            .header_at(Zone::Cyprus1, u64::MAX)
            .await
            .is_err()
    );
    mock.drained();
}
