//! Captured isolated-chain observations plus adversarial, bounded source simulations.
use quai_consensus::{
    AccessTuple, Denomination, QiInput, QiOutput, QiTransaction, QuaiTransaction,
};
use quai_crypto::{PublicKey, RecoverableSignature, SchnorrSignature};
use quai_primitives::{Hash32, Zone};
use quai_provider::{
    BlockReference, ConversionEffect, ConversionOriginObservation, ConversionReference,
    ConversionSpendability, EtxScanRequest, Provider, ProviderError, ReceiptOutcome, ScanCoverage,
    Transaction, TransactionDetails,
};
use quai_rpc::{Endpoint, Routing, RpcError, Transport, U256};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
const GENESIS: &str = "0xff38a93744ee5aae738addc88da4f6b171528244e81d34aa4b25579fa3f44ed2";
fn origin_fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../test-infra/local-chain/conversion-origin-fixtures.json"
    ))
    .unwrap()
}
fn settlement_fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../test-infra/local-chain/conversion-settlement-fixtures.json"
    ))
    .unwrap()
}
fn block_fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/conversion-block16.json")).unwrap()
}
fn reference(qi: bool) -> ConversionReference {
    let value = origin_fixture();
    let kind = if qi { "qiToQuai" } else { "quaiToQi" };
    let transaction = Transaction::try_from(value["transactions"][kind].clone()).unwrap();
    match transaction.details {
        TransactionDetails::Qi(tx) => {
            let raw = QiTransaction {
                chain_id: tx.chain_id,
                inputs: tx
                    .inputs
                    .iter()
                    .map(|input| QiInput {
                        previous_output: quai_consensus::OutPoint {
                            transaction_hash: input.previous_out_point.tx_hash,
                            index: input.previous_out_point.index,
                        },
                        public_key: PublicKey::from_sec1_bytes(input.public_key.bytes()).unwrap(),
                    })
                    .collect(),
                outputs: tx
                    .outputs
                    .iter()
                    .map(|output| QiOutput {
                        address: output.address,
                        denomination: Denomination::new(output.denomination).unwrap(),
                    })
                    .collect(),
                data: transaction.input.bytes().to_vec(),
            };
            let conversion =
                quai_consensus::QiConversionTransaction::from_transaction(raw).unwrap();
            let signature =
                SchnorrSignature::from_bytes(&tx.signature.bytes().try_into().unwrap()).unwrap();
            let signed = conversion.attach_signature(signature).unwrap();
            assert_eq!(signed.hash().unwrap(), transaction.hash);
            ConversionReference::from_qi(GENESIS.parse().unwrap(), &signed).unwrap()
        }
        TransactionDetails::Quai(tx) => {
            let raw = QuaiTransaction {
                chain_id: tx.chain_id,
                nonce: tx.nonce,
                to: tx.to,
                value: tx.value,
                gas_limit: tx.gas,
                gas_price: tx.gas_price,
                data: transaction.input.bytes().to_vec(),
                access_list: tx
                    .access_list
                    .iter()
                    .map(|tuple| AccessTuple {
                        address: tuple.address,
                        storage_keys: tuple.storage_keys.clone(),
                    })
                    .collect(),
            };
            let mut compact = [0; 64];
            compact[..32].copy_from_slice(&tx.signature.r.to_be_bytes::<32>());
            compact[32..].copy_from_slice(&tx.signature.s.to_be_bytes::<32>());
            let signed = raw
                .attach_signature(
                    RecoverableSignature::from_compact(&compact, tx.signature.v.to::<u8>())
                        .unwrap(),
                )
                .unwrap();
            assert_eq!(signed.hash().unwrap(), transaction.hash);
            ConversionReference::from_quai(GENESIS.parse().unwrap(), &signed).unwrap()
        }
        _ => panic!("unexpected fixture type"),
    }
}
struct State {
    block: Value,
    receipts: BTreeMap<String, Value>,
    headers: BTreeMap<u64, String>,
    genesis: String,
    latest_reads: u64,
    moving_head: bool,
    calls: Vec<(String, Value)>,
}
#[derive(Clone)]
struct Mock(Arc<Mutex<State>>);
impl Mock {
    fn new() -> Self {
        let origin = origin_fixture();
        let settlements = settlement_fixture();
        let mut receipts = BTreeMap::new();
        for kind in ["quaiToQi", "qiToQuai"] {
            let receipt = &origin["originReceipts"][kind];
            receipts.insert(
                receipt["transactionHash"].as_str().unwrap().to_owned(),
                receipt.clone(),
            );
        }
        for settled in settlements.as_array().unwrap() {
            let receipt = &settled["receipt"];
            receipts.insert(
                receipt["transactionHash"].as_str().unwrap().to_owned(),
                receipt.clone(),
            );
        }
        let block = block_fixture();
        let headers = BTreeMap::from([
            (
                8,
                origin["originReceipts"]["qiToQuai"]["blockHash"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            ),
            (
                15,
                block["woHeader"]["parentHash"].as_str().unwrap().to_owned(),
            ),
            (16, block["hash"].as_str().unwrap().to_owned()),
        ]);
        Self(Arc::new(Mutex::new(State {
            block,
            receipts,
            headers,
            genesis: GENESIS.into(),
            latest_reads: 0,
            moving_head: false,
            calls: vec![],
        })))
    }
    fn calls(&self, method: &str) -> usize {
        self.0
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(name, _)| name == method)
            .count()
    }
}
impl Transport for Mock {
    async fn request(&self, _: &Endpoint, method: &str, params: Value) -> Result<Value, RpcError> {
        let mut state = self.0.lock().unwrap();
        state.calls.push((method.to_owned(), params.clone()));
        Ok(match method {
            "quai_chainId" => json!("0x539"),
            "quai_getHeaderByNumber" if params[0] == "0x0" => {
                json!({"woHeader":{"hash":state.genesis,"number":"0x0","parentHash":Hash32::ZERO.to_string(),"location":"0x"}})
            }
            "quai_getHeaderByNumber" => {
                let number = if params[0] == "latest" {
                    state.latest_reads += 1;
                    20 + u64::from(state.moving_head && state.latest_reads > 1)
                } else {
                    u64::from_str_radix(params[0].as_str().unwrap().trim_start_matches("0x"), 16)
                        .unwrap()
                };
                let hash = state
                    .headers
                    .get(&number)
                    .cloned()
                    .unwrap_or_else(|| format!("0x{number:064x}"));
                json!({"woHeader":{"hash":hash,"number":format!("0x{number:x}"),"parentHash":GENESIS,"location":"0x0000","primeTerminusNumber":"0x4"},"gasLimit":"0xb71b00","stateLimit":"0xb71b00"})
            }
            "quai_getBlockByNumber" => {
                assert_eq!(params[1], true);
                if params[0] == "0x10" {
                    state.block.clone()
                } else {
                    Value::Null
                }
            }
            "quai_getTransactionReceipt" => state
                .receipts
                .get(params[0].as_str().unwrap())
                .cloned()
                .unwrap_or(Value::Null),
            "quai_getLockedBalance" => {
                assert_eq!(params.as_array().unwrap().len(), 1);
                json!("0x0")
            }
            _ => panic!("unexpected {method}"),
        })
    }
}
fn provider(mock: &Mock) -> Provider<Mock> {
    Provider::new(
        mock.clone(),
        Routing::direct("http://127.0.0.1:19200/exact", Zone::Cyprus1.into()).unwrap(),
        U256::from(1337),
    )
}
fn request() -> EtxScanRequest {
    EtxScanRequest {
        zone: Zone::Cyprus1,
        from: 16,
        to: 16,
        max_transactions_per_block: 16,
        max_total_transactions: 32,
        preceding_block: None,
    }
}
#[tokio::test]
async fn captured_block_and_final_hashes_correlate_conversion_and_refund_without_maturity_claims() {
    for qi in [true, false] {
        let mock = Mock::new();
        let reference = reference(qi);
        let observed = provider(&mock)
            .observe_conversion(&reference, request())
            .await
            .unwrap();
        let ConversionOriginObservation::Emitted {
            transaction: initial,
            ..
        } = &observed.origin
        else {
            panic!("missing initial ETX");
        };
        let scan = observed.scan.as_ref().unwrap();
        assert_eq!(scan.coverage, ScanCoverage::Complete);
        assert_eq!(scan.transactions_examined, 8);
        let executed = scan.execution.as_ref().unwrap();
        assert_ne!(initial.hash, executed.transaction.hash);
        assert_eq!(
            executed.receipt.as_ref().unwrap().outcome,
            ReceiptOutcome::Succeeded
        );
        assert_eq!(observed.spendability, ConversionSpendability::Unverified);
        assert_eq!(
            observed.effect,
            Some(if qi {
                ConversionEffect::ConversionReported
            } else {
                ConversionEffect::RefundReported {
                    beneficiary: "0x0049CdA3305ccB9cB23E7Ce2528ceF555E9a5B32"
                        .parse()
                        .unwrap(),
                }
            })
        );
        if !qi {
            let TransactionDetails::External(etx) = &executed.transaction.details else {
                panic!()
            };
            assert_eq!(etx.to.ledger(), quai_primitives::Ledger::Qi);
        }
        assert_eq!(mock.calls("quai_getBlockByNumber"), 1);
    }
}
#[tokio::test]
async fn reorgs_parent_discontinuity_and_origin_reorgs_are_explicit() {
    let mock = Mock::new();
    mock.0.lock().unwrap().headers.insert(16, GENESIS.into());
    assert!(matches!(
        provider(&mock)
            .observe_conversion(&reference(true), request())
            .await,
        Err(ProviderError::ObservationChanged)
    ));
    let mock = Mock::new();
    let mut scan = request();
    scan.preceding_block = Some(BlockReference {
        number: 15,
        hash: GENESIS.parse().unwrap(),
    });
    assert!(matches!(
        provider(&mock)
            .scan_external_transactions(reference(true).correlation(), scan)
            .await,
        Err(ProviderError::ObservationChanged)
    ));
    let mock = Mock::new();
    mock.0.lock().unwrap().headers.insert(8, GENESIS.into());
    let observed = provider(&mock)
        .observe_conversion(&reference(true), request())
        .await
        .unwrap();
    assert!(matches!(
        observed.origin,
        ConversionOriginObservation::Noncanonical { .. }
    ));
    assert!(observed.scan.is_none());
    assert_eq!(mock.calls("quai_getBlockByNumber"), 0);
}
#[tokio::test]
async fn unavailable_history_and_receipts_are_not_rejection_or_success() {
    let mock = Mock::new();
    let mut scan = request();
    scan.to = 17;
    let observed = provider(&mock)
        .observe_conversion(&reference(true), scan)
        .await
        .unwrap();
    assert_eq!(
        observed.scan.as_ref().unwrap().coverage,
        ScanCoverage::Unavailable { block_number: 17 }
    );
    assert!(observed.scan.unwrap().execution.is_some());
    let mock = Mock::new();
    mock.0.lock().unwrap().block = Value::Null;
    let observed = provider(&mock)
        .observe_conversion(&reference(true), request())
        .await
        .unwrap();
    let scan = observed.scan.unwrap();
    assert_eq!(
        scan.coverage,
        ScanCoverage::Unavailable { block_number: 16 }
    );
    assert!(scan.execution.is_none());
    assert!(observed.effect.is_none());
    let mock = Mock::new();
    let hash = settlement_fixture()[0]["transaction"]["hash"]
        .as_str()
        .unwrap()
        .to_owned();
    mock.0.lock().unwrap().receipts.remove(&hash);
    let observed = provider(&mock)
        .observe_conversion(&reference(true), request())
        .await
        .unwrap();
    assert_eq!(
        observed.effect,
        Some(ConversionEffect::ReceiptUnavailable { etx_type: 2 })
    );
    let mock = Mock::new();
    mock.0.lock().unwrap().receipts.remove(
        &reference(true)
            .correlation()
            .originating_tx_hash
            .to_string(),
    );
    let observed = provider(&mock)
        .observe_conversion(&reference(true), request())
        .await
        .unwrap();
    assert_eq!(observed.origin, ConversionOriginObservation::Unavailable);
    assert!(observed.scan.is_none());
}
#[tokio::test]
async fn unknown_subtypes_failed_execution_and_lossy_success_are_preserved() {
    let mock = Mock::new();
    let hash = settlement_fixture()[0]["transaction"]["hash"]
        .as_str()
        .unwrap()
        .to_owned();
    {
        let mut state = mock.0.lock().unwrap();
        state.block["transactions"][0]["etxType"] = json!("0xff");
        state.receipts.get_mut(&hash).unwrap()["etxType"] = json!("0xff");
    }
    assert_eq!(
        provider(&mock)
            .observe_conversion(&reference(true), request())
            .await
            .unwrap()
            .effect,
        Some(ConversionEffect::UnknownSubtype(255))
    );
    let mock = Mock::new();
    mock.0.lock().unwrap().receipts.get_mut(&hash).unwrap()["status"] = json!("0x0");
    assert_eq!(
        provider(&mock)
            .observe_conversion(&reference(true), request())
            .await
            .unwrap()
            .effect,
        Some(ConversionEffect::ExecutionFailed { etx_type: 2 })
    );
    let mock = Mock::new();
    {
        let mut state = mock.0.lock().unwrap();
        let receipt = state
            .receipts
            .get_mut(&hash)
            .unwrap()
            .as_object_mut()
            .unwrap();
        receipt.remove("status");
        receipt.insert("root".into(), json!(GENESIS));
    }
    assert_eq!(
        provider(&mock)
            .observe_conversion(&reference(true), request())
            .await
            .unwrap()
            .effect,
        Some(ConversionEffect::LegacyOutcome { etx_type: 2 })
    );
}
#[tokio::test]
async fn mismatched_intent_origin_emission_and_receipt_associations_fail_closed() {
    for field in ["to", "from", "input"] {
        let mock = Mock::new();
        mock.0.lock().unwrap().block["transactions"][0][field] = if field == "input" {
            json!("0x232800d7bfbbdd71a5ac547956d138a72ce7f9527368")
        } else {
            json!("0x0000000000000000000000000000000000000001")
        };
        assert!(
            provider(&mock)
                .observe_conversion(&reference(true), request())
                .await
                .is_err(),
            "accepted altered {field}"
        );
    }
    let mock = Mock::new();
    let origin = reference(true)
        .correlation()
        .originating_tx_hash
        .to_string();
    mock.0.lock().unwrap().receipts.get_mut(&origin).unwrap()["outboundEtxs"][0]["blockHash"] =
        json!(GENESIS);
    assert!(
        provider(&mock)
            .observe_conversion(&reference(true), request())
            .await
            .is_err()
    );
    let mock = Mock::new();
    let hash = settlement_fixture()[0]["transaction"]["hash"]
        .as_str()
        .unwrap()
        .to_owned();
    mock.0.lock().unwrap().receipts.get_mut(&hash).unwrap()["originatingTxHash"] = json!(GENESIS);
    assert!(
        provider(&mock)
            .observe_conversion(&reference(true), request())
            .await
            .is_err()
    );
    let mock = Mock::new();
    mock.0.lock().unwrap().genesis = Hash32::ZERO.to_string();
    assert!(
        provider(&mock)
            .observe_conversion(&reference(true), request())
            .await
            .is_err()
    );
    assert_eq!(mock.calls("quai_getBlockByNumber"), 0);
}
#[tokio::test]
async fn duplicate_correlations_and_malformed_block_positions_are_rejected() {
    let mock = Mock::new();
    {
        let mut state = mock.0.lock().unwrap();
        let mut duplicate = state.block["transactions"][0].clone();
        duplicate["hash"] = json!(GENESIS);
        duplicate["transactionIndex"] = json!("0x8");
        state.block["transactions"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
    }
    assert!(
        provider(&mock)
            .scan_external_transactions(reference(true).correlation(), request())
            .await
            .is_err()
    );
    for field in ["blockHash", "transactionIndex", "blockNumber"] {
        let mock = Mock::new();
        mock.0.lock().unwrap().block["transactions"][0][field] = if field == "blockHash" {
            json!(GENESIS)
        } else {
            json!("0x3")
        };
        assert!(
            provider(&mock)
                .block_with_transactions(Zone::Cyprus1, 16, 16)
                .await
                .is_err()
        );
    }
    let mock = Mock::new();
    mock.0.lock().unwrap().block["hash"] = json!(GENESIS);
    assert!(
        provider(&mock)
            .block_with_transactions(Zone::Cyprus1, 16, 16)
            .await
            .is_err()
    );
}
#[tokio::test]
async fn scan_resource_budgets_precede_requests_and_typed_block_allocations() {
    let mock = Mock::new();
    let key = reference(true).correlation();
    for (from, to) in [(0, 16), (16, 15), (1, 257), (16, u64::MAX)] {
        let mut scan = request();
        scan.from = from;
        scan.to = to;
        assert!(
            provider(&mock)
                .scan_external_transactions(key, scan)
                .await
                .is_err()
        );
    }
    let mut scan = request();
    scan.max_total_transactions = 0;
    assert!(
        provider(&mock)
            .scan_external_transactions(key, scan)
            .await
            .is_err()
    );
    assert_eq!(mock.calls("quai_chainId"), 0);
    let mut scan = request();
    scan.max_total_transactions = 7;
    assert!(
        provider(&mock)
            .scan_external_transactions(key, scan)
            .await
            .is_err()
    );
    assert_eq!(mock.calls("quai_getTransactionReceipt"), 0);
}
#[tokio::test]
async fn locked_balance_is_latest_aggregate_with_explicit_moving_head() {
    let mock = Mock::new();
    mock.0.lock().unwrap().moving_head = true;
    let address = "0x0049CdA3305ccB9cB23E7Ce2528ceF555E9a5B32"
        .parse()
        .unwrap();
    let observation = provider(&mock).locked_quai_balance(address).await.unwrap();
    assert_eq!(observation.balance, U256::ZERO);
    assert_eq!(observation.before.number, 20);
    assert_eq!(observation.after.number, 21);
    assert_eq!(mock.calls("quai_getLockedBalance"), 1);
}

#[cfg(all(feature = "http", not(target_arch = "wasm32")))]
#[tokio::test]
#[ignore = "requires explicit disposable conversion profile at localhost:19200; read-only"]
async fn live_disposable_conversion_correlation_and_locked_balance() {
    let transport = quai_rpc::HttpTransport::new(quai_rpc::HttpConfig::default()).unwrap();
    let provider = Provider::new(
        transport,
        Routing::direct("http://127.0.0.1:19200", Zone::Cyprus1.into()).unwrap(),
        U256::from(1337),
    );
    assert_eq!(
        provider.genesis_hash(Zone::Cyprus1).await.unwrap(),
        GENESIS.parse::<Hash32>().unwrap()
    );
    for qi in [true, false] {
        let observed = provider
            .observe_conversion(&reference(qi), request())
            .await
            .unwrap();
        assert_eq!(
            observed.scan.as_ref().unwrap().coverage,
            ScanCoverage::Complete
        );
        assert_eq!(
            observed.effect,
            Some(if qi {
                ConversionEffect::ConversionReported
            } else {
                ConversionEffect::RefundReported {
                    beneficiary: "0x0049CdA3305ccB9cB23E7Ce2528ceF555E9a5B32"
                        .parse()
                        .unwrap(),
                }
            })
        );
        assert_eq!(observed.spendability, ConversionSpendability::Unverified);
    }
    let balance = provider
        .locked_quai_balance(
            "0x0049CdA3305ccB9cB23E7Ce2528ceF555E9a5B32"
                .parse()
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(balance.after.number >= balance.before.number);
}

#[tokio::test]
async fn refund_amount_is_preserved_and_genesis_pagination_anchor_supports_unavailable_history() {
    let mock = Mock::new();
    mock.0.lock().unwrap().block["transactions"][1]["value"] = json!("0x1");
    assert!(
        provider(&mock)
            .observe_conversion(&reference(false), request())
            .await
            .is_err()
    );
    let mock = Mock::new();
    let mut scan = request();
    scan.from = 1;
    scan.to = 1;
    scan.preceding_block = Some(BlockReference {
        number: 0,
        hash: GENESIS.parse().unwrap(),
    });
    let result = provider(&mock)
        .scan_external_transactions(reference(true).correlation(), scan)
        .await
        .unwrap();
    assert_eq!(
        result.coverage,
        ScanCoverage::Unavailable { block_number: 1 }
    );
    assert!(result.last_block.is_none());
}

#[tokio::test]
async fn failed_origin_receipt_still_requires_signed_sender_and_destination_association() {
    for field in ["from", "to"] {
        let mock = Mock::new();
        let reference = reference(false);
        {
            let mut state = mock.0.lock().unwrap();
            let receipt = state
                .receipts
                .get_mut(&reference.correlation().originating_tx_hash.to_string())
                .unwrap();
            receipt["status"] = json!("0x0");
            receipt[field] = json!("0x0000000000000000000000000000000000000001");
        }
        assert!(
            provider(&mock)
                .observe_conversion(&reference, request())
                .await
                .is_err()
        );
        assert_eq!(mock.calls("quai_getBlockByNumber"), 0);
    }
    let mock = Mock::new();
    let reference = reference(false);
    mock.0
        .lock()
        .unwrap()
        .receipts
        .get_mut(&reference.correlation().originating_tx_hash.to_string())
        .unwrap()["status"] = json!("0x0");
    assert!(matches!(
        provider(&mock)
            .observe_conversion(&reference, request())
            .await
            .unwrap()
            .origin,
        ConversionOriginObservation::Failed { .. }
    ));
}
