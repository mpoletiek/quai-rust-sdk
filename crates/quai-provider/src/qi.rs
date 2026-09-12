use crate::{Provider, ProviderError, RpcData};
use quai_consensus::{QiConversionTransaction, QiTransaction};
use quai_rpc::{Transport, U256};
use serde_json::json;

impl<T: Transport> Provider<T> {
    /// Fail explicitly without RPC: the pinned estimator drops conversion data,
    /// charges per-output gas differently from block aggregation, and misses the
    /// additional conversion gas requirement. An ordinary quote is not substituted.
    /// Future support requires independently qualified current-state calculations.
    pub async fn estimate_qi_conversion_fee(
        &self,
        transaction: &QiConversionTransaction,
    ) -> Result<U256, ProviderError> {
        if transaction.chain_id() != self.expected_chain_id {
            return Err(ProviderError::ChainMismatch {
                expected: self.expected_chain_id,
                actual: transaction.chain_id(),
            });
        }
        Err(ProviderError::ConversionFeeEstimationUnavailable)
    }

    /// Estimate a fee in Qits for the exact ordinary Qi input/output shape.
    ///
    /// Uses pinned go-quai `quai_estimateFeeForQi`, which derives scaling, fee and
    /// exchange rate from current state. The quote is advisory and can change.
    /// Nonempty data fails explicitly: that estimator omits conversion/wrapping
    /// data in its internal gas object, so it cannot qualify specialized transfers.
    /// No signature, input ownership or node acceptance is implied by a quote.
    pub async fn estimate_qi_fee(
        &self,
        transaction: &QiTransaction,
    ) -> Result<U256, ProviderError> {
        if transaction.chain_id != self.expected_chain_id {
            return Err(ProviderError::ChainMismatch {
                expected: self.expected_chain_id,
                actual: transaction.chain_id,
            });
        }
        if !transaction.data.is_empty() {
            return Err(ProviderError::InvalidRequest(
                "ordinary Qi fee request requires empty data",
            ));
        }
        transaction
            .unsigned_bytes()
            .map_err(|_| ProviderError::InvalidRequest("invalid Qi transaction shape"))?;
        let zone = transaction
            .origin_zone()
            .map_err(|_| ProviderError::InvalidRequest("invalid Qi origin"))?;
        let inputs=transaction.inputs.iter().map(|input| {
            Ok(json!({"previousOutPoint":{"txHash":input.previous_output.transaction_hash.to_string(),"index":format!("{:#x}",input.previous_output.index)},"pubKey":RpcData::new(input.public_key.to_compressed().to_vec())?.to_hex()}))
        }).collect::<Result<Vec<_>,ProviderError>>()?;
        let outputs=transaction.outputs.iter().map(|output| json!({"address":output.address.to_string(),"denomination":format!("{:#x}",output.denomination.index()),"lock":"0x0"})).collect::<Vec<_>>();
        crate::quantity(
            self.read(
                zone.into(),
                "quai_estimateFeeForQi",
                json!([{"txType":2,"txIn":inputs,"txOut":outputs}]),
            )
            .await?,
        )
    }
}
