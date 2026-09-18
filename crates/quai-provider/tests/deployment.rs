//! Captured funded development-chain creation plus adversarial source views.
use quai_consensus::SignedQuaiTransaction;
use quai_primitives::{Hash32, Zone, get_bytes};
use quai_provider::{
    DeploymentObservation, DeploymentReference, Provider, ProviderError, ReceiptOutcome,
};
use quai_rpc::{Endpoint, Routing, RpcError, Transport, U256};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
fn captured() -> Value {
    serde_json::from_str(include_str!(
        "fixtures/shared/test-infra/local-chain/confirmed-account-evidence/verify-rpc.json"
    ))
    .unwrap()
}
fn signed() -> SignedQuaiTransaction {
    let fixture: Value = serde_json::from_str(include_str!(
        "fixtures/shared/test-infra/local-chain/confirmed-account-evidence/deployment-signed.json"
    ))
    .unwrap();
    SignedQuaiTransaction::decode(&get_bytes(fixture["signedBytes"].as_str().unwrap()).unwrap())
        .unwrap()
}
fn genesis() -> Hash32 {
    captured()
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["method"] == "quai_getHeaderByNumber" && r["params"][0] == "0x0")
        .unwrap()["result"]["woHeader"]["hash"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}
#[derive(Clone)]
struct Mock(Arc<Mutex<State>>);
struct State {
    receipt: Value,
    header: Value,
    genesis: Value,
    code: Value,
    calls: Vec<(String, Value)>,
    mode: u8,
    code_reads: usize,
}
impl Mock {
    fn new() -> Self {
        let rows = captured();
        let rows = rows.as_array().unwrap();
        let get = |method: &str, param: &str| {
            rows.iter()
                .find(|r| r["method"] == method && r["params"][0] == param)
                .unwrap()["result"]
                .clone()
        };
        Self(Arc::new(Mutex::new(State {
            receipt: get(
                "quai_getTransactionReceipt",
                &signed().hash().unwrap().to_string(),
            ),
            header: get("quai_getHeaderByNumber", "0x6"),
            genesis: get("quai_getHeaderByNumber", "0x0"),
            code: json!("0x602a60005260206000f3"),
            calls: vec![],
            mode: 0,
            code_reads: 0,
        })))
    }
}
impl Transport for Mock {
    async fn request(&self, _: &Endpoint, method: &str, params: Value) -> Result<Value, RpcError> {
        let mut s = self.0.lock().unwrap();
        s.calls.push((method.into(), params.clone()));
        Ok(match method {
            "quai_chainId" => json!("0x539"),
            "quai_getTransactionReceipt" => s.receipt.clone(),
            "quai_getTransactionByHash" => Value::Null,
            "quai_getHeaderByNumber" if params[0] == "0x0" => s.genesis.clone(),
            "quai_getHeaderByNumber" => {
                let mut h = s.header.clone();
                if s.mode == 1 || (s.mode == 2 && s.code_reads > 0) {
                    h["woHeader"]["hash"] = json!(genesis().to_string());
                }
                h
            }
            "quai_getCode" => {
                s.code_reads += 1;
                if s.mode == 3 {
                    return Err(RpcError::Timeout);
                }
                s.code.clone()
            }
            _ => panic!("unexpected {method}"),
        })
    }
}
fn provider(m: &Mock) -> Provider<Mock> {
    Provider::new(
        m.clone(),
        Routing::direct("http://127.0.0.1:19200", Zone::Cyprus1.into()).unwrap(),
        U256::from(1337),
    )
}
#[tokio::test]
async fn captured_creation_binds_exact_signed_address_and_inclusion_block_runtime() {
    let actual = Hash32::from_bytes(quai_crypto::keccak256(
        &get_bytes("0x602a60005260206000f3").unwrap(),
    ));
    for expected in [None, Some(actual), Some(Hash32::ZERO)] {
        let mock = Mock::new();
        let reference = DeploymentReference::from_signed(genesis(), &signed(), expected).unwrap();
        assert_eq!(
            reference.address().to_string(),
            "0x004C1e917c53fFE0bA85bA0Be73E03047ed455B6"
        );
        let result = provider(&mock)
            .observe_deployment(&reference)
            .await
            .unwrap();
        let DeploymentObservation::Included {
            block,
            outcome,
            confirmations,
            code: Some(code),
        } = result
        else {
            panic!("{result:?}")
        };
        assert_eq!(block.number, 6);
        assert_eq!(outcome, ReceiptOutcome::Succeeded);
        assert_eq!(confirmations, 1);
        assert_eq!(code.hash, actual);
        assert_eq!(code.matches_expected, expected.map(|h| h == actual));
        assert_eq!(code.bytes.to_hex(), "0x602a60005260206000f3");
        assert!(mock.0.lock().unwrap().calls.contains(&(
            "quai_getCode".into(),
            json!([reference.address().to_string(), "0x6"])
        )));
    }
}
#[tokio::test]
async fn failed_locked_absent_and_empty_code_are_distinct() {
    let reference = DeploymentReference::from_signed(genesis(), &signed(), None).unwrap();
    for (status, outcome) in [
        ("0x0", ReceiptOutcome::Failed),
        ("0x2", ReceiptOutcome::Locked),
    ] {
        let mock = Mock::new();
        mock.0.lock().unwrap().receipt["status"] = json!(status);
        let result = provider(&mock)
            .observe_deployment(&reference)
            .await
            .unwrap();
        assert!(
            matches!(result,DeploymentObservation::Included{outcome:o,code:None,..} if o==outcome)
        );
        assert_eq!(mock.0.lock().unwrap().code_reads, 0);
    }
    let mock = Mock::new();
    mock.0.lock().unwrap().receipt = Value::Null;
    assert_eq!(
        provider(&mock)
            .observe_deployment(&reference)
            .await
            .unwrap(),
        DeploymentObservation::NoReceipt {
            transaction_known: false
        }
    );
    assert_eq!(mock.0.lock().unwrap().code_reads, 0);
    let mock = Mock::new();
    mock.0.lock().unwrap().code = json!("0x");
    let DeploymentObservation::Included {
        code: Some(code), ..
    } = provider(&mock)
        .observe_deployment(&reference)
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(code.bytes.bytes().is_empty());
    assert_eq!(code.hash, Hash32::from_bytes(quai_crypto::keccak256(&[])));
}
#[tokio::test]
async fn receipt_mismatches_reorgs_and_unavailable_state_fail_closed() {
    let reference = DeploymentReference::from_signed(genesis(), &signed(), None).unwrap();
    for field in ["from", "to", "contractAddress"] {
        let mock = Mock::new();
        mock.0.lock().unwrap().receipt[field] = json!("0x0011223344556677889900112233445566778899");
        assert!(
            provider(&mock)
                .observe_deployment(&reference)
                .await
                .is_err()
        );
        assert_eq!(mock.0.lock().unwrap().code_reads, 0);
    }
    let mock = Mock::new();
    mock.0.lock().unwrap().receipt["contractAddress"] = Value::Null;
    assert!(
        provider(&mock)
            .observe_deployment(&reference)
            .await
            .is_err()
    );
    let mock = Mock::new();
    mock.0.lock().unwrap().mode = 1;
    assert!(matches!(
        provider(&mock)
            .observe_deployment(&reference)
            .await
            .unwrap(),
        DeploymentObservation::Noncanonical { .. }
    ));
    assert_eq!(mock.0.lock().unwrap().code_reads, 0);
    let mock = Mock::new();
    mock.0.lock().unwrap().mode = 2;
    assert!(matches!(
        provider(&mock).observe_deployment(&reference).await,
        Err(ProviderError::ObservationChanged)
    ));
    let mock = Mock::new();
    mock.0.lock().unwrap().mode = 3;
    assert!(matches!(
        provider(&mock).observe_deployment(&reference).await,
        Err(ProviderError::Rpc(RpcError::Timeout))
    ));
    let mock = Mock::new();
    let wrong =
        DeploymentReference::from_signed(Hash32::from_bytes([1; 32]), &signed(), None).unwrap();
    assert!(provider(&mock).observe_deployment(&wrong).await.is_err());
    assert_eq!(mock.0.lock().unwrap().code_reads, 0);
    assert!(DeploymentReference::from_signed(Hash32::ZERO, &signed(), None).is_err());
    let fixture: Value = serde_json::from_str(include_str!(
        "fixtures/shared/test-infra/local-chain/confirmed-account-evidence/account-signed.json"
    ))
    .unwrap();
    let ordinary = SignedQuaiTransaction::decode(
        &get_bytes(fixture["signedBytes"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    assert!(DeploymentReference::from_signed(genesis(), &ordinary, None).is_err());
}

#[cfg(all(feature = "polling", not(target_arch = "wasm32")))]
#[tokio::test]
async fn bounded_wait_returns_failed_execution_and_times_out_without_releasing_intent() {
    use quai_provider::{DeploymentWaitError, WaitConfig};
    use std::time::Duration;
    let reference = DeploymentReference::from_signed(genesis(), &signed(), None).unwrap();
    let config = WaitConfig::new(1, Duration::from_secs(1), Duration::from_millis(1));
    let mock = Mock::new();
    assert!(matches!(
        provider(&mock)
            .wait_for_deployment(&reference, config)
            .await
            .unwrap(),
        DeploymentObservation::Included {
            outcome: ReceiptOutcome::Succeeded,
            code: Some(_),
            ..
        }
    ));
    let mock = Mock::new();
    mock.0.lock().unwrap().receipt["status"] = json!("0x0");
    assert!(matches!(
        provider(&mock)
            .wait_for_deployment(&reference, config)
            .await
            .unwrap(),
        DeploymentObservation::Included {
            outcome: ReceiptOutcome::Failed,
            code: None,
            ..
        }
    ));
    let mock = Mock::new();
    mock.0.lock().unwrap().receipt = Value::Null;
    assert!(
        matches!(provider(&mock).wait_for_deployment(&reference,config.with_timeout(Duration::from_millis(20))).await,Err(DeploymentWaitError::Timeout{transaction_hash,last_observed:None}) if transaction_hash==reference.transaction_hash())
    );
    let mock = Mock::new();
    assert!(matches!(
        provider(&mock)
            .wait_for_deployment(&reference, config.with_confirmations(0))
            .await,
        Err(DeploymentWaitError::InvalidConfig)
    ));
    assert!(mock.0.lock().unwrap().calls.is_empty());
    let mock = Mock::new();
    mock.0.lock().unwrap().mode = 3;
    assert!(matches!(
        provider(&mock)
            .wait_for_deployment(&reference, config)
            .await,
        Err(DeploymentWaitError::Provider(ProviderError::Rpc(
            RpcError::Timeout
        )))
    ));
}
