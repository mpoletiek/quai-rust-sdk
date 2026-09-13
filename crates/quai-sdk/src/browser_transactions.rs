//! Browser account prepare/review/sign and explicit persisted submission.
use crate::account_preflight::{
    AccountAccessListPolicy, AccountIntent, AccountNonce, AccountObservationPolicy,
    AccountOperationIntent, AccountPreflightError, AccountQuote, FeePolicy, QuaiConversionIntent,
    check_network, quote_operation,
};
use crate::browser_accounts::{BrowserAccountBook, BrowserAccountError};
use quai_consensus::{QuaiTransaction, SignedQuaiTransaction};
use quai_primitives::Hash32;
use quai_provider::{BroadcastError, BroadcastResult, Provider};
use quai_rpc::Transport;
use quai_signer::Signer;
use quai_wallet::account_custody::{ReservationId, ReservationState};

/// Errors retain custody and never retry a send or select a different nonce.
#[derive(Debug, thiserror::Error)]
pub enum BrowserTransactionError {
    /// Browser custody or revision conflict.
    #[error(transparent)]
    Custody(#[from] BrowserAccountError),
    /// Provider observations, payload or fee limits failed.
    #[error(transparent)]
    Preflight(#[from] AccountPreflightError),
    /// Submission failure, including an explicitly ambiguous outcome.
    #[error(transparent)]
    Broadcast(#[from] BroadcastError),
}
/// Fixed reviewed transaction tied to the exact book that reserved its nonce.
/// Dropping it retains the unsigned reservation. Signing is explicit and persists
/// verified bytes before returning them; no provider call occurs during signing.
pub struct PreparedBrowserAccountTransaction {
    book: BrowserAccountBook,
    id: ReservationId,
    quote: AccountQuote,
}
impl PreparedBrowserAccountTransaction {
    /// Exact public operation identity for cancellation/restart recovery.
    pub fn reservation_id(&self) -> ReservationId {
        self.id
    }
    /// Immutable fields to review before signing.
    pub fn transaction(&self) -> &QuaiTransaction {
        self.quote.transaction()
    }
    /// Maximum gas debit in Its.
    pub fn maximum_fee(&self) -> quai_rpc::U256 {
        self.quote.maximum_fee()
    }
    /// Exact digest of the frozen fields.
    pub fn signing_digest(&self) -> Hash32 {
        self.quote.signing_digest()
    }
    /// Sign and commit the reviewed payload to its original book. Claims remain
    /// held if cancellation occurs after the write dispatch.
    pub async fn sign(
        &self,
        signer: &impl Signer,
    ) -> Result<SignedQuaiTransaction, BrowserTransactionError> {
        Ok(self
            .book
            .sign(self.id, self.quote.transaction(), signer)
            .await?)
    }
}
/// Account orchestration over a non-Send browser provider and durable book.
/// The provider may be Fetch, WebSocket or an explicit submission-capable adapter.
pub struct BrowserAccountSession<'a, T> {
    provider: &'a Provider<T>,
    book: &'a BrowserAccountBook,
    observation: AccountObservationPolicy,
    access: AccountAccessListPolicy,
}
impl<'a, T: Transport> BrowserAccountSession<'a, T> {
    /// Bind explicit application-owned objects. No prompt or network access occurs.
    pub fn new(provider: &'a Provider<T>, book: &'a BrowserAccountBook) -> Self {
        Self {
            provider,
            book,
            observation: AccountObservationPolicy::Pending,
            access: AccountAccessListPolicy::Preserve,
        }
    }
    /// Choose pending or explicitly pinned latest observations, without fallback.
    pub fn with_observation_policy(mut self, policy: AccountObservationPolicy) -> Self {
        self.observation = policy;
        self
    }
    /// Discover access requirements for ordinary calls and deployments before
    /// estimation. Conversion envelopes retain their typed access declarations.
    pub fn with_access_list_policy(mut self, policy: AccountAccessListPolicy) -> Self {
        self.access = policy;
        self
    }
    /// Prepare an ordinary same-zone account transfer or contract call. Estimate
    /// the exact max(remote nonce, retained floor), then reserve it with the revision
    /// captured before network I/O. A race rejects without retry or partial mutation.
    pub async fn prepare(
        &self,
        id: ReservationId,
        intent: AccountIntent,
        fee: FeePolicy,
    ) -> Result<PreparedBrowserAccountTransaction, BrowserTransactionError> {
        self.prepare_inner(id, intent, fee, false, false).await
    }
    /// Explicit cross-zone account preparation; origin success is not settlement.
    pub async fn prepare_cross_zone(
        &self,
        id: ReservationId,
        intent: AccountIntent,
        fee: FeePolicy,
    ) -> Result<PreparedBrowserAccountTransaction, BrowserTransactionError> {
        self.prepare_inner(id, intent, fee, false, true).await
    }
    /// Reprepare the exact retained unsigned nonce after interruption/restart.
    /// Released gaps must be explicitly reopened first. The result requires review.
    pub async fn prepare_reserved(
        &self,
        id: ReservationId,
        intent: AccountIntent,
        fee: FeePolicy,
    ) -> Result<PreparedBrowserAccountTransaction, BrowserTransactionError> {
        self.prepare_inner(id, intent, fee, true, false).await
    }
    /// Reprepare an existing unsigned cross-zone operation.
    pub async fn prepare_cross_zone_reserved(
        &self,
        id: ReservationId,
        intent: AccountIntent,
        fee: FeePolicy,
    ) -> Result<PreparedBrowserAccountTransaction, BrowserTransactionError> {
        self.prepare_inner(id, intent, fee, true, true).await
    }
    async fn prepare_inner(
        &self,
        id: ReservationId,
        intent: AccountIntent,
        fee: FeePolicy,
        reuse: bool,
        cross_zone: bool,
    ) -> Result<PreparedBrowserAccountTransaction, BrowserTransactionError> {
        self.prepare_operation(
            id,
            AccountOperationIntent::Call(intent),
            fee,
            reuse,
            cross_zone,
        )
        .await
    }
    /// Prepare a same-zone Quai-to-Qi conversion with explicit native value and
    /// slippage. The specialized estimator receives the real Qi recipient, nonce
    /// and payload. Signing/submission reuse the durable account custody workflow.
    pub async fn prepare_conversion(
        &self,
        id: ReservationId,
        intent: QuaiConversionIntent,
        fee: FeePolicy,
    ) -> Result<PreparedBrowserAccountTransaction, BrowserTransactionError> {
        self.prepare_operation(
            id,
            AccountOperationIntent::Conversion(intent),
            fee,
            false,
            false,
        )
        .await
    }
    /// Reprepare an explicitly retained unsigned conversion nonce after restart.
    pub async fn prepare_conversion_reserved(
        &self,
        id: ReservationId,
        intent: QuaiConversionIntent,
        fee: FeePolicy,
    ) -> Result<PreparedBrowserAccountTransaction, BrowserTransactionError> {
        self.prepare_operation(
            id,
            AccountOperationIntent::Conversion(intent),
            fee,
            true,
            false,
        )
        .await
    }
    /// Prepare the offline-ground deployment at an already reserved nonce. The
    /// same ID and nonce must have been retained before grinding. Changed sender,
    /// scope, reservation or init-code prediction rejects without reallocation.
    #[cfg(feature = "abi")]
    pub async fn prepare_deployment(
        &self,
        id: ReservationId,
        deployment: crate::contracts::PreparedDeployment,
        fee: FeePolicy,
    ) -> Result<PreparedBrowserAccountTransaction, BrowserTransactionError> {
        self.prepare_operation(
            id,
            AccountOperationIntent::Deployment(deployment),
            fee,
            true,
            false,
        )
        .await
    }
    async fn prepare_operation(
        &self,
        id: ReservationId,
        intent: AccountOperationIntent,
        fee: FeePolicy,
        reuse: bool,
        cross_zone: bool,
    ) -> Result<PreparedBrowserAccountTransaction, BrowserTransactionError> {
        let mut snapshot = self.book.snapshot().await?;
        if (snapshot.book.address().zone() != intent.zone()) != cross_zone {
            return Err(AccountPreflightError::Invalid.into());
        }
        let nonce = if reuse {
            let op = snapshot
                .book
                .operation(id)
                .filter(|op| op.state == ReservationState::Reserved)
                .ok_or(AccountPreflightError::Invalid)?;
            AccountNonce::Exact(op.nonce)
        } else {
            if snapshot.book.operation(id).is_some() {
                return Err(AccountPreflightError::Invalid.into());
            }
            AccountNonce::AtLeast(snapshot.book.next_nonce())
        };
        let quote = quote_operation(
            self.provider,
            snapshot.book.scope(),
            snapshot.book.address(),
            intent,
            nonce,
            self.observation,
            fee,
            self.access,
        )
        .await?;
        if !reuse {
            let actual = snapshot
                .book
                .reserve_nonce(id, quote.transaction().nonce)
                .map_err(BrowserAccountError::from)?;
            if actual != quote.transaction().nonce {
                return Err(AccountPreflightError::Invalid.into());
            }
        }
        self.book
            .store
            .compare_exchange(
                Some(snapshot.revision),
                Some(
                    &snapshot
                        .book
                        .export_state()
                        .map_err(BrowserAccountError::from)?,
                ),
            )
            .await
            .map_err(BrowserAccountError::from)?;
        Ok(PreparedBrowserAccountTransaction {
            book: self.book.clone(),
            id,
            quote,
        })
    }
    /// Load the exact signed root by ID, recheck the network, persist Submitted,
    /// then perform one explicit broadcast. Reopening and calling this again uses
    /// the same bytes; timeout/cancellation never authorizes a new payment.
    /// Replacement candidates require their own explicit candidate selection.
    pub async fn broadcast(
        &self,
        id: ReservationId,
    ) -> Result<BroadcastResult, BrowserTransactionError> {
        let mut snapshot = self.book.snapshot().await?;
        let op = snapshot
            .book
            .operation(id)
            .filter(|op| {
                matches!(
                    op.state,
                    ReservationState::Signed | ReservationState::Submitted
                )
            })
            .ok_or(AccountPreflightError::Invalid)?;
        let signed = SignedQuaiTransaction::decode(
            op.payload
                .as_deref()
                .ok_or(AccountPreflightError::Invalid)?,
        )
        .map_err(|_| AccountPreflightError::Invalid)?;
        check_network(self.provider, snapshot.book.scope()).await?;
        snapshot
            .book
            .mark_submitted(id)
            .map_err(BrowserAccountError::from)?;
        self.book
            .store
            .compare_exchange(
                Some(snapshot.revision),
                Some(
                    &snapshot
                        .book
                        .export_state()
                        .map_err(BrowserAccountError::from)?,
                ),
            )
            .await
            .map_err(BrowserAccountError::from)?;
        Ok(self.provider.broadcast(&signed).await?)
    }
}
