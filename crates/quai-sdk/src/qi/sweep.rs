//! Sweep and explicit first-Qi-block aggregation preparation.
use super::*;
use quai_wallet::{SweepMode, select_sweep};

impl<T: Transport> QiSession<'_, T> {
    /// Spend all eligible coins into fresh owned outputs with exact fee
    /// convergence. Allocate the output pool, then refresh before calling.
    /// `Aggregate` increases denominations and requires first-Qi block placement
    /// on the pinned node; this SDK cannot reserve that position with miners.
    pub async fn prepare_sweep(
        &mut self,
        id: ReservationId,
        mode: SweepMode,
        policy: QiPolicy,
        outputs: QiChangePool,
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
        let metadata: BTreeMap<_, _> = self
            .store
            .addresses()?
            .into_iter()
            .map(|entry| (entry.address(), entry))
            .collect();
        for address in &outputs.addresses {
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
        let mut fee = policy.initial_fee;
        for _ in 0..policy.max_fee_rounds {
            let selection = select_sweep(
                &snapshot.coins,
                &SelectionRequest {
                    zone: scope.zone,
                    candidate_height: height,
                    target: U256::ZERO,
                    fee,
                    max_fee: policy.max_fee,
                    max_inputs: policy.max_inputs,
                    max_outputs: policy.max_outputs,
                },
                mode,
            )?;
            if selection.spend_outputs.len() > outputs.addresses.len() {
                return Err(QiError::InsufficientChange);
            }
            let inputs = selection
                .inputs
                .iter()
                .map(|coin| {
                    let key = self.key_for(
                        metadata
                            .get(&coin.address.address())
                            .ok_or(QiError::IdentityMismatch)?,
                    )?;
                    Ok(QiInput {
                        previous_output: coin.outpoint,
                        public_key: key.public_key(),
                    })
                })
                .collect::<Result<Vec<_>, QiError>>()?;
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
