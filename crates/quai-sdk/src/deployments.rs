//! Durable deployment/code observation for exact stored account candidates.
use crate::qi::QiError;
use quai_consensus::SignedQuaiTransaction;
use quai_primitives::Hash32;
use quai_provider::{DeploymentObservation, DeploymentReference, Provider, ReceiptOutcome};
use quai_rpc::Transport;
use quai_wallet::storage::{ReservationId, SqliteStore};
use serde_json::json;
/// Current deployment observation saved before return. The observation cache
/// is reconstructible and excluded from backups; signed candidates are retained.
#[derive(Clone, Debug)]
pub struct DeploymentUpdate {
    /// Atomic public cache revision.
    pub revision: u64,
    /// Current canonicality, execution outcome and inclusion-block runtime code.
    pub observation: DeploymentObservation,
}
/// Reconstruct the exact creation from durable signed bytes, observe and store a
/// bounded public summary. Optional runtime hash comparison never changes signed
/// intent. Errors invalidate only the old cache revision; concurrent updates win.
/// No nonce claim is released and no transaction is broadcast by this operation.
pub async fn track_deployment<T: Transport>(
    provider: &Provider<T>,
    store: &mut SqliteStore,
    id: ReservationId,
    candidate: Hash32,
    expected_runtime: Option<Hash32>,
) -> Result<DeploymentUpdate, QiError> {
    let previous = store.observation_cache(id, candidate, 0)?;
    let expected_revision = previous.as_ref().map(|c| c.revision);
    let result = async {
        let root = store
            .signed_payload(id)?
            .ok_or(QiError::MissingSignedPayload)?;
        let variants = store.replacement_candidates(id)?;
        let signed = std::iter::once(root)
            .chain(variants.into_iter().map(|v| v.payload))
            .filter_map(|bytes| SignedQuaiTransaction::decode(&bytes).ok())
            .find(|tx| tx.hash().ok() == Some(candidate))
            .ok_or(QiError::MissingSignedPayload)?;
        let reference =
            DeploymentReference::from_signed(store.scope().genesis, &signed, expected_runtime)?;
        Ok::<_, QiError>((
            provider.observe_deployment(&reference).await?,
            reference.address(),
        ))
    }
    .await;
    let (observation, address) = match result {
        Ok(value) => value,
        Err(error) => {
            let _ = store.compare_exchange_observation(id, candidate, 0, expected_revision, None);
            return Err(error);
        }
    };
    let state = match &observation {
        DeploymentObservation::NoReceipt { transaction_known } => {
            json!({"state":"no_receipt","transaction_known":transaction_known})
        }
        DeploymentObservation::Noncanonical {
            reported,
            canonical,
        } => {
            json!({"state":"noncanonical","reported":{"number":reported.number,"hash":reported.hash.to_string()},"canonical":canonical.map(|b|json!({"number":b.number,"hash":b.hash.to_string()}))})
        }
        DeploymentObservation::Included {
            block,
            outcome,
            confirmations,
            code,
        } => {
            json!({"state":"included","block":{"number":block.number,"hash":block.hash.to_string()},"outcome":match outcome {ReceiptOutcome::Failed=>"failed",ReceiptOutcome::Succeeded=>"succeeded",ReceiptOutcome::Locked=>"locked",ReceiptOutcome::PostState(_)=>"legacy"},"confirmations":confirmations,"code":code.as_ref().map(|c|json!({"length":c.bytes.bytes().len(),"hash":c.hash.to_string(),"matches_expected":c.matches_expected}))})
        }
    };
    let payload=serde_json::to_vec(&json!({"version":1,"kind":"deployment","candidate":candidate.to_string(),"address":address.to_string(),"expected_runtime":expected_runtime.map(|h|h.to_string()),"observation":state})).map_err(|_|QiError::InvalidPolicy)?;
    let revision =
        store.compare_exchange_observation(id, candidate, 0, expected_revision, Some(&payload))?;
    Ok(DeploymentUpdate {
        revision,
        observation,
    })
}
