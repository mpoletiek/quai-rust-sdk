//! Exact ordinary Qi fee shape, routing, and conservative broadcast regressions.
use quai_consensus::{QiTransaction, SignedQiTransaction};
use quai_primitives::Zone;
use quai_provider::{Provider, ProviderError, RpcData};
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
        assert_eq!(endpoint.as_str(), "http://127.0.0.1:9200/custom");
        self.0.lock().unwrap().push((method.to_owned(), params));
        match method {
            "quai_chainId" => Ok(json!("0x3a98")),
            "quai_estimateFeeForQi" => Ok(json!("0x13")),
            "quai_sendRawTransaction" => Err(RpcError::Timeout),
            _ => panic!("unexpected request"),
        }
    }
}
fn provider(mock: &Mock, chain: u64) -> Provider<Mock> {
    Provider::new(
        mock.clone(),
        Routing::direct("http://127.0.0.1:9200/custom", Zone::Cyprus1.into()).unwrap(),
        U256::from(chain),
    )
}
fn signed() -> SignedQiTransaction {
    let fixtures: Value = serde_json::from_str(include_str!(
        "fixtures/shared/compatibility/fixtures/transactions.json"
    ))
    .unwrap();
    let v = fixtures["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == "qi-multi-0")
        .unwrap();
    let data: RpcData = v["signed"].as_str().unwrap().parse().unwrap();
    SignedQiTransaction::decode(data.bytes()).unwrap()
}
#[tokio::test]
async fn fee_estimation_uses_exact_order_public_keys_outputs_and_zero_locks() {
    let signed = signed();
    let tx = signed.transaction();
    let mock = Mock::default();
    assert_eq!(
        provider(&mock, 15000).estimate_qi_fee(tx).await.unwrap(),
        U256::from(19)
    );
    let calls = mock.0.lock().unwrap();
    assert_eq!(calls.len(), 2);
    let params = &calls[1].1[0];
    assert_eq!(params["txType"], 2);
    assert_eq!(params["txIn"].as_array().unwrap().len(), tx.inputs.len());
    assert_eq!(params["txOut"].as_array().unwrap().len(), tx.outputs.len());
    for (got, input) in params["txIn"].as_array().unwrap().iter().zip(&tx.inputs) {
        assert_eq!(
            got["previousOutPoint"]["txHash"],
            input.previous_output.transaction_hash.to_string()
        );
        assert_eq!(
            got["previousOutPoint"]["index"],
            format!("{:#x}", input.previous_output.index)
        );
        assert_eq!(
            got["pubKey"],
            RpcData::new(input.public_key.to_compressed().to_vec())
                .unwrap()
                .to_hex()
        );
    }
    for (got, output) in params["txOut"].as_array().unwrap().iter().zip(&tx.outputs) {
        assert_eq!(got["address"], output.address.to_string());
        assert_eq!(got["lock"], "0x0");
        assert_eq!(
            got["denomination"],
            format!("{:#x}", output.denomination.index())
        );
    }
}
#[tokio::test]
async fn unsupported_fee_data_chain_and_malformed_inputs_fail_before_io() {
    let mut tx: QiTransaction = signed().transaction().clone();
    let mock = Mock::default();
    assert!(matches!(
        provider(&mock, 9).estimate_qi_fee(&tx).await,
        Err(ProviderError::ChainMismatch { .. })
    ));
    tx.data = vec![0; 22];
    assert!(provider(&mock, 15000).estimate_qi_fee(&tx).await.is_err());
    tx.data.clear();
    tx.inputs.clear();
    assert!(provider(&mock, 15000).estimate_qi_fee(&tx).await.is_err());
    assert!(mock.0.lock().unwrap().is_empty());
}
#[tokio::test]
async fn qi_send_retains_id_on_timeout_and_never_retries() {
    let signed = signed();
    let mock = Mock::default();
    assert!(
        !provider(&mock, 9)
            .broadcast_qi(&signed)
            .await
            .unwrap_err()
            .acceptance_is_ambiguous()
    );
    assert!(mock.0.lock().unwrap().is_empty());
    let error = provider(&mock, 15000)
        .broadcast_qi(&signed)
        .await
        .unwrap_err();
    assert!(error.acceptance_is_ambiguous());
    assert_eq!(error.transaction_hash(), Some(signed.hash().unwrap()));
    let calls = mock.0.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].0, "quai_sendRawTransaction");
    assert_eq!(
        calls[1].1,
        json!([RpcData::new(signed.signed_bytes().unwrap())
            .unwrap()
            .to_hex()])
    );
}
