//! Exact fee-only replacement preparation with durable candidate custody.
use super::*;
/// Explicit pool bump and maximum debit policy. Pinned go-quai defaults to a
/// five-percent pool bump, but nodes can configure a different value.
#[derive(Clone, Copy, Debug)]
pub struct ReplacementPolicy {
    /// Required bump relative to the selected parent, 1..=1000 percent.
    pub minimum_price_bump_percent: u16,
    /// Reviewed operation fee/debit limits.
    pub fees: FeePolicy,
}
/// Frozen fee-only replacement; all other transaction fields match its parent.
#[derive(Debug)]
pub struct PreparedAccountReplacement {
    instance: StoreInstance,
    genesis: Hash32,
    id: ReservationId,
    parent: Hash32,
    sender: QuaiAddress,
    transaction: QuaiTransaction,
}
impl PreparedAccountReplacement {
    /// Exact unsigned candidate for review before signing.
    pub fn transaction(&self) -> &QuaiTransaction {
        &self.transaction
    }
    /// Candidate being replaced; it remains recoverable and may still mine.
    pub fn parent_hash(&self) -> Hash32 {
        self.parent
    }
    /// Original durable family reservation.
    pub fn reservation_id(&self) -> ReservationId {
        self.id
    }
}
impl<T: Transport, S: Signer> AccountSession<'_, T, S> {
    /// Load all validated signed candidates, original first. Each shares exactly
    /// one original nonce claim. This enables restart receipt/replacement lookup.
    pub fn signed_candidates(
        &mut self,
        id: ReservationId,
    ) -> Result<Vec<SignedQuaiTransaction>, AccountError> {
        let root = self
            .store
            .signed_payload(id)?
            .ok_or(AccountError::MissingSignedPayload)?;
        let mut candidates =
            vec![SignedQuaiTransaction::decode(&root).map_err(|_| AccountError::InvalidOperation)?];
        candidates.extend(
            self.store
                .quai_replacements(id)?
                .iter()
                .map(|edge| {
                    SignedQuaiTransaction::decode(&edge.payload)
                        .map_err(|_| AccountError::InvalidOperation)
                })
                .collect::<Result<Vec<_>, _>>()?,
        );
        if candidates.iter().any(|tx| {
            tx.from().address() != self.signer.address()
                || tx.transaction().chain_id != self.store.scope().chain_id
        }) {
            return Err(AccountError::IdentityMismatch);
        }
        Ok(candidates)
    }
    /// Estimate an increased gas price without changing recipient, amount, data,
    /// gas limit, access list or nonce. A currently confirmed nonce is refused.
    /// No new nonce is allocated and no old candidate or claim is removed.
    pub async fn prepare_replacement(
        &mut self,
        id: ReservationId,
        parent: Hash32,
        policy: ReplacementPolicy,
    ) -> Result<PreparedAccountReplacement, AccountError> {
        if !(1..=1000).contains(&policy.minimum_price_bump_percent)
            || policy.fees.gas_margin_bps > 10_000
        {
            return Err(AccountError::InvalidOperation);
        }
        if self.store.reservation(id)?.is_none_or(|r| {
            !matches!(
                r.state,
                ReservationState::Signed | ReservationState::Submitted
            )
        }) {
            return Err(AccountError::InvalidOperation);
        }
        let candidates = self.signed_candidates(id)?;
        if candidates.len() > 32 {
            return Err(AccountError::InvalidOperation);
        }
        let parent_tx = candidates
            .iter()
            .find(|tx| tx.hash().ok() == Some(parent))
            .ok_or(AccountError::InvalidOperation)?;
        let sender = parent_tx.from();
        let mut transaction = parent_tx.transaction().clone();
        if transaction.gas_limit > policy.fees.max_gas {
            return Err(AccountError::FeeLimit);
        }
        self.verify_network().await?;
        let confirmed = self
            .provider
            .latest_header(sender.zone())
            .await?
            .ok_or(AccountError::ObservationChanged)?;
        if self
            .provider
            .transaction_count(sender, BlockTag::Number(U256::from(confirmed.number)))
            .await?
            > transaction.nonce
        {
            return Err(AccountError::InvalidOperation);
        }
        let observation = self.observation().await?;
        let old = transaction.gas_price;
        let numerator = old
            .checked_mul(U256::from(
                100 + u64::from(policy.minimum_price_bump_percent),
            ))
            .ok_or(AccountError::FeeLimit)?;
        let minimum = (numerator / U256::from(100))
            .checked_add(U256::from(u8::from(
                numerator % U256::from(100) != U256::ZERO,
            )))
            .ok_or(AccountError::FeeLimit)?;
        transaction.gas_price = self
            .provider
            .gas_price(sender.zone())
            .await?
            .max(minimum)
            .max(
                old.checked_add(U256::from(1))
                    .ok_or(AccountError::FeeLimit)?,
            );
        let fee = transaction
            .gas_price
            .checked_mul(U256::from(transaction.gas_limit))
            .ok_or(AccountError::FeeLimit)?;
        if transaction.gas_price > policy.fees.max_gas_price || fee > policy.fees.max_total_fee {
            return Err(AccountError::FeeLimit);
        }
        let gas = if transaction
            .to
            .is_some_and(|to| to.ledger() == quai_primitives::Ledger::Qi)
        {
            self.quote_conversion(sender, &transaction, policy.fees, observation.0)
                .await?
                .0
        } else {
            let request = CallRequest {
                from: sender,
                to: transaction
                    .to
                    .map(QuaiAddress::try_from)
                    .transpose()
                    .map_err(|_| AccountError::InvalidOperation)?,
                gas: Some(transaction.gas_limit),
                gas_price: Some(transaction.gas_price),
                value: Some(transaction.value),
                nonce: Some(transaction.nonce),
                input: RpcData::new(transaction.data.clone())?,
                access_list: transaction
                    .access_list
                    .iter()
                    .map(|a| AccessListItem {
                        address: a.address,
                        storage_keys: a.storage_keys.clone(),
                    })
                    .collect(),
            };
            self.quote_fee(&request, policy.fees, observation.0)
                .await?
                .0
        };
        if gas > transaction.gas_limit {
            return Err(AccountError::FeeLimit);
        }
        if fee
            .checked_add(transaction.value)
            .ok_or(AccountError::FeeLimit)?
            > self.provider.balance(sender, observation.0).await?
        {
            return Err(AccountError::InsufficientBalance);
        }
        self.verify_observation(observation).await?;
        if self
            .provider
            .header_at(sender.zone(), confirmed.number)
            .await?
            .is_none_or(|h| h.hash != confirmed.hash)
        {
            return Err(AccountError::ObservationChanged);
        }
        transaction
            .signing_digest()
            .map_err(|_| AccountError::InvalidOperation)?;
        Ok(PreparedAccountReplacement {
            instance: self.store.instance(),
            genesis: self.store.scope().genesis,
            id,
            parent,
            sender,
            transaction,
        })
    }
    /// Sign the reviewed candidate and persist its replacement edge before exposure.
    pub fn sign_replacement(
        &mut self,
        prepared: &PreparedAccountReplacement,
    ) -> Result<SignedQuaiTransaction, AccountError> {
        if prepared.instance != self.store.instance()
            || prepared.genesis != self.store.scope().genesis
            || prepared.sender.address() != self.signer.address()
        {
            return Err(AccountError::IdentityMismatch);
        }
        let signed = self.signer.sign_quai(&prepared.transaction)?;
        if signed.transaction() != &prepared.transaction || signed.from() != prepared.sender {
            return Err(AccountError::PayloadMismatch);
        }
        self.store
            .commit_quai_replacement(prepared.id, prepared.parent, &signed)?;
        Ok(signed)
    }
    /// Submit one explicitly selected, already-persisted candidate once. The
    /// entire family retains its nonce claim even on timeout or node rejection.
    pub async fn broadcast_candidate(
        &mut self,
        id: ReservationId,
        hash: Hash32,
    ) -> Result<BroadcastResult, AccountError> {
        let signed = self
            .signed_candidates(id)?
            .into_iter()
            .find(|tx| tx.hash().ok() == Some(hash))
            .ok_or(AccountError::MissingSignedPayload)?;
        self.verify_network().await?;
        let record = self
            .store
            .reservation(id)?
            .ok_or(AccountError::InvalidOperation)?;
        if record.state == ReservationState::Signed {
            self.store.mark_submitted(id)?;
        } else if record.state != ReservationState::Submitted {
            return Err(AccountError::InvalidOperation);
        }
        Ok(self.provider.broadcast(&signed).await?)
    }
}

/// Current source-reported status of one immutable candidate. Absence never
/// releases its family claim or proves propagation failed.
#[derive(Clone, Debug)]
pub enum AccountCandidateStatus {
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
/// Reconciled candidate identities, with at most one canonical member per nonce.
#[derive(Clone, Debug)]
pub struct AccountFamilyObservation {
    /// Original followed by persisted replacement candidates.
    pub candidates: Vec<(Hash32, AccountCandidateStatus)>,
    /// Canonically included member, if observed.
    pub canonical: Option<Hash32>,
}
impl<T: Transport, S: Signer> AccountSession<'_, T, S> {
    /// Reconcile every durable candidate, including after backup restoration or
    /// an ambiguous send. Checks origin block hashes and the sampled head. No
    /// automatic broadcast, candidate deletion or nonce release occurs.
    pub async fn observe_candidates(
        &mut self,
        id: ReservationId,
    ) -> Result<AccountFamilyObservation, AccountError> {
        let candidates = self.signed_candidates(id)?;
        self.verify_network().await?;
        let zone = self.store.scope().zone;
        let tip = self
            .provider
            .latest_header(zone)
            .await?
            .ok_or(AccountError::ObservationChanged)?;
        let mut observations = Vec::with_capacity(candidates.len());
        let mut canonical = None;
        let mut anchors = Vec::new();
        for candidate in candidates {
            let hash = candidate
                .hash()
                .map_err(|_| AccountError::InvalidOperation)?;
            let status = if let Some(receipt) = self.provider.receipt(zone, hash).await? {
                if receipt.kind != quai_provider::TransactionKind::Quai
                    || receipt
                        .from
                        .is_some_and(|from| from != candidate.from().address())
                    || receipt
                        .to
                        .is_some_and(|to| Some(to) != candidate.transaction().to)
                {
                    return Err(AccountError::PayloadMismatch);
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
                        return Err(AccountError::ObservationChanged);
                    }
                    let confirmations = tip
                        .number
                        .checked_sub(block.number)
                        .and_then(|d| d.checked_add(1))
                        .ok_or(AccountError::ObservationChanged)?;
                    anchors.push(block);
                    AccountCandidateStatus::Included {
                        block,
                        outcome: receipt.outcome,
                        confirmations,
                    }
                } else {
                    AccountCandidateStatus::Noncanonical
                }
            } else if self.provider.transaction(zone, hash).await?.is_some() {
                AccountCandidateStatus::Pending
            } else {
                AccountCandidateStatus::NotObserved
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
                return Err(AccountError::ObservationChanged);
            }
        }
        Ok(AccountFamilyObservation {
            candidates: observations,
            canonical,
        })
    }
}
