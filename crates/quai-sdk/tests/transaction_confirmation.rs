//! Receipt-independent transaction confirmation and verified Qi RPC responses.
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
use quai_sdk::primitives::Hash32;
use quai_sdk::provider::{ProviderError, TransactionConfirmation};
use quai_sdk::rpc::{Endpoint, RpcError, Transport};
use quai_sdk::{Provider, Routing, U256, Zone};
use serde_json::{Value, json};
use std::collections::VecDeque;
#[cfg(target_arch = "wasm32")]
type Shared<T> = std::rc::Rc<std::cell::RefCell<T>>;
#[cfg(not(target_arch = "wasm32"))]
type Shared<T> = std::sync::Arc<std::sync::Mutex<T>>;
#[derive(Default)]
struct State<T>(Shared<T>);
impl<T> Clone for State<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<T> State<T> {
    fn borrow_mut(&self) -> impl std::ops::DerefMut<Target = T> {
        #[cfg(target_arch = "wasm32")]
        {
            self.0.borrow_mut()
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.0.lock().unwrap()
        }
    }
}
impl State<bool> {
    fn get(&self) -> bool {
        *self.borrow_mut()
    }
    fn set(&self, value: bool) {
        *self.borrow_mut() = value;
    }
}
const TX: &str = "0x00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
enum Action {
    Value(Value),
    Pending,
    Error,
}
type Step = (String, Value, Action);
#[derive(Clone, Default)]
struct Mock {
    steps: State<VecDeque<Step>>,
    in_flight: State<bool>,
}
struct Flight(State<bool>);
impl Drop for Flight {
    fn drop(&mut self) {
        self.0.set(false);
    }
}
impl Transport for Mock {
    async fn request(&self, _: &Endpoint, method: &str, params: Value) -> Result<Value, RpcError> {
        let (expected, args, action) = self
            .steps
            .borrow_mut()
            .pop_front()
            .expect("unexpected read");
        assert_eq!(method, expected);
        assert_eq!(params, args);
        match action {
            Action::Value(value) => Ok(value),
            Action::Error => Err(RpcError::Transport),
            Action::Pending => {
                self.in_flight.set(true);
                let _flight = Flight(self.in_flight.clone());
                std::future::pending().await
            }
        }
    }
}
impl Mock {
    fn read(&self, method: &str, params: Value, value: Value) {
        self.steps.borrow_mut().push_back((
            "quai_chainId".into(),
            json!([]),
            Action::Value(json!("0x9")),
        ));
        self.steps
            .borrow_mut()
            .push_back((method.into(), params, Action::Value(value)));
    }
    fn provider(&self) -> Provider<Self> {
        Provider::new(
            self.clone(),
            Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
            U256::from(9),
        )
    }
    fn drained(&self) {
        assert!(self.steps.borrow_mut().is_empty());
    }
}
fn hash(n: u8) -> String {
    Hash32::from_bytes([n; 32]).to_string()
}
fn transaction() -> Value {
    json!({"hash":TX,"type":"0x2","blockHash":hash(1),"blockNumber":"0x10","transactionIndex":"0x0","chainId":"0x9","gas":"0x0","nonce":"0x0","input":"0x","utxoSignature":format!("0x{}","00".repeat(64)),"inputs":[{"previousOutPoint":{"txHash":hash(128),"index":"0x0"},"pubKey":format!("0x02{}","11".repeat(32))}],"outputs":[{"address":"0x0080000000000000000000000000000000000000","denomination":"0x0","lock":null}]})
}
fn header(height: u64, n: u8) -> Value {
    json!({"gasLimit":"0x10000","stateLimit":"0x10000","woHeader":{"hash":hash(n),"parentHash":hash(0),"number":format!("0x{height:x}"),"primeTerminusNumber":"0x4","location":"0x0000"}})
}
fn block() -> Value {
    let mut v = header(16, 1);
    v["hash"] = json!(hash(1));
    v["transactions"] = json!([TX]);
    v
}
fn initial(mock: &Mock) {
    mock.read("quai_getTransactionByHash", json!([TX]), transaction());
    mock.read("quai_getHeaderByNumber", json!(["latest"]), header(17, 2));
}
fn success(mock: &Mock) {
    initial(mock);
    mock.read("quai_getBlockByNumber", json!(["0x10", false]), block());
    mock.read("quai_getTransactionByHash", json!([TX]), transaction());
    mock.read("quai_getHeaderByNumber", json!(["0x11"]), header(17, 2));
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn portable_transaction_wait_observes_qi_without_receipt_queries() {
    let mock = Mock::default();
    let provider = mock.provider();
    let tx = TX.parse().unwrap();
    assert!(matches!(
        provider
            .observe_transaction_confirmation(Zone::Cyprus1, tx, 0)
            .await,
        Err(ProviderError::InvalidRequest(_))
    ));
    mock.read("quai_getTransactionByHash", json!([TX]), Value::Null);
    assert!(matches!(
        provider
            .observe_transaction_confirmation(Zone::Cyprus1, tx, 2)
            .await
            .unwrap(),
        TransactionConfirmation::Pending {
            last_observed_inclusion: None
        }
    ));
    let mut pending = transaction();
    pending["blockHash"] = Value::Null;
    pending["blockNumber"] = Value::Null;
    pending["transactionIndex"] = Value::Null;
    mock.read("quai_getTransactionByHash", json!([TX]), pending);
    assert!(matches!(
        provider
            .observe_transaction_confirmation(Zone::Cyprus1, tx, 2)
            .await
            .unwrap(),
        TransactionConfirmation::Pending {
            last_observed_inclusion: None
        }
    ));
    initial(&mock);
    assert!(matches!(
        provider
            .observe_transaction_confirmation(Zone::Cyprus1, tx, 3)
            .await
            .unwrap(),
        TransactionConfirmation::Pending {
            last_observed_inclusion: Some(_)
        }
    ));
    success(&mock);
    let TransactionConfirmation::Confirmed(result) = provider
        .observe_transaction_confirmation(Zone::Cyprus1, tx, 2)
        .await
        .unwrap()
    else {
        panic!("confirmed expected")
    };
    assert_eq!(result.confirmations, 2);
    assert_eq!(result.transaction.hash, tx);
    assert_eq!(result.observed_head.number, 17);
    // Inclusion observation is distinct from cryptographic payload verification.
    assert!(result.transaction.verified_qi().is_err());
    mock.drained();
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn wrong_block_position_changed_payload_inclusion_and_head_never_confirm() {
    for variant in 0..5 {
        let mock = Mock::default();
        let provider = mock.provider();
        initial(&mock);
        let mut b = block();
        if variant == 0 {
            b["woHeader"]["hash"] = json!(hash(3));
            b["hash"] = json!(hash(3));
        }
        if variant == 1 {
            b["transactions"] = json!([hash(3)]);
        }
        mock.read("quai_getBlockByNumber", json!(["0x10", false]), b);
        if variant >= 2 {
            let mut refreshed = transaction();
            if variant == 2 {
                refreshed["blockHash"] = json!(hash(3));
            }
            if variant == 3 {
                refreshed["input"] = json!("0x01");
            }
            mock.read("quai_getTransactionByHash", json!([TX]), refreshed);
            if variant == 4 {
                mock.read("quai_getHeaderByNumber", json!(["0x11"]), header(17, 3));
            }
        }
        assert!(matches!(
            provider
                .observe_transaction_confirmation(Zone::Cyprus1, TX.parse().unwrap(), 2)
                .await
                .unwrap(),
            TransactionConfirmation::Pending { .. }
        ));
        mock.drained();
    }
}
fn vectors() -> Vec<Value> {
    let mut rows = Vec::new();
    for text in [
        include_str!(
            "fixtures/shared/crates/quai-consensus/tests/fixtures/shared/compatibility/fixtures/transactions.json"
        ),
        include_str!("fixtures/shared/crates/quai-consensus/tests/conversion-vectors.json"),
        include_str!("fixtures/shared/crates/quai-consensus/tests/wrapping-vectors.json"),
    ] {
        rows.extend(
            serde_json::from_str::<Value>(text).unwrap()["vectors"]
                .as_array()
                .unwrap()
                .clone(),
        );
    }
    rows
}
fn rpc_qi(row: &Value) -> Value {
    let input = &row["input"];
    let quantity = |value: &Value| {
        format!(
            "0x{:x}",
            U256::from_str_radix(&value.to_string().replace('"', ""), 10).unwrap()
        )
    };
    json!({"hash":row["hash"],"type":"0x2","blockHash":null,"blockNumber":null,"transactionIndex":null,"chainId":quantity(&input["chainId"]),"gas":"0x0","nonce":"0x0","input":input["data"].as_str().unwrap_or("0x"),"utxoSignature":input["signature"],"inputs":input["txInputs"].as_array().unwrap().iter().map(|i|json!({"previousOutPoint":{"txHash":i["txhash"],"index":quantity(&i["index"])},"pubKey":i["pubkey"]})).collect::<Vec<_>>(),"outputs":input["txOutputs"].as_array().unwrap().iter().map(|o|json!({"address":o["address"],"denomination":quantity(&o["denomination"]),"lock":null})).collect::<Vec<_>>()})
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn rpc_qi_verification_covers_transfer_multi_input_conversion_and_wrapping() {
    use quai_sdk::provider::{RpcData, Transaction, TransactionDetails};
    let mut verified = 0;
    for row in vectors().iter().filter(|r| r["kind"] == "qi") {
        let tx = Transaction::try_from(rpc_qi(row)).unwrap();
        if row["id"] == "qi-1" || row["id"] == "qi-2" {
            assert!(tx.verified_qi().is_err());
            continue;
        }
        assert_eq!(
            tx.verified_qi().unwrap().signed_bytes().unwrap(),
            quai_sdk::primitives::get_bytes(row["signed"].as_str().unwrap()).unwrap()
        );
        verified += 1;
        let mut forged = tx.clone();
        forged.hash = Hash32::from_bytes([255; 32]);
        assert!(forged.verified_qi().is_err());
        for variant in 0..5 {
            let mut forged = tx.clone();
            let TransactionDetails::Qi(ref mut fields) = forged.details else {
                unreachable!()
            };
            match variant {
                0 => fields.outputs[0].lock = Some(U256::from(1)),
                1 => fields.inputs[0].previous_out_point.index ^= 1,
                2 => fields.chain_id += U256::from(1),
                3 => fields.signature = RpcData::new(vec![0; 64]).unwrap(),
                _ => fields.inputs[0].public_key = RpcData::new(vec![255; 33]).unwrap(),
            }
            assert!(forged.verified_qi().is_err());
        }
    }
    assert_eq!(verified, 14);
}
#[cfg(all(not(target_arch = "wasm32"), any(feature = "http", feature = "ws")))]
async fn wait(
    provider: &Provider<Mock>,
    timeout: u32,
) -> Result<quai_sdk::provider::ConfirmedTransaction, String> {
    use std::time::Duration;
    provider
        .wait_for_transaction(
            Zone::Cyprus1,
            TX.parse().unwrap(),
            quai_sdk::provider::WaitConfig {
                confirmations: 2,
                timeout: Duration::from_millis(u64::from(timeout)),
                poll_interval: Duration::from_millis(1),
            },
        )
        .await
        .map_err(|e| e.to_string())
}
#[cfg(target_arch = "wasm32")]
async fn wait(
    provider: &Provider<Mock>,
    timeout: u32,
) -> Result<quai_sdk::provider::ConfirmedTransaction, String> {
    quai_sdk::browser::wait_for_transaction(
        provider,
        Zone::Cyprus1,
        TX.parse().unwrap(),
        quai_sdk::browser::BrowserWaitConfig {
            confirmations: 2,
            timeout_ms: timeout,
            poll_interval_ms: 1,
            max_polls: 10,
        },
    )
    .await
    .map_err(|e| e.to_string())
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg(any(target_arch = "wasm32", feature = "http", feature = "ws"))]
async fn native_and_browser_waits_poll_missing_then_return_rechecked_transactions() {
    let mock = Mock::default();
    let provider = mock.provider();
    assert!(wait(&provider, 0).await.is_err());
    mock.drained();
    mock.read("quai_getTransactionByHash", json!([TX]), Value::Null);
    success(&mock);
    let result = wait(&provider, 2000).await.unwrap();
    assert_eq!(result.confirmations, 2);
    mock.drained();
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg(any(target_arch = "wasm32", feature = "http", feature = "ws"))]
async fn deadlines_drop_stalled_reads_and_provider_errors_do_not_retry() {
    let mock = Mock::default();
    let provider = mock.provider();
    mock.steps
        .borrow_mut()
        .push_back(("quai_chainId".into(), json!([]), Action::Pending));
    let error = wait(&provider, 30).await.unwrap_err();
    assert!(error.contains("timed out"));
    assert!(!mock.in_flight.get());
    mock.drained();
    mock.steps
        .borrow_mut()
        .push_back(("quai_chainId".into(), json!([]), Action::Error));
    assert!(wait(&provider, 1000).await.is_err());
    mock.drained();
}

#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn dropping_portable_observation_releases_read_and_error_stops_immediately() {
    use std::{future::Future, task::Poll};
    let mock = Mock::default();
    let provider = mock.provider();
    assert!(
        provider
            .observe_transaction_confirmation(Zone::Cyprus1, Hash32::ZERO, 1)
            .await
            .is_err()
    );
    mock.drained();
    mock.steps
        .borrow_mut()
        .push_back(("quai_chainId".into(), json!([]), Action::Pending));
    let mut future =
        Box::pin(provider.observe_transaction_confirmation(Zone::Cyprus1, TX.parse().unwrap(), 1));
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    assert!(mock.in_flight.get());
    drop(future);
    assert!(!mock.in_flight.get());
    mock.drained();
    mock.steps
        .borrow_mut()
        .push_back(("quai_chainId".into(), json!([]), Action::Error));
    assert!(matches!(
        provider
            .observe_transaction_confirmation(Zone::Cyprus1, TX.parse().unwrap(), 1)
            .await,
        Err(ProviderError::Rpc(RpcError::Transport))
    ));
    mock.drained();
    #[cfg(target_arch = "wasm32")]
    {
        mock.read("quai_getTransactionByHash", json!([TX]), Value::Null);
        assert!(matches!(
            quai_sdk::browser::wait_for_transaction(
                &provider,
                Zone::Cyprus1,
                TX.parse().unwrap(),
                quai_sdk::browser::BrowserWaitConfig {
                    confirmations: 1,
                    timeout_ms: 1000,
                    poll_interval_ms: 1,
                    max_polls: 1
                }
            )
            .await,
            Err(quai_sdk::browser::BrowserTransactionWaitError::PollLimit {
                polls_completed: 1,
                ..
            })
        ));
        mock.drained();
    }
}
