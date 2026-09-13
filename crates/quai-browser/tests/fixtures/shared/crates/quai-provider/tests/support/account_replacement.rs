//! Signed public-toy competitors, bounded coverage and hostile observation tests.
use quai_consensus::{QuaiTransaction, SignedQuaiTransaction};
use quai_crypto::SecretKey;
use quai_primitives::{Hash32, Zone};
use quai_provider::{Provider, ReplacementReason};
use quai_rpc::{Endpoint, Routing, RpcError, Transport, U256};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
pub(super) fn hash(n: u8) -> Hash32 {
    Hash32::from_bytes([n; 32])
}
fn key() -> SecretKey {
    let mut b = [0; 32];
    b[30..].copy_from_slice(&805u16.to_be_bytes());
    SecretKey::from_bytes(&b).unwrap()
}
pub(super) fn original() -> SignedQuaiTransaction {
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
pub(super) fn replacement(reason: ReplacementReason) -> SignedQuaiTransaction {
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
pub(super) struct Mock {
    tx: SignedQuaiTransaction,
    mode: u8,
    pub(super) counters: Arc<Mutex<[usize; 3]>>,
}
impl Mock {
    pub(super) fn new(tx: SignedQuaiTransaction, mode: u8) -> Self {
        Self {
            tx,
            mode,
            counters: Default::default(),
        }
    }
    pub(super) fn provider(&self) -> Provider<Self> {
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
                if self.mode == 15 && params[0] == "0x10f" {
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
