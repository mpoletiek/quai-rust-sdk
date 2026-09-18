//! Signer-free, atomic public recovery summaries for immutable candidate families.
use super::*;
use crate::candidate_observation::{FamilyObservationError, observe_signed_candidates};
use quai_primitives::Hash32;
use quai_provider::BlockReference;
use serde_json::json;

pub use crate::candidate_observation::CandidateObservation;
/// A bounded family view saved atomically before returning. No private keys are needed.
#[derive(Clone, Debug)]
#[non_exhaustive]
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
    let candidates: Vec<_> = std::iter::once(root)
        .chain(variants.into_iter().map(|v| v.payload))
        .collect();
    let hashes: Vec<_> = candidates
        .iter()
        .map(|bytes| {
            if let Ok(tx) = SignedQuaiTransaction::decode(bytes) {
                tx.hash()
            } else {
                SignedQiOperation::decode(bytes).and_then(|tx| tx.hash())
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let expected = store
        .observation_cache(id, hashes[0], u16::MAX)?
        .map(|c| c.revision);
    let result = observe_signed_candidates(provider, store.scope(), &candidates)
        .await
        .map_err(|e| match e {
            FamilyObservationError::Provider(e) => QiError::Provider(e),
            FamilyObservationError::Transaction(e) => QiError::Transaction(e),
            FamilyObservationError::Invalid => QiError::IdentityMismatch,
            FamilyObservationError::Changed => QiError::StaleSnapshot,
        });
    let (observations, canonical, head) = match result {
        Ok(result) => (result.candidates, result.canonical, result.head),
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
