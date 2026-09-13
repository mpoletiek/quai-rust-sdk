//! Portable bounded observations of persisted signed transaction candidates.
use quai_consensus::{SignedQiOperation, SignedQuaiTransaction, TransactionError};
use quai_primitives::Hash32;
use quai_provider::{BlockReference, Provider, ProviderError, ReceiptOutcome, TransactionKind};
use quai_rpc::Transport;
use quai_wallet::discovery::NetworkScope;
/// Invalid signed identity, failed RPC or changing canonical observations.
#[derive(Debug, thiserror::Error)]
pub enum FamilyObservationError {
    /// Bounded provider read failed.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// Signed bytes failed consensus validation.
    #[error(transparent)]
    Transaction(#[from] TransactionError),
    /// Invalid scope, candidate bound, duplicate or receipt identity.
    #[error("invalid signed candidate observation")]
    Invalid,
    /// Canonical anchors changed, or competing candidates appear canonical.
    #[error("candidate canonical observations changed")]
    Changed,
}
/// One source-observed candidate. Absence never proves that a signed transaction was dropped.
#[derive(Clone, Debug)]
pub enum CandidateObservation {
    /// No receipt or indexed transaction.
    NotObserved,
    /// Indexed without a receipt.
    Pending,
    /// Receipt names an unavailable or noncanonical block.
    Noncanonical,
    /// Origin inclusion, not destination settlement or irreversible finality.
    Included {
        /// Canonical origin block sampled twice.
        block: BlockReference,
        /// Execution status (failure still consumes an account nonce).
        outcome: ReceiptOutcome,
        /// Sampled depth, including this block.
        confirmations: u64,
    },
}
/// Advisory view of a caller-validated mutually exclusive family. A quote does
/// not persist state, prove finality or authorize releasing signed claims.
#[derive(Clone, Debug)]
pub struct SignedFamilyObservation {
    /// Input order is retained; the caller supplies the root first.
    pub candidates: Vec<(Hash32, CandidateObservation)>,
    /// At most one observed canonical member.
    pub canonical: Option<Hash32>,
    /// Rechecked head used to count confirmations.
    pub head: BlockReference,
}
/// Observe one validated family of 1..=33 canonical signed payloads, bounded to
/// 16 MiB. The caller must validate replacement relationships and ownership (the
/// native/browser journals do). Checks each payload's chain and origin, receipt
/// identity, canonical inclusions and head, and surrounding chain/genesis. Absence
/// never means dropped. Latest-only reads are advisory, not an atomic snapshot.
pub async fn observe_signed_candidates<T: Transport>(
    provider: &Provider<T>,
    scope: NetworkScope,
    payloads: &[Vec<u8>],
) -> Result<SignedFamilyObservation, FamilyObservationError> {
    if payloads.is_empty()
        || payloads.len() > 33
        || scope.chain_id == quai_rpc::U256::ZERO
        || scope.genesis == Hash32::ZERO
        || payloads
            .iter()
            .try_fold(0usize, |n, p| n.checked_add(p.len()))
            .is_none_or(|n| n > 16 * 1024 * 1024)
    {
        return Err(FamilyObservationError::Invalid);
    }
    let candidates = payloads
        .iter()
        .map(|bytes| SignedIntent::decode(bytes, scope))
        .collect::<Result<Vec<_>, _>>()?;
    let unique: std::collections::BTreeSet<_> = candidates.iter().map(|v| v.hash).collect();
    if unique.len() != candidates.len() {
        return Err(FamilyObservationError::Invalid);
    }
    let (candidates, canonical, head) = observe(provider, scope, &candidates).await?;
    Ok(SignedFamilyObservation {
        candidates,
        canonical,
        head,
    })
}
struct SignedIntent {
    hash: Hash32,
    account: Option<SignedQuaiTransaction>,
}
impl SignedIntent {
    fn decode(bytes: &[u8], scope: NetworkScope) -> Result<Self, FamilyObservationError> {
        if let Ok(account) = SignedQuaiTransaction::decode(bytes) {
            if account.transaction().chain_id != scope.chain_id
                || account.from().address().zone().ok() != Some(scope.zone)
            {
                return Err(FamilyObservationError::Invalid);
            }
            return Ok(Self {
                hash: account.hash()?,
                account: Some(account),
            });
        }
        let qi = SignedQiOperation::decode(bytes)?;
        if qi.transaction().chain_id != scope.chain_id || qi.origin_zone().ok() != Some(scope.zone)
        {
            return Err(FamilyObservationError::Invalid);
        }
        Ok(Self {
            hash: qi.hash()?,
            account: None,
        })
    }
}
type Observations = (
    Vec<(Hash32, CandidateObservation)>,
    Option<Hash32>,
    BlockReference,
);
async fn observe<T: Transport>(
    provider: &Provider<T>,
    scope: NetworkScope,
    candidates: &[SignedIntent],
) -> Result<Observations, FamilyObservationError> {
    if provider.chain_id(scope.zone.into()).await? != scope.chain_id
        || provider.genesis_hash(scope.zone).await? != scope.genesis
    {
        return Err(FamilyObservationError::Invalid);
    }
    let tip = provider
        .latest_header(scope.zone)
        .await?
        .ok_or(FamilyObservationError::Changed)?;
    let head = BlockReference {
        number: tip.number,
        hash: tip.hash,
    };
    let mut canonical = None;
    let mut anchors = vec![head];
    let mut rows = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let hash = candidate.hash;
        let status = if let Some(receipt) = provider.receipt(scope.zone, hash).await? {
            if let Some(signed) = &candidate.account {
                if receipt.kind != TransactionKind::Quai
                    || receipt
                        .from
                        .is_some_and(|from| from != signed.from().address())
                    || receipt
                        .to
                        .is_some_and(|to| Some(to) != signed.transaction().to)
                {
                    return Err(FamilyObservationError::Invalid);
                }
            } else if receipt.kind != TransactionKind::Qi {
                return Err(FamilyObservationError::Invalid);
            }
            let block = BlockReference {
                number: receipt.inclusion.block_number,
                hash: receipt.inclusion.block_hash,
            };
            if provider
                .header_at(scope.zone, block.number)
                .await?
                .is_some_and(|h| h.hash == block.hash)
            {
                if canonical.replace(hash).is_some() {
                    return Err(FamilyObservationError::Changed);
                }
                let confirmations = head
                    .number
                    .checked_sub(block.number)
                    .and_then(|n| n.checked_add(1))
                    .ok_or(FamilyObservationError::Changed)?;
                anchors.push(block);
                CandidateObservation::Included {
                    block,
                    outcome: receipt.outcome,
                    confirmations,
                }
            } else {
                CandidateObservation::Noncanonical
            }
        } else if provider.transaction(scope.zone, hash).await?.is_some() {
            CandidateObservation::Pending
        } else {
            CandidateObservation::NotObserved
        };
        rows.push((hash, status));
    }
    for block in anchors {
        if provider
            .header_at(scope.zone, block.number)
            .await?
            .is_none_or(|h| h.hash != block.hash)
        {
            return Err(FamilyObservationError::Changed);
        }
    }
    if provider.chain_id(scope.zone.into()).await? != scope.chain_id
        || provider.genesis_hash(scope.zone).await? != scope.genesis
    {
        return Err(FamilyObservationError::Changed);
    }
    Ok((rows, canonical, head))
}
