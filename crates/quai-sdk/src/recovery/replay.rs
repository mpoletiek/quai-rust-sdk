//! Apply canonical head removals to durable public wallet state before advancing.
use super::*;
use quai_provider::{HeadTracker, HeadUpdate, ProviderError};
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
/// For restart-safe ancestry and atomic cursor custody use
/// `reconcile_persisted_head_replay` instead. This variant owns only memory.
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

/// Canonical replay with its ancestry committed to the same wallet database.
#[derive(Clone, Debug)]
pub struct PersistedWalletReplayUpdate {
    /// Applied heads and any conservative source-state invalidation.
    pub update: WalletReplayUpdate,
    /// Durable ancestry revision after the atomic commit.
    pub revision: u64,
}
/// Resume the scope's saved ancestry, poll one bounded page and commit ancestry
/// plus any reorg invalidation atomically. Competing processes are fenced by the
/// pre-RPC scope generation and ancestry revision. No cursor advances on error.
///
/// `initial` is used only when no saved payload exists (including a tombstone).
/// It must be a caller-trusted cursor bound to this scope; otherwise it is ignored.
/// Missing initial state or a reorg older than retained history fails explicitly.
/// Current discovery and settlement refresh remain separate after changed heads.
/// Backups exclude this observation cache and restore tombstones existing ancestry.
pub async fn reconcile_persisted_head_replay<T: Transport>(
    provider: &Provider<T>,
    store: &mut SqliteStore,
    initial: Option<&HeadTracker>,
) -> Result<PersistedWalletReplayUpdate, QiError> {
    let scope = store.scope();
    let generation = store.observation_generation()?;
    let saved = store.head_replay_state()?;
    let mut tracker = match saved.as_ref().and_then(|state| state.payload.as_deref()) {
        Some(bytes) => HeadTracker::from_state(bytes, scope.zone, scope.genesis)?,
        None => initial
            .cloned()
            .ok_or(ProviderError::ReplayHistoryUnavailable)?,
    };
    if tracker.zone() != scope.zone
        || tracker.genesis() != scope.genesis
        || provider.chain_id(scope.zone.into()).await? != scope.chain_id
    {
        return Err(QiError::IdentityMismatch);
    }
    let heads = tracker.poll(provider).await?;
    let committed = store.commit_head_replay(
        generation,
        saved.as_ref().map(|state| state.revision),
        Some(&tracker.export_state()),
        heads.removed.last().map(|head| U256::from(head.number)),
    )?;
    let refresh_required = !heads.added.is_empty() || !heads.removed.is_empty();
    Ok(PersistedWalletReplayUpdate {
        update: WalletReplayUpdate {
            heads,
            invalidation: committed.invalidation,
            refresh_required,
        },
        revision: committed.revision,
    })
}
