use quai_sdk::Endpoint;
use quai_sdk::rpc::{RpcError, Transport};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
pub const BLOCK: &str = "0x0000000000000000000000000000000000000000000000000000000000000010";
pub const HEAD: &str = "0x0000000000000000000000000000000000000000000000000000000000000011";
#[derive(Clone)]
pub struct RecoveryMock<T> {
    pub base: T,
    pub receipt: Value,
    pub change_head: bool,
    pub head_rechecked: Arc<AtomicBool>,
}
impl<T: Transport + Sync> Transport for RecoveryMock<T> {
    async fn request(
        &self,
        endpoint: &Endpoint,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        match method {
            "quai_getTransactionReceipt" => Ok(if self.receipt.get("transactionHash").is_some() {
                if self.receipt["transactionHash"] == params[0] {
                    self.receipt.clone()
                } else {
                    Value::Null
                }
            } else {
                self.receipt
                    .get(params[0].as_str().unwrap())
                    .cloned()
                    .unwrap_or(Value::Null)
            }),
            "quai_getHeaderByNumber" if params[0] != "0x0" => {
                let tip = params[0] != "0x10";
                let hash = if tip { HEAD } else { BLOCK };
                let changed = params[0] == "0x11" && self.change_head;
                if params[0] == "0x11" {
                    self.head_rechecked.store(true, Ordering::SeqCst);
                }
                Ok(
                    json!({"woHeader":{"hash":if changed {BLOCK} else {hash},"number":if tip {"0x11"} else {"0x10"},"location":"0x0000","parentHash":BLOCK,"primeTerminusNumber":"0x10"},"gasLimit":"0x100000","stateLimit":"0x100000"}),
                )
            }
            _ => self.base.request(endpoint, method, params).await,
        }
    }
}
pub fn receipt(hash: String, kind: u8, from: Option<String>, to: Option<String>) -> Value {
    json!({"transactionHash":hash,"type":format!("0x{kind:x}"),"from":from,"to":to,"blockHash":BLOCK,"blockNumber":"0x10","transactionIndex":"0x0","status":"0x1","gasUsed":"0x5208","cumulativeGasUsed":"0x5208","effectiveGasPrice":"0x1","logsBloom":format!("0x{}","00".repeat(10240)),"logs":[]})
}
