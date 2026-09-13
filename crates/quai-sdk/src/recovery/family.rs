//! Signer-free, atomic public recovery summaries for immutable candidate families.
use super::*;
use quai_primitives::Hash32;
use quai_provider::BlockReference;
use serde_json::json;

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
/// A bounded family view saved atomically before returning. No private keys are needed.
#[derive(Clone, Debug)]
pub struct FamilyUpdate {
    /// Revision in root candidate cache slot 65535.
    pub revision: u64,
    /// Root first, followed by validated durable replacement edges (at most 33 total).
    pub candidates: Vec<(Hash32, CandidateObservation)>,
    /// At most one canonical winner, including a failed execution.
    pub canonical: Option<Hash32>,
    /// Revalidated head used to count confirmations.
    pub head: BlockReference,
}
/// Reconstruct and observe the complete signed family without unlocking keys.
/// Rechecks the network, each inclusion, and the sampled head. Conflicting
/// canonical members fail explicitly. The SQLite writer checks that no candidate
/// was added and no newer cache revision was committed while the RPCs ran.
/// Errors invalidate only the old family cache; signed bytes and claims survive.
/// Root reservation inclusion is independent; this cache also tracks replacements.
pub async fn track_family<T: Transport>(
    provider: &Provider<T>,
    store: &mut SqliteStore,
    id: ReservationId,
) -> Result<FamilyUpdate, QiError> {
    let generation = store.observation_generation()?;
    let root = store
        .signed_payload(id)?
        .ok_or(QiError::MissingSignedPayload)?;
    let variants = store.replacement_candidates(id)?;
    let candidates = std::iter::once(root)
        .chain(variants.into_iter().map(|v| v.payload))
        .map(|bytes| SignedIntent::decode(&bytes, store.scope()))
        .collect::<Result<Vec<_>, _>>()?;
    let hashes: Vec<_> = candidates.iter().map(|c| c.hash).collect();
    let expected = store
        .observation_cache(id, hashes[0], u16::MAX)?
        .map(|c| c.revision);
    let result = observe(provider, store.scope(), &candidates).await;
    let (observations, canonical, head) = match result {
        Ok(result) => result,
        Err(error) => {
            // A new candidate also prevents this stale attempt from invalidating
            // a newer family view, even if its cache has not yet been updated.
            let _ = store.compare_exchange_family_observation_scoped(
                id,
                &hashes,
                (generation, expected),
                None,
            );
            return Err(error);
        }
    };
    let rows: Vec<_> = observations.iter().map(|(hash, state)| {
        let value = match state {
            CandidateObservation::NotObserved => json!("not_observed"),
            CandidateObservation::Pending => json!("pending"),
            CandidateObservation::Noncanonical => json!("noncanonical"),
            CandidateObservation::Included {block,outcome,confirmations} => json!({"block":block.number,"hash":block.hash.to_string(),"outcome":match outcome {ReceiptOutcome::Succeeded=>"succeeded",ReceiptOutcome::Failed=>"failed",ReceiptOutcome::Locked=>"locked",ReceiptOutcome::PostState(_)=>"legacy"},"confirmations":confirmations}),
        };
        json!([hash.to_string(),value])
    }).collect();
    let payload = serde_json::to_vec(&json!({"version":1,"kind":"family","head":{"number":head.number,"hash":head.hash.to_string()},"canonical":canonical.map(|h|h.to_string()),"candidates":rows})).map_err(|_|QiError::InvalidPolicy)?;
    let revision = store.compare_exchange_family_observation_scoped(
        id,
        &hashes,
        (generation, expected),
        Some(&payload),
    )?;
    Ok(FamilyUpdate {
        revision,
        candidates: observations,
        canonical,
        head,
    })
}
struct SignedIntent {
    hash: Hash32,
    account: Option<SignedQuaiTransaction>,
}
impl SignedIntent {
    fn decode(bytes: &[u8], scope: quai_wallet::storage::NetworkScope) -> Result<Self, QiError> {
        if let Ok(account) = SignedQuaiTransaction::decode(bytes) {
            if account.transaction().chain_id != scope.chain_id
                || account.from().address().zone().ok() != Some(scope.zone)
            {
                return Err(QiError::IdentityMismatch);
            }
            return Ok(Self {
                hash: account.hash()?,
                account: Some(account),
            });
        }
        let qi = SignedQiOperation::decode(bytes)?;
        if qi.transaction().chain_id != scope.chain_id || qi.origin_zone().ok() != Some(scope.zone)
        {
            return Err(QiError::IdentityMismatch);
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
    scope: quai_wallet::storage::NetworkScope,
    candidates: &[SignedIntent],
) -> Result<Observations, QiError> {
    if provider.chain_id(scope.zone.into()).await? != scope.chain_id
        || provider.genesis_hash(scope.zone).await? != scope.genesis
    {
        return Err(QiError::IdentityMismatch);
    }
    let tip = provider
        .latest_header(scope.zone)
        .await?
        .ok_or(QiError::StaleSnapshot)?;
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
                    return Err(QiError::IdentityMismatch);
                }
            } else if receipt.kind != TransactionKind::Qi {
                return Err(QiError::IdentityMismatch);
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
                    return Err(QiError::StaleSnapshot);
                }
                let confirmations = head
                    .number
                    .checked_sub(block.number)
                    .and_then(|n| n.checked_add(1))
                    .ok_or(QiError::StaleSnapshot)?;
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
            return Err(QiError::StaleSnapshot);
        }
    }
    Ok((rows, canonical, head))
}
