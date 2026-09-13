//! Apply canonical head removals to durable public wallet state before advancing.
use super::*;
use quai_provider::{HeadTracker, HeadUpdate};
use quai_wallet::storage::ReorgInvalidation;

/// Head replay result after any required durable rollback has committed.
#[derive(Clone, Debug)]
pub struct WalletReplayUpdate {
    /// Bounded linked head page; removals precede additions.
    pub heads: HeadUpdate,
    /// Atomic wallet invalidation performed for a reorganized suffix.
    pub invalidation: Option<ReorgInvalidation>,
    /// Changed heads require current discovery/operation refresh. Added headers
    /// alone cannot reconstruct historical Qi outpoints from latest-only RPC.
    pub refresh_required: bool,
}
/// Poll a cloned head cursor and atomically invalidate reorganized wallet state.
/// Only advance the supplied cursor after the database commits. A network error,
/// missing replay history or concurrent snapshot update leaves it unchanged.
/// No keys, broadcasts, claim release or fabricated historical balances occur.
/// On restart, construct a cursor from an explicitly trusted checkpoint and
/// revalidate observations; the in-memory ancestry window is not a backup.
pub async fn reconcile_head_replay<T: Transport>(
    provider: &Provider<T>,
    store: &mut SqliteStore,
    tracker: &mut HeadTracker,
) -> Result<WalletReplayUpdate, QiError> {
    let scope = store.scope();
    if tracker.zone() != scope.zone
        || tracker.genesis() != scope.genesis
        || provider.chain_id(scope.zone.into()).await? != scope.chain_id
    {
        return Err(QiError::IdentityMismatch);
    }
    let generation = store.observation_generation()?;
    let mut staged = tracker.clone();
    let heads = staged.poll(provider).await?;
    let invalidation = if let Some(first) = heads.removed.last() {
        Some(store.invalidate_reorg_from(generation, U256::from(first.number))?)
    } else {
        if store.observation_generation()? != generation {
            return Err(quai_wallet::storage::StorageError::StaleSnapshot.into());
        }
        None
    };
    let refresh_required = !heads.added.is_empty() || !heads.removed.is_empty();
    *tracker = staged;
    Ok(WalletReplayUpdate {
        heads,
        invalidation,
        refresh_required,
    })
}
