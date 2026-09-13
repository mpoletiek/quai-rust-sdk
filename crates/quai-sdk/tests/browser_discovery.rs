//! Actual worker Fetch composition of the facade's bounded account discovery source.
#![cfg(all(target_arch = "wasm32", feature = "wallet", feature = "browser"))]
use quai_sdk::browser::{BrowserConfig, BrowserFetchTransport};
use quai_sdk::discovery::AccountRpcSource;
use quai_sdk::wallet::discovery::{
    CanonicalStatus, DiscoveryError, DiscoveryRequest, HistoryCapability, IndexRange, NetworkScope,
    ObservationSource, discover,
};
use quai_sdk::wallet::{CoinType, HdWallet, Search};
use quai_sdk::{Provider, Routing, U256, Zone};
use wasm_bindgen_test::*;
wasm_bindgen_test_configure!(run_in_dedicated_worker);
fn scope() -> NetworkScope {
    NetworkScope {
        chain_id: U256::from(15000),
        genesis: "0x663a73416275109a01aad3a4c29ea9e310aded63c5eea491243b7312ad8cd16b"
            .parse()
            .unwrap(),
        zone: Zone::Cyprus1,
    }
}
fn provider(path: &str) -> Provider<BrowserFetchTransport> {
    Provider::new(
        BrowserFetchTransport::new(BrowserConfig::default()).unwrap(),
        Routing::direct(
            &format!(
                "{}{}",
                option_env!("QUAI_BROWSER_FIXTURE_URL").unwrap_or("http://127.0.0.1:18080"),
                path
            ),
            Zone::Cyprus1.into(),
        )
        .unwrap(),
        scope().chain_id,
    )
}
#[wasm_bindgen_test(async)]
async fn worker_discovers_numbered_account_state_without_native_runtime_or_signer() {
    let provider = provider("/account");
    let source = AccountRpcSource::new(&provider);
    let wallet = HdWallet::from_seed(&[1; 16], CoinType::Quai).unwrap();
    let account = wallet.account_public(0).unwrap();
    let found = account
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
    let mut request = DiscoveryRequest {
        scope: scope(),
        receive: IndexRange {
            start: found.index,
            end: found.index + 1,
        },
        change: IndexRange { start: 0, end: 0 },
        gap_limit: Some(50),
        require_history: false,
        max_addresses: 1,
        max_coins: 1,
    };
    let report = discover(&source, &account, &request, || false)
        .await
        .unwrap();
    assert_eq!(report.canonical, CanonicalStatus::Matches);
    assert_eq!(report.history, HistoryCapability::CurrentStateOnly);
    assert_eq!(report.addresses.len(), 1);
    let observed = &report.addresses[0].observation;
    assert_eq!(observed.account_balance, Some(U256::from(256)));
    assert_eq!(observed.account_nonce, Some(5));
    assert_eq!(observed.ever_used, None);
    request.require_history = true;
    assert_eq!(
        discover(&source, &account, &request, || false)
            .await
            .unwrap_err(),
        DiscoveryError::HistoryUnavailable
    );
    assert_eq!(
        discover(&source, &account, &request, || true)
            .await
            .unwrap_err(),
        DiscoveryError::Cancelled
    );
}
#[wasm_bindgen_test(async)]
async fn worker_account_observation_rejects_a_changed_checkpoint() {
    let provider = provider("/account-reorg");
    let source = AccountRpcSource::new(&provider);
    let wallet = HdWallet::from_seed(&[1; 16], CoinType::Quai).unwrap();
    let found = wallet
        .search(
            0,
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
    let checkpoint = source.tip(scope()).await.unwrap().checkpoint;
    assert_eq!(
        source
            .observe(scope(), &found, checkpoint)
            .await
            .unwrap_err(),
        DiscoveryError::InvalidObservation
    );
}

#[wasm_bindgen_test(async)]
async fn worker_qi_gap_scan_returns_fixed_denominations_and_reported_locks() {
    use quai_sdk::discovery::{QiDiscoveryOptions, discover_qi};
    use quai_sdk::wallet::discovery::ScanStop;
    let provider = provider("/qi");
    let account = HdWallet::from_seed(&[1; 16], CoinType::Qi)
        .unwrap()
        .account_public(0)
        .unwrap();
    let report = discover_qi(
        &provider,
        scope(),
        &account,
        &QiDiscoveryOptions {
            gap_limit: Some(2),
            max_addresses: 16,
            max_outpoints: 8,
            ..Default::default()
        },
        || false,
    )
    .await
    .unwrap();
    assert_eq!(report.canonical, CanonicalStatus::Matches);
    assert_eq!(report.stopped, [ScanStop::GapLimit; 2]);
    assert_eq!(report.addresses.len(), 5); // One funded receive + two empty on each branch.
    assert_eq!(
        report.addresses.iter().filter(|a| a.derived.change).count(),
        2
    );
    assert_eq!(report.addresses[0].outputs[0].denomination.value(), 10);
    let balance = report.balance_at(U256::from(100)).unwrap();
    assert_eq!(balance.total, U256::from(10));
    assert_eq!(balance.locked, U256::from(10));
    assert_eq!(balance.unlocked, U256::ZERO);
    assert_eq!(
        report.balance_at(U256::from(101)).unwrap().unlocked,
        U256::from(10)
    );
}

#[wasm_bindgen_test(async)]
async fn worker_qi_use_hint_accepts_thread_local_async_state() {
    use quai_sdk::discovery::{QiDiscoveryOptions, discover_qi_with_use_checker};
    use quai_sdk::wallet::discovery::ScanStop;
    use std::{cell::Cell, rc::Rc};
    let account = HdWallet::from_seed(&[1; 16], CoinType::Qi)
        .unwrap()
        .account_public(0)
        .unwrap();
    let calls = Rc::new(Cell::new(0));
    let callback_calls = calls.clone();
    let report = discover_qi_with_use_checker(
        &provider("/qi-hints"),
        scope(),
        &account,
        &QiDiscoveryOptions {
            gap_limit: Some(1),
            max_addresses: 4,
            max_outpoints: 1,
            ..Default::default()
        },
        || false,
        move |actual_scope, _| {
            let calls = callback_calls.clone();
            async move {
                assert_eq!(actual_scope, scope());
                calls.set(calls.get() + 1);
                Ok(calls.get() == 1)
            }
        },
    )
    .await
    .unwrap();
    assert_eq!(calls.get(), 3);
    assert_eq!(report.addresses.len(), 3);
    assert!(report.addresses[0].use_hint);
    assert_eq!(report.stopped, [ScanStop::GapLimit; 2]);
    assert_eq!(
        report.balance_at(U256::from(100)).unwrap().total,
        U256::ZERO
    );
}

#[wasm_bindgen_test]
fn worker_signs_and_verifies_qi_message_with_browser_auxiliary_entropy() {
    use quai_sdk::crypto::SecretKey;
    use quai_sdk::signer::{LocalSigner, Signer, verify_qi_message};
    let mut scalar = [0; 32];
    scalar[31] = 130;
    let signer =
        LocalSigner::new(SecretKey::from_bytes(&scalar).unwrap(), U256::from(15000)).unwrap();
    let message = "Qi worker: café 🐬".as_bytes();
    let signature = signer.sign_qi_message(message).unwrap();
    verify_qi_message(
        signer.address().try_into().unwrap(),
        &signer.public_key(),
        message,
        &signature,
    )
    .unwrap();
    assert!(
        verify_qi_message(
            signer.address().try_into().unwrap(),
            &signer.public_key(),
            b"changed",
            &signature
        )
        .is_err()
    );
}
