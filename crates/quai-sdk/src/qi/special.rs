//! Durable specialized Qi operations with an explicitly authorized fee.
use super::*;
use quai_provider::{QiFeeProfile, QiFeeQuote};

#[derive(Clone, Copy)]
enum FeeMode {
    Explicit(U256),
    Estimated(QiFeeProfile),
}
use quai_consensus::{
    QiConversionIntent, QiConversionTransaction, QiWrappingIntent, QiWrappingTransaction,
    SignedQiOperation,
};

/// Specialized native Qi operation, always distinct from an ordinary transfer.
#[derive(Clone, Copy, Debug)]
pub enum QiSpecialIntent {
    /// Native Qi-to-Quai conversion, including explicit slippage/refund.
    Conversion(QiConversionIntent),
    /// Qi backing deposit to an owner contract for a Quai beneficiary.
    Wrapping(QiWrappingIntent),
}
/// Exact immutable unsigned specialized transaction for review.
#[derive(Clone, Debug)]
pub enum QiSpecialTransaction {
    /// Conversion payload.
    Conversion(QiConversionTransaction),
    /// Wrapping payload.
    Wrapping(QiWrappingTransaction),
}
impl QiSpecialTransaction {
    /// Exact input/output/data fields, retaining operation semantics.
    pub fn transaction(&self) -> &QiTransaction {
        match self {
            Self::Conversion(tx) => tx.transaction(),
            Self::Wrapping(tx) => tx.transaction(),
        }
    }
    /// Exact signing digest.
    pub fn signing_digest(&self) -> Result<Hash32, TransactionError> {
        match self {
            Self::Conversion(tx) => tx.signing_digest(),
            Self::Wrapping(tx) => tx.signing_digest(),
        }
    }
}
/// Frozen conversion/wrapping associated with exact durable Qi input claims.
#[derive(Debug)]
pub struct PreparedQiOperation {
    instance: StoreInstance,
    scope: NetworkScope,
    id: ReservationId,
    transaction: QiSpecialTransaction,
    fee: U256,
    quote: Option<QiFeeQuote>,
}
impl QiChangePool {
    /// [`QiChangePool::reclaim`] for a prepared conversion or wrapping.
    pub fn reclaim_operation(
        &mut self,
        store: &mut SqliteStore,
        prepared: PreparedQiOperation,
    ) -> Result<(), QiError> {
        self.reclaim_outputs(
            store,
            (prepared.instance, prepared.scope, prepared.id),
            prepared.transaction.transaction(),
        )
    }
}
impl PreparedQiOperation {
    /// Durable reservation for recovery and explicit broadcast.
    pub fn reservation_id(&self) -> ReservationId {
        self.id
    }
    /// Frozen operation for user review.
    pub fn transaction(&self) -> &QiSpecialTransaction {
        &self.transaction
    }
    /// Last advisory quote used for bounded automatic fee convergence.
    pub fn fee_quote(&self) -> Option<QiFeeQuote> {
        self.quote
    }
    /// Authorized fee in Qits according to the refreshed input observations.
    pub fn fee(&self) -> U256 {
        self.fee
    }
}
impl<T: Transport> QiSession<'_, T> {
    /// Prepare conversion/wrapping with an explicit fee in Qits. The fee is not
    /// presented as an automatic node estimate: pinned ordinary estimation is
    /// invalid for these operations. Insufficient fees may be rejected by the node.
    /// All exact inputs, outputs, fee, refund/slippage and owner data are frozen
    /// before claims/signing. Allocate change and refresh discovery beforehand.
    pub async fn prepare_special(
        &mut self,
        id: ReservationId,
        amount: U256,
        intent: QiSpecialIntent,
        explicit_fee: U256,
        policy: QiPolicy,
        mut change: impl BorrowMut<QiChangePool>,
    ) -> Result<PreparedQiOperation, QiError> {
        self.prepare_special_inner(
            id,
            amount,
            intent,
            FeeMode::Explicit(explicit_fee),
            policy,
            change.borrow_mut(),
        )
        .await
    }
    /// Prepare with bounded fee convergence for an explicitly selected node profile.
    /// Each quote covers the exact selected shape; no inputs are reserved on failure.
    pub async fn prepare_special_estimated(
        &mut self,
        id: ReservationId,
        amount: U256,
        intent: QiSpecialIntent,
        profile: QiFeeProfile,
        policy: QiPolicy,
        mut change: impl BorrowMut<QiChangePool>,
    ) -> Result<PreparedQiOperation, QiError> {
        self.prepare_special_inner(
            id,
            amount,
            intent,
            FeeMode::Estimated(profile),
            policy,
            change.borrow_mut(),
        )
        .await
    }
    async fn prepare_special_inner(
        &mut self,
        id: ReservationId,
        amount: U256,
        intent: QiSpecialIntent,
        mode: FeeMode,
        policy: QiPolicy,
        change: &mut QiChangePool,
    ) -> Result<PreparedQiOperation, QiError> {
        match self
            .prepare_special_once(id, amount, intent, mode, policy, change)
            .await
        {
            Err(error) if lost_race(&error) => {
                self.prepare_special_once(id, amount, intent, mode, policy, change)
                    .await
            }
            result => result,
        }
    }
    async fn prepare_special_once(
        &mut self,
        id: ReservationId,
        amount: U256,
        intent: QiSpecialIntent,
        mode: FeeMode,
        policy: QiPolicy,
        change: &mut QiChangePool,
    ) -> Result<PreparedQiOperation, QiError> {
        let (mut fee, rounds) = match mode {
            FeeMode::Explicit(fee) => (fee, 1),
            FeeMode::Estimated(_) if (1..=32).contains(&policy.max_fee_rounds) => {
                (policy.initial_fee, policy.max_fee_rounds)
            }
            FeeMode::Estimated(_) => return Err(QiError::InvalidPolicy),
        };
        if amount == U256::ZERO
            || fee > policy.max_fee
            || !(1..=1024).contains(&policy.max_inputs)
            || !(1..=1024).contains(&policy.max_outputs)
        {
            return Err(QiError::InvalidPolicy);
        }
        let scope = self.store.scope();
        if change.instance != self.store.instance()
            || change.scope != scope
            || change.burned_through.is_some_and(|minimum| {
                self.store
                    .next_derivation_index(&change.account, true)
                    .ok()
                    .flatten()
                    .is_none_or(|next| next < minimum)
            })
        {
            return Err(QiError::IdentityMismatch);
        }
        let metadata: BTreeMap<_, _> = self
            .store
            .addresses()?
            .into_iter()
            .map(|entry| (entry.address(), entry))
            .collect();
        for address in &change.addresses {
            if metadata.get(&address.address()) != Some(address) {
                return Err(QiError::IdentityMismatch);
            }
            self.key_for(address)?;
        }
        let snapshot = self.store.snapshot()?;
        let checkpoint = snapshot.checkpoint.ok_or(QiError::MissingSnapshot)?;
        self.verify_network().await?;
        let height = self
            .candidate_height(snapshot.generation, checkpoint, policy.max_snapshot_age)
            .await?;
        for _ in 0..rounds {
            let selection = select_fewest(
                &snapshot.coins,
                &SelectionRequest {
                    zone: scope.zone,
                    candidate_height: height,
                    target: amount,
                    fee,
                    max_fee: policy.max_fee,
                    max_inputs: policy.max_inputs,
                    max_outputs: policy.max_outputs,
                },
            )?;
            if selection.change_outputs.len() > change.addresses.len() {
                return Err(QiError::InsufficientChange);
            }
            let mut inputs = Vec::with_capacity(selection.inputs.len());
            for coin in &selection.inputs {
                let address = metadata
                    .get(&coin.address.address())
                    .ok_or(QiError::IdentityMismatch)?;
                let key = self.key_for(address)?;
                inputs.push(QiInput {
                    previous_output: coin.outpoint,
                    public_key: key.public_key(),
                });
            }
            let outputs = selection
                .change_outputs
                .iter()
                .zip(&change.addresses)
                .map(|(denomination, address)| QiOutput {
                    denomination: *denomination,
                    address: address.address(),
                })
                .collect();
            let transaction = match intent {
                QiSpecialIntent::Conversion(intent) => {
                    QiSpecialTransaction::Conversion(QiConversionTransaction::new(
                        scope.chain_id,
                        inputs,
                        selection.spend_outputs,
                        outputs,
                        intent,
                    )?)
                }
                QiSpecialIntent::Wrapping(intent) => {
                    QiSpecialTransaction::Wrapping(QiWrappingTransaction::new(
                        scope.chain_id,
                        inputs,
                        selection.spend_outputs,
                        outputs,
                        intent,
                    )?)
                }
            };
            transaction.signing_digest()?;
            let quote = match mode {
                FeeMode::Explicit(_) => None,
                FeeMode::Estimated(profile) => {
                    let quote = self
                        .provider
                        .estimate_qi_special_fee(transaction.transaction(), profile)
                        .await?;
                    if quote.qits > policy.max_fee {
                        return Err(SelectionError::FeeBudgetExceeded.into());
                    }
                    if quote.qits > fee {
                        fee = quote.qits;
                        continue;
                    }
                    Some(quote)
                }
            };
            let final_height = self
                .candidate_height(snapshot.generation, checkpoint, policy.max_snapshot_age)
                .await?;
            if selection.inputs.iter().any(|coin| {
                coin.unlock_height > final_height
                    || coin.expires_at.is_some_and(|end| final_height >= end)
            }) {
                return Err(QiError::StaleSnapshot);
            }
            self.store.reserve_qi(
                id,
                snapshot.generation,
                final_height,
                &selection
                    .inputs
                    .iter()
                    .map(|coin| coin.outpoint)
                    .collect::<Vec<_>>(),
            )?;
            change.addresses.drain(..selection.change_outputs.len());
            return Ok(PreparedQiOperation {
                instance: self.store.instance(),
                scope,
                id,
                transaction,
                fee,
                quote,
            });
        }
        Err(SelectionError::FeeDidNotConverge.into())
    }
    /// Sign a frozen specialized operation and persist its verified bytes before
    /// returning. `broadcast(reservation_id)` recovers/submits the exact operation.
    pub fn sign_special(
        &mut self,
        prepared: &PreparedQiOperation,
    ) -> Result<SignedQiOperation, QiError> {
        if prepared.instance != self.store.instance()
            || prepared.scope != self.store.scope()
            || self
                .store
                .reservation(prepared.id)?
                .is_none_or(|r| r.state != ReservationState::Reserved)
        {
            return Err(QiError::IdentityMismatch);
        }
        let mut expected: Vec<_> = prepared
            .transaction
            .transaction()
            .inputs
            .iter()
            .map(|input| input.previous_output)
            .collect();
        let mut actual = self.store.reserved_outpoints(prepared.id)?;
        expected.sort_unstable();
        actual.sort_unstable();
        if expected != actual {
            return Err(QiError::IdentityMismatch);
        }
        let metadata: BTreeMap<_, _> = self
            .store
            .addresses()?
            .into_iter()
            .map(|entry| (entry.address(), entry))
            .collect();
        let keys = prepared
            .transaction
            .transaction()
            .inputs
            .iter()
            .map(|input| {
                self.key_for(
                    metadata
                        .get(&input.public_key.address())
                        .ok_or(QiError::IdentityMismatch)?,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let keys: Vec<_> = keys.iter().collect();
        let signed = match &prepared.transaction {
            QiSpecialTransaction::Conversion(tx) => {
                SignedQiOperation::Conversion(tx.sign_local(&keys)?)
            }
            QiSpecialTransaction::Wrapping(tx) => {
                SignedQiOperation::Wrapping(tx.sign_local(&keys)?)
            }
        };
        self.store
            .commit_signed_qi_operation(prepared.id, &signed)?;
        Ok(signed)
    }
}
