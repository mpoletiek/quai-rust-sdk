//! Explicit conversion submission and unsupported estimator capability; no live node.
use quai_consensus::SignedQiConversionTransaction;
use quai_primitives::Zone;
use quai_provider::{BroadcastError, Provider, ProviderError, RpcData};
use quai_rpc::{Endpoint, Routing, RpcError, Transport, U256};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
#[derive(Clone, Default)]
struct Mock(Arc<Mutex<Vec<(String, Value)>>>);
impl Transport for Mock {
    async fn request(
        &self,
        endpoint: &Endpoint,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        assert_eq!(endpoint.as_str(), "http://127.0.0.1:9200/exact");
        self.0.lock().unwrap().push((method.to_owned(), params));
        match method {
            "quai_chainId" => Ok(json!("0x3a98")),
            "quai_sendRawTransaction" => Err(RpcError::Timeout),
            _ => panic!("unexpected conversion request {method}"),
        }
    }
}
fn provider(mock: &Mock, chain: u64) -> Provider<Mock> {
    Provider::new(
        mock.clone(),
        Routing::direct("http://127.0.0.1:9200/exact", Zone::Cyprus1.into()).unwrap(),
        U256::from(chain),
    )
}
fn signed() -> SignedQiConversionTransaction {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../quai-consensus/tests/conversion-vectors.json"
    ))
    .unwrap();
    let bytes: RpcData = fixture["vectors"][4]["signed"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    SignedQiConversionTransaction::decode(bytes.bytes()).unwrap()
}
#[tokio::test]
async fn conversion_estimator_fails_explicitly_before_any_rpc_or_data_loss() {
    let mock = Mock::default();
    let signed = signed();
    assert!(matches!(
        provider(&mock, 15000)
            .estimate_qi_conversion_fee(signed.transaction())
            .await,
        Err(ProviderError::ConversionFeeEstimationUnavailable)
    ));
    assert!(matches!(
        provider(&mock, 9)
            .estimate_qi_conversion_fee(signed.transaction())
            .await,
        Err(ProviderError::ChainMismatch { .. })
    ));
    assert!(
        provider(&mock, 15000)
            .estimate_qi_fee(signed.transaction().transaction())
            .await
            .is_err()
    );
    assert!(mock.0.lock().unwrap().is_empty());
}
#[tokio::test]
async fn explicit_broadcast_preserves_all_conversion_bytes_and_ambiguity_without_retry() {
    let mock = Mock::default();
    let signed = signed();
    assert!(matches!(
        provider(&mock, 9).broadcast_qi_conversion(&signed).await,
        Err(BroadcastError::Preflight(
            ProviderError::ChainMismatch { .. }
        ))
    ));
    assert!(mock.0.lock().unwrap().is_empty());
    let error = provider(&mock, 15000)
        .broadcast_qi_conversion(&signed)
        .await
        .unwrap_err();
    assert!(error.acceptance_is_ambiguous());
    assert_eq!(error.transaction_hash(), Some(signed.hash().unwrap()));
    let calls = mock.0.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].0, "quai_sendRawTransaction");
    let raw: RpcData = calls[1].1[0].as_str().unwrap().parse().unwrap();
    assert_eq!(raw.bytes(), signed.signed_bytes().unwrap());
    assert_eq!(
        SignedQiConversionTransaction::decode(raw.bytes())
            .unwrap()
            .transaction(),
        signed.transaction()
    );
}
