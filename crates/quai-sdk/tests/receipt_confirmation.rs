//! Portable confirmation reads and browser deadline, cancellation and reorg checks.
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
use quai_sdk::primitives::Hash32;
use quai_sdk::provider::{ProviderError, ReceiptConfirmation};
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
fn receipt(n: u8) -> Value {
    let fixture: Value = serde_json::from_slice(include_bytes!(
        "fixtures/shared/crates/quai-provider/tests/fixtures/lan-mainnet.json"
    ))
    .unwrap();
    let mut value = fixture["records"][0]["receipt"].clone();
    value["transactionHash"] = json!(TX);
    value["blockHash"] = json!(hash(n));
    value["blockNumber"] = json!("0x10");
    value
}
fn header(height: u64, n: u8) -> Value {
    json!({"gasLimit":"0x10000","stateLimit":"0x10000","woHeader":{"hash":hash(n),"parentHash":hash(0),"number":format!("0x{height:x}"),"primeTerminusNumber":"0x4","location":"0x0000"}})
}
fn success(mock: &Mock, n: u8, outcome: &str) {
    let mut value = receipt(n);
    value["status"] = json!(outcome);
    mock.read("quai_getTransactionReceipt", json!([TX]), value.clone());
    mock.read("quai_getHeaderByNumber", json!(["latest"]), header(17, 2));
    mock.read("quai_getHeaderByNumber", json!(["0x10"]), header(16, n));
    mock.read("quai_getTransactionReceipt", json!([TX]), value);
    mock.read("quai_getHeaderByNumber", json!(["0x11"]), header(17, 2));
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn portable_non_send_observer_returns_missing_pending_and_rechecked_receipts() {
    let mock = Mock::default();
    let provider = mock.provider();
    assert!(matches!(
        provider
            .observe_receipt_confirmation(Zone::Cyprus1, TX.parse().unwrap(), 0)
            .await,
        Err(ProviderError::InvalidRequest(_))
    ));
    mock.read("quai_getTransactionReceipt", json!([TX]), Value::Null);
    assert!(matches!(
        provider
            .observe_receipt_confirmation(Zone::Cyprus1, TX.parse().unwrap(), 2)
            .await
            .unwrap(),
        ReceiptConfirmation::Pending {
            last_observed_inclusion: None
        }
    ));
    mock.read("quai_getTransactionReceipt", json!([TX]), receipt(1));
    mock.read("quai_getHeaderByNumber", json!(["latest"]), header(16, 1));
    assert!(
        matches!(provider.observe_receipt_confirmation(Zone::Cyprus1,TX.parse().unwrap(),2).await.unwrap(),ReceiptConfirmation::Pending{last_observed_inclusion:Some(i)} if i.block_number==16)
    );
    success(&mock, 3, "0x0");
    let ReceiptConfirmation::Confirmed(result) = provider
        .observe_receipt_confirmation(Zone::Cyprus1, TX.parse().unwrap(), 2)
        .await
        .unwrap()
    else {
        panic!("expected confirmed observation")
    };
    assert_eq!(result.confirmations, 2);
    assert_eq!(result.receipt.inclusion.block_hash.to_string(), hash(3));
    assert_eq!(
        result.receipt.outcome,
        quai_sdk::provider::ReceiptOutcome::Failed
    );
    mock.drained();
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn changing_candidate_receipt_or_head_never_returns_confirmation() {
    for changed in 0..3 {
        let mock = Mock::default();
        mock.read("quai_getTransactionReceipt", json!([TX]), receipt(1));
        mock.read("quai_getHeaderByNumber", json!(["latest"]), header(17, 2));
        mock.read(
            "quai_getHeaderByNumber",
            json!(["0x10"]),
            header(16, if changed == 0 { 9 } else { 1 }),
        );
        if changed > 0 {
            mock.read(
                "quai_getTransactionReceipt",
                json!([TX]),
                if changed == 1 {
                    Value::Null
                } else {
                    receipt(1)
                },
            );
        }
        if changed == 2 {
            mock.read("quai_getHeaderByNumber", json!(["0x11"]), header(17, 9));
        }
        assert!(matches!(
            mock.provider()
                .observe_receipt_confirmation(Zone::Cyprus1, TX.parse().unwrap(), 2)
                .await
                .unwrap(),
            ReceiptConfirmation::Pending {
                last_observed_inclusion: Some(_)
            }
        ));
        mock.drained();
    }
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn cancelled_observation_drops_pending_non_send_read_and_errors_do_not_retry() {
    use std::{future::Future, task::Poll};
    let mock = Mock::default();
    let provider = mock.provider();
    mock.steps
        .borrow_mut()
        .push_back(("quai_chainId".into(), json!([]), Action::Pending));
    let mut future =
        Box::pin(provider.observe_receipt_confirmation(Zone::Cyprus1, TX.parse().unwrap(), 2));
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    assert!(mock.in_flight.get());
    drop(future);
    assert!(!mock.in_flight.get());
    mock.steps
        .borrow_mut()
        .push_back(("quai_chainId".into(), json!([]), Action::Error));
    assert!(matches!(
        provider
            .observe_receipt_confirmation(Zone::Cyprus1, TX.parse().unwrap(), 2)
            .await,
        Err(ProviderError::Rpc(RpcError::Transport))
    ));
    mock.drained();
}

#[cfg(all(target_arch = "wasm32", feature = "browser"))]
mod browser {
    use super::*;
    use quai_sdk::browser::{BrowserReceiptWaitError, BrowserWaitConfig, wait_for_receipt};
    fn config() -> BrowserWaitConfig {
        BrowserWaitConfig::new(2, 1000, 1, 4)
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn missing_then_reorged_receipt_is_repolled_until_canonical_success() {
        let mock = Mock::default();
        mock.read("quai_getTransactionReceipt", json!([TX]), Value::Null);
        mock.read("quai_getTransactionReceipt", json!([TX]), receipt(1));
        mock.read("quai_getHeaderByNumber", json!(["latest"]), header(17, 2));
        mock.read("quai_getHeaderByNumber", json!(["0x10"]), header(16, 3));
        success(&mock, 3, "0x0");
        let observed = wait_for_receipt(
            &mock.provider(),
            Zone::Cyprus1,
            TX.parse().unwrap(),
            config(),
        )
        .await
        .unwrap();
        assert_eq!(observed.receipt.inclusion.block_hash.to_string(), hash(3));
        assert_eq!(
            observed.receipt.outcome,
            quai_sdk::provider::ReceiptOutcome::Failed
        );
        mock.drained();
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn invalid_limits_and_provider_errors_do_not_start_another_read() {
        let mock = Mock::default();
        for c in [
            config().with_confirmations(0),
            config().with_timeout_ms(0),
            config().with_timeout_ms(u32::MAX),
            config().with_poll_interval_ms(0),
            config().with_poll_interval_ms(1001),
            config().with_max_polls(0),
            config().with_max_polls(100_001),
        ] {
            assert!(matches!(
                wait_for_receipt(&mock.provider(), Zone::Cyprus1, TX.parse().unwrap(), c).await,
                Err(BrowserReceiptWaitError::InvalidConfig)
            ));
        }
        mock.steps
            .borrow_mut()
            .push_back(("quai_chainId".into(), json!([]), Action::Error));
        assert!(matches!(
            wait_for_receipt(
                &mock.provider(),
                Zone::Cyprus1,
                TX.parse().unwrap(),
                config()
            )
            .await,
            Err(BrowserReceiptWaitError::Provider {
                source: ProviderError::Rpc(RpcError::Transport),
                ..
            })
        ));
        mock.drained();
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn deadline_drops_stalled_read_and_retains_only_completed_poll_context() {
        let mock = Mock::default();
        mock.read("quai_getTransactionReceipt", json!([TX]), receipt(1));
        mock.read("quai_getHeaderByNumber", json!(["latest"]), header(16, 1));
        mock.steps
            .borrow_mut()
            .push_back(("quai_chainId".into(), json!([]), Action::Pending));
        let result = wait_for_receipt(
            &mock.provider(),
            Zone::Cyprus1,
            TX.parse().unwrap(),
            config().with_timeout_ms(50),
        )
        .await;
        assert!(
            matches!(result,Err(BrowserReceiptWaitError::Timeout{polls_completed:1,last_completed_inclusion:Some(i),..}) if i.block_number==16)
        );
        assert!(!mock.in_flight.get());
        mock.drained();
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn poll_budget_and_deadline_during_delay_stop_without_extra_reads() {
        let mock = Mock::default();
        for _ in 0..2 {
            mock.read("quai_getTransactionReceipt", json!([TX]), Value::Null);
        }
        let result = wait_for_receipt(
            &mock.provider(),
            Zone::Cyprus1,
            TX.parse().unwrap(),
            config().with_max_polls(2),
        )
        .await;
        assert!(matches!(
            result,
            Err(BrowserReceiptWaitError::PollLimit {
                polls_completed: 2,
                last_completed_inclusion: None,
                ..
            })
        ));
        mock.read("quai_getTransactionReceipt", json!([TX]), Value::Null);
        let result = wait_for_receipt(
            &mock.provider(),
            Zone::Cyprus1,
            TX.parse().unwrap(),
            config().with_timeout_ms(30).with_poll_interval_ms(30),
        )
        .await;
        assert!(matches!(
            result,
            Err(BrowserReceiptWaitError::Timeout {
                polls_completed: 1,
                last_completed_inclusion: None,
                ..
            })
        ));
        mock.drained();
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn dropping_outer_future_cancels_active_read() {
        use std::{future::Future, task::Poll};
        let mock = Mock::default();
        let provider = mock.provider();
        mock.steps
            .borrow_mut()
            .push_back(("quai_chainId".into(), json!([]), Action::Pending));
        let mut future = Box::pin(wait_for_receipt(
            &provider,
            Zone::Cyprus1,
            TX.parse().unwrap(),
            config(),
        ));
        std::future::poll_fn(|cx| {
            assert!(future.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        assert!(mock.in_flight.get());
        drop(future);
        assert!(!mock.in_flight.get());
        mock.drained();
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn deadline_cancels_actual_fetch_and_releases_its_only_request_permit() {
        use quai_sdk::browser::{BrowserConfig, BrowserFetchTransport};
        let transport = BrowserFetchTransport::new(
            BrowserConfig::default()
                .with_request_timeout_ms(1000)
                .with_max_in_flight(1),
        )
        .unwrap();
        let base = option_env!("QUAI_BROWSER_FIXTURE_URL").unwrap_or("http://127.0.0.1:18080");
        let provider = Provider::new(
            transport.clone(),
            Routing::direct(&format!("{base}/slow"), Zone::Cyprus1.into()).unwrap(),
            U256::from(15000),
        );
        assert!(matches!(
            wait_for_receipt(
                &provider,
                Zone::Cyprus1,
                TX.parse().unwrap(),
                config().with_timeout_ms(20)
            )
            .await,
            Err(BrowserReceiptWaitError::Timeout {
                polls_completed: 0,
                ..
            })
        ));
        assert_eq!(
            transport
                .request(
                    &Endpoint::parse(&format!("{base}/prefix/cyprus1?token=PUBLIC")).unwrap(),
                    "quai_chainId",
                    json!([])
                )
                .await
                .unwrap(),
            json!("0x3a98")
        );
    }
}
