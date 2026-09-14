//! Wallet-facing read RPC contract and captured real-node response regressions.
use quai_primitives::{Hash32, QiAddress, Zone};
use quai_provider::{
    BlockTag, CallRequest, MAX_RPC_DATA_BYTES, Provider, Receipt, ReceiptOutcome, RpcData,
    Transaction, TransactionDetails, TransactionKind,
};
use quai_rpc::{Endpoint, Routing, RpcError, Transport, U256};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

const URL: &str = "http://127.0.0.1:9200/rpc?fixture=public";
const ADDRESS: &str = "0x0000000000000000000000000000000000000000";
const QI: &str = "0x0080000000000000000000000000000000000000";
const HASH: &str = "0x00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

#[tokio::test]
async fn grouped_outpoints_bound_concurrency_and_drop_pending_reads_on_failure() {
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    #[derive(Clone, Default)]
    struct Delayed {
        active: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
        started: Arc<AtomicUsize>,
        fail: bool,
    }
    struct Active(Arc<AtomicUsize>);
    impl Drop for Active {
        fn drop(&mut self) {
            self.0.fetch_sub(1, SeqCst);
        }
    }
    impl Transport for Delayed {
        async fn request(&self, _: &Endpoint, method: &str, _: Value) -> Result<Value, RpcError> {
            if method == "quai_chainId" {
                return Ok(json!("0x9"));
            }
            assert_eq!(method, "quai_getOutpointsByAddress");
            let order = self.started.fetch_add(1, SeqCst);
            let active = self.active.fetch_add(1, SeqCst) + 1;
            self.peak.fetch_max(active, SeqCst);
            let _active = Active(self.active.clone());
            tokio::task::yield_now().await;
            if self.fail && order == 0 {
                return Err(RpcError::Timeout);
            }
            Ok(json!([]))
        }
    }
    let addresses: Vec<QiAddress> = (0..9)
        .map(|n| {
            format!("0x00800000000000000000000000000000000000{n:02x}")
                .parse()
                .unwrap()
        })
        .collect();
    for fail in [false, true] {
        let transport = Delayed {
            fail,
            ..Default::default()
        };
        let provider = Provider::new(
            transport.clone(),
            Routing::direct(URL, Zone::Cyprus1.into()).unwrap(),
            U256::from(9),
        );
        let result = provider.outpoints_many(&addresses).await;
        assert_eq!(transport.peak.load(SeqCst), 4);
        assert_eq!(transport.active.load(SeqCst), 0);
        if fail {
            assert!(matches!(
                result,
                Err(quai_provider::ProviderError::Rpc(RpcError::Timeout))
            ));
            assert_eq!(transport.started.load(SeqCst), 4);
        } else {
            let result = result.unwrap();
            assert_eq!(result.len(), addresses.len());
            assert!(addresses.iter().all(|a| result[a].is_empty()));
            assert_eq!(transport.started.load(SeqCst), 9);
        }
    }
}

type ExpectedRead = (String, Value, Result<Value, RpcError>);

#[tokio::test]
async fn grouped_outpoints_use_batches_and_reject_chain_changes_without_replay() {
    #[derive(Clone)]
    struct Batch {
        wrong_chain: bool,
    }
    impl Transport for Batch {
        async fn request(&self, _: &Endpoint, _: &str, _: Value) -> Result<Value, RpcError> {
            panic!("batch must not fall back to individual RPC")
        }
        async fn request_batch(
            &self,
            _: &Endpoint,
            requests: Vec<(&str, Value)>,
        ) -> Option<quai_rpc::BatchResult> {
            assert_eq!(requests.len(), 4);
            assert_eq!(requests[0].0, "quai_chainId");
            assert_eq!(requests[1].0, "quai_getOutpointsByAddress");
            assert_eq!(requests[3].0, "quai_chainId");
            Some(Ok(vec![
                Ok(json!("0x9")),
                Ok(json!([])),
                Ok(json!([])),
                Ok(json!(if self.wrong_chain { "0xa" } else { "0x9" })),
            ]))
        }
    }
    let addresses = [
        QI.parse().unwrap(),
        "0x0080000000000000000000000000000000000001"
            .parse()
            .unwrap(),
    ];
    for wrong_chain in [false, true] {
        let provider = Provider::new(
            Batch { wrong_chain },
            Routing::direct(URL, Zone::Cyprus1.into()).unwrap(),
            U256::from(9),
        );
        let result = provider.outpoints_many(&addresses).await;
        if wrong_chain {
            assert!(matches!(
                result,
                Err(quai_provider::ProviderError::ChainMismatch { .. })
            ));
        } else {
            assert_eq!(result.unwrap().len(), 2);
        }
    }
}
#[derive(Clone, Default)]
struct Mock(Arc<Mutex<VecDeque<ExpectedRead>>>);
impl Mock {
    fn read(method: &str, params: Value, response: Value, chain: u64) -> Self {
        Self(Arc::new(Mutex::new(VecDeque::from([
            (
                "quai_chainId".into(),
                json!([]),
                Ok(json!(format!("0x{chain:x}"))),
            ),
            (method.into(), params, Ok(response)),
        ]))))
    }
    fn provider(&self, chain: u64) -> Provider<Self> {
        Provider::new(
            self.clone(),
            Routing::direct(URL, Zone::Cyprus1.into()).unwrap(),
            U256::from(chain),
        )
    }
    fn drained(&self) {
        assert!(self.0.lock().unwrap().is_empty());
    }
}
impl Transport for Mock {
    async fn request(
        &self,
        endpoint: &Endpoint,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        assert_eq!(endpoint.as_str(), URL);
        let (expected_method, expected_params, result) = self
            .0
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected request");
        assert_eq!(method, expected_method);
        assert_eq!(params, expected_params);
        result
    }
}
fn fixture(name: &str) -> Value {
    serde_json::from_str(match name {
        "orchard" => include_str!("fixtures/orchard.json"),
        _ => include_str!("fixtures/lan-mainnet.json"),
    })
    .unwrap()
}
fn receipt() -> Value {
    fixture("lan")["records"][0]["receipt"].clone()
}
fn quai() -> Value {
    fixture("lan")["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["transaction"]["type"] == "0x0")
        .unwrap()["transaction"]
        .clone()
}
fn qi() -> Value {
    json!({"blockHash":null,"blockNumber":null,"transactionIndex":null,"hash":HASH,
           "type":"0x2","gas":"0x0","nonce":"0x0","chainId":"0x9","input":"0x",
           "inputs":[{"previousOutPoint":{"txHash":HASH,"index":"0xffff"},"pubKey":format!("0x02{}","11".repeat(32))}],
           "outputs":[{"denomination":"0xe","address":QI,"lock":null}],
           "utxoSignature":format!("0x{}","00".repeat(64))})
}

#[test]
fn parse_captured_etx_and_account_receipts_from_both_node_versions() {
    let mut count = 0;
    for name in ["orchard", "lan"] {
        for record in fixture(name)["records"].as_array().unwrap() {
            let tx = Transaction::try_from(record["transaction"].clone()).unwrap();
            let receipt = Receipt::try_from(record["receipt"].clone()).unwrap();
            assert_eq!(tx.hash, receipt.transaction_hash);
            assert_eq!(tx.inclusion, Some(receipt.inclusion));
            assert_eq!(tx.kind(), receipt.kind);
            assert_eq!(receipt.logs_bloom.bytes().len(), 10_240);
            if let TransactionDetails::External(etx) = tx.details {
                assert_eq!(receipt.originating_tx_hash, Some(etx.originating_tx_hash));
            }
            count += 1;
        }
    }
    assert!(count >= 8);
}

#[tokio::test]
async fn captured_read_methods_route_and_decode_with_exact_parameters() {
    for name in ["orchard", "lan"] {
        let fixture = fixture(name);
        let chain = fixture["provenance"]["chain_id"].as_u64().unwrap();
        for read in fixture["reads"].as_array().unwrap() {
            let method = read["method"].as_str().unwrap();
            let mock = Mock::read(
                method,
                read["params"].clone(),
                read["result"].clone(),
                chain,
            );
            let provider = mock.provider(chain);
            let address = ADDRESS.parse().unwrap();
            let block = BlockTag::Number(
                quai_rpc::parse_quantity(fixture["provenance"]["zone_height"].as_str().unwrap())
                    .unwrap(),
            );
            match method {
                "quai_gasPrice" => {
                    assert!(provider.gas_price(Zone::Cyprus1).await.unwrap() > U256::ZERO);
                }
                "quai_getTransactionCount" => {
                    provider.transaction_count(address, block).await.unwrap();
                }
                "quai_getCode" => {
                    assert!(
                        provider
                            .code(address, block)
                            .await
                            .unwrap()
                            .bytes()
                            .is_empty()
                    );
                }
                "quai_getStorageAt" => {
                    assert_eq!(
                        provider
                            .storage_at(address, U256::ZERO, block)
                            .await
                            .unwrap(),
                        Hash32::ZERO
                    );
                }
                "quai_getOutpointsByAddress" => {
                    assert!(
                        provider
                            .outpoints(QI.parse().unwrap())
                            .await
                            .unwrap()
                            .is_empty()
                    );
                }
                _ => panic!("unexpected fixture method"),
            }
            mock.drained();
        }
    }
}

#[tokio::test]
async fn lookup_hash_and_embedded_chain_are_checked() {
    let transaction = quai();
    let hash: Hash32 = transaction["hash"].as_str().unwrap().parse().unwrap();
    let mock = Mock::read(
        "quai_getTransactionByHash",
        json!([hash.to_string()]),
        transaction.clone(),
        9,
    );
    assert_eq!(
        mock.provider(9)
            .transaction(Zone::Cyprus1, hash)
            .await
            .unwrap()
            .unwrap()
            .hash,
        hash
    );
    mock.drained();
    for (field, replacement) in [("hash", json!(HASH)), ("chainId", json!("0x3a98"))] {
        let mut wrong = transaction.clone();
        wrong[field] = replacement;
        let mock = Mock::read(
            "quai_getTransactionByHash",
            json!([hash.to_string()]),
            wrong,
            9,
        );
        assert!(
            mock.provider(9)
                .transaction(Zone::Cyprus1, hash)
                .await
                .is_err()
        );
        mock.drained();
    }
    let hash: Hash32 = HASH.parse().unwrap();
    let mock = Mock::read("quai_getTransactionReceipt", json!([HASH]), receipt(), 9);
    assert!(mock.provider(9).receipt(Zone::Cyprus1, hash).await.is_err());
    mock.drained();
}

#[tokio::test]
async fn null_lookups_preserve_unknown_instead_of_fabricated_objects() {
    let hash = HASH.parse().unwrap();
    let mock = Mock::read("quai_getTransactionByHash", json!([HASH]), Value::Null, 9);
    assert!(
        mock.provider(9)
            .transaction(Zone::Cyprus1, hash)
            .await
            .unwrap()
            .is_none()
    );
    mock.drained();
    let mock = Mock::read("quai_getTransactionReceipt", json!([HASH]), Value::Null, 9);
    assert!(
        mock.provider(9)
            .receipt(Zone::Cyprus1, hash)
            .await
            .unwrap()
            .is_none()
    );
    mock.drained();
}

#[tokio::test]
async fn simulations_use_actual_input_gasprice_txtype_and_block_contract() {
    let from = ADDRESS.parse().unwrap();
    let mut request = CallRequest::new(from, from);
    request.gas = Some(21_000);
    request.gas_price = Some(U256::MAX);
    request.value = Some(U256::from(1));
    request.nonce = Some(u64::MAX);
    request.input = "0x0001".parse().unwrap();
    let args = json!({"from":ADDRESS,"to":ADDRESS,"txType":0,"input":"0x0001","gas":"0x5208","gasPrice":format!("{:#x}",U256::MAX),"value":"0x1","nonce":"0xffffffffffffffff"});
    let mock = Mock::read(
        "quai_call",
        json!([args.clone(), "pending"]),
        json!("0x0000"),
        9,
    );
    assert_eq!(
        mock.provider(9)
            .call(&request, BlockTag::Pending)
            .await
            .unwrap()
            .bytes(),
        &[0, 0]
    );
    mock.drained();
    let mock = Mock::read("quai_estimateGas", json!([args, "0x1"]), json!("0x5208"), 9);
    assert_eq!(
        mock.provider(9)
            .estimate_gas(&request, BlockTag::Number(U256::from(1)))
            .await
            .unwrap(),
        21_000
    );
    mock.drained();
}

#[test]
fn raw_call_objects_reject_ambiguous_unsupported_and_overflow_fields() {
    let valid = json!({"from":ADDRESS,"to":ADDRESS,"input":"0x01","txType":0});
    assert!(CallRequest::try_from(valid.clone()).is_ok());
    for (key, value) in [
        ("data", json!("0x01")),
        ("maxFeePerGas", json!("0x1")),
        ("gas", json!("0x10000000000000000")),
        ("txType", json!(2)),
        ("txType", json!("0x0")),
        ("from", json!(QI)),
        ("value", json!(1)),
        ("input", json!("0x1")),
        ("chainId", json!("0x9")),
    ] {
        let mut invalid = valid.clone();
        invalid[key] = value;
        assert!(CallRequest::try_from(invalid).is_err(), "accepted {key}");
    }
}

#[tokio::test]
async fn invalid_simulations_and_out_of_range_selectors_stop_before_io() {
    let from = ADDRESS.parse().unwrap();
    let mut request = CallRequest::new(from, from);
    let mock = Mock::default();
    request.to = None;
    assert!(
        mock.provider(9)
            .estimate_gas(&request, BlockTag::Latest)
            .await
            .is_err()
    );
    assert!(
        mock.provider(9)
            .code(from, BlockTag::Number(U256::MAX))
            .await
            .is_err()
    );
    assert!(
        mock.provider(9)
            .transaction_count(from, BlockTag::Number(U256::MAX))
            .await
            .is_err()
    );
    assert!(
        mock.provider(9)
            .storage_at(from, U256::ZERO, BlockTag::Number(U256::MAX))
            .await
            .is_err()
    );
    mock.drained();
}

#[test]
fn bytes_are_bounded_preserve_zeroes_and_do_not_leak_in_debug() {
    let value: RpcData = "0x0001aB".parse().unwrap();
    assert_eq!(value.to_hex(), "0x0001ab");
    for input in [
        "0x1".into(),
        "0X00".into(),
        "00".into(),
        "0xé".into(),
        format!("0x{}", "00".repeat(MAX_RPC_DATA_BYTES + 1)),
    ] {
        assert!(input.parse::<RpcData>().is_err());
    }
    let data = RpcData::new(b"PRIVATE_DATA".to_vec()).unwrap();
    assert!(!format!("{data:?}").contains("PRIVATE_DATA"));
}

#[tokio::test]
async fn uint64_reads_and_storage_words_reject_overflow_or_wrong_width() {
    let from = ADDRESS.parse().unwrap();
    let mock = Mock::read(
        "quai_getTransactionCount",
        json!([ADDRESS, "pending"]),
        json!("0x10000000000000000"),
        9,
    );
    assert!(
        mock.provider(9)
            .transaction_count(from, BlockTag::Pending)
            .await
            .is_err()
    );
    let mock = Mock::read(
        "quai_getStorageAt",
        json!([ADDRESS, format!("{:#x}", U256::MAX), "latest"]),
        json!("0x0"),
        9,
    );
    assert!(
        mock.provider(9)
            .storage_at(from, U256::MAX, BlockTag::Latest)
            .await
            .is_err()
    );
}

#[test]
fn transaction_variants_fail_closed_and_preserve_pending_and_wide_amounts() {
    let tx = Transaction::try_from(qi()).unwrap();
    assert_eq!(tx.kind(), TransactionKind::Qi);
    assert!(tx.inclusion.is_none());
    let mut account = quai();
    account["value"] = json!(format!("{:#x}", U256::MAX));
    account["futureField"] = json!("PRIVATE_EXTENSION");
    let tx = Transaction::try_from(account).unwrap();
    assert_eq!(tx.extensions.fields()["futureField"], "PRIVATE_EXTENSION");
    assert!(!format!("{tx:?}").contains("PRIVATE_EXTENSION"));
    for (key, value) in [
        ("type", json!("0x3")),
        ("blockNumber", Value::Null),
        ("nonce", json!("0x10000000000000000")),
        ("hash", json!("0x01")),
    ] {
        let mut tx = quai();
        tx[key] = value;
        assert!(Transaction::try_from(tx).is_err());
    }
    for (key, value) in [
        ("utxoSignature", json!("0x00")),
        ("gas", json!("0x1")),
        ("nonce", json!("0x1")),
    ] {
        let mut tx = qi();
        tx[key] = value;
        assert!(Transaction::try_from(tx).is_err());
    }
    let mut tx = qi();
    tx["inputs"][0]["pubKey"] = json!(format!("0x05{}", "11".repeat(32)));
    assert!(Transaction::try_from(tx).is_err());
    let mut tx = qi();
    tx["outputs"][0]["denomination"] = json!("0xf");
    assert!(Transaction::try_from(tx).is_err());
}

#[test]
fn receipt_rejects_ethereum_bloom_wrong_status_and_log_association() {
    for (key, value) in [
        ("logsBloom", json!(format!("0x{}", "00".repeat(256)))),
        ("status", json!("0x3")),
        ("root", json!(HASH)),
        ("etxType", Value::Null),
        ("blockHash", Value::Null),
    ] {
        let mut r = receipt();
        r[key] = value;
        assert!(Receipt::try_from(r).is_err(), "accepted {key}");
    }
    let mut r = receipt();
    r["logs"] = json!([{"address":ADDRESS,"topics":[],"data":"0x","transactionHash":HASH,
        "blockHash":r["blockHash"],"blockNumber":r["blockNumber"],"transactionIndex":r["transactionIndex"],"logIndex":"0x0","removed":false}]);
    assert!(Receipt::try_from(r.clone()).is_err());
    r["logs"][0]["transactionHash"] = r["transactionHash"].clone();
    assert!(Receipt::try_from(r).is_ok());
    let mut r = receipt();
    r.as_object_mut().unwrap().remove("status");
    r["root"] = json!(HASH);
    assert!(matches!(
        Receipt::try_from(r).unwrap().outcome,
        ReceiptOutcome::PostState(_)
    ));
}

#[tokio::test]
async fn outpoint_index_results_validate_duplicates_bounds_and_locks() {
    let qi: QiAddress = QI.parse().unwrap();
    let item = json!({"txHash":HASH,"index":"0xffff","denomination":"0xe","lock":format!("{:#x}",U256::MAX)});
    let mock = Mock::read(
        "quai_getOutpointsByAddress",
        json!([qi.to_string()]),
        json!([item]),
        9,
    );
    let out = mock.provider(9).outpoints(qi).await.unwrap();
    assert_eq!(out[0].lock, U256::MAX);
    assert_eq!(out[0].outpoint.index, u16::MAX);
    for bad in [
        json!([item, item]),
        json!([{ "txHash":HASH,"index":"0x10000","denomination":"0x0","lock":"0x0"}]),
        json!([{ "txHash":HASH,"index":"0x0","denomination":"0xf","lock":"0x0"}]),
    ] {
        let mock = Mock::read(
            "quai_getOutpointsByAddress",
            json!([qi.to_string()]),
            bad,
            9,
        );
        assert!(mock.provider(9).outpoints(qi).await.is_err());
    }
}

#[tokio::test]
async fn typed_conversion_quotes_preserve_units_selectors_and_missing_data() {
    let mock = Mock::read(
        "quai_qiToQuai",
        json!(["0x3e8", "0x10"]),
        json!("0x10000000000000000"),
        9,
    );
    let quote = mock
        .provider(9)
        .qi_to_quai(
            Zone::Cyprus1,
            U256::from(1000),
            BlockTag::Number(U256::from(16)),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(quote, U256::from(1) << 64);
    mock.drained();
    let mock = Mock::read("quai_quaiToQi", json!(["0x1", "latest"]), Value::Null, 9);
    assert!(
        mock.provider(9)
            .quai_to_qi(Zone::Cyprus1, U256::from(1), BlockTag::Latest)
            .await
            .unwrap()
            .is_none()
    );
    mock.drained();
    let from = ADDRESS.parse().unwrap();
    let to = QI.parse().unwrap();
    let mock = Mock::read(
        "quai_calculateConversionAmount",
        json!([{"from":ADDRESS,"to":QI,"value":"0x2a"}]),
        json!("0x3"),
        9,
    );
    assert_eq!(
        mock.provider(9)
            .calculate_conversion_amount(from, to, U256::from(42))
            .await
            .unwrap(),
        U256::from(3)
    );
    mock.drained();
    assert!(
        Mock::default()
            .provider(9)
            .calculate_conversion_amount(from, from, U256::from(1))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn wrapped_qi_protocol_deposit_and_delta_queries_have_strict_identity_and_bounds() {
    let owner = "0x002b2596EcF05C93a31ff916E8b456DF6C77c750";
    let mock = Mock::read(
        "quai_getWrappedQiDeposit",
        json!([owner, ADDRESS, "latest"]),
        json!("0x3e8"),
        9,
    );
    assert_eq!(
        mock.provider(9)
            .wrapped_qi_deposit(
                owner.parse().unwrap(),
                ADDRESS.parse().unwrap(),
                BlockTag::Latest
            )
            .await
            .unwrap(),
        U256::from(1000)
    );
    mock.drained();
    let hash: Hash32 = HASH.parse().unwrap();
    let address: QiAddress = QI.parse().unwrap();
    let response = json!({QI:{"created":{HASH:[{"index":"0x0","denomination":"0x2","lock":"0x10"}]},"deleted":{}}});
    let mock = Mock::read(
        "quai_getOutpointDeltasForAddressesInRange",
        json!([[QI], HASH, HASH]),
        response,
        9,
    );
    let changes = mock
        .provider(9)
        .outpoint_deltas(Zone::Cyprus1, &[address], hash, hash)
        .await
        .unwrap();
    assert_eq!(changes[&address].created[0].outpoint.tx_hash, hash);
    assert_eq!(changes[&address].created[0].lock, U256::from(16));
    mock.drained();
    assert!(
        Mock::default()
            .provider(9)
            .outpoint_deltas(Zone::Cyprus1, &[address, address], hash, hash)
            .await
            .is_err()
    );
    let mock = Mock::read(
        "quai_getOutpointDeltasForAddressesInRange",
        json!([[QI], HASH, HASH]),
        json!({}),
        9,
    );
    assert!(
        mock.provider(9)
            .outpoint_deltas(Zone::Cyprus1, &[address], hash, hash)
            .await
            .is_err()
    );
    mock.drained();
}

#[tokio::test]
async fn cross_zone_account_simulation_routes_by_sender_and_preserves_destination() {
    let from = ADDRESS.parse().unwrap();
    let destination = "0x1000000000000000000000000000000000000000";
    let request = CallRequest::new(from, destination.parse().unwrap());
    let mock = Mock::read(
        "quai_estimateGas",
        json!([{"from": ADDRESS, "to": destination, "input": "0x", "txType": 0}, "latest"]),
        json!("0x5208"),
        9,
    );
    assert_eq!(
        mock.provider(9)
            .estimate_gas(&request, BlockTag::Latest)
            .await
            .unwrap(),
        21000
    );
    mock.drained();
}

#[tokio::test]
async fn access_list_creation_preserves_order_and_rejects_vm_errors_and_overflow() {
    let request = CallRequest::new(ADDRESS.parse().unwrap(), ADDRESS.parse().unwrap());
    let params = json!([{"from":ADDRESS,"to":ADDRESS,"input":"0x","txType":0},"latest"]);
    let response =
        json!({"accessList":[{"address":ADDRESS,"storageKeys":[HASH]}],"gasUsed":"0x5208"});
    let mock = Mock::read("quai_createAccessList", params.clone(), response.clone(), 9);
    let estimate = mock
        .provider(9)
        .create_access_list(&request, BlockTag::Latest)
        .await
        .unwrap();
    assert_eq!(estimate.gas_used, 21000);
    assert_eq!(
        estimate.access_list[0].storage_keys,
        [HASH.parse().unwrap()]
    );
    mock.drained();
    for (field, value) in [
        ("error", json!("execution reverted")),
        ("gasUsed", json!("0x10000000000000000")),
        ("accessList", json!(null)),
        ("future", json!(true)),
    ] {
        let mut response = response.clone();
        response[field] = value;
        let mock = Mock::read("quai_createAccessList", params.clone(), response, 9);
        assert!(
            mock.provider(9)
                .create_access_list(&request, BlockTag::Latest)
                .await
                .is_err()
        );
        mock.drained();
    }
}

#[tokio::test]
async fn pool_content_checks_sender_nonce_chain_and_pending_identity() {
    let mut pending = quai();
    for field in ["blockHash", "blockNumber", "transactionIndex"] {
        pending[field] = Value::Null;
    }
    let address = pending["from"].as_str().unwrap().to_owned();
    let nonce = quai_rpc::parse_quantity(pending["nonce"].as_str().unwrap())
        .unwrap()
        .to_string();
    let content = json!({"pending":{address.clone():{nonce.clone():pending.clone()}},"queued":{}});
    let mock = Mock::read("txpool_content", json!([]), content.clone(), 9);
    let rows = mock
        .provider(9)
        .pool_content(Zone::Cyprus1, 16)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].queued);
    mock.drained();
    for field in ["from", "nonce", "chainId", "blockHash"] {
        let mut content = content.clone();
        content["pending"][&address][&nonce][field] = match field {
            "from" => json!(ADDRESS),
            "nonce" => json!("0xffff"),
            "chainId" => json!("0x539"),
            _ => json!(HASH),
        };
        let mock = Mock::read("txpool_content", json!([]), content, 9);
        assert!(
            mock.provider(9)
                .pool_content(Zone::Cyprus1, 16)
                .await
                .is_err(),
            "{field}"
        );
        mock.drained();
    }
    let mock = Mock::read(
        "txpool_inspect",
        json!([]),
        json!({"pending":{ADDRESS:{"7":"public diagnostic"}},"queued":{}}),
        9,
    );
    let rows = mock
        .provider(9)
        .pool_inspect(Zone::Cyprus1, 1)
        .await
        .unwrap();
    assert_eq!(rows[0].nonce, 7);
    assert_eq!(rows[0].summary, "public diagnostic");
    assert!(!format!("{:?}", rows[0]).contains("public diagnostic"));
    mock.drained();
    let mock = Mock::read(
        "txpool_content",
        json!([]),
        json!({"pending":{address:{"00":pending}},"queued":{}}),
        9,
    );
    assert!(
        mock.provider(9)
            .pool_content(Zone::Cyprus1, 16)
            .await
            .is_err()
    );
    let mock = Mock::default();
    assert!(
        mock.provider(9)
            .pool_content(Zone::Cyprus1, 0)
            .await
            .is_err()
    );
    mock.drained();
}

#[tokio::test]
async fn pool_counts_pending_wire_and_advertised_regions_have_explicit_routes() {
    let mock = Mock::read(
        "txpool_status",
        json!([]),
        json!({"pending":"0x2","queued":"0x1","qi":"0x5"}),
        9,
    );
    assert_eq!(
        mock.provider(9).pool_status(Zone::Cyprus1).await.unwrap(),
        quai_provider::PoolStatus {
            pending: 2,
            queued: 1,
            qi: 5
        }
    );
    mock.drained();
    let mock = Mock::read("quai_getPendingHeader", json!([]), json!("0x000102"), 9);
    assert_eq!(
        mock.provider(9)
            .pending_header_bytes(Zone::Cyprus1)
            .await
            .unwrap()
            .bytes(),
        [0, 1, 2]
    );
    mock.drained();
    let mock = Mock::read(
        "quai_getProtocolExpansionNumber",
        json!([]),
        json!("0x3"),
        9,
    );
    assert_eq!(
        mock.provider(9)
            .protocol_expansion(Zone::Cyprus1.into())
            .await
            .unwrap(),
        3
    );
    mock.drained();
    let mock = Mock::read(
        "quai_listRunningChains",
        json!([]),
        json!([[0, 0], [2, 1]]),
        9,
    );
    let provider = Provider::new(
        mock.clone(),
        Routing::direct(URL, quai_primitives::Shard::Prime).unwrap(),
        U256::from(9),
    );
    assert_eq!(
        provider.running_regions().await.unwrap(),
        [
            quai_primitives::Region::Cyprus,
            quai_primitives::Region::Hydra
        ]
    );
    mock.drained();
}
