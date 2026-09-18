//! Bounded durable reconciliation of signed operations against a configured node.
mod family;
mod replay;
use crate::qi::QiError;
pub use family::{CandidateObservation, FamilyUpdate, track_family};
use quai_consensus::{SignedQiOperation, SignedQuaiTransaction};
use quai_provider::{Provider, ReceiptOutcome, TransactionKind};
use quai_rpc::{Transport, U256};
use quai_wallet::discovery::Checkpoint;
use quai_wallet::storage::{ReservationId, ReservationState, SqliteStore};
pub use replay::{
    PersistedWalletReplayUpdate, WalletReplayUpdate, reconcile_head_replay,
    reconcile_persisted_head_replay,
};

/// Latest observation of an existing signed operation. Absence never releases claims.
#[derive(Clone, Debug)]
pub enum OperationObservation {
    /// The exact signed transaction has no observed receipt or pending entry.
    /// It may have propagated elsewhere; this is not a definitive dropped state.
    NotObserved,
    /// The node reports the transaction pending or indexed without a receipt.
    Pending,
    /// The receipt's block is canonical in the node view, with sampled confirmations.
    Included {
        /// Canonical inclusion observed for the exact signed transaction.
        block: Checkpoint,
        /// Origin execution status; it does not prove ETX destination settlement.
        outcome: ReceiptOutcome,
        /// Sampled depth, including the inclusion block. Not a finality proof.
        confirmations: u64,
    },
    /// An old inclusion was removed; signed claims remain held for rescan/rebroadcast.
    Reorganized,
}

/// Reconcile one persisted signed payload. Rechecks origin canonicality and
/// updates durable inclusion state; never rebroadcasts or releases claims.
pub async fn reconcile_operation<T: Transport>(
    provider: &Provider<T>,
    store: &mut SqliteStore,
    id: ReservationId,
) -> Result<OperationObservation, QiError> {
    let generation = store.observation_generation()?;
    let scope = store.scope();
    if !crate::network::on_network(provider, scope, scope.zone).await? {
        return Err(QiError::NetworkMismatch);
    }
    let record = store
        .reservation(id)?
        .ok_or(QiError::MissingSignedPayload)?;
    if !matches!(
        record.state,
        ReservationState::Signed | ReservationState::Submitted | ReservationState::Confirmed
    ) {
        return Err(QiError::MissingSignedPayload);
    }
    let hash = record.transaction.ok_or(QiError::MissingSignedPayload)?;
    let bytes = store
        .signed_payload(id)?
        .ok_or(QiError::MissingSignedPayload)?;
    let account = SignedQuaiTransaction::decode(&bytes).ok();
    if let Some(signed) = &account {
        if signed.hash().ok() != Some(hash)
            || signed.transaction().chain_id != scope.chain_id
            || signed.from().address().zone().ok() != Some(scope.zone)
        {
            return Err(QiError::IdentityMismatch);
        }
    } else {
        let signed =
            SignedQiOperation::decode(&bytes).map_err(|_| QiError::MissingSignedPayload)?;
        if signed.hash().ok() != Some(hash)
            || signed.transaction().chain_id != scope.chain_id
            || signed.origin_zone().ok() != Some(scope.zone)
        {
            return Err(QiError::IdentityMismatch);
        }
    }
    let mut reorganized = false;
    if let Some(old) = record.inclusion {
        let old_height = u64::try_from(old.height).map_err(|_| QiError::StaleSnapshot)?;
        if provider
            .header_at(scope.zone, old_height)
            .await?
            .is_none_or(|header| header.hash != old.hash)
        {
            store.invalidate_inclusion(id, old)?;
            reorganized = true;
        }
    }
    let Some(receipt) = provider.receipt(scope.zone, hash).await? else {
        if reorganized {
            return Ok(OperationObservation::Reorganized);
        }
        return Ok(if provider.transaction(scope.zone, hash).await?.is_some() {
            OperationObservation::Pending
        } else {
            OperationObservation::NotObserved
        });
    };
    if let Some(signed) = &account {
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
    let block = Checkpoint {
        hash: receipt.inclusion.block_hash,
        height: U256::from(receipt.inclusion.block_number),
    };
    let canonical = provider
        .header_at(scope.zone, receipt.inclusion.block_number)
        .await?;
    if canonical.is_none_or(|header| header.hash != block.hash) {
        clear_inclusion(store, id)?;
        return Ok(OperationObservation::Reorganized);
    }
    let tip = provider
        .latest_header(scope.zone)
        .await?
        .ok_or(QiError::StaleSnapshot)?;
    let confirmations = tip
        .number
        .checked_sub(receipt.inclusion.block_number)
        .and_then(|depth| depth.checked_add(1))
        .ok_or(QiError::StaleSnapshot)?;
    if provider
        .header_at(scope.zone, receipt.inclusion.block_number)
        .await?
        .is_none_or(|header| header.hash != block.hash)
    {
        clear_inclusion(store, id)?;
        return Err(QiError::StaleSnapshot);
    }
    if provider
        .header_at(scope.zone, tip.number)
        .await?
        .is_none_or(|header| header.hash != tip.hash)
    {
        return Err(QiError::StaleSnapshot);
    }
    // A previous included observation can move only after explicit invalidation.
    if let Some(current) = store.reservation(id)?.and_then(|r| r.inclusion)
        && current != block
    {
        store.invalidate_inclusion(id, current)?;
    }
    store.observe_inclusion_scoped(generation, id, hash, block)?;
    Ok(OperationObservation::Included {
        block,
        outcome: receipt.outcome,
        confirmations,
    })
}

fn clear_inclusion(store: &mut SqliteStore, id: ReservationId) -> Result<(), QiError> {
    if let Some(block) = store.reservation(id)?.and_then(|record| record.inclusion) {
        store.invalidate_inclusion(id, block)?;
    }
    Ok(())
}
