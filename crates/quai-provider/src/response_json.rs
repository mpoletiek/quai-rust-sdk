//! Explicit normalized JSON export of typed node observations; no signing or proofs.
use crate::{
    Inclusion, Log, ProviderError, Receipt, ReceiptOutcome, Transaction, TransactionDetails,
    TransactionKind, types,
};
use quai_rpc::U256;
use serde_json::{Map, Value, json};
fn inclusion(value: &mut Value, observed: Option<Inclusion>) {
    value["blockHash"] = json!(observed.map(|i| i.block_hash.to_string()));
    value["blockNumber"] = json!(observed.map(|i| format!("{:#x}", i.block_number)));
    value["transactionIndex"] = json!(observed.map(|i| format!("{:#x}", i.transaction_index)));
}
fn kind(kind: TransactionKind) -> &'static str {
    match kind {
        TransactionKind::Quai => "0x0",
        TransactionKind::External => "0x1",
        TransactionKind::Qi => "0x2",
    }
}
impl Transaction {
    /// Export normalized node RPC JSON with exact hexadecimal quantities and
    /// retained top-level extensions. Inactive ETX nonce is normalized to zero;
    /// JSON spelling and omitted optional fields are not byte-for-byte preserved.
    /// Revalidates public fields before returning; this does not verify signatures.
    pub fn to_rpc_json(&self) -> Result<Value, ProviderError> {
        let mut v = Value::Object(self.extensions.fields().clone());
        v["type"] = json!(kind(self.kind()));
        v["hash"] = json!(self.hash.to_string());
        v["input"] = json!(self.input.to_hex());
        inclusion(&mut v, self.inclusion);
        match &self.details {
            TransactionDetails::Quai(t) => {
                v["from"] = json!(t.from.to_string());
                v["to"] = json!(t.to.map(|a| a.to_string()));
                v["gas"] = json!(format!("{:#x}", t.gas));
                v["nonce"] = json!(format!("{:#x}", t.nonce));
                v["gasPrice"] = json!(format!("{:#x}", t.gas_price));
                v["value"] = json!(format!("{:#x}", t.value));
                v["chainId"] = json!(format!("{:#x}", t.chain_id));
                v["v"] = json!(format!("{:#x}", t.signature.v));
                v["r"] = json!(format!("{:#x}", t.signature.r));
                v["s"] = json!(format!("{:#x}", t.signature.s));
                v["accessList"]=json!(t.access_list.iter().map(|a|json!({"address":a.address.to_string(),"storageKeys":a.storage_keys.iter().map(ToString::to_string).collect::<Vec<_>>()})).collect::<Vec<_>>());
            }
            TransactionDetails::External(t) => {
                v["from"] = json!(t.from.to_string());
                v["to"] = json!(t.to.to_string());
                v["gas"] = json!(format!("{:#x}", t.gas));
                v["nonce"] = json!("0x0");
                v["value"] = json!(format!("{:#x}", t.value));
                v["originatingTxHash"] = json!(t.originating_tx_hash.to_string());
                v["etxIndex"] = json!(format!("{:#x}", t.etx_index));
                v["etxType"] = json!(format!("{:#x}", t.etx_type));
            }
            TransactionDetails::Qi(t) => {
                v["gas"] = json!("0x0");
                v["nonce"] = json!("0x0");
                v["chainId"] = json!(format!("{:#x}", t.chain_id));
                v["utxoSignature"] = json!(t.signature.to_hex());
                v["inputs"]=json!(t.inputs.iter().map(|i|json!({"previousOutPoint":{"txHash":i.previous_out_point.tx_hash.to_string(),"index":format!("{:#x}",i.previous_out_point.index)},"pubKey":i.public_key.to_hex()})).collect::<Vec<_>>());
                v["outputs"]=json!(t.outputs.iter().map(|o|json!({"address":o.address.to_string(),"denomination":format!("{:#x}",o.denomination),"lock":o.lock.map(|n|format!("{n:#x}"))})).collect::<Vec<_>>());
            }
        }
        Self::try_from(v.clone())?;
        Ok(v)
    }
}
impl TryFrom<Value> for Log {
    type Error = ProviderError;
    fn try_from(value: Value) -> Result<Self, Self::Error> {
        types::parse_log(value)
    }
}
impl Log {
    /// Export normalized node RPC JSON, preserving unknown top-level fields and
    /// revalidating public metadata. Removed status is a source claim, not a proof.
    pub fn to_rpc_json(&self) -> Result<Value, ProviderError> {
        let mut v = Value::Object(self.extensions.fields().clone());
        v["address"] = json!(self.address.to_string());
        v["topics"] = json!(
            self.topics
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );
        v["data"] = json!(self.data.to_hex());
        v["transactionHash"] = json!(self.transaction_hash.to_string());
        v["logIndex"] = json!(format!("{:#x}", self.log_index));
        v["removed"] = json!(self.removed);
        inclusion(&mut v, Some(self.inclusion));
        Self::try_from(v.clone())?;
        Ok(v)
    }
}
impl Receipt {
    /// Actual execution gas debit in Its; checked U256 arithmetic rejects overflow.
    /// Value transfers, ETX effects and other accounting are not included.
    pub fn fee(&self) -> Result<U256, ProviderError> {
        self.effective_gas_price
            .checked_mul(U256::from(self.gas_used))
            .ok_or(ProviderError::InvalidResult(
                "receipt gas fee overflows U256",
            ))
    }
    /// Export normalized node receipt JSON with logs, outbound ETXs, exact
    /// quantities, status/root and preserved top-level extensions. Revalidates
    /// associations and public fields; it does not establish chain canonicality.
    pub fn to_rpc_json(&self) -> Result<Value, ProviderError> {
        let mut v = Value::Object(self.extensions.fields().clone());
        v["transactionHash"] = json!(self.transaction_hash.to_string());
        inclusion(&mut v, Some(self.inclusion));
        v["type"] = json!(kind(self.kind));
        v["from"] = json!(self.from.map(|a| a.to_string()));
        v["to"] = json!(self.to.map(|a| a.to_string()));
        v["gasUsed"] = json!(format!("{:#x}", self.gas_used));
        v["cumulativeGasUsed"] = json!(format!("{:#x}", self.cumulative_gas_used));
        v["effectiveGasPrice"] = json!(format!("{:#x}", self.effective_gas_price));
        v["contractAddress"] = json!(self.contract_address.map(|a| a.to_string()));
        let map = v.as_object_mut().expect("object constructed above");
        map.remove("status");
        map.remove("root");
        match self.outcome {
            ReceiptOutcome::Failed => v["status"] = json!("0x0"),
            ReceiptOutcome::Succeeded => v["status"] = json!("0x1"),
            ReceiptOutcome::Locked => v["status"] = json!("0x2"),
            ReceiptOutcome::PostState(root) => v["root"] = json!(root.to_string()),
        }
        v["logs"] = Value::Array(
            self.logs
                .iter()
                .map(Log::to_rpc_json)
                .collect::<Result<_, _>>()?,
        );
        v["logsBloom"] = json!(self.logs_bloom.to_hex());
        v["outboundEtxs"] = Value::Array(
            self.outbound_etxs
                .iter()
                .map(Transaction::to_rpc_json)
                .collect::<Result<_, _>>()?,
        );
        v["originatingTxHash"] = json!(self.originating_tx_hash.map(|h| h.to_string()));
        v["etxType"] = json!(self.etx_type.map(|n| format!("{n:#x}")));
        Self::try_from(v.clone())?;
        Ok(v)
    }
}
/// Assemble block JSON from validated identity, execution list and retained fields.
pub(crate) fn block_json(
    hash: quai_primitives::Hash32,
    extensions: &Map<String, Value>,
    transactions: Vec<Value>,
) -> Value {
    let mut v = Value::Object(extensions.clone());
    v["hash"] = json!(hash.to_string());
    v["transactions"] = Value::Array(transactions);
    v
}
