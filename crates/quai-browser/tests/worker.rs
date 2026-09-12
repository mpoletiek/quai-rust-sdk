//! Dedicated-worker runtime smoke tests for browser transport and entropy boundaries.
#![cfg(target_arch = "wasm32")]
use quai_browser::{BrowserConfig, BrowserFetchTransport, fill_random};
use quai_rpc::{Endpoint, RpcError, Transport, U256};
use serde_json::json;
use wasm_bindgen_test::*;
wasm_bindgen_test_configure!(run_in_dedicated_worker);
fn endpoint(path: &str) -> Endpoint {
    Endpoint::parse(&format!(
        "{}{}",
        option_env!("QUAI_BROWSER_FIXTURE_URL").unwrap_or("http://127.0.0.1:18080"),
        path
    ))
    .unwrap()
}
#[wasm_bindgen_test(async)]
async fn worker_fetch_and_timer_require_no_window_or_tokio() {
    let transport = BrowserFetchTransport::new(BrowserConfig {
        request_timeout_ms: 100,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(
        transport
            .request(
                &endpoint("/prefix/cyprus1?token=PUBLIC"),
                "quai_chainId",
                json!([])
            )
            .await
            .unwrap(),
        json!("0x3a98")
    );
    assert!(matches!(
        transport
            .request(&endpoint("/slow"), "quai_chainId", json!([]))
            .await,
        Err(RpcError::Timeout)
    ));
}
#[wasm_bindgen_test]
fn worker_entropy_uses_web_crypto_without_window() {
    let mut bytes = [0; 32];
    fill_random(&mut bytes).unwrap();
    assert_ne!(bytes, [0; 32]);
}

#[wasm_bindgen_test(async)]
async fn worker_provider_composition_checks_real_fetch_chain_identity() {
    use quai_primitives::Zone;
    use quai_provider::Provider;
    use quai_rpc::Routing;
    let routing = Routing::direct(
        endpoint("/prefix/cyprus1?token=PUBLIC").as_str(),
        Zone::Cyprus1.into(),
    )
    .unwrap();
    let provider = Provider::new(
        BrowserFetchTransport::new(BrowserConfig::default()).unwrap(),
        routing,
        U256::from(15000),
    );
    assert_eq!(
        provider.chain_id(Zone::Cyprus1.into()).await.unwrap(),
        U256::from(15000)
    );
}

#[wasm_bindgen_test]
fn worker_hd_derivation_and_qi_schnorr_use_real_browser_crypto() {
    use quai_consensus::{
        Denomination, OutPoint, QiInput, QiOutput, QiTransaction, SignedQiTransaction,
    };
    use quai_crypto::SecretKey;
    use quai_primitives::{Hash32, Zone};
    use quai_wallet::{CoinType, HdWallet, Language, Mnemonic, Search};
    // Public all-zero BIP39 test entropy; this identity must never be funded.
    let mnemonic = Mnemonic::from_entropy(Language::English, &[0u8; 16]).unwrap();
    let wallet = HdWallet::from_mnemonic(&mnemonic, "", CoinType::Qi).unwrap();
    let found = wallet
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
        .unwrap();
    assert_eq!(found.address.index, 669);
    assert_eq!(
        found.address.address.to_string(),
        "0x00AA16b5AF6Bc831D200917005898b9645e717Bd"
    );
    let key = wallet
        .derive_key(0, false, found.address.index)
        .unwrap()
        .secret_key()
        .unwrap();
    let tx = QiTransaction {
        chain_id: U256::from(15000),
        inputs: vec![QiInput {
            previous_output: OutPoint {
                transaction_hash:
                    "0x0088008800112233445566778899001122334455667788990011223344556677"
                        .parse::<Hash32>()
                        .unwrap(),
                index: 0,
            },
            public_key: key.public_key(),
        }],
        outputs: vec![QiOutput {
            address: "0x0088223344556677889900112233445566778899"
                .parse()
                .unwrap(),
            denomination: Denomination::new(0).unwrap(),
        }],
        data: vec![],
    };
    let signed = tx.sign_single(&key).unwrap();
    assert_eq!(
        SignedQiTransaction::decode(&signed.signed_bytes().unwrap())
            .unwrap()
            .transaction(),
        &tx
    );
    // Test generated private-key entropy through the real getrandom Web Crypto backend.
    let generated = SecretKey::generate().unwrap();
    let digest = [0x42; 32];
    let signature = generated.sign_prehash(&digest).unwrap();
    assert_eq!(
        signature.recover_prehash(&digest).unwrap(),
        generated.public_key()
    );
}

#[wasm_bindgen_test]
fn worker_legacy_keystore_decryption_checks_mnemonic_ownership() {
    use quai_keystore::{KdfLimits, Keystore, KeystoreError, Password};
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../quai-keystore/tests/fixtures/keystores.json"
    ))
    .unwrap();
    let vector = fixture["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "mnemonic-en")
        .unwrap();
    let parsed = Keystore::from_json(
        &serde_json::to_vec(&vector["json"]).unwrap(),
        KdfLimits::default(),
    )
    .unwrap();
    let account = parsed
        .decrypt(Password::Text("PUBLIC mnemonic"), KdfLimits::default())
        .unwrap();
    assert_eq!(account.address().to_string(), vector["expected"]["address"]);
    assert!(account.mnemonic().is_some());
    assert_eq!(
        parsed
            .decrypt(Password::Text("wrong"), KdfLimits::default())
            .unwrap_err(),
        KeystoreError::Authentication
    );
}
