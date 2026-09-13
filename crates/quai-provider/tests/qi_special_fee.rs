//! Specialized fee profile, rounding, and changing-head regression tests.
use quai_consensus::SignedQiConversionTransaction;
use quai_primitives::Zone;
use quai_provider::{Provider, ProviderError, QiFeeProfile, RpcData, qi_special_gas};
use quai_rpc::{Endpoint, Routing, RpcError, Transport, U256};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Mock {
    mode: u8,
    calls: Arc<Mutex<Vec<(String, Value)>>>,
}
impl Transport for Mock {
    async fn request(&self, _: &Endpoint, method: &str, params: Value) -> Result<Value, RpcError> {
        let mut calls = self.calls.lock().unwrap();
        calls.push((method.to_owned(), params.clone()));
        Ok(match method {
            "quai_chainId" => json!("0x3a98"),
            "quai_getHeaderByNumber" => {
                let mut h: Value =
                    serde_json::from_str(include_str!("fixtures/conversion-block16.json")).unwrap();
                // This fixture is a block, while getHeaderByNumber returns its header.
                let wo = h["woHeader"].clone();
                h = h["header"].take();
                h["woHeader"] = wo;
                h["woHeader"]["primeTerminusNumber"] = json!(format!(
                    "0x{:x}",
                    if self.mode == 1 { 1_754_999 } else { 1_755_000 }
                ));
                h["baseFeePerGas"] = if self.mode == 4 {
                    json!(format!("0x{}", "ff".repeat(32)))
                } else {
                    json!("0x1")
                };
                if self.mode == 2 && calls.iter().filter(|(m, _)| m == method).count() > 1 {
                    h["woHeader"]["hash"] = json!(format!("0x{}", "22".repeat(32)));
                }
                h
            }
            "quai_getLatestUTXOSetSize" => json!("0x1"),
            "quai_quaiToQi" => {
                assert_eq!(params[1], "0x10");
                json!("0x2")
            }
            "quai_qiToQuai" => {
                assert_eq!(params[1], "0x10");
                if params[0] == "0x2" || self.mode == 3 {
                    json!("0x1")
                } else {
                    json!("0xffffffffffff")
                }
            }
            _ => panic!("unexpected method {method}"),
        })
    }
}
fn fixture() -> SignedQiConversionTransaction {
    let f: Value = serde_json::from_str(include_str!(
        "fixtures/shared/crates/quai-consensus/tests/conversion-vectors.json"
    ))
    .unwrap();
    let bytes: RpcData = f["vectors"][4]["signed"].as_str().unwrap().parse().unwrap();
    SignedQiConversionTransaction::decode(bytes.bytes()).unwrap()
}
fn provider(mode: u8) -> Provider<Mock> {
    Provider::new(
        Mock {
            mode,
            calls: Default::default(),
        },
        Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
        U256::from(15000),
    )
}
#[tokio::test]
async fn specialized_fee_rounds_up_and_pins_rates_to_sampled_height() {
    let tx = fixture();
    let q = provider(0)
        .estimate_qi_special_fee(
            tx.transaction().transaction(),
            QiFeeProfile::V056ShaAnchored,
        )
        .await
        .unwrap();
    assert_eq!(q.qits, U256::from(3));
    assert_eq!(q.gas_price, U256::from(1));
    assert_eq!(
        q.required_gas,
        100_000
            + 3_000
            + 800 * tx.transaction().transaction().inputs.len() as u64
            + 9000 * tx.transaction().transaction().outputs.len() as u64
    );
}
#[tokio::test]
async fn rejects_unsupported_profile_head_drift_inconsistent_rounding_and_overflow() {
    let tx = fixture();
    for mode in 1..=4 {
        let error = provider(mode)
            .estimate_qi_special_fee(
                tx.transaction().transaction(),
                QiFeeProfile::V056ShaAnchored,
            )
            .await
            .unwrap_err();
        if mode == 1 {
            assert!(matches!(
                error,
                ProviderError::ConversionFeeEstimationUnavailable
            ));
        } else {
            assert!(matches!(error, ProviderError::InvalidResult(_)), "{error}");
        }
    }
}
#[test]
fn special_gas_bounds_and_scaling_threshold() {
    assert_eq!(qi_special_gas(1, 1, 0).unwrap(), 112_800);
    assert_eq!(qi_special_gas(1, 1, 3_269_017).unwrap(), 112_800);
    assert_eq!(qi_special_gas(1, 1, 3_269_018).unwrap(), 112_802);
    assert!(qi_special_gas(0, 1, 1).is_err());
    assert!(qi_special_gas(1, 1025, 1).is_err());
    assert!(qi_special_gas(1024, 1024, u64::MAX).unwrap() < 31_000_000);
}
