//! Conflicting same-input candidates fund a larger fee only from explicit owned change.
use super::*;
use quai_consensus::{QiConversionTransaction, QiWrappingTransaction, SignedQiOperation};
/// Explicit parent and owned change to reduce. All nonselected parent outputs are
/// preserved, including payment recipients and conversion/wrapping destinations.
#[derive(Clone, Debug)]
pub struct QiReplacementIntent {
    /// Original or earlier persisted candidate to replace.
    pub parent: Hash32,
    /// Distinct parent output indexes asserted to be local change; ownership is checked.
    pub change_indexes: Vec<u16>,
    /// Replacement owned change outputs; their total must be strictly lower.
    pub change_outputs: Vec<QiOutput>,
}
/// Exact same-input payload, new fee and family identity for explicit review.
#[derive(Debug)]
pub struct PreparedQiReplacement {
    instance: StoreInstance,
    scope: NetworkScope,
    id: ReservationId,
    parent: Hash32,
    transaction: QiTransaction,
    fee: U256,
}
impl PreparedQiReplacement {
    /// Exact transaction, including all retained recipient and specialized data fields.
    pub fn transaction(&self) -> &QiTransaction {
        &self.transaction
    }
    /// Total fee according to the refreshed input observations.
    pub fn fee(&self) -> U256 {
        self.fee
    }
    /// Original durable family claim.
    pub fn reservation_id(&self) -> ReservationId {
        self.id
    }
}
fn value(outputs: &[QiOutput]) -> Result<U256, QiError> {
    outputs.iter().try_fold(U256::ZERO, |sum, o| {
        sum.checked_add(U256::from(o.denomination.value()))
            .ok_or(QiError::InvalidPolicy)
    })
}
fn validate_shape(tx: &QiTransaction) -> Result<(), QiError> {
    match tx.data.len() {
        0 => {
            tx.signing_digest()?;
        }
        20 => {
            QiWrappingTransaction::from_transaction(tx.clone())?.signing_digest()?;
        }
        22 => {
            QiConversionTransaction::from_transaction(tx.clone())?.signing_digest()?;
        }
        _ => return Err(QiError::InvalidPolicy),
    }
    Ok(())
}
impl<T: Transport> QiSession<'_, T> {
    /// Recover every validated candidate, original first, retaining one input claim set.
    pub fn signed_candidates(
        &mut self,
        id: ReservationId,
    ) -> Result<Vec<SignedQiOperation>, QiError> {
        let root = self
            .store
            .signed_payload(id)?
            .ok_or(QiError::MissingSignedPayload)?;
        let mut candidates = vec![SignedQiOperation::decode(&root)?];
        for edge in self.store.replacement_candidates(id)? {
            candidates.push(SignedQiOperation::decode(&edge.payload)?);
        }
        Ok(candidates)
    }
    /// Prepare a higher-fee conflicting Qi candidate from a fresh snapshot. Only
    /// explicitly selected, locally owned change outputs may change. Inputs,
    /// recipients and specialized data remain fixed. Ordinary fees are estimated;
    /// special operations use a supplied profile or an explicitly planned fee.
    /// Both candidates may remain in the pool; inclusion preference is not guaranteed.
    pub async fn prepare_replacement(
        &mut self,
        id: ReservationId,
        intent: QiReplacementIntent,
        policy: QiPolicy,
        special_profile: Option<quai_provider::QiFeeProfile>,
    ) -> Result<PreparedQiReplacement, QiError> {
        if !(1..=1024).contains(&policy.max_inputs)
            || !(1..=1024).contains(&policy.max_outputs)
            || intent.change_indexes.is_empty()
            || intent.change_indexes.len() > 1024
            || intent.change_outputs.len() > 1024
        {
            return Err(QiError::InvalidPolicy);
        }
        if self.store.reservation(id)?.is_none_or(|r| {
            !matches!(
                r.state,
                ReservationState::Signed | ReservationState::Submitted
            )
        }) {
            return Err(QiError::IdentityMismatch);
        }
        let candidates = self.signed_candidates(id)?;
        if candidates.len() > 32 {
            return Err(QiError::InvalidPolicy);
        }
        let parent = candidates
            .iter()
            .find(|c| c.hash().ok() == Some(intent.parent))
            .ok_or(QiError::MissingSignedPayload)?;
        let mut transaction = parent.transaction().clone();
        let selected: BTreeSet<_> = intent.change_indexes.iter().copied().collect();
        if selected.len() != intent.change_indexes.len() {
            return Err(QiError::InvalidPolicy);
        }
        let metadata: BTreeMap<_, _> = self
            .store
            .addresses()?
            .into_iter()
            .map(|m| (m.address(), m))
            .collect();
        let mut old_change = Vec::new();
        for &index in &selected {
            old_change.push(
                transaction
                    .outputs
                    .get(index as usize)
                    .ok_or(QiError::InvalidPolicy)?
                    .clone(),
            );
        }
        for output in old_change.iter().chain(&intent.change_outputs) {
            let address =
                QiAddress::try_from(output.address).map_err(|_| QiError::IdentityMismatch)?;
            if address.zone() != self.store.scope().zone {
                return Err(QiError::IdentityMismatch);
            }
            self.key_for(
                metadata
                    .get(&output.address)
                    .ok_or(QiError::IdentityMismatch)?,
            )?;
        }
        if value(&intent.change_outputs)? >= value(&old_change)? {
            return Err(QiError::InvalidPolicy);
        }
        transaction.outputs = transaction
            .outputs
            .into_iter()
            .enumerate()
            .filter(|(i, _)| !selected.contains(&(*i as u16)))
            .map(|(_, o)| o)
            .chain(intent.change_outputs)
            .collect();
        if transaction.inputs.len() > policy.max_inputs
            || transaction.outputs.len() > policy.max_outputs
        {
            return Err(QiError::InvalidPolicy);
        }
        validate_shape(&transaction)?;
        let snapshot = self.store.snapshot()?;
        let checkpoint = snapshot.checkpoint.ok_or(QiError::MissingSnapshot)?;
        self.verify_network().await?;
        let height = self
            .candidate_height(snapshot.generation, checkpoint, policy.max_snapshot_age)
            .await?;
        let coins: BTreeMap<_, _> = snapshot.coins.iter().map(|c| (c.outpoint, c)).collect();
        let mut input_value = U256::ZERO;
        let mut denominations = Vec::new();
        let claims: BTreeSet<_> = self.store.reserved_outpoints(id)?.into_iter().collect();
        if claims
            != transaction
                .inputs
                .iter()
                .map(|i| i.previous_output)
                .collect()
        {
            return Err(QiError::IdentityMismatch);
        }
        for input in &transaction.inputs {
            let coin = coins
                .get(&input.previous_output)
                .ok_or(QiError::StaleSnapshot)?;
            if coin.address.address() != input.public_key.address()
                || coin.unlock_height > height
                || coin.expires_at.is_some_and(|end| height >= end)
            {
                return Err(QiError::StaleSnapshot);
            }
            self.key_for(
                metadata
                    .get(&input.public_key.address())
                    .ok_or(QiError::IdentityMismatch)?,
            )?;
            input_value = input_value
                .checked_add(U256::from(coin.denomination.value()))
                .ok_or(QiError::InvalidPolicy)?;
            denominations.push(coin.denomination);
        }
        quai_wallet::preserves_denominations(
            &denominations,
            &transaction
                .outputs
                .iter()
                .map(|o| o.denomination)
                .collect::<Vec<_>>(),
        )?;
        let fee = input_value
            .checked_sub(value(&transaction.outputs)?)
            .ok_or(QiError::InvalidPolicy)?;
        if input_value < value(&parent.transaction().outputs)? || fee > policy.max_fee {
            return Err(SelectionError::FeeBudgetExceeded.into());
        }
        let quote = if transaction.data.is_empty() {
            Some(self.provider.estimate_qi_fee(&transaction).await?)
        } else if let Some(profile) = special_profile {
            Some(
                self.provider
                    .estimate_qi_special_fee(&transaction, profile)
                    .await?
                    .qits,
            )
        } else {
            None
        };
        if quote.is_some_and(|required| required > fee) {
            return Err(SelectionError::FeeBudgetExceeded.into());
        }
        let final_height = self
            .candidate_height(snapshot.generation, checkpoint, policy.max_snapshot_age)
            .await?;
        let current = self.store.snapshot()?;
        if current.generation != snapshot.generation || current.checkpoint != Some(checkpoint) {
            return Err(QiError::StaleSnapshot);
        }
        if transaction.inputs.iter().any(|i| {
            coins[&i.previous_output].unlock_height > final_height
                || coins[&i.previous_output]
                    .expires_at
                    .is_some_and(|end| final_height >= end)
        }) {
            return Err(QiError::StaleSnapshot);
        }
        Ok(PreparedQiReplacement {
            instance: self.store.instance(),
            scope: self.store.scope(),
            id,
            parent: intent.parent,
            transaction,
            fee,
        })
    }
    /// Sign the exact candidate with locally verified origins and persist the edge.
    pub fn sign_replacement(
        &mut self,
        prepared: &PreparedQiReplacement,
    ) -> Result<SignedQiOperation, QiError> {
        if prepared.instance != self.store.instance() || prepared.scope != self.store.scope() {
            return Err(QiError::IdentityMismatch);
        }
        // MuSig signatures use fresh nonce randomness. Reopening the same frozen
        // candidate must return its existing bytes rather than create another hash.
        for edge in self.store.replacement_candidates(prepared.id)? {
            let signed = SignedQiOperation::decode(&edge.payload)?;
            if edge.parent == prepared.parent && signed.transaction() == &prepared.transaction {
                return Ok(signed);
            }
        }
        let metadata: BTreeMap<_, _> = self
            .store
            .addresses()?
            .into_iter()
            .map(|m| (m.address(), m))
            .collect();
        let keys = prepared
            .transaction
            .inputs
            .iter()
            .map(|i| {
                self.key_for(
                    metadata
                        .get(&i.public_key.address())
                        .ok_or(QiError::IdentityMismatch)?,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let refs: Vec<_> = keys.iter().collect();
        let signed = match prepared.transaction.data.len() {
            0 => SignedQiOperation::Transfer(prepared.transaction.sign_local(&refs)?),
            20 => SignedQiOperation::Wrapping(
                QiWrappingTransaction::from_transaction(prepared.transaction.clone())?
                    .sign_local(&refs)?,
            ),
            22 => SignedQiOperation::Conversion(
                QiConversionTransaction::from_transaction(prepared.transaction.clone())?
                    .sign_local(&refs)?,
            ),
            _ => return Err(QiError::InvalidPolicy),
        };
        self.store
            .commit_qi_replacement(prepared.id, prepared.parent, &signed)?;
        Ok(signed)
    }
    /// Submit a selected persisted candidate once; retain all candidates and input
    /// claims on timeout, pool conflict, reorg or node rejection.
    pub async fn broadcast_candidate(
        &mut self,
        id: ReservationId,
        hash: Hash32,
    ) -> Result<BroadcastResult, QiError> {
        let signed = self
            .signed_candidates(id)?
            .into_iter()
            .find(|c| c.hash().ok() == Some(hash))
            .ok_or(QiError::MissingSignedPayload)?;
        self.verify_network().await?;
        self.store.mark_submitted(id)?;
        Ok(match &signed {
            SignedQiOperation::Transfer(tx) => self.provider.broadcast_qi(tx).await?,
            SignedQiOperation::Conversion(tx) => self.provider.broadcast_qi_conversion(tx).await?,
            SignedQiOperation::Wrapping(tx) => self.provider.broadcast_qi_wrapping(tx).await?,
        })
    }
}

/// Current source-reported status of one immutable candidate. Absence never
/// releases its family claim or proves propagation failed.
#[derive(Clone, Debug)]
pub enum QiCandidateStatus {
    /// No receipt or indexed transaction was observed.
    NotObserved,
    /// Indexed without an observed receipt.
    Pending,
    /// Receipt names a noncanonical or unavailable block.
    Noncanonical,
    /// Canonical origin inclusion; does not imply external destination settlement.
    Included {
        /// Current canonical block association.
        block: quai_provider::BlockReference,
        /// Origin execution outcome, including failures that still consume the nonce.
        outcome: quai_provider::ReceiptOutcome,
        /// Sampled depth including the origin block, not finality proof.
        confirmations: u64,
    },
}
/// Reconciled candidate identities, with at most one canonical member per input set.
#[derive(Clone, Debug)]
pub struct QiFamilyObservation {
    /// Original followed by persisted replacement candidates.
    pub candidates: Vec<(Hash32, QiCandidateStatus)>,
    /// Canonically included member, if observed.
    pub canonical: Option<Hash32>,
}
impl<T: Transport> QiSession<'_, T> {
    /// Reconcile every durable candidate, including after backup restoration or
    /// an ambiguous send. Checks origin block hashes and the sampled head. No
    /// automatic broadcast, candidate deletion or input release occurs.
    pub async fn observe_candidates(
        &mut self,
        id: ReservationId,
    ) -> Result<QiFamilyObservation, QiError> {
        let candidates = self.signed_candidates(id)?;
        self.verify_network().await?;
        let zone = self.store.scope().zone;
        let tip = self
            .provider
            .latest_header(zone)
            .await?
            .ok_or(QiError::StaleSnapshot)?;
        let mut observations = Vec::with_capacity(candidates.len());
        let mut canonical = None;
        let mut anchors = Vec::new();
        for candidate in candidates {
            let hash = candidate.hash().map_err(|_| QiError::InvalidPolicy)?;
            let status = if let Some(receipt) = self.provider.receipt(zone, hash).await? {
                if receipt.kind != quai_provider::TransactionKind::Qi {
                    return Err(QiError::IdentityMismatch);
                }
                let block = quai_provider::BlockReference {
                    number: receipt.inclusion.block_number,
                    hash: receipt.inclusion.block_hash,
                };
                if self
                    .provider
                    .header_at(zone, block.number)
                    .await?
                    .is_some_and(|h| h.hash == block.hash)
                {
                    if canonical.replace(hash).is_some() {
                        return Err(QiError::StaleSnapshot);
                    }
                    let confirmations = tip
                        .number
                        .checked_sub(block.number)
                        .and_then(|d| d.checked_add(1))
                        .ok_or(QiError::StaleSnapshot)?;
                    anchors.push(block);
                    QiCandidateStatus::Included {
                        block,
                        outcome: receipt.outcome,
                        confirmations,
                    }
                } else {
                    QiCandidateStatus::Noncanonical
                }
            } else if self.provider.transaction(zone, hash).await?.is_some() {
                QiCandidateStatus::Pending
            } else {
                QiCandidateStatus::NotObserved
            };
            observations.push((hash, status));
        }
        anchors.push(quai_provider::BlockReference {
            number: tip.number,
            hash: tip.hash,
        });
        for block in anchors {
            if self
                .provider
                .header_at(zone, block.number)
                .await?
                .is_none_or(|h| h.hash != block.hash)
            {
                return Err(QiError::StaleSnapshot);
            }
        }
        Ok(QiFamilyObservation {
            candidates: observations,
            canonical,
        })
    }
}
