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
    let mut request = DiscoveryRequest::new(
        scope(),
        IndexRange {
            start: found.index,
            end: found.index + 1,
        },
        IndexRange { start: 0, end: 0 },
    )
    .with_gap_limit(Some(50))
    .with_require_history(false)
    .with_max_addresses(1)
    .with_max_coins(1);
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
    // A reorg across the reads is stale, not malformed: observe again.
    assert_eq!(
        source
            .observe(scope(), &found, checkpoint)
            .await
            .unwrap_err(),
        DiscoveryError::ObservationChanged
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
        &QiDiscoveryOptions::default()
            .with_gap_limit(Some(2))
            .with_max_addresses(16)
            .with_max_outpoints(8),
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
        &QiDiscoveryOptions::default()
            .with_gap_limit(Some(1))
            .with_max_addresses(4)
            .with_max_outpoints(1),
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

#[wasm_bindgen_test]
fn worker_generates_all_mnemonic_lengths_and_exports_guarded_entropy() {
    use quai_sdk::wallet::{Language, Mnemonic};
    for count in [12, 15, 18, 21, 24] {
        for language in [Language::English, Language::Japanese, Language::Spanish] {
            let mnemonic = Mnemonic::generate(language, count).unwrap();
            let entropy = mnemonic.entropy();
            assert_eq!(entropy.expose().len(), count / 3 * 4);
            assert_eq!(format!("{entropy:?}"), "MnemonicEntropy([REDACTED])");
            assert_eq!(format!("{mnemonic:?}"), "Mnemonic([REDACTED])");
            let restored = Mnemonic::from_entropy(language, entropy.expose()).unwrap();
            assert_eq!(mnemonic.phrase().expose(), restored.phrase().expose());
            assert_eq!(
                mnemonic.to_seed("public worker passphrase").expose(),
                restored.to_seed("public worker passphrase").expose()
            );
        }
    }
    assert!(Mnemonic::generate(Language::English, 13).is_err());
}

#[wasm_bindgen_test(async)]
async fn worker_restores_public_ancestry_from_indexeddb_and_revalidates_node() {
    use quai_sdk::browser::{BrowserSnapshotStore, BrowserStorageScope};
    use quai_sdk::primitives::Hash32;
    use quai_sdk::provider::{BlockReference, HeadTracker, MAX_HEAD_STATE_BYTES};
    let mut random = [0; 8];
    quai_sdk::browser::fill_random(&mut random).unwrap();
    let name = format!("quai-head-worker-{}", u64::from_be_bytes(random));
    let storage_scope = BrowserStorageScope {
        chain_id: scope().chain_id,
        genesis: scope().genesis,
        zone: scope().zone,
        wallet: Hash32::from_bytes([42; 32]),
    };
    let tracker = HeadTracker::new(
        scope().zone,
        scope().genesis,
        BlockReference {
            number: 100,
            hash: Hash32::from_bytes([0x11; 32]),
        },
        8,
        2,
    )
    .unwrap();
    let bytes = tracker.export_state();
    let store = BrowserSnapshotStore::open(&name, storage_scope, MAX_HEAD_STATE_BYTES)
        .await
        .unwrap();
    assert_eq!(store.compare_exchange(None, Some(&bytes)).await.unwrap(), 1);
    drop(store);
    let reopened = BrowserSnapshotStore::open(&name, storage_scope, MAX_HEAD_STATE_BYTES)
        .await
        .unwrap();
    let saved = reopened.read().await.unwrap().unwrap();
    let mut restored =
        HeadTracker::from_state(saved.bytes.as_ref().unwrap(), scope().zone, scope().genesis)
            .unwrap();
    let page = restored.poll(&provider("/account")).await.unwrap();
    assert!(page.caught_up && page.removed.is_empty() && page.added.is_empty());
    assert_eq!(restored.export_state(), bytes);
    assert_eq!(
        reopened
            .compare_exchange(Some(saved.revision), None)
            .await
            .unwrap(),
        2
    );
    assert!(
        reopened
            .compare_exchange(Some(saved.revision), Some(&bytes))
            .await
            .is_err()
    );
    assert!(reopened.read().await.unwrap().unwrap().bytes.is_none());
}

#[wasm_bindgen_test]
fn worker_text_hash_uuid_and_entropy_utilities_match_native_contracts() {
    use quai_sdk::primitives::{
        Utf8Normalization, hexlify, to_utf8_bytes, to_utf8_code_points, to_utf8_string, uuid_v4,
    };
    let normalized = to_utf8_bytes("e\u{301} ﬃ", Some(Utf8Normalization::Nfkc)).unwrap();
    assert_eq!(to_utf8_string(&normalized).unwrap(), "é ffi");
    assert_eq!(to_utf8_code_points("🍊", None).unwrap(), [0x1f34a]);
    assert!(to_utf8_string(&[0xed, 0xa0, 0x80]).is_err());
    assert_eq!(uuid_v4(&[0; 16]), "00000000-0000-4000-8000-000000000000");
    assert_eq!(
        hexlify(&quai_sdk::crypto::ripemd160(b"abc")).unwrap(),
        "0x8eb208f7e05d987a9b044a8e98c6b087f15a0bfc"
    );
    let mut random = [0; 32];
    quai_sdk::crypto::fill_random(&mut random).unwrap();
    assert!(random.iter().any(|b| *b != 0));
    let mut oversized = vec![17; quai_sdk::crypto::MAX_RANDOM_BYTES + 1];
    assert!(quai_sdk::crypto::fill_random(&mut oversized).is_err());
    assert!(oversized.iter().all(|b| *b == 17));
}
