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
    switch_chain_after_output: bool,
    outputs: Arc<Mutex<std::collections::BTreeMap<String, Value>>>,
}
impl Transport for Mock {
    async fn request(&self, _: &Endpoint, method: &str, params: Value) -> Result<Value, RpcError> {
        self.calls
            .lock()
            .unwrap()
            .push((method.to_owned(), params.clone()));
        Ok(match method {
            "quai_chainId" => json!(if self.switch_chain_after_output
                && self
                    .calls
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|(m, _)| m == "quai_getOutpointsByAddress")
            {
                "0x9"
            } else {
                "0x3a98"
            }),
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
            "quai_getOutpointsByAddress" => self
                .outputs
                .lock()
                .unwrap()
                .get(params[0].as_str().unwrap())
                .cloned()
                .unwrap_or(json!([])),
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

#[tokio::test]
async fn portable_qi_default_gap_counts_fifty_matching_addresses_on_each_branch() {
    use quai_sdk::discovery::{QiDiscoveryOptions, discover_qi};
    use quai_sdk::wallet::discovery::{CanonicalStatus, ScanStop};
    let mock = Mock::default();
    let provider = provider(mock.clone());
    let account = HdWallet::from_seed(&[7; 32], CoinType::Qi)
        .unwrap()
        .account_public(0)
        .unwrap();
    let report = discover_qi(
        &provider,
        scope(),
        &account,
        &QiDiscoveryOptions::default(),
        || false,
    )
    .await
    .unwrap();
    assert_eq!(report.addresses.len(), 100);
    assert_eq!(
        report.addresses.iter().filter(|a| a.derived.change).count(),
        50
    );
    assert_eq!(report.stopped, [ScanStop::GapLimit; 2]);
    assert!(report.next_index.iter().all(|i| *i > 50));
    assert_eq!(report.canonical, CanonicalStatus::Matches);
    assert_eq!(
        report.balance_at(U256::from(100)).unwrap().total,
        U256::ZERO
    );
    assert_eq!(
        mock.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, _)| m == "quai_getOutpointsByAddress")
            .count(),
        100
    );
}
#[tokio::test]
async fn portable_qi_deep_scan_preserves_fixed_values_locks_and_cancellation_progress() {
    use quai_sdk::discovery::{QiDiscoveryOptions, discover_qi};
    use quai_sdk::wallet::discovery::{CanonicalStatus, IndexRange, ScanStop};
    let mock = Mock::default();
    let provider = provider(mock.clone());
    let account = HdWallet::from_seed(&[7; 32], CoinType::Qi)
        .unwrap()
        .account_public(0)
        .unwrap();
    let search = |start| {
        account
            .search(
                false,
                Search {
                    zone: Zone::Cyprus1,
                    start_index: start,
                    max_attempts: 10000,
                },
                || false,
            )
            .unwrap()
            .address
    };
    let first = search(0);
    let second = search(first.index + 1);
    let third = search(second.index + 1);
    mock.outputs.lock().unwrap().insert(first.address.to_string(),json!([{"txHash":format!("0x00000080{}","00".repeat(28)),"index":"0x0","denomination":"0x2","lock":"0x65","unknown":"not retained"}]));
    let options = QiDiscoveryOptions {
        receive: IndexRange {
            start: first.index,
            end: third.index + 1,
        },
        change: IndexRange { start: 0, end: 0 },
        gap_limit: None,
        max_addresses: 3,
        max_outpoints: 1,
    };
    let report = discover_qi(&provider, scope(), &account, &options, || false)
        .await
        .unwrap();
    assert_eq!(report.addresses.len(), 3);
    assert_eq!(report.stopped, [ScanStop::RangeEnd; 2]);
    let balance = report.balance_at(U256::from(100)).unwrap();
    assert_eq!(
        (balance.total, balance.locked, balance.unlocked),
        (U256::from(10), U256::from(10), U256::ZERO)
    );
    assert_eq!(
        report.balance_at(U256::from(101)).unwrap().unlocked,
        U256::from(10)
    );
    assert!(report.balance_at(U256::from(99)).is_err());
    mock.calls.lock().unwrap().clear();
    let partial = discover_qi(&provider, scope(), &account, &options, || {
        mock.calls
            .lock()
            .unwrap()
            .iter()
            .any(|(m, _)| m == "quai_getOutpointsByAddress")
    })
    .await
    .unwrap();
    assert_eq!(partial.addresses.len(), 1);
    assert_eq!(partial.next_index[0], first.index + 1);
    assert_eq!(partial.canonical, CanonicalStatus::NotChecked);
    assert!(partial.balance_at(U256::from(101)).is_err());
}
#[tokio::test]
async fn portable_qi_rejects_duplicate_outputs_limits_and_changed_heads() {
    use quai_sdk::discovery::{QiDiscoveryError, QiDiscoveryOptions, discover_qi};
    use quai_sdk::wallet::discovery::{CanonicalStatus, IndexRange};
    let account = HdWallet::from_seed(&[7; 32], CoinType::Qi)
        .unwrap()
        .account_public(0)
        .unwrap();
    let first = account
        .search(
            false,
            Search {
                zone: Zone::Cyprus1,
                start_index: 0,
                max_attempts: 10000,
            },
            || false,
        )
        .unwrap()
        .address;
    let second = account
        .search(
            false,
            Search {
                zone: Zone::Cyprus1,
                start_index: first.index + 1,
                max_attempts: 10000,
            },
            || false,
        )
        .unwrap()
        .address;
    let options = QiDiscoveryOptions {
        receive: IndexRange {
            start: first.index,
            end: second.index + 1,
        },
        change: IndexRange { start: 0, end: 0 },
        gap_limit: None,
        max_addresses: 2,
        max_outpoints: 2,
    };
    let output = json!([{"txHash":format!("0x00000080{}","00".repeat(28)),"index":"0x0","denomination":"0x2","lock":"0x0"}]);
    let mock = Mock::default();
    mock.outputs
        .lock()
        .unwrap()
        .insert(first.address.to_string(), output.clone());
    mock.outputs
        .lock()
        .unwrap()
        .insert(second.address.to_string(), output);
    let source = provider(mock.clone());
    assert!(matches!(
        discover_qi(&source, scope(), &account, &options, || false).await,
        Err(QiDiscoveryError::InvalidOutputs)
    ));
    let limited = QiDiscoveryOptions {
        max_outpoints: 1,
        ..options.clone()
    };
    assert!(matches!(
        discover_qi(&source, scope(), &account, &limited, || false).await,
        Err(QiDiscoveryError::OutputLimit)
    ));
    let bad = QiDiscoveryOptions {
        max_addresses: 0,
        ..options.clone()
    };
    mock.calls.lock().unwrap().clear();
    assert!(matches!(
        discover_qi(&source, scope(), &account, &bad, || false).await,
        Err(QiDiscoveryError::InvalidRequest)
    ));
    assert!(mock.calls.lock().unwrap().is_empty());
    let switched = provider(Mock {
        switch_chain_after_output: true,
        ..Default::default()
    });
    assert!(matches!(
        discover_qi(&switched, scope(), &account, &options, || false).await,
        Err(QiDiscoveryError::Provider(
            quai_sdk::provider::ProviderError::ChainMismatch { .. }
        ))
    ));
    let changed = provider(Mock {
        reorg_after: 1,
        ..Default::default()
    });
    let report = discover_qi(&changed, scope(), &account, &options, || false)
        .await
        .unwrap();
    assert_eq!(report.canonical, CanonicalStatus::Changed);
    assert!(report.balance_at(U256::from(101)).is_err());
}

#[tokio::test]
async fn optional_qi_use_checker_matches_pinned_short_circuit_and_error_behavior() {
    use quai_sdk::discovery::{
        QiDiscoveryError, QiDiscoveryOptions, discover_qi, discover_qi_with_use_checker,
    };
    use quai_sdk::wallet::discovery::IndexRange;
    let fixture: Value = serde_json::from_str(include_str!(
        "fixtures/shared/compatibility/fixtures/qi-use-hints.json"
    ))
    .unwrap();
    let account = HdWallet::from_seed(&[7; 32], CoinType::Qi)
        .unwrap()
        .account_public(0)
        .unwrap();
    let first = account
        .search(
            false,
            Search {
                zone: Zone::Cyprus1,
                start_index: 0,
                max_attempts: 10000,
            },
            || false,
        )
        .unwrap()
        .address;
    let options = QiDiscoveryOptions {
        receive: IndexRange {
            start: first.index,
            end: first.index + 1,
        },
        change: IndexRange { start: 0, end: 0 },
        gap_limit: Some(1),
        max_addresses: 1,
        max_outpoints: 1,
    };
    for case in fixture["vectors"].as_array().unwrap() {
        let mock = Mock::default();
        if case["outputs"] == 1 {
            mock.outputs.lock().unwrap().insert(first.address.to_string(),json!([{"txHash":format!("0x00000080{}","00".repeat(28)),"index":"0x0","denomination":"0x2","lock":"0x0"}]));
        }
        let source = provider(mock);
        let calls = AtomicUsize::new(0);
        let result = if case["checker"].is_null() {
            discover_qi(&source, scope(), &account, &options, || false).await
        } else {
            discover_qi_with_use_checker(
                &source,
                scope(),
                &account,
                &options,
                || false,
                |network, address| {
                    assert_eq!(network, scope());
                    assert_eq!(address.address(), first.address);
                    calls.fetch_add(1, Ordering::SeqCst);
                    std::future::ready(if case["checker"] == "error" {
                        Err(QiDiscoveryError::UseCheckFailed)
                    } else {
                        Ok(case["checker"] == true)
                    })
                },
            )
            .await
        };
        assert_eq!(
            calls.load(Ordering::SeqCst),
            case["calls"].as_u64().unwrap() as usize
        );
        if case["error"] == true {
            assert!(matches!(result, Err(QiDiscoveryError::UseCheckFailed)));
        } else {
            let report = result.unwrap();
            let address = &report.addresses[0];
            assert_eq!(
                !address.outputs.is_empty() || address.use_hint,
                case["used"].as_bool().unwrap()
            );
            assert_eq!(
                address.outputs.len(),
                case["outputs"].as_u64().unwrap() as usize
            );
        }
    }
}
#[tokio::test]
async fn known_spent_address_hint_prevents_early_gap_stop_without_creating_coins() {
    use quai_sdk::discovery::{QiDiscoveryOptions, discover_qi_with_use_checker};
    use quai_sdk::wallet::discovery::{IndexRange, ScanStop};
    let account = HdWallet::from_seed(&[7; 32], CoinType::Qi)
        .unwrap()
        .account_public(0)
        .unwrap();
    let first = account
        .search(
            false,
            Search {
                zone: Zone::Cyprus1,
                start_index: 0,
                max_attempts: 10000,
            },
            || false,
        )
        .unwrap()
        .address;
    let second = account
        .search(
            false,
            Search {
                zone: Zone::Cyprus1,
                start_index: first.index + 1,
                max_attempts: 10000,
            },
            || false,
        )
        .unwrap()
        .address;
    let options = QiDiscoveryOptions {
        receive: IndexRange {
            start: first.index,
            end: second.index + 1,
        },
        change: IndexRange { start: 0, end: 0 },
        gap_limit: Some(1),
        max_addresses: 2,
        max_outpoints: 1,
    };
    let source = provider(Mock::default());
    let report = discover_qi_with_use_checker(
        &source,
        scope(),
        &account,
        &options,
        || false,
        |_, address| std::future::ready(Ok(address.address() == first.address)),
    )
    .await
    .unwrap();
    assert_eq!(report.addresses.len(), 2);
    assert!(report.addresses[0].use_hint);
    assert!(!report.addresses[1].use_hint);
    assert_eq!(report.stopped[0], ScanStop::GapLimit);
    assert_eq!(
        report.balance_at(U256::from(100)).unwrap().total,
        U256::ZERO
    );
}

#[tokio::test]
async fn one_observation_establishes_identity_once_and_keeps_the_reorg_bracket() {
    // `observe` used to call `identity` three times: once directly and once
    // inside each bracketing canonical read. Each of those re-read
    // `quai_getHeaderByNumber ["0x0"]`, the height-zero header, which is
    // immutable for the life of the chain.
    let mock = Mock::default();
    let provider = provider(mock.clone());
    let source = AccountRpcSource::new(&provider);
    let checkpoint = source.tip(scope()).await.unwrap().checkpoint;
    mock.calls.lock().unwrap().clear();

    source
        .observe(scope(), &address(CoinType::Quai), checkpoint)
        .await
        .unwrap();

    let calls = mock.calls.lock().unwrap().clone();
    let genesis = calls
        .iter()
        .filter(|(m, p)| m == "quai_getHeaderByNumber" && p[0] == "0x0")
        .count();
    let pinned = calls
        .iter()
        .filter(|(m, p)| m == "quai_getHeaderByNumber" && p[0] != "0x0")
        .count();
    let chain = calls.iter().filter(|(m, _)| m == "quai_chainId").count();

    // Identity is established once per observation, not once per inner read.
    assert_eq!(genesis, 1, "genesis is immutable; read it once: {calls:?}");
    // The reorg bracket is the property that matters and is unchanged: the
    // pinned height is read before and after the balance and nonce reads.
    assert_eq!(pinned, 2, "the before/after bracket must remain: {calls:?}");
    // Every read still carries its own chain guard, so dropping the repeated
    // identity reads does not drop a chain check.
    assert!(chain >= 4, "each read stays chain-guarded: {calls:?}");
}
