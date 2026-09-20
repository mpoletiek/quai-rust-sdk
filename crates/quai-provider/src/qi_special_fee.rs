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
    /// Binding gas floor: the larger of the node's execution and inclusion
    /// charges for this exact shape. See [`qi_special_gas`].
    pub required_gas: u64,
    /// Current base fee with the pinned estimator's 20 percent margin.
    pub gas_price: U256,
    /// Exact rounded-up Qit quote, including the margin; at least one Qit.
    pub qits: U256,
    /// The same sample without the margin: the smallest fee that clears both of
    /// the node's floors for this shape. A fee below this is valid but will not
    /// be included while the base fee holds, and the node reports nothing.
    pub floor_qits: U256,
}

/// Conservative native gas bound for a same-zone conversion or wrapping.
///
/// The node applies two independent fee floors, and a fee must clear both:
///
/// - **Execution** (`ProcessQiTx`) charges the intrinsic input/output gas plus
///   `QiToQuaiConversionGas`, 100,000, once for the aggregated conversion.
/// - **Inclusion** (`CalculateBlockQiTxGas`, used by the miner's filter and by
///   block validation) charges the intrinsic gas plus `ETXGas`, 21,000, for
///   *each* output that creates an ETX. Every same-zone Quai-ledger destination
///   output is one, so this grows with the destination's output count and
///   overtakes the flat execution charge past four of them.
///
/// `destination_outputs` counts the outputs that create an ETX: the Quai-ledger
/// destination outputs of a conversion or wrap. The result is the larger of the
/// two floors, with one extra gas unit above the scaled ceiling to cover
/// cross-platform logarithm rounding.
///
/// Modelling only the execution floor quotes a fee no miner will take: a mainnet
/// wrap with twelve destination outputs needed 409,400 gas and was priced for
/// 257,400, so it sat in the pool unmined while remaining perfectly valid.
pub fn qi_special_gas(
    inputs: usize,
    outputs: usize,
    destination_outputs: usize,
    utxo_set_size: u64,
) -> Result<u64, ProviderError> {
    if !(1..=1024).contains(&inputs)
        || !(1..=1024).contains(&outputs)
        || !(1..=outputs).contains(&destination_outputs)
    {
        return Err(ProviderError::InvalidRequest("special Qi shape limit"));
    }
    let base = inputs as u64 * 800 + outputs as u64 * 9000 + 3000;
    let factor = (utxo_set_size as f64).ln();
    let intrinsic = if factor < 15.0 {
        base
    } else {
        (factor * base as f64 / 15.0).ceil() as u64 + 1
    };
    let execution = intrinsic
        .checked_add(100_000)
        .ok_or(ProviderError::InvalidRequest("special Qi shape limit"))?;
    let inclusion = (destination_outputs as u64)
        .checked_mul(21_000)
        .and_then(|etx| intrinsic.checked_add(etx))
        .ok_or(ProviderError::InvalidRequest("special Qi shape limit"))?;
    Ok(execution.max(inclusion))
}

impl<T: Transport> Provider<T> {
    /// Fewest whole Qits whose credited Quai value covers `its` at `block`.
    ///
    /// The Qit quote truncates, so the conversion is checked in the other
    /// direction and raised by one when it falls short. Without that a quote
    /// converts back to less than the fee it was meant to cover, which for a
    /// floor means the node's filter skips a transaction that met it.
    async fn qits_covering(
        &self,
        zone: Zone,
        its: U256,
        block: BlockTag,
    ) -> Result<U256, ProviderError> {
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
        Ok(qits.max(U256::from(1)))
    }

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
        // Every same-zone Quai-ledger output is one conversion ETX in the
        // node's inclusion gas, so the count drives the floor.
        let destination_outputs = transaction
            .outputs
            .iter()
            .filter(|output| output.address.ledger() == quai_primitives::Ledger::Quai)
            .count();
        let required_gas = qi_special_gas(
            transaction.inputs.len(),
            transaction.outputs.len(),
            destination_outputs,
            utxo_set_size,
        )?;
        let its = gas_price
            .checked_mul(U256::from(required_gas))
            .ok_or(ProviderError::InvalidResult("fee overflow"))?;
        let block = BlockTag::Number(U256::from(before.number));
        let qits = self.qits_covering(zone, its, block).await?;
        // The same figure without the estimator's margin: the smallest fee that
        // clears both of the node's floors, corrected the same way so that
        // converting it back never falls short.
        let floor = base_fee
            .checked_mul(U256::from(required_gas))
            .ok_or(ProviderError::InvalidResult("fee overflow"))?;
        let floor_qits = self.qits_covering(zone, floor, block).await?;
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
            floor_qits,
        })
    }
}
