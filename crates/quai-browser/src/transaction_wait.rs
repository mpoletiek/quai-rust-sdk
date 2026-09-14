//! Window/worker waits for indexed transactions without requiring a receipt.
use crate::receipt_wait::{WaitObservation, WaitPoll, wait_for_observation};
use crate::{BrowserReceiptWaitError, BrowserWaitConfig};
use quai_primitives::{Hash32, Zone};
use quai_provider::{ConfirmedTransaction, Provider, ProviderError, TransactionConfirmation};
use quai_rpc::Transport;
/// Shared deadline, completed-observation and provider-error information.
pub type BrowserTransactionWaitError = BrowserReceiptWaitError;
struct TransactionPoll<'a, T> {
    provider: &'a Provider<T>,
    zone: Zone,
    hash: Hash32,
    confirmations: u64,
}
impl<T: Transport> WaitPoll for TransactionPoll<'_, T> {
    type Output = ConfirmedTransaction;
    async fn poll(&mut self) -> Result<WaitObservation<Self::Output>, ProviderError> {
        Ok(
            match self
                .provider
                .observe_transaction_confirmation(self.zone, self.hash, self.confirmations)
                .await?
            {
                TransactionConfirmation::Confirmed(tx) => WaitObservation::Ready(*tx),
                TransactionConfirmation::Pending {
                    last_observed_inclusion,
                } => WaitObservation::Pending {
                    inclusion: last_observed_inclusion,
                    immediate: false,
                },
            },
        )
    }
}
/// Wait for rechecked transaction inclusion, including Qi without receipts.
/// Explicit monotonic deadline covers stalled RPCs; max_polls bounds completed
/// observations. Browser suspension can delay delivery but not permit success
/// after observed expiry. Drop clears timers and active reads. No submission,
/// signature verification, replacement adoption or release of custody claims.
pub async fn wait_for_transaction<T: Transport>(
    provider: &Provider<T>,
    zone: Zone,
    transaction_hash: Hash32,
    config: BrowserWaitConfig,
) -> Result<ConfirmedTransaction, BrowserTransactionWaitError> {
    if transaction_hash == Hash32::ZERO {
        return Err(BrowserTransactionWaitError::InvalidConfig);
    }
    wait_for_observation(
        transaction_hash,
        config,
        TransactionPoll {
            provider,
            zone,
            hash: transaction_hash,
            confirmations: config.confirmations,
        },
    )
    .await
}
