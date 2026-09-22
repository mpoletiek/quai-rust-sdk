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
    estimated_gas: u64,
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
    /// Gas the candidate needs now; `transaction().gas_limit` minus this is
    /// the headroom left. See `AccountReplacementQuote::estimated_gas`.
    pub fn estimated_gas(&self) -> u64 {
        self.estimated_gas
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
            crate::account_preflight::AccountPreflightError::NetworkMismatch => {
                AccountError::NetworkMismatch
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
            estimated_gas: quote.estimated_gas(),
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
#[non_exhaustive]
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

/// Nonce reconciliation outcome across durable candidates and unregistered occupants.
#[derive(Clone, Debug)]
pub enum AccountNonceOutcome {
    /// A durable candidate is canonically included.
    Registered(Hash32),
    /// A verified same-sender, same-nonce transaction unknown to this store occupies the
    /// nonce, for example from another device or a restored copy. The claim stays held;
    /// the application decides how to account for the external transaction.
    Unregistered(Box<quai_provider::AccountNonceCandidate>),
    /// No occupant was found. Absence covers only `scanned_through` and never
    /// means dropped, cancelled or safe to reuse.
    Unresolved {
        /// Last contiguous block examined; the next page's preceding anchor.
        scanned_through: Option<quai_provider::BlockReference>,
        /// First unavailable block, when the page stopped early.
        missing_block: Option<u64>,
    },
}
/// Durable family observation together with its nonce outcome.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct AccountNonceObservation {
    /// Registered candidates, exactly as from `observe_candidates`.
    pub family: AccountFamilyObservation,
    /// Registered winner, unregistered occupant or bounded absence.
    pub outcome: AccountNonceOutcome,
}
impl<T: Transport, S: Signer> AccountSession<'_, T, S> {
    /// Reconcile durable candidates, then, if none is canonical, scan one bounded
    /// page for any verified same-sender, same-nonce transaction. This combines
    /// registered-family reconciliation with unregistered replacement discovery.
    /// No broadcast, candidate deletion or nonce release occurs.
    pub async fn observe_nonce(
        &mut self,
        id: ReservationId,
        request: quai_provider::AccountReplacementScanRequest,
    ) -> Result<AccountNonceObservation, AccountError> {
        let family = self.observe_candidates(id).await?;
        if let Some(hash) = family.canonical {
            return Ok(AccountNonceObservation {
                family,
                outcome: AccountNonceOutcome::Registered(hash),
            });
        }
        let original = self
            .signed_candidates(id)?
            .into_iter()
            .next()
            .ok_or(AccountError::MissingSignedPayload)?;
        let scan = self
            .provider
            .observe_account_replacements(&original, self.store.scope().genesis, request)
            .await?;
        let known: Vec<Hash32> = family.candidates.iter().map(|(hash, _)| *hash).collect();
        let outcome = resolve_nonce(
            &known,
            scan.candidate,
            scan.scanned_through,
            scan.missing_block,
        )?;
        Ok(AccountNonceObservation { family, outcome })
    }
}
/// A scan occupant that is itself a durable candidate contradicts the family
/// observation just made, so the caller must observe again.
fn resolve_nonce(
    known: &[Hash32],
    candidate: Option<quai_provider::AccountNonceCandidate>,
    scanned_through: Option<quai_provider::BlockReference>,
    missing_block: Option<u64>,
) -> Result<AccountNonceOutcome, AccountError> {
    match candidate {
        Some(candidate) => {
            let hash = candidate
                .transaction
                .hash()
                .map_err(|_| AccountError::InvalidOperation)?;
            if known.contains(&hash) {
                Err(AccountError::ObservationChanged)
            } else {
                Ok(AccountNonceOutcome::Unregistered(Box::new(candidate)))
            }
        }
        None => Ok(AccountNonceOutcome::Unresolved {
            scanned_through,
            missing_block,
        }),
    }
}
#[cfg(test)]
mod nonce_tests {
    use super::*;
    use quai_crypto::SecretKey;
    use quai_provider::{AccountNonceCandidate, BlockReference, Inclusion, ReplacementReason};

    /// Deterministic public test key whose address is in the Cyprus-1 Quai scope.
    fn key() -> SecretKey {
        (1u32..)
            .find_map(|n| {
                let mut bytes = [7u8; 32];
                bytes[28..].copy_from_slice(&n.to_be_bytes());
                let key = SecretKey::from_bytes(&bytes).ok()?;
                quai_primitives::QuaiAddress::try_from(key.public_key().address())
                    .ok()
                    .filter(|a| a.zone() == quai_primitives::Zone::Cyprus1)
                    .map(|_| key)
            })
            .unwrap()
    }
    fn signed(value: u64) -> SignedQuaiTransaction {
        QuaiTransaction {
            chain_id: U256::from(9),
            nonce: 13,
            to: Some(
                "0x0006506bDE7140b85DED58a40D7444F84cde4821"
                    .parse()
                    .unwrap(),
            ),
            value: U256::from(value),
            gas_limit: 21_000,
            gas_price: U256::from(1),
            data: vec![],
            access_list: vec![],
        }
        .sign(&key())
        .unwrap()
    }
    fn candidate(tx: SignedQuaiTransaction) -> AccountNonceCandidate {
        let mut candidate = AccountNonceCandidate::new(
            tx,
            Inclusion {
                block_hash: Hash32::from_bytes([1; 32]),
                block_number: 10,
                transaction_index: 0,
            },
        );
        candidate.reason = Some(ReplacementReason::Cancelled);
        candidate.confirmations = 2;
        candidate
    }

    #[test]
    fn unregistered_occupants_absence_and_contradictions_are_distinct() {
        let original = signed(1);
        let known = vec![original.hash().unwrap()];
        let external = signed(0);
        let AccountNonceOutcome::Unregistered(found) =
            resolve_nonce(&known, Some(candidate(external.clone())), None, None).unwrap()
        else {
            panic!("external occupant not reported")
        };
        assert_eq!(found.transaction.hash().unwrap(), external.hash().unwrap());
        let anchor = BlockReference {
            number: 9,
            hash: Hash32::from_bytes([2; 32]),
        };
        assert!(matches!(
            resolve_nonce(&known, None, Some(anchor), Some(11)).unwrap(),
            AccountNonceOutcome::Unresolved { scanned_through: Some(a), missing_block: Some(11) } if a == anchor
        ));
        // The family just reported no canonical member; a scan finding one must re-observe.
        assert!(matches!(
            resolve_nonce(&known, Some(candidate(original)), None, None),
            Err(AccountError::ObservationChanged)
        ));
    }
}
