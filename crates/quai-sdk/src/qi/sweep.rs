//! Sweep and explicit first-Qi-block aggregation preparation.
use super::*;
use quai_wallet::{SweepMode, select_sweep};

impl<T: Transport> QiSession<'_, T> {
    /// Sweep or threshold-aggregate eligible coins into fresh owned outputs with exact fee
    /// convergence. Allocate the output pool, then refresh before calling. As
    /// with `prepare`, a pool passed by `&mut` loses only the addresses used.
    /// `Aggregate` increases denominations and requires first-Qi block placement
    /// on the pinned node; this SDK cannot reserve that position with miners.
    /// AggregateThreshold applies its input threshold and may leave larger coins
    /// unspent; every convergence round reselects and covers the quoted fee.
    pub async fn prepare_sweep(
        &mut self,
        id: ReservationId,
        mode: SweepMode,
        policy: QiPolicy,
        mut outputs: impl BorrowMut<QiChangePool>,
    ) -> Result<PreparedQiTransaction, QiError> {
        let outputs = outputs.borrow_mut();
        match self.prepare_sweep_once(id, mode, policy, outputs).await {
            Err(error) if lost_race(&error) => {
                self.prepare_sweep_once(id, mode, policy, outputs).await
            }
            result => result,
        }
    }
    async fn prepare_sweep_once(
        &mut self,
        id: ReservationId,
        mode: SweepMode,
        policy: QiPolicy,
        outputs: &mut QiChangePool,
    ) -> Result<PreparedQiTransaction, QiError> {
        if !(1..=1024).contains(&policy.max_inputs)
            || !(1..=1024).contains(&policy.max_outputs)
            || !(1..=32).contains(&policy.max_fee_rounds)
            || policy.initial_fee > policy.max_fee
        {
            return Err(QiError::InvalidPolicy);
        }
        let scope = self.store.scope();
        if outputs.instance != self.store.instance() || outputs.scope != scope {
            return Err(QiError::IdentityMismatch);
        }
        if let Some(minimum) = outputs.burned_through
            && self
                .store
                .next_derivation_index(&outputs.account, true)?
                .is_none_or(|next| next < minimum)
        {
            return Err(QiError::IdentityMismatch);
        }
        let owned = self
            .store
            .public_addresses(outputs.addresses.iter().map(PublicAddress::address))?;
        for address in &outputs.addresses {
            if owned.get(&address.address()) != Some(address) {
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
        let mut fee = policy.initial_fee;
        for _ in 0..policy.max_fee_rounds {
            let selection = select_sweep(
                &snapshot.coins,
                &SelectionRequest::new(
                    scope.zone,
                    height,
                    U256::ZERO,
                    policy.max_inputs,
                    policy.max_outputs,
                )
                .with_fee(fee, policy.max_fee),
                mode,
            )?;
            if selection.spend_outputs.len() > outputs.addresses.len() {
                return Err(QiError::InsufficientChange);
            }
            let (inputs, entries) = self.selected_inputs(&selection.inputs)?;
            let transaction = QiTransaction {
                chain_id: scope.chain_id,
                inputs,
                outputs: selection
                    .spend_outputs
                    .iter()
                    .zip(&outputs.addresses)
                    .map(|(denomination, address)| QiOutput {
                        denomination: *denomination,
                        address: address.address(),
                    })
                    .collect(),
                data: vec![],
            };
            let digest = transaction.signing_digest()?;
            let quote = self.provider.estimate_qi_fee(&transaction).await?;
            if quote > policy.max_fee {
                return Err(SelectionError::FeeBudgetExceeded.into());
            }
            if quote > fee {
                fee = quote;
                continue;
            }
            self.confirm_signable(&entries)?;
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
            outputs.addresses.drain(..selection.spend_outputs.len());
            let recipient_outputs = transaction.outputs.len();
            return Ok(PreparedQiTransaction {
                instance: self.store.instance(),
                id,
                scope,
                transaction,
                fee,
                recipient_outputs,
                digest,
            });
        }
        Err(SelectionError::FeeDidNotConverge.into())
    }
}
