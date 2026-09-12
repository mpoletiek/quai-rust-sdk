//! Block-pinned account source refuses latest-only Qi and detects changed canonical views.
#![cfg(all(feature = "wallet", not(target_arch = "wasm32")))]
use quai_sdk::discovery::AccountRpcSource;
use quai_sdk::rpc::{RpcError, Transport};
use quai_sdk::wallet::discovery::{
    DiscoveryError, HistoryCapability, NetworkScope, ObservationSource,
};
use quai_sdk::wallet::{CoinType, HdWallet, Language, Mnemonic, Search};
use quai_sdk::{Endpoint, Provider, Routing, U256, Zone};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
const GENESIS: &str = "0x663a73416275109a01aad3a4c29ea9e310aded63c5eea491243b7312ad8cd16b";
#[derive(Clone, Default)]
struct Mock {
    calls: Arc<Mutex<Vec<(String, Value)>>>,
    headers: Arc<AtomicUsize>,
    reorg_after: usize,
}
impl Transport for Mock {
    async fn request(&self, _: &Endpoint, method: &str, params: Value) -> Result<Value, RpcError> {
        self.calls
            .lock()
            .unwrap()
            .push((method.to_owned(), params.clone()));
        Ok(match method {
            "quai_chainId" => json!("0x3a98"),
            "quai_getHeaderByNumber" if params[0] == "0x0" => {
                json!({"woHeader":{"hash":GENESIS,"number":"0x0","location":"0x","parentHash":format!("0x{}","00".repeat(32))}})
            }
            "quai_getHeaderByNumber" => {
                let count = self.headers.fetch_add(1, Ordering::Relaxed);
                let byte = if self.reorg_after > 0 && count >= self.reorg_after {
                    "22"
                } else {
                    "11"
                };
                json!({"woHeader":{"hash":format!("0x{}",byte.repeat(32)),"parentHash":format!("0x{}","10".repeat(32)),"number":"0x64","primeTerminusNumber":"0x32","location":"0x0000"},"gasLimit":"0x100000","stateLimit":"0x100000"})
            }
            "quai_getTransactionCount" => json!("0x5"),
            "quai_getBalance" => json!("0x100"),
            _ => panic!("unexpected request"),
        })
    }
}
fn scope() -> NetworkScope {
    NetworkScope {
        chain_id: U256::from(15000),
        genesis: GENESIS.parse().unwrap(),
        zone: Zone::Cyprus1,
    }
}
fn address(coin: CoinType) -> quai_sdk::wallet::DerivedAddress {
    let mnemonic = Mnemonic::from_entropy(Language::English, &[0; 16]).unwrap();
    HdWallet::from_mnemonic(&mnemonic, "", coin)
        .unwrap()
        .search(
            0,
            false,
            Search {
                zone: Zone::Cyprus1,
                start_index: 0,
                max_attempts: 4096,
            },
            || false,
        )
        .unwrap()
        .address
}
fn provider(mock: Mock) -> Provider<Mock> {
    Provider::new(
        mock,
        Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
        scope().chain_id,
    )
}

#[tokio::test]
async fn account_observations_use_explicit_height_and_do_not_claim_history() {
    let mock = Mock::default();
    let provider = provider(mock.clone());
    let source = AccountRpcSource::new(&provider);
    assert_eq!(
        source.history_capability(scope(), CoinType::Quai),
        HistoryCapability::CurrentStateOnly
    );
    let checkpoint = source.tip(scope()).await.unwrap().checkpoint;
    let observed = source
        .observe(scope(), &address(CoinType::Quai), checkpoint)
        .await
        .unwrap();
    assert_eq!(observed.account_balance, Some(U256::from(256)));
    assert_eq!(observed.account_nonce, Some(5));
    assert_eq!(observed.ever_used, None);
    for (method, params) in mock.calls.lock().unwrap().iter() {
        if method == "quai_getBalance" || method == "quai_getTransactionCount" {
            assert_eq!(params[1], "0x64");
        }
    }
}
#[tokio::test]
async fn reorg_during_account_reads_and_latest_only_qi_fail_explicitly() {
    let mock = Mock {
        reorg_after: 2,
        ..Default::default()
    };
    let provider = provider(mock.clone());
    let source = AccountRpcSource::new(&provider);
    let checkpoint = source.tip(scope()).await.unwrap().checkpoint;
    assert_eq!(
        source
            .observe(scope(), &address(CoinType::Quai), checkpoint)
            .await
            .unwrap_err(),
        DiscoveryError::InvalidObservation
    );
    mock.calls.lock().unwrap().clear();
    assert_eq!(
        source
            .observe(scope(), &address(CoinType::Qi), checkpoint)
            .await
            .unwrap_err(),
        DiscoveryError::SourceUnavailable
    );
    assert!(mock.calls.lock().unwrap().is_empty());
}
