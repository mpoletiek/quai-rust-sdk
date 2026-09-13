//! Signed public-toy competitors, bounded coverage and hostile observation tests.
use quai_consensus::{QuaiTransaction, SignedQuaiTransaction};
use quai_crypto::SecretKey;
use quai_primitives::{Hash32, Zone};
use quai_provider::{
    AccountReplacementScanRequest, BlockReference, Provider, ProviderError, ReceiptOutcome,
    ReplacementReason,
};
use quai_rpc::{Endpoint, Routing, RpcError, Transport, U256};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
fn hash(n: u8) -> Hash32 {
    Hash32::from_bytes([n; 32])
}
fn key() -> SecretKey {
    let mut b = [0; 32];
    b[30..].copy_from_slice(&805u16.to_be_bytes());
    SecretKey::from_bytes(&b).unwrap()
}
fn original() -> SignedQuaiTransaction {
    QuaiTransaction {
        chain_id: U256::from(9),
        nonce: 5,
        to: Some(key().public_key().address()),
        value: U256::from(7),
        gas_limit: 21000,
        gas_price: U256::from(2),
        data: vec![],
        access_list: vec![],
    }
    .sign(&key())
    .unwrap()
}
fn replacement(reason: ReplacementReason) -> SignedQuaiTransaction {
    let mut tx = original().transaction().clone();
    tx.gas_price = U256::from(3);
    match reason {
        ReplacementReason::Repriced => {}
        ReplacementReason::Cancelled => tx.value = U256::ZERO,
        ReplacementReason::Replaced => {
            tx.to = Some(
                "0x0000000000000000000000000000000000000001"
                    .parse()
                    .unwrap(),
            )
        }
    }
    tx.sign(&key()).unwrap()
}
fn block_hash(n: u16) -> Hash32 {
    if n < 256 {
        hash(n as u8)
    } else {
        let mut b = [0x7f; 32];
        b[30..].copy_from_slice(&n.to_be_bytes());
        Hash32::from_bytes(b)
    }
}
fn rpc(tx: &SignedQuaiTransaction, height: u16) -> Value {
    let t = tx.transaction();
    json!({"hash":tx.hash().unwrap().to_string(),"type":"0x0","from":tx.from().to_string(),"to":t.to.map(|v|v.to_string()),
    "nonce":format!("{:#x}",t.nonce),"chainId":"0x9","gas":format!("{:#x}",t.gas_limit),"gasPrice":format!("{:#x}",t.gas_price),"value":format!("{:#x}",t.value),
    "input":quai_provider::RpcData::new(t.data.clone()).unwrap().to_hex(),"accessList":[],
    "r":format!("{:#x}",U256::from_be_bytes(tx.signature().r())),"s":format!("{:#x}",U256::from_be_bytes(tx.signature().s())),"v":format!("{:#x}",tx.signature().recovery_id()),
    "blockHash":block_hash(height).to_string(),"blockNumber":format!("0x{height:x}"),"transactionIndex":"0x0"})
}
fn header(n: u16) -> Value {
    json!({"gasLimit":"0x10000","stateLimit":"0x10000","woHeader":{"hash":block_hash(n).to_string(),"parentHash":block_hash(n-1).to_string(),"number":format!("0x{n:x}"),"primeTerminusNumber":"0x4","location":"0x0000"}})
}
#[derive(Clone)]
struct Mock {
    tx: SignedQuaiTransaction,
    mode: u8,
    counters: Arc<Mutex<[usize; 3]>>,
}
impl Mock {
    fn new(tx: SignedQuaiTransaction, mode: u8) -> Self {
        Self {
            tx,
            mode,
            counters: Default::default(),
        }
    }
    fn provider(&self) -> Provider<Self> {
        Provider::new(
            self.clone(),
            Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
            U256::from(9),
        )
    }
}
impl Transport for Mock {
    async fn request(&self, _: &Endpoint, method: &str, params: Value) -> Result<Value, RpcError> {
        self.counters.lock().unwrap()[0] += 1;
        if self.mode == 13 && method == "quai_getBlockByNumber" {
            return std::future::pending().await;
        }
        Ok(match method {
            "quai_chainId" => json!(if self.mode == 7 { "0xa" } else { "0x9" }),
            "quai_getHeaderByNumber" if params[0] == "0x0" => {
                let mut c = self.counters.lock().unwrap();
                c[1] += 1;
                json!({"woHeader":{"hash":hash(if self.mode==8 && c[1]>1 {2} else {1}).to_string(),"parentHash":Hash32::ZERO.to_string(),"number":"0x0","location":"0x"}})
            }
            "quai_getHeaderByNumber" => {
                let n = if params[0] == "latest" {
                    if self.mode == 14 { 272 } else { 17 }
                } else {
                    u16::from_str_radix(params[0].as_str().unwrap().trim_start_matches("0x"), 16)
                        .unwrap()
                };
                let mut h = header(n);
                if self.mode == 4 && params[0] == "0x11" {
                    h["woHeader"]["hash"] = json!(hash(9).to_string());
                }
                if self.mode == 9 && params[0] == "0xf" && self.counters.lock().unwrap()[2] > 1 {
                    h["woHeader"]["hash"] = json!(hash(9).to_string());
                }
                h
            }
            "quai_getBlockByNumber" => {
                assert_eq!(params[1], true);
                let n =
                    u16::from_str_radix(params[0].as_str().unwrap().trim_start_matches("0x"), 16)
                        .unwrap();
                if self.mode == 1 && n == 16 {
                    Value::Null
                } else {
                    let mut h = header(n);
                    h["hash"] = json!(block_hash(n).to_string());
                    let mut transactions = vec![];
                    if (self.mode != 14 && n == 16)
                        || (self.mode == 14 && n == 272)
                        || self.mode == 10
                    {
                        let mut tx = rpc(&self.tx, n);
                        if self.mode == 2 {
                            tx["value"] = json!("0xf");
                        }
                        transactions.push(tx);
                    }
                    h["transactions"] = json!(transactions);
                    if self.mode == 5 && n == 17 {
                        h["woHeader"]["parentHash"] = json!(hash(9).to_string());
                    }
                    h
                }
            }
            "quai_getTransactionReceipt" => {
                assert_eq!(params[0], self.tx.hash().unwrap().to_string());
                let mut c = self.counters.lock().unwrap();
                c[2] += 1;
                if self.mode == 12 || (self.mode == 6 && c[2] > 1) {
                    Value::Null
                } else {
                    json!({"transactionHash":self.tx.hash().unwrap().to_string(),"type":"0x0",
                    "from":if self.mode==3 {"0x0000000000000000000000000000000000000001".to_string()} else {self.tx.from().to_string()},"to":self.tx.transaction().to.map(|a|a.to_string()),
                    "blockHash":block_hash(if self.mode==14 {272} else {16}).to_string(),"blockNumber":if self.mode==14 {"0x110"} else {"0x10"},"transactionIndex":"0x0","status":if self.mode==11 {"0x0"} else {"0x1"},
                    "gasUsed":"0x5208","cumulativeGasUsed":"0x5208","effectiveGasPrice":"0x3","logs":[],"logsBloom":format!("0x{}","00".repeat(10240))})
                }
            }
            _ => panic!("unexpected read {method}"),
        })
    }
}
fn request() -> AccountReplacementScanRequest {
    AccountReplacementScanRequest {
        from_block: 16,
        to_block: 17,
        max_transactions_per_block: 16,
        max_total_transactions: 32,
        preceding_block: Some(BlockReference {
            number: 15,
            hash: hash(15),
        }),
    }
}
#[tokio::test]
async fn detects_unregistered_repricing_cancellation_and_changed_recipient() {
    for reason in [
        ReplacementReason::Repriced,
        ReplacementReason::Cancelled,
        ReplacementReason::Replaced,
    ] {
        let tx = replacement(reason);
        let m = Mock::new(tx.clone(), 0);
        let r = m
            .provider()
            .observe_account_replacements(&original(), hash(1), request())
            .await
            .unwrap();
        let c = r.candidate.unwrap();
        assert_eq!(c.reason, Some(reason));
        assert_eq!(
            c.transaction.signed_bytes().unwrap(),
            tx.signed_bytes().unwrap()
        );
        assert_eq!(c.confirmations, 2);
        assert_eq!(c.receipt.unwrap().outcome, ReceiptOutcome::Succeeded);
        assert_eq!(r.scanned_through.unwrap().number, 17);
        assert_eq!(r.missing_block, None);
    }
}
#[tokio::test]
async fn original_failed_and_missing_receipts_remain_distinct_from_missing_blocks() {
    for mode in [0, 11, 12] {
        let m = Mock::new(original(), mode);
        let r = m
            .provider()
            .observe_account_replacements(&original(), hash(1), request())
            .await
            .unwrap();
        let c = r.candidate.unwrap();
        assert_eq!(c.reason, None);
        assert_eq!(
            c.receipt.as_ref().map(|r| r.outcome),
            match mode {
                11 => Some(ReceiptOutcome::Failed),
                12 => None,
                _ => Some(ReceiptOutcome::Succeeded),
            }
        );
    }
    let m = Mock::new(original(), 1);
    let r = m
        .provider()
        .observe_account_replacements(&original(), hash(1), request())
        .await
        .unwrap();
    assert!(r.candidate.is_none());
    assert_eq!(r.missing_block, Some(16));
    assert!(r.scanned_through.is_none());
    let m = Mock::new(original(), 0);
    let mut q = request();
    q.from_block = 17;
    q.preceding_block = Some(BlockReference {
        number: 16,
        hash: hash(16),
    });
    let r = m
        .provider()
        .observe_account_replacements(&original(), hash(1), q)
        .await
        .unwrap();
    assert!(r.candidate.is_none());
    assert_eq!(r.scanned_through.unwrap().number, 17);
    assert!(r.missing_block.is_none());
}
#[tokio::test]
async fn forged_signatures_receipts_reorgs_and_competing_occupants_reject() {
    for mode in [2, 3, 4, 5, 6, 7, 8, 9, 10] {
        let m = Mock::new(replacement(ReplacementReason::Replaced), mode);
        assert!(
            m.provider()
                .observe_account_replacements(&original(), hash(1), request())
                .await
                .is_err(),
            "mode {mode}"
        );
    }
}

#[cfg(feature = "polling")]
#[tokio::test]
async fn waiter_returns_unregistered_failed_winner_and_bounds_missing_or_stalled_reads() {
    use quai_provider::{WaitConfig, WaitError};
    use std::time::Duration;
    let config = WaitConfig {
        confirmations: 2,
        timeout: Duration::from_millis(100),
        poll_interval: Duration::from_millis(1),
    };
    let m = Mock::new(replacement(ReplacementReason::Cancelled), 11);
    let c = m
        .provider()
        .wait_for_account_transaction(&original(), hash(1), 16, config)
        .await
        .unwrap();
    assert_eq!(c.reason, Some(ReplacementReason::Cancelled));
    assert_eq!(c.receipt.unwrap().outcome, ReceiptOutcome::Failed);
    for mode in [1, 12, 13] {
        let m = Mock::new(original(), mode);
        assert!(matches!(
            m.provider()
                .wait_for_account_transaction(&original(), hash(1), 16, config)
                .await,
            Err(WaitError::Timeout { .. })
        ));
        let count = m.counters.lock().unwrap()[0];
        tokio::time::sleep(Duration::from_millis(3)).await;
        assert_eq!(m.counters.lock().unwrap()[0], count);
    }
    let m = Mock::new(original(), 13);
    let p = m.provider();
    let original = original();
    let mut wait = Box::pin(p.wait_for_account_transaction(&original, hash(1), 16, config));
    tokio::select! {
        _ = &mut wait => panic!("stalled read completed"),
        _ = tokio::time::sleep(Duration::from_millis(3)) => {},
    }
    drop(wait);
    let count = m.counters.lock().unwrap()[0];
    tokio::time::sleep(Duration::from_millis(3)).await;
    assert_eq!(m.counters.lock().unwrap()[0], count);
}
#[tokio::test]
async fn invalid_page_and_scope_reject_before_io_and_budgets_do_not_silently_truncate() {
    for which in 0..6 {
        let mut q = request();
        match which {
            0 => q.from_block = 0,
            1 => q.to_block = 300,
            2 => q.max_transactions_per_block = 0,
            3 => q.max_total_transactions = 0,
            4 => q.preceding_block.as_mut().unwrap().number = 14,
            _ => q.to_block = 15,
        };
        let m = Mock::new(original(), 0);
        assert!(
            m.provider()
                .observe_account_replacements(&original(), hash(1), q)
                .await
                .is_err()
        );
        assert_eq!(m.counters.lock().unwrap()[0], 0);
    }
    let m = Mock::new(original(), 0);
    assert!(
        m.provider()
            .observe_account_replacements(&original(), Hash32::ZERO, request())
            .await
            .is_err()
    );
    assert_eq!(m.counters.lock().unwrap()[0], 0);
    let mut q = request();
    q.max_total_transactions = 1;
    let m = Mock::new(original(), 10);
    assert!(matches!(
        m.provider()
            .observe_account_replacements(&original(), hash(1), q)
            .await,
        Err(ProviderError::InvalidResult(
            "replacement scan budget exceeded"
        ))
    ));
}

#[cfg(feature = "polling")]
#[tokio::test]
async fn waiter_drains_multiple_bounded_pages_without_skipping_the_next_block() {
    let m = Mock::new(replacement(ReplacementReason::Repriced), 14);
    let c = m
        .provider()
        .wait_for_account_transaction(
            &original(),
            hash(1),
            16,
            quai_provider::WaitConfig {
                confirmations: 1,
                timeout: std::time::Duration::from_secs(5),
                poll_interval: std::time::Duration::from_millis(1),
            },
        )
        .await
        .unwrap();
    assert_eq!(c.inclusion.block_number, 272);
    assert_eq!(c.reason, Some(ReplacementReason::Repriced));
    assert_eq!(c.confirmations, 1);
}
