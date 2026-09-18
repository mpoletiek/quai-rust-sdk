//! Same immutable transaction-signing contract in window and dedicated worker.
use quai_browser::{BrowserConfig, BrowserError, InjectedProvider};
use quai_consensus::{AccessTuple, QuaiTransaction};
use quai_crypto::SecretKey;
use quai_primitives::{Hash32, QuaiAddress, Zone};
use quai_rpc::{Endpoint, RpcError, U256};
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;
use wasm_bindgen_test::*;
#[wasm_bindgen(module = "/tests/fixture.js")]
extern "C" {
    #[wasm_bindgen(js_name=fixtureProvider)]
    fn fixture_provider(mode: &str) -> JsValue;
    #[wasm_bindgen(js_name=setSignature)]
    fn set_signature(provider: &JsValue, signature: &str);
    #[wasm_bindgen(js_name=callCount)]
    fn call_count(provider: &JsValue) -> usize;
    #[wasm_bindgen(js_name=callJson)]
    fn call_json(provider: &JsValue, index: usize) -> String;
    #[wasm_bindgen(js_name=setTransaction)]
    fn set_transaction(provider: &JsValue, value: &str);
    #[wasm_bindgen(js_name=finishProvider)]
    fn finish_provider(provider: &JsValue);
}
fn key(scalar: u16) -> SecretKey {
    let mut b = [0; 32];
    b[30..].copy_from_slice(&scalar.to_be_bytes());
    SecretKey::from_bytes(&b).unwrap()
}
fn address() -> QuaiAddress {
    QuaiAddress::try_from(key(805).public_key().address()).unwrap()
}
fn tx() -> QuaiTransaction {
    QuaiTransaction {
        chain_id: U256::from(15000),
        nonce: 7,
        to: Some(address().address()),
        value: U256::from(9007199254740993u64),
        gas_limit: 25000,
        gas_price: U256::from(123456789),
        data: vec![0, 1, 2],
        access_list: vec![],
    }
}
fn setup(mode: &str, config: BrowserConfig) -> (InjectedProvider, JsValue) {
    let value = fixture_provider(mode);
    (
        InjectedProvider::new(
            value.clone(),
            Endpoint::parse("https://wallet.invalid/cyprus1").unwrap(),
            Zone::Cyprus1.into(),
            U256::from(15000),
            config,
        )
        .unwrap(),
        value,
    )
}
fn install(wallet: &JsValue, tx: &QuaiTransaction) {
    let bytes = tx.sign(&key(805)).unwrap().signed_bytes().unwrap();
    set_signature(wallet, &quai_primitives::hexlify(&bytes).unwrap());
}
fn requests(wallet: &JsValue) -> Vec<Value> {
    (0..call_count(wallet))
        .map(|i| serde_json::from_str(&call_json(wallet, i)).unwrap())
        .collect()
}
#[wasm_bindgen_test(async)]
async fn injected_transaction_requests_match_pinned_js_and_exact_signed_protobuf() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../fixtures/shared/compatibility/fixtures/browser-requests.json"
    ))
    .unwrap();
    for v in fixture["vectors"].as_array().unwrap() {
        let mut tx = tx();
        match v["name"].as_str().unwrap() {
            "creation" => {
                tx.to = None;
                tx.data = vec![0x60, 0, 0x60, 0];
            }
            "access_list" => {
                tx.access_list = vec![
                    AccessTuple {
                        address: address().address(),
                        storage_keys: vec![Hash32::from_bytes([1; 32]); 2],
                    },
                    AccessTuple {
                        address: address().address(),
                        storage_keys: vec![],
                    },
                ];
            }
            _ => {}
        }
        let (provider, wallet) = setup("normal", BrowserConfig::default());
        install(&wallet, &tx);
        let signed = provider
            .sign_quai_transaction(address(), &tx)
            .await
            .unwrap();
        assert_eq!(signed.transaction(), &tx);
        assert_eq!(signed.from(), address());
        let calls = requests(&wallet);
        let sign: Vec<_> = calls
            .iter()
            .filter(|r| r["method"] == "quai_signTransaction")
            .collect();
        assert_eq!(sign.len(), 1);
        assert_eq!(sign[0]["params"], json!([v["request"]]));
        assert_eq!(sign[0]["shard"], "0x00");
        assert!(!calls.iter().any(|r| r["method"] == "quai_requestAccounts"
            || r["method"] == "quai_sendTransaction"
            || r["method"] == "quai_sendRawTransaction"));
    }
}
#[wasm_bindgen_test(async)]
async fn injected_transaction_rejects_every_changed_unsigned_field_and_wrong_sender() {
    let (provider, wallet) = setup("normal", BrowserConfig::default());
    let original = tx();
    for changed in 0..8 {
        let mut returned = original.clone();
        match changed {
            0 => returned.chain_id = U256::from(9),
            1 => returned.nonce += 1,
            2 => returned.to = None,
            3 => returned.value += U256::from(1),
            4 => returned.gas_limit += 1,
            5 => returned.gas_price += U256::from(1),
            6 => returned.data.push(0),
            _ => returned.access_list.push(AccessTuple {
                address: address().address(),
                storage_keys: vec![],
            }),
        }
        install(&wallet, &returned);
        assert!(matches!(
            provider.sign_quai_transaction(address(), &original).await,
            Err(BrowserError::InvalidResult)
        ));
    }
    let other = (806..5000)
        .map(key)
        .find(|k| QuaiAddress::try_from(k.public_key().address()).is_ok())
        .unwrap();
    set_signature(
        &wallet,
        &quai_primitives::hexlify(&original.sign(&other).unwrap().signed_bytes().unwrap()).unwrap(),
    );
    assert!(matches!(
        provider.sign_quai_transaction(address(), &original).await,
        Err(BrowserError::InvalidResult)
    ));
    let mut ordered = original.clone();
    ordered.access_list = vec![
        AccessTuple {
            address: address().address(),
            storage_keys: vec![Hash32::from_bytes([1; 32]), Hash32::from_bytes([2; 32])],
        },
        AccessTuple {
            address: address().address(),
            storage_keys: vec![],
        },
    ];
    let mut reordered = ordered.clone();
    reordered.access_list.reverse();
    install(&wallet, &reordered);
    assert!(
        provider
            .sign_quai_transaction(address(), &ordered)
            .await
            .is_err()
    );
    reordered = ordered.clone();
    reordered.access_list[0].storage_keys.reverse();
    install(&wallet, &reordered);
    assert!(
        provider
            .sign_quai_transaction(address(), &ordered)
            .await
            .is_err()
    );
}
#[wasm_bindgen_test(async)]
async fn injected_transaction_preflight_permissions_and_context_fail_without_retry() {
    let (provider, wallet) = setup("normal", BrowserConfig::default());
    let mut wrong = tx();
    wrong.chain_id = U256::from(9);
    assert!(matches!(
        provider.sign_quai_transaction(address(), &wrong).await,
        Err(BrowserError::ChainMismatch)
    ));
    assert_eq!(call_count(&wallet), 0);
    let (provider, wallet) = setup(
        "normal",
        BrowserConfig::default().with_max_request_bytes(128),
    );
    assert!(matches!(
        provider.sign_quai_transaction(address(), &tx()).await,
        Err(BrowserError::Rpc(RpcError::RequestTooLarge))
    ));
    assert_eq!(call_count(&wallet), 0);
    for (mode, code) in [("unsupported_sign", 4200), ("deny_sign", 4001)] {
        let (provider, wallet) = setup(mode, BrowserConfig::default());
        let error = provider
            .sign_quai_transaction(address(), &tx())
            .await
            .unwrap_err();
        assert!(matches!(error,BrowserError::Provider(c) if c==code));
        assert!(!error.to_string().contains("SECRET"));
        assert_eq!(
            requests(&wallet)
                .iter()
                .filter(|r| r["method"] == "quai_signTransaction")
                .count(),
            1
        );
    }
    let (provider, wallet) = setup("unavailable", BrowserConfig::default());
    assert!(matches!(
        provider.sign_quai_transaction(address(), &tx()).await,
        Err(BrowserError::AccountUnavailable)
    ));
    assert!(
        !requests(&wallet)
            .iter()
            .any(|r| r["method"] == "quai_signTransaction")
    );
    let (provider, wallet) = setup("change_during_sign", BrowserConfig::default());
    install(&wallet, &tx());
    assert!(matches!(
        provider.sign_quai_transaction(address(), &tx()).await,
        Err(BrowserError::ContextChanged)
    ));
}
#[wasm_bindgen_test(async)]
async fn injected_transaction_malformed_outputs_are_not_signed_transactions() {
    let (provider, wallet) = setup("normal", BrowserConfig::default());
    let tx = tx();
    for raw in [
        "0x".to_owned(),
        "0xéé".to_owned(),
        "0x00".to_owned(),
        quai_primitives::hexlify(&tx.unsigned_bytes().unwrap()).unwrap(),
    ] {
        set_signature(&wallet, &raw);
        assert!(matches!(
            provider.sign_quai_transaction(address(), &tx).await,
            Err(BrowserError::InvalidResult)
        ));
    }
    install(&wallet, &tx);
    assert!(provider.sign_quai_transaction(address(), &tx).await.is_ok());
}
#[wasm_bindgen_test(async)]
async fn injected_transaction_timeout_and_cancellation_release_local_capacity() {
    let (provider, wallet) = setup(
        "hang_sign",
        BrowserConfig::default()
            .with_request_timeout_ms(30)
            .with_max_in_flight(1),
    );
    let tx = tx();
    assert!(matches!(
        provider.sign_quai_transaction(address(), &tx).await,
        Err(BrowserError::Rpc(RpcError::Timeout))
    ));
    let mut pending = Box::pin(provider.sign_quai_transaction(address(), &tx));
    // Drive the passive chain/account checks until the signing call is pending.
    for _ in 0..64 {
        assert!(futures_util::poll!(&mut pending).is_pending());
        wasm_bindgen_futures::JsFuture::from(js_sys::Promise::resolve(&JsValue::NULL))
            .await
            .unwrap();
        if requests(&wallet)
            .iter()
            .filter(|r| r["method"] == "quai_signTransaction")
            .count()
            == 2
        {
            break;
        }
    }
    assert_eq!(
        requests(&wallet)
            .iter()
            .filter(|r| r["method"] == "quai_signTransaction")
            .count(),
        2
    );
    drop(pending);
    finish_provider(&wallet);
    assert!(provider.accounts().await.is_ok());
}

fn submit_provider(
    injected: &InjectedProvider,
) -> quai_provider::Provider<quai_browser::InjectedSubmissionTransport> {
    quai_provider::Provider::new(
        injected.signed_submission_transport(),
        quai_rpc::Routing::direct("https://wallet.invalid/cyprus1", Zone::Cyprus1.into()).unwrap(),
        U256::from(15000),
    )
}
#[wasm_bindgen_test(async)]
async fn injected_submission_is_explicit_and_preserves_quai_and_all_qi_operation_bytes() {
    use quai_rpc::Transport;
    let (injected, wallet) = setup("normal", BrowserConfig::default());
    let signed = tx().sign(&key(805)).unwrap();
    let raw = quai_primitives::hexlify(&signed.signed_bytes().unwrap()).unwrap();
    let endpoint = Endpoint::parse("https://wallet.invalid/cyprus1").unwrap();
    assert!(
        injected
            .request(&endpoint, "quai_sendRawTransaction", json!([raw]))
            .await
            .is_err()
    );
    assert_eq!(call_count(&wallet), 0);
    let transport = injected.signed_submission_transport();
    assert_eq!(call_count(&wallet), 0);
    for params in [
        json!([]),
        json!(["0x"]),
        json!(["0x", "0x"]),
        json!([quai_primitives::hexlify(&tx().unsigned_bytes().unwrap()).unwrap()]),
    ] {
        assert!(
            transport
                .request(&endpoint, "quai_sendRawTransaction", params)
                .await
                .is_err()
        );
    }
    assert_eq!(call_count(&wallet), 0);
    set_signature(&wallet, &signed.hash().unwrap().to_string());
    let provider = submit_provider(&injected);
    let sent = provider.broadcast(&signed).await.unwrap();
    assert_eq!(sent.transaction_hash, signed.hash().unwrap());
    let calls = requests(&wallet);
    assert_eq!(
        calls
            .iter()
            .find(|r| r["method"] == "quai_sendRawTransaction")
            .unwrap()["params"],
        json!([raw])
    );
    let fixture: Value = serde_json::from_str(include_str!(
        "../fixtures/shared/compatibility/fixtures/browser-requests.json"
    ))
    .unwrap();
    for v in fixture["submissions"].as_array().unwrap() {
        let bytes = quai_primitives::get_bytes(v["signed"].as_str().unwrap()).unwrap();
        let signed = quai_consensus::SignedQiOperation::decode(&bytes).unwrap();
        set_signature(&wallet, v["hash"].as_str().unwrap());
        let sent = match &signed {
            quai_consensus::SignedQiOperation::Transfer(tx) => provider.broadcast_qi(tx).await,
            quai_consensus::SignedQiOperation::Conversion(tx) => {
                provider.broadcast_qi_conversion(tx).await
            }
            quai_consensus::SignedQiOperation::Wrapping(tx) => {
                provider.broadcast_qi_wrapping(tx).await
            }
        }
        .unwrap();
        assert_eq!(sent.transaction_hash, signed.hash().unwrap());
        let calls = requests(&wallet);
        let last = calls
            .iter()
            .rev()
            .find(|r| r["method"] == "quai_sendRawTransaction")
            .unwrap();
        assert_eq!(last["params"], json!([v["signed"]]));
    }
    assert!(!requests(&wallet).iter().any(|r| matches!(
        r["method"].as_str(),
        Some("quai_requestAccounts" | "quai_signTransaction" | "quai_sendTransaction")
    )));
}
#[wasm_bindgen_test(async)]
async fn injected_submission_errors_retain_exact_id_and_never_retry() {
    let signed = tx().sign(&key(805)).unwrap();
    let hash = signed.hash().unwrap();
    for mode in [
        "reject_send",
        "wrong_ack",
        "change_during_send",
        "hang_send",
    ] {
        let (injected, wallet) = setup(mode, BrowserConfig::default().with_request_timeout_ms(100));
        set_signature(&wallet, &hash.to_string());
        let provider = submit_provider(&injected);
        let error = provider.broadcast(&signed).await.unwrap_err();
        assert!(error.acceptance_is_ambiguous());
        assert_eq!(error.transaction_hash(), Some(hash));
        assert!(!error.to_string().contains("SECRET"));
        assert_eq!(
            requests(&wallet)
                .iter()
                .filter(|r| r["method"] == "quai_sendRawTransaction")
                .count(),
            1
        );
    }
    let (injected, wallet) = setup("normal", BrowserConfig::default());
    let provider = submit_provider(&injected);
    let mut wrong = tx();
    wrong.chain_id = U256::from(9);
    let error = provider
        .broadcast(&wrong.sign(&key(805)).unwrap())
        .await
        .unwrap_err();
    assert!(!error.acceptance_is_ambiguous());
    assert_eq!(call_count(&wallet), 0);
}

fn transaction_rpc(signed: &quai_consensus::SignedQuaiTransaction) -> Value {
    let tx = signed.transaction();
    let compact = signed.signature().to_compact();
    json!({"type":"0x0","hash":signed.hash().unwrap().to_string(),"blockHash":null,"blockNumber":null,"transactionIndex":null,"from":signed.from().to_string(),"to":tx.to.map(|a|a.to_string()),"chainId":format!("{:#x}",tx.chain_id),"nonce":format!("{:#x}",tx.nonce),"gas":format!("{:#x}",tx.gas_limit),"gasPrice":format!("{:#x}",tx.gas_price),"value":format!("{:#x}",tx.value),"input":quai_primitives::hexlify(&tx.data).unwrap(),"accessList":tx.access_list.iter().map(|a|json!({"address":a.address.to_string(),"storageKeys":a.storage_keys.iter().map(ToString::to_string).collect::<Vec<_>>()})).collect::<Vec<_>>(),"v":format!("{:#x}",signed.signature().recovery_id()),"r":format!("{:#x}",U256::from_be_slice(&compact[..32])),"s":format!("{:#x}",U256::from_be_slice(&compact[32..]))})
}
fn read_provider(injected: &InjectedProvider) -> quai_provider::Provider<InjectedProvider> {
    quai_provider::Provider::new(
        injected.clone(),
        quai_rpc::Routing::direct("https://wallet.invalid/cyprus1", Zone::Cyprus1.into()).unwrap(),
        U256::from(15000),
    )
}
#[wasm_bindgen_test(async)]
async fn wallet_send_needs_no_offline_signing_and_verifies_observed_exact_transaction() {
    let (injected, wallet) = setup("unsupported_sign", BrowserConfig::default());
    let tx = tx();
    let signed = tx.sign(&key(805)).unwrap();
    set_signature(&wallet, &signed.hash().unwrap().to_string());
    let identity = quai_browser::WalletSendIdentity::new(address(), &tx).unwrap();
    let acknowledgement = injected
        .send_quai_transaction(address(), &tx)
        .await
        .unwrap();
    assert_eq!(acknowledgement.identity(), identity);
    assert_eq!(acknowledgement.reported_hash(), signed.hash().unwrap());
    assert_eq!(acknowledgement.requested_transaction(), &tx);
    let reported = acknowledgement.reported_hash();
    drop(acknowledgement);
    let acknowledgement = quai_browser::WalletSendAcknowledgement::from_reported_hash(
        address(),
        tx.clone(),
        reported,
    )
    .unwrap();
    let provider = read_provider(&injected);
    assert!(acknowledgement.observe(&provider).await.unwrap().is_none());
    set_transaction(&wallet, &transaction_rpc(&signed).to_string());
    let observed = acknowledgement.observe(&provider).await.unwrap().unwrap();
    assert!(observed.matches_request());
    assert_eq!(
        observed.signed().signed_bytes().unwrap(),
        signed.signed_bytes().unwrap()
    );
    assert!(observed.inclusion().is_none());
    let calls = requests(&wallet);
    assert_eq!(
        calls
            .iter()
            .filter(|r| r["method"] == "quai_sendTransaction")
            .count(),
        1
    );
    assert!(!calls.iter().any(|r| matches!(
        r["method"].as_str(),
        Some("quai_signTransaction" | "quai_sendRawTransaction" | "quai_requestAccounts")
    )));
    let fixture: Value = serde_json::from_str(include_str!(
        "../fixtures/shared/compatibility/fixtures/browser-requests.json"
    ))
    .unwrap();
    assert_eq!(
        calls
            .iter()
            .find(|r| r["method"] == "quai_sendTransaction")
            .unwrap()["params"],
        json!([fixture["vectors"][0]["request"]])
    );
}
#[wasm_bindgen_test(async)]
async fn wallet_send_reports_wallet_changes_and_rejects_forged_node_signature_fields() {
    let (injected, wallet) = setup("normal", BrowserConfig::default());
    let requested = tx();
    let provider = read_provider(&injected);
    for change in 0..7 {
        let mut actual = requested.clone();
        match change {
            0 => actual.nonce += 1,
            1 => actual.gas_limit += 1,
            2 => actual.gas_price += U256::from(1),
            3 => actual.value += U256::from(1),
            4 => actual.to = None,
            5 => actual.data.push(0),
            _ => actual.access_list.push(AccessTuple {
                address: address().address(),
                storage_keys: vec![],
            }),
        };
        let signed = actual.sign(&key(805)).unwrap();
        set_signature(&wallet, &signed.hash().unwrap().to_string());
        set_transaction(&wallet, &transaction_rpc(&signed).to_string());
        let acknowledgement = injected
            .send_quai_transaction(address(), &requested)
            .await
            .unwrap();
        let observed = acknowledgement.observe(&provider).await.unwrap().unwrap();
        assert!(!observed.matches_request());
        assert_eq!(observed.signed().transaction(), &actual);
    }
    let signed = requested.sign(&key(805)).unwrap();
    set_signature(&wallet, &signed.hash().unwrap().to_string());
    let acknowledgement = injected
        .send_quai_transaction(address(), &requested)
        .await
        .unwrap();
    let mut forged = transaction_rpc(&signed);
    forged["nonce"] = json!("0x8");
    set_transaction(&wallet, &forged.to_string());
    assert!(acknowledgement.observe(&provider).await.is_err());
}
#[wasm_bindgen_test(async)]
async fn wallet_send_distinguishes_preflight_and_ambiguous_outcomes_and_retains_reply_hash() {
    use quai_browser::WalletSendError;
    let transaction = tx();
    let identity = quai_browser::WalletSendIdentity::new(address(), &transaction).unwrap();
    let hash = transaction.sign(&key(805)).unwrap().hash().unwrap();
    for mode in [
        "wallet_denied",
        "wallet_unsupported",
        "wallet_hang",
        "wallet_context_change",
    ] {
        let (injected, wallet) = setup(mode, BrowserConfig::default().with_request_timeout_ms(100));
        set_signature(&wallet, &hash.to_string());
        let error = injected
            .send_quai_transaction(address(), &transaction)
            .await
            .unwrap_err();
        assert!(error.acceptance_is_ambiguous());
        assert!(!error.to_string().contains("SECRET"));
        let WalletSendError::Ambiguous {
            identity: retained,
            reported_hash,
            ..
        } = error
        else {
            unreachable!()
        };
        assert_eq!(retained, identity);
        assert_eq!(
            reported_hash,
            if mode == "wallet_context_change" {
                Some(hash)
            } else {
                None
            }
        );
        assert_eq!(
            requests(&wallet)
                .iter()
                .filter(|r| r["method"] == "quai_sendTransaction")
                .count(),
            1
        );
    }
    let (injected, wallet) = setup("unavailable", BrowserConfig::default());
    let error = injected
        .send_quai_transaction(address(), &transaction)
        .await
        .unwrap_err();
    assert!(!error.acceptance_is_ambiguous());
    assert!(
        !requests(&wallet)
            .iter()
            .any(|r| r["method"] == "quai_sendTransaction")
    );
    let (injected, wallet) = setup("normal", BrowserConfig::default());
    let mut wrong = transaction.clone();
    wrong.chain_id = U256::from(9);
    assert!(
        !injected
            .send_quai_transaction(address(), &wrong)
            .await
            .unwrap_err()
            .acceptance_is_ambiguous()
    );
    assert_eq!(call_count(&wallet), 0);
}
#[wasm_bindgen_test(async)]
async fn wallet_send_rejects_malformed_and_wrong_ledger_or_zone_acknowledgements() {
    let (injected, wallet) = setup("normal", BrowserConfig::default());
    let transaction = tx();
    let mut wrong_zone = *transaction.sign(&key(805)).unwrap().hash().unwrap().bytes();
    wrong_zone[0] = 1;
    assert!(
        quai_browser::WalletSendAcknowledgement::from_reported_hash(
            address(),
            transaction.clone(),
            Hash32::from_bytes(wrong_zone),
        )
        .is_err()
    );
    let mut qi = wrong_zone;
    qi[0] = 0;
    qi[1] |= 0x80;
    for value in [
        "0x17".to_owned(),
        "not a hash".to_owned(),
        Hash32::ZERO.to_string(),
        Hash32::from_bytes(wrong_zone).to_string(),
        Hash32::from_bytes(qi).to_string(),
    ] {
        set_signature(&wallet, &value);
        assert!(
            injected
                .send_quai_transaction(address(), &transaction)
                .await
                .unwrap_err()
                .acceptance_is_ambiguous()
        );
    }
}
#[wasm_bindgen_test(async)]
async fn wallet_send_cancellation_retains_caller_identity_and_does_not_resubmit() {
    let (injected, wallet) = setup(
        "wallet_hang",
        BrowserConfig::default()
            .with_request_timeout_ms(100)
            .with_max_in_flight(1),
    );
    let transaction = tx();
    let identity = quai_browser::WalletSendIdentity::new(address(), &transaction).unwrap();
    let mut pending = Box::pin(injected.send_quai_transaction(address(), &transaction));
    for _ in 0..64 {
        assert!(futures_util::poll!(&mut pending).is_pending());
        wasm_bindgen_futures::JsFuture::from(js_sys::Promise::resolve(&JsValue::NULL))
            .await
            .unwrap();
        if requests(&wallet)
            .iter()
            .any(|r| r["method"] == "quai_sendTransaction")
        {
            break;
        }
    }
    assert_eq!(
        requests(&wallet)
            .iter()
            .filter(|r| r["method"] == "quai_sendTransaction")
            .count(),
        1
    );
    drop(pending);
    finish_provider(&wallet);
    assert!(injected.accounts().await.is_ok());
    assert_eq!(
        identity.signing_digest,
        transaction.signing_digest().unwrap()
    );
    assert_eq!(
        requests(&wallet)
            .iter()
            .filter(|r| r["method"] == "quai_sendTransaction")
            .count(),
        1
    );
}
