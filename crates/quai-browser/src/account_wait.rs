//! Window/worker replacement-aware account waiting without custody side effects.
use crate::receipt_wait::{WaitObservation, WaitPoll, wait_for_observation};
use crate::{BrowserReceiptWaitError, BrowserWaitConfig};
use quai_consensus::SignedQuaiTransaction;
use quai_primitives::Hash32;
use quai_provider::{
    AccountNonceCandidate, AccountReplacementPoll, AccountReplacementTracker, Provider,
    ProviderError,
};
use quai_rpc::Transport;

/// Account replacement waits share bounded timer/observation error information.
pub type BrowserAccountWaitError = BrowserReceiptWaitError;
struct AccountPoll<'a, T> {
    provider: &'a Provider<T>,
    tracker: AccountReplacementTracker,
}
impl<T: Transport> WaitPoll for AccountPoll<'_, T> {
    type Output = AccountNonceCandidate;
    async fn poll(&mut self) -> Result<WaitObservation<Self::Output>, ProviderError> {
        Ok(match self.tracker.poll(self.provider).await? {
            AccountReplacementPoll::Confirmed(candidate) => WaitObservation::Ready(*candidate),
            AccountReplacementPoll::Pending {
                inclusion,
                more_available,
            } => WaitObservation::Pending {
                inclusion,
                immediate: more_available,
            },
        })
    }
}
/// Wait for a verified original or unregistered same-sender/nonce competitor.
/// Uses the same portable page tracker as native waiting and window/worker
/// monotonic timers. max_polls bounds completed pages/idle observations; an overall
/// deadline also covers stalled reads. No Send requirement, custody adoption,
/// claim release or submission. Missing history never advances the cursor;
/// changed anchors return an error requiring explicit restart at a trusted height.
/// Browser suspension may delay timeout delivery but cannot allow success after
/// observed expiry. Dropping the future clears timers and the active read future.
pub async fn wait_for_account_transaction<T: Transport>(
    provider: &Provider<T>,
    original: &SignedQuaiTransaction,
    genesis: Hash32,
    start_block: u64,
    config: BrowserWaitConfig,
) -> Result<AccountNonceCandidate, BrowserAccountWaitError> {
    config
        .validate()
        .map_err(|_| BrowserAccountWaitError::InvalidConfig)?;
    let transaction_hash = original
        .hash()
        .map_err(|_| BrowserAccountWaitError::InvalidConfig)?;
    let tracker =
        AccountReplacementTracker::new(original, genesis, start_block, config.confirmations)
            .map_err(|_| BrowserAccountWaitError::InvalidConfig)?;
    wait_for_observation(transaction_hash, config, AccountPoll { provider, tracker }).await
}
