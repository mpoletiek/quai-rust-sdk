//! Exact fee-only replacement preparation with durable candidate custody.
use super::*;
pub use crate::account_replacement::ReplacementPolicy;
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
        let quote = crate::account_replacement::quote_account_replacement(
            self.provider,
            self.store.scope(),
            parent_tx,
            self.observation_policy,
            policy,
        )
        .await
        .map_err(|e| match e {
            crate::account_preflight::AccountPreflightError::Provider(e) => {
                AccountError::Provider(e)
            }
            crate::account_preflight::AccountPreflightError::FeeLimit => AccountError::FeeLimit,
            crate::account_preflight::AccountPreflightError::InsufficientBalance => {
                AccountError::InsufficientBalance
            }
            crate::account_preflight::AccountPreflightError::ObservationChanged => {
                AccountError::ObservationChanged
            }
            crate::account_preflight::AccountPreflightError::Invalid => {
                AccountError::InvalidOperation
            }
        })?;
        let transaction = quote.transaction().clone();
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
