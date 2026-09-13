//! Portable canonicality-checked receipt confirmation observations.
use crate::{Inclusion, Provider, ProviderError, Receipt, ZoneHeader};
use quai_primitives::{Hash32, Zone};
use quai_rpc::Transport;

/// A receipt that passed the requested observed confirmations and consistency checks.
#[derive(Clone, Debug)]
pub struct ConfirmedReceipt {
    /// Receipt re-read after validating its block association.
    pub receipt: Receipt,
    /// Observed confirmations, including the containing block.
    pub confirmations: u64,
    /// Observed head used for the count and subsequently checked by number/hash.
    pub observed_head: ZoneHeader,
}

/// Result of one bounded observation, without timers or automatic retries.
#[derive(Clone, Debug)]
pub enum ReceiptConfirmation {
    /// Missing, insufficient depth or changed association. The last inclusion is
    /// unconfirmed and may already have been reorged away.
    Pending {
        /// Most recent inclusion observed during this completed poll, if any.
        last_observed_inclusion: Option<Inclusion>,
    },
    /// Receipt, canonical association and observed head were rechecked.
    Confirmed(Box<ConfirmedReceipt>),
}
impl<T: Transport> Provider<T> {
    /// Poll once for receipt confirmations on native or browser transports.
    /// Requires positive depth and performs at most five typed reads (each checks
    /// chain identity). No sleep, subscription, automatic retry or submission.
    /// Missing/reorged observations are pending, execution failure remains explicit
    /// in a confirmed receipt, and separate node reads never authenticate finality.
    pub async fn observe_receipt_confirmation(
        &self,
        zone: Zone,
        transaction_hash: Hash32,
        required_confirmations: u64,
    ) -> Result<ReceiptConfirmation, ProviderError> {
        let mut last = None;
        Ok(
            match self
                .confirmation_poll(zone, transaction_hash, required_confirmations, &mut last)
                .await?
            {
                Some(receipt) => ReceiptConfirmation::Confirmed(Box::new(receipt)),
                None => ReceiptConfirmation::Pending {
                    last_observed_inclusion: last,
                },
            },
        )
    }
    pub(crate) async fn confirmation_poll(
        &self,
        zone: Zone,
        transaction_hash: Hash32,
        required_confirmations: u64,
        last_observed_inclusion: &mut Option<Inclusion>,
    ) -> Result<Option<ConfirmedReceipt>, ProviderError> {
        if required_confirmations == 0 {
            return Err(ProviderError::InvalidRequest(
                "confirmation depth must be positive",
            ));
        }
        let candidate = self.receipt(zone, transaction_hash).await?;
        if let Some(receipt) = candidate {
            *last_observed_inclusion = Some(receipt.inclusion);
            let observed_head = self.latest_header(zone).await?;
            if let Some(head) = observed_head
                && head.number >= receipt.inclusion.block_number
            {
                let confirmations = (head.number - receipt.inclusion.block_number)
                    .checked_add(1)
                    .ok_or(ProviderError::InvalidResult("confirmation count overflow"))?;
                if confirmations >= required_confirmations {
                    let canonical = self.header_at(zone, receipt.inclusion.block_number).await?;
                    if canonical.is_some_and(|header| header.hash == receipt.inclusion.block_hash) {
                        let latest_receipt = self.receipt(zone, transaction_hash).await?;
                        if let Some(latest_receipt) = latest_receipt
                            && latest_receipt.inclusion == receipt.inclusion
                        {
                            let canonical_head = self.header_at(zone, head.number).await?;
                            if canonical_head.is_some_and(|header| header.hash == head.hash) {
                                return Ok(Some(ConfirmedReceipt {
                                    receipt: latest_receipt,
                                    confirmations,
                                    observed_head: head,
                                }));
                            }
                        }
                    }
                }
            }
        } else {
            *last_observed_inclusion = None;
        }
        Ok(None)
    }
}
