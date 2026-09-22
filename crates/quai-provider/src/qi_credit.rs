//! Current indexed Qi output attribution after intent-bound ETX execution.
use crate::{
    AddressOutpoint, BlockReference, ConversionEffect, ConversionObservation,
    ConversionOriginObservation, ConversionReference, EtxScanRequest, ExternalObservation,
    ExternalReference, Provider, ProviderError, ReceiptOutcome, TransactionDetails,
};
use quai_primitives::{Hash32, QiAddress};
use quai_rpc::{Transport, U256};

/// Current source-reported outputs attributable to one canonical external hash.
/// This is not a complete historical credit or an atomic UTXO snapshot.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct QiCreditObservation {
    /// Actual beneficiary, including the signed refund address for Qi refunds.
    pub beneficiary: QiAddress,
    /// Final executed ETX hash, which can differ from its origin emission hash.
    pub transaction_hash: Hash32,
    /// Hash used by the destination UTXO index; cross-zone Qi uses the original
    /// signed Qi hash rather than the final ETX hash.
    pub creating_hash: Hash32,
    /// Canonical execution anchor rechecked around the index query.
    pub execution: BlockReference,
    /// Sampled canonical head before and after the latest-only query.
    pub head: BlockReference,
    /// Currently indexed outputs from this exact hash, ordered by index.
    pub outputs: Vec<AddressOutpoint>,
    /// Sum of observed output values whose reported lock exceeds the sampled head.
    pub locked_qits: U256,
    /// Sum with reported lock at or below the head. Wallet claims, source accuracy,
    /// trimming and a later spend can still prevent spending these outputs.
    pub unlocked_qits: U256,
    /// ETX value minus currently observed outputs. Includes spent, trimmed,
    /// gas-truncated, not-yet-indexed or unavailable outputs; never assumed lost.
    pub unobserved_qits: U256,
}
impl<T: Transport> Provider<T> {
    /// Observe a signed conversion/refund and attribute current Qi outputs to its
    /// final executed hash. Locked and failed receipts are inspected too: the
    /// pinned node can create partial outputs before reporting execution failure.
    /// Quai destination/account credits return no Qi result.
    pub async fn observe_conversion_qi_credit(
        &self,
        reference: &ConversionReference,
        request: EtxScanRequest,
        max_outputs: usize,
    ) -> Result<(ConversionObservation, Option<QiCreditObservation>), ProviderError> {
        validate_bound(max_outputs)?;
        let observation = self.observe_conversion(reference, request).await?;
        let beneficiary = match observation.effect {
            Some(ConversionEffect::ConversionReported) => Some(reference.destination()),
            Some(ConversionEffect::RefundReported { beneficiary }) => Some(beneficiary),
            Some(
                ConversionEffect::Locked { etx_type: 2 }
                | ConversionEffect::ExecutionFailed { etx_type: 2 },
            ) => Some(reference.destination()),
            Some(
                ConversionEffect::Locked { etx_type: 5 }
                | ConversionEffect::ExecutionFailed { etx_type: 5 },
            ) => Some(reference.refund_destination()),
            _ => None,
        }
        .and_then(|address| QiAddress::try_from(address).ok());
        let credit = if let (
            Some(beneficiary),
            Some(scan),
            ConversionOriginObservation::Emitted { block, .. },
        ) = (beneficiary, &observation.scan, &observation.origin)
        {
            if let Some(execution) = &scan.execution {
                Some(
                    self.qi_credit(
                        beneficiary,
                        reference.zone(),
                        *block,
                        execution,
                        max_outputs,
                    )
                    .await?,
                )
            } else {
                None
            }
        } else {
            None
        };
        Ok((observation, credit))
    }
    /// Observe a WQI redemption and then its currently indexed output locks.
    /// Receipt failure may still accompany partial output creation on the pinned
    /// node, so current outputs are inspected for either status outcome.
    pub async fn observe_external_qi_credit(
        &self,
        reference: &ExternalReference,
        request: EtxScanRequest,
        max_outputs: usize,
    ) -> Result<(ExternalObservation, Option<QiCreditObservation>), ProviderError> {
        validate_bound(max_outputs)?;
        let observation = self.observe_external(reference, request).await?;
        let credit = if let (
            Ok(beneficiary),
            Some(scan),
            ConversionOriginObservation::Emitted { block, .. },
        ) = (
            QiAddress::try_from(reference.destination()),
            &observation.scan,
            &observation.origin,
        ) {
            if let Some(execution) = &scan.execution
                && execution.receipt.as_ref().is_some_and(|r| {
                    matches!(
                        r.outcome,
                        ReceiptOutcome::Succeeded | ReceiptOutcome::Failed | ReceiptOutcome::Locked
                    )
                })
            {
                Some(
                    self.qi_credit(
                        beneficiary,
                        reference.origin_zone(),
                        *block,
                        execution,
                        max_outputs,
                    )
                    .await?,
                )
            } else {
                None
            }
        } else {
            None
        };
        Ok((observation, credit))
    }
    async fn qi_credit(
        &self,
        beneficiary: QiAddress,
        origin_zone: quai_primitives::Zone,
        origin: BlockReference,
        execution: &crate::EtxExecutionObservation,
        max_outputs: usize,
    ) -> Result<QiCreditObservation, ProviderError> {
        let TransactionDetails::External(etx) = &execution.transaction.details else {
            return Err(ProviderError::InvalidResult("expected external credit"));
        };
        let inclusion = execution
            .transaction
            .inclusion
            .ok_or(ProviderError::InvalidResult(
                "missing external credit inclusion",
            ))?;
        let block = BlockReference {
            number: inclusion.block_number,
            hash: inclusion.block_hash,
        };
        let zone = beneficiary.zone();
        let head = self
            .latest_header(zone)
            .await?
            .ok_or(ProviderError::ObservationChanged)?;
        if head.number < block.number {
            return Err(ProviderError::ObservationChanged);
        }
        let cross_qi = etx.etx_type == 0 && origin_zone != zone;
        let creating_hash = if cross_qi {
            etx.originating_tx_hash
        } else {
            execution.transaction.hash
        };
        let expected_value = if cross_qi {
            let index = u8::try_from(etx.value)
                .map_err(|_| ProviderError::InvalidResult("invalid cross-zone denomination"))?;
            U256::from(
                quai_consensus::Denomination::new(index)
                    .map_err(|_| ProviderError::InvalidResult("invalid cross-zone denomination"))?
                    .value(),
            )
        } else {
            etx.value
        };
        let mut outputs: Vec<_> = self
            .outpoints(beneficiary)
            .await?
            .into_iter()
            .filter(|o| {
                o.outpoint.tx_hash == creating_hash
                    && (!cross_qi || o.outpoint.index == etx.etx_index)
            })
            .collect();
        if outputs.len() > max_outputs {
            return Err(ProviderError::InvalidResult(
                "Qi credit output budget exceeded",
            ));
        }
        outputs.sort_unstable_by_key(|o| o.outpoint.index);
        let mut locked = U256::ZERO;
        let mut unlocked = U256::ZERO;
        for output in &outputs {
            let denomination = quai_consensus::Denomination::new(output.denomination)
                .map_err(|_| ProviderError::InvalidResult("invalid credit denomination"))?;
            if cross_qi
                && (U256::from(output.denomination) != etx.value || output.lock != U256::ZERO)
            {
                return Err(ProviderError::InvalidResult(
                    "cross-zone Qi output differs from signed denomination",
                ));
            }
            let total = if output.lock > U256::from(head.number) {
                &mut locked
            } else {
                &mut unlocked
            };
            *total = total
                .checked_add(U256::from(denomination.value()))
                .ok_or(ProviderError::InvalidResult("credit amount overflow"))?;
        }
        let total = locked
            .checked_add(unlocked)
            .ok_or(ProviderError::InvalidResult("credit amount overflow"))?;
        let unobserved = expected_value
            .checked_sub(total)
            .ok_or(ProviderError::InvalidResult(
                "indexed credit exceeds external value",
            ))?;
        let head = BlockReference {
            number: head.number,
            hash: head.hash,
        };
        for (anchor_zone, anchor) in [(origin_zone, origin), (zone, block), (zone, head)] {
            if self
                .header_at(anchor_zone, anchor.number)
                .await?
                .is_none_or(|h| h.hash != anchor.hash)
            {
                return Err(ProviderError::ObservationChanged);
            }
        }
        if self
            .latest_header(zone)
            .await?
            .is_none_or(|h| h.hash != head.hash || h.number != head.number)
        {
            return Err(ProviderError::ObservationChanged);
        }
        Ok(QiCreditObservation {
            beneficiary,
            transaction_hash: execution.transaction.hash,
            creating_hash,
            execution: block,
            head,
            outputs,
            locked_qits: locked,
            unlocked_qits: unlocked,
            unobserved_qits: unobserved,
        })
    }
}
fn validate_bound(max_outputs: usize) -> Result<(), ProviderError> {
    if !(1..=4096).contains(&max_outputs) {
        return Err(ProviderError::InvalidRequest(
            "invalid Qi credit output budget",
        ));
    }
    Ok(())
}
