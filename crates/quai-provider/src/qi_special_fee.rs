//! Specialized Qi fees for an explicitly selected, independently tested node profile.
use crate::{BlockTag, Provider, ProviderError, quantity, types};
use quai_consensus::{QiConversionTransaction, QiTransaction, QiWrappingTransaction};
use quai_primitives::{Hash32, Zone};
use quai_rpc::{Transport, U256};
use serde_json::json;

/// Caller-selected rules. Chain ID/client-version strings do not attest software.
#[derive(Clone, Copy, Debug)]
pub enum QiFeeProfile {
    /// Pinned go-quai v0.56.0 after prime 1,755,000. SHA-anchored reward
    /// conversion ignores the difficulty argument, making the public rate RPCs
    /// equivalent to fee conversion. Earlier profiles are deliberately rejected.
    V056ShaAnchored,
}

/// Advisory quote for the exact specialized input/output shape and sampled head.
#[derive(Clone, Copy, Debug)]
pub struct QiFeeQuote {
    /// Header sampled before and after all latest-state queries.
    pub block_hash: Hash32,
    /// Zone block height of the sample, not guaranteed inclusion height.
    pub block_number: u64,
    /// Sampled current UTXO set size used for gas scaling.
    pub utxo_set_size: u64,
    /// Conservatively rounded intrinsic gas plus the aggregate special-operation gas.
    pub required_gas: u64,
    /// Current base fee with the pinned estimator's 20 percent margin.
    pub gas_price: U256,
    /// Exact rounded-up Qit quote; at least one Qit.
    pub qits: U256,
}

/// Conservative native gas bound for one same-zone conversion/wrapping ETX.
/// Uses the fixed input/output costs and UTXO scaling, with one extra gas unit
/// above the scaled ceiling to cover cross-platform logarithm rounding.
pub fn qi_special_gas(
    inputs: usize,
    outputs: usize,
    utxo_set_size: u64,
) -> Result<u64, ProviderError> {
    if !(1..=1024).contains(&inputs) || !(1..=1024).contains(&outputs) {
        return Err(ProviderError::InvalidRequest("special Qi shape limit"));
    }
    let base = inputs as u64 * 800 + outputs as u64 * 9000 + 3000;
    let factor = (utxo_set_size as f64).ln();
    let intrinsic = if factor < 15.0 {
        base
    } else {
        (factor * base as f64 / 15.0).ceil() as u64 + 1
    };
    Ok(intrinsic + 100_000)
}

impl<T: Transport> Provider<T> {
    /// Current indexed UTXO count. Latest-only and not a historical proof.
    pub async fn latest_utxo_set_size(&self, zone: Zone) -> Result<u64, ProviderError> {
        types::uint64(
            self.read(zone.into(), "quai_getLatestUTXOSetSize", json!([]))
                .await?,
        )
    }

    /// Quote a validated conversion or wrapping operation without passing it to
    /// the ordinary estimator (which drops specialized data). Requires a known
    /// SHA-anchored profile and equal head samples. Estimates remain advisory;
    /// network state and fees can change before signing or inclusion.
    pub async fn estimate_qi_special_fee(
        &self,
        transaction: &QiTransaction,
        profile: QiFeeProfile,
    ) -> Result<QiFeeQuote, ProviderError> {
        if transaction.chain_id != self.expected_chain_id {
            return Err(ProviderError::ChainMismatch {
                expected: self.expected_chain_id,
                actual: transaction.chain_id,
            });
        }
        let zone = match transaction.data.len() {
            20 => QiWrappingTransaction::from_transaction(transaction.clone())
                .map(|tx| tx.origin_zone()),
            22 => QiConversionTransaction::from_transaction(transaction.clone())
                .map(|tx| tx.origin_zone()),
            _ => {
                return Err(ProviderError::InvalidRequest(
                    "explicit specialized Qi transaction required",
                ));
            }
        }
        .map_err(|_| ProviderError::InvalidRequest("invalid specialized Qi transaction"))?;
        let before = self
            .latest_header(zone)
            .await?
            .ok_or(ProviderError::InvalidResult("missing fee header"))?;
        match profile {
            QiFeeProfile::V056ShaAnchored if before.prime_terminus_number < 1_755_000 => {
                return Err(ProviderError::ConversionFeeEstimationUnavailable);
            }
            QiFeeProfile::V056ShaAnchored => (),
        }
        let base_fee = quantity(
            before
                .extensions
                .fields()
                .get("baseFeePerGas")
                .cloned()
                .ok_or(ProviderError::InvalidResult("missing base fee"))?,
        )?;
        let gas_price = base_fee
            .checked_mul(U256::from(120))
            .ok_or(ProviderError::InvalidResult("fee overflow"))?
            / U256::from(100);
        let utxo_set_size = self.latest_utxo_set_size(zone).await?;
        let required_gas = qi_special_gas(
            transaction.inputs.len(),
            transaction.outputs.len(),
            utxo_set_size,
        )?;
        let its = gas_price
            .checked_mul(U256::from(required_gas))
            .ok_or(ProviderError::InvalidResult("fee overflow"))?;
        let block = BlockTag::Number(U256::from(before.number));
        let mut qits = self
            .quai_to_qi(zone, its, block)
            .await?
            .ok_or(ProviderError::InvalidResult("missing fee conversion"))?;
        let credited = self
            .qi_to_quai(zone, qits, block)
            .await?
            .ok_or(ProviderError::InvalidResult("missing fee conversion"))?;
        if credited < its {
            qits = qits
                .checked_add(U256::from(1))
                .ok_or(ProviderError::InvalidResult("fee overflow"))?;
            if self
                .qi_to_quai(zone, qits, block)
                .await?
                .is_none_or(|value| value < its)
            {
                return Err(ProviderError::InvalidResult("inconsistent fee conversion"));
            }
        }
        qits = qits.max(U256::from(1));
        let after = self
            .latest_header(zone)
            .await?
            .ok_or(ProviderError::InvalidResult("missing fee header"))?;
        if before.hash != after.hash || before.number != after.number {
            return Err(ProviderError::InvalidResult("fee head changed"));
        }
        Ok(QiFeeQuote {
            block_hash: before.hash,
            block_number: before.number,
            utxo_set_size,
            required_gas,
            gas_price,
            qits,
        })
    }
}
