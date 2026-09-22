//! Explicit browser candidate submission and durable canonical reconciliation.
use crate::browser_accounts::{BrowserAccountBook, BrowserAccountError, BrowserAccountSnapshot};
use crate::browser_qi::{BrowserQiBook, BrowserQiError, BrowserQiSnapshot};
use crate::candidate_observation::{
    CandidateObservation, FamilyObservationError, SignedFamilyObservation,
    observe_signed_candidates,
};
use quai_browser::{BrowserError, BrowserSnapshotStore};
use quai_consensus::{SignedQiOperation, SignedQuaiTransaction};
use quai_primitives::Hash32;
use quai_provider::{BroadcastError, BroadcastResult, Provider};
use quai_rpc::{Transport, U256};
use quai_wallet::account_custody::{ReservationId, ReservationState};
use quai_wallet::discovery::{Checkpoint, NetworkScope};
use quai_wallet::metadata::StorageError;

/// Browser custody, canonical read and explicit-send errors. No automatic retry.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BrowserRecoveryError {
    /// Account journal validation failed.
    #[error(transparent)]
    Account(#[from] BrowserAccountError),
    /// Qi journal validation failed.
    #[error(transparent)]
    Qi(#[from] BrowserQiError),
    /// Revision conflict or ambiguous browser persistence failure.
    #[error(transparent)]
    Browser(#[from] BrowserError),
    /// Invalid journal transition or missing signed payload.
    #[error(transparent)]
    State(#[from] StorageError),
    /// Invalid or changed provider observation.
    #[error(transparent)]
    Observation(#[from] FamilyObservationError),
    /// Exact signed settlement interpretation or destination observation failed.
    #[error(transparent)]
    Settlement(#[from] crate::settlement_observation::SettlementObservationError),
    /// Explicit submission failed; an ambiguous outcome retains every candidate.
    #[error(transparent)]
    Broadcast(#[from] BroadcastError),
}
/// A candidate view committed under the revision captured before network reads.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct BrowserFamilyUpdate {
    /// Resulting IndexedDB revision.
    pub revision: u64,
    /// Current source-reported statuses, root first.
    pub observation: SignedFamilyObservation,
    /// Persisted inclusion, possibly retained from an earlier read if its header
    /// remains canonical while receipt indexing is temporarily unavailable.
    pub inclusion: Option<(Hash32, Checkpoint)>,
}
/// Destination result fenced against the original candidate journal. The signed
/// family remains persisted; this view and its page anchors are not stored in the
/// custody frame. Reopen/reconstruct and recheck explicit ranges after restart.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct BrowserSettlementUpdate {
    /// Custody revision committed after the destination reads.
    pub revision: u64,
    /// Advisory external execution, conversion/refund and current Qi credit.
    pub observation: crate::settlement_observation::SignedSettlementObservation,
}
enum Custody<'a> {
    Account(&'a BrowserAccountBook),
    Qi(&'a BrowserQiBook),
}
enum Journal {
    Account(BrowserAccountSnapshot),
    Qi(BrowserQiSnapshot),
}
struct Snapshot {
    revision: u64,
    scope: NetworkScope,
    state: ReservationState,
    inclusion: Option<(Hash32, Checkpoint)>,
    payloads: Vec<Vec<u8>>,
    journal: Journal,
}
impl Snapshot {
    fn submitted(&mut self, id: ReservationId) -> Result<(), StorageError> {
        match &mut self.journal {
            Journal::Account(s) => s.book.mark_submitted(id),
            Journal::Qi(s) => s.book.mark_submitted(id),
        }
    }
    fn inclusion(
        &mut self,
        id: ReservationId,
        next: Option<(Hash32, Checkpoint)>,
    ) -> Result<(), StorageError> {
        if self.inclusion == next {
            return Ok(());
        }
        if let Some(old) = self.inclusion {
            match &mut self.journal {
                Journal::Account(s) => s.book.invalidate_inclusion(id, old)?,
                Journal::Qi(s) => s.book.invalidate_inclusion(id, old)?,
            }
        }
        if let Some((hash, block)) = next {
            match &mut self.journal {
                Journal::Account(s) => s.book.observe_inclusion(id, hash, block)?,
                Journal::Qi(s) => s.book.observe_inclusion(id, hash, block)?,
            }
        }
        self.inclusion = next;
        Ok(())
    }
    fn bytes(&self) -> Result<Vec<u8>, StorageError> {
        match &self.journal {
            Journal::Account(s) => s.book.export_state(),
            Journal::Qi(s) => s.book.export_state(),
        }
    }
}
/// Signer-free recovery of account or Qi families in their original browser book.
/// Construction does not read storage, prompt, query a node or send a transaction.
/// Every writer for the same wallet must use the same durable namespace.
pub struct BrowserRecoverySession<'a, T> {
    provider: &'a Provider<T>,
    custody: Custody<'a>,
}
impl<'a, T: Transport> BrowserRecoverySession<'a, T> {
    /// Bind an account journal, including conversion and deployment families.
    pub fn for_account(provider: &'a Provider<T>, book: &'a BrowserAccountBook) -> Self {
        Self {
            provider,
            custody: Custody::Account(book),
        }
    }
    /// Bind a Qi journal, including ordinary, conversion and wrapping families.
    pub fn for_qi(provider: &'a Provider<T>, book: &'a BrowserQiBook) -> Self {
        Self {
            provider,
            custody: Custody::Qi(book),
        }
    }
    fn store(&self) -> &BrowserSnapshotStore {
        match self.custody {
            Custody::Account(b) => &b.store,
            Custody::Qi(b) => &b.store,
        }
    }
    async fn snapshot(&self, id: ReservationId) -> Result<Snapshot, BrowserRecoveryError> {
        let snapshot = match self.custody {
            Custody::Account(b) => {
                let s = b.snapshot().await?;
                let op = s.book.operation(id).ok_or(StorageError::Transition)?;
                Snapshot {
                    revision: s.revision,
                    scope: s.book.scope(),
                    state: op.state,
                    inclusion: op.inclusion,
                    payloads: std::iter::once(op.payload.clone().ok_or(StorageError::Transition)?)
                        .chain(op.replacements.iter().map(|r| r.payload.clone()))
                        .collect(),
                    journal: Journal::Account(s),
                }
            }
            Custody::Qi(b) => {
                let s = b.snapshot().await?;
                let op = s.book.operation(id).ok_or(StorageError::Transition)?;
                Snapshot {
                    revision: s.revision,
                    scope: s.book.scope(),
                    state: op.state,
                    inclusion: op.inclusion,
                    payloads: std::iter::once(op.payload.clone().ok_or(StorageError::Transition)?)
                        .chain(op.replacements.iter().map(|r| r.payload.clone()))
                        .collect(),
                    journal: Journal::Qi(s),
                }
            }
        };
        if !matches!(
            snapshot.state,
            ReservationState::Signed | ReservationState::Submitted | ReservationState::Confirmed
        ) {
            return Err(StorageError::Transition.into());
        }
        Ok(snapshot)
    }
    /// Load canonical signed bytes, root first, with all retained replacements.
    /// No network or key access; validates the journal's complete candidate family.
    pub async fn signed_candidates(
        &self,
        id: ReservationId,
    ) -> Result<Vec<Vec<u8>>, BrowserRecoveryError> {
        Ok(self.snapshot(id).await?.payloads)
    }
    async fn network(&self, scope: NetworkScope) -> Result<(), BrowserRecoveryError> {
        if !crate::network::on_network(self.provider, scope, scope.zone)
            .await
            .map_err(FamilyObservationError::from)?
        {
            return Err(FamilyObservationError::Changed.into());
        }
        Ok(())
    }
    /// Submit the exact original persisted payload once. Explicit replay retains
    /// identical bytes; no replacement or new payment is selected automatically.
    pub async fn broadcast_root(
        &self,
        id: ReservationId,
    ) -> Result<BroadcastResult, BrowserRecoveryError> {
        self.broadcast(id, None).await
    }
    /// Submit one explicitly selected persisted candidate once. Persist Submitted
    /// with the pre-RPC revision before dispatch, including on repeated sends.
    pub async fn broadcast_candidate(
        &self,
        id: ReservationId,
        hash: Hash32,
    ) -> Result<BroadcastResult, BrowserRecoveryError> {
        self.broadcast(id, Some(hash)).await
    }
    async fn broadcast(
        &self,
        id: ReservationId,
        selected: Option<Hash32>,
    ) -> Result<BroadcastResult, BrowserRecoveryError> {
        let mut s = self.snapshot(id).await?;
        if s.state == ReservationState::Confirmed {
            return Err(StorageError::Transition.into());
        }
        let bytes = s
            .payloads
            .iter()
            .find(|bytes| selected.is_none_or(|h| payload_hash(bytes).ok() == Some(h)))
            .ok_or(StorageError::Transition)?
            .clone();
        self.network(s.scope).await?;
        s.submitted(id)?;
        self.store()
            .compare_exchange(Some(s.revision), Some(&s.bytes()?))
            .await?;
        Ok(if let Ok(tx) = SignedQuaiTransaction::decode(&bytes) {
            self.provider.broadcast(&tx).await?
        } else {
            match SignedQiOperation::decode(&bytes).map_err(FamilyObservationError::from)? {
                SignedQiOperation::Transfer(tx) => self.provider.broadcast_qi(&tx).await?,
                SignedQiOperation::Conversion(tx) => {
                    self.provider.broadcast_qi_conversion(&tx).await?
                }
                SignedQiOperation::Wrapping(tx) => self.provider.broadcast_qi_wrapping(&tx).await?,
            }
        })
    }
    /// Observe one explicit conversion, wrapping, redemption or cross-zone
    /// destination range from an exact persisted candidate. Uses the same portable
    /// intent reconstruction as native settlement tracking. A concurrent candidate,
    /// backup or custody write rejects the view. Signed claims are never changed.
    /// The result/page anchors are not a persisted settlement cursor; resume with
    /// explicit bounded ranges and revalidated preceding blocks after restart.
    pub async fn observe_settlement(
        &self,
        id: ReservationId,
        candidate: Hash32,
        kind: crate::settlement_observation::SettlementKind,
        request: quai_provider::EtxScanRequest,
        max_outputs: usize,
    ) -> Result<BrowserSettlementUpdate, BrowserRecoveryError> {
        let s = self.snapshot(id).await?;
        let bytes = s
            .payloads
            .iter()
            .find(|bytes| payload_hash(bytes).ok() == Some(candidate))
            .ok_or(StorageError::Transition)?;
        let observation = crate::settlement_observation::observe_signed_settlement(
            self.provider,
            s.scope,
            bytes,
            kind,
            request,
            max_outputs,
        )
        .await?;
        let revision = self
            .store()
            .compare_exchange(Some(s.revision), Some(&s.bytes()?))
            .await?;
        Ok(BrowserSettlementUpdate {
            revision,
            observation,
        })
    }
    /// Reconcile every candidate and atomically apply a canonical winner or lost
    /// inclusion. Receipt absence alone cannot invalidate a still-canonical prior
    /// inclusion. Conflicts/failed reads preserve custody without retries; signed
    /// nonce/input claims remain held after success, failure, locked status or reorg.
    pub async fn reconcile(
        &self,
        id: ReservationId,
    ) -> Result<BrowserFamilyUpdate, BrowserRecoveryError> {
        let mut s = self.snapshot(id).await?;
        let observation = observe_signed_candidates(self.provider, s.scope, &s.payloads).await?;
        let mut next = observation
            .candidates
            .iter()
            .find_map(|(hash, state)| match state {
                CandidateObservation::Included { block, .. } => Some((
                    *hash,
                    Checkpoint {
                        hash: block.hash,
                        height: U256::from(block.number),
                    },
                )),
                _ => None,
            });
        if let Some(old) = s.inclusion {
            let height =
                u64::try_from(old.1.height).map_err(|_| FamilyObservationError::Invalid)?;
            let still_canonical = self
                .provider
                .header_at(s.scope.zone, height)
                .await
                .map_err(FamilyObservationError::from)?
                .is_some_and(|h| h.hash == old.1.hash);
            if still_canonical {
                if next.is_some_and(|n| n != old) {
                    return Err(FamilyObservationError::Changed.into());
                }
                next = Some(old);
            }
        }
        if self
            .provider
            .header_at(s.scope.zone, observation.head.number)
            .await
            .map_err(FamilyObservationError::from)?
            .is_none_or(|h| h.hash != observation.head.hash)
        {
            return Err(FamilyObservationError::Changed.into());
        }
        self.network(s.scope).await?;
        s.inclusion(id, next)?;
        let revision = self
            .store()
            .compare_exchange(Some(s.revision), Some(&s.bytes()?))
            .await?;
        Ok(BrowserFamilyUpdate {
            revision,
            observation,
            inclusion: next,
        })
    }
}
fn payload_hash(bytes: &[u8]) -> Result<Hash32, quai_consensus::TransactionError> {
    if let Ok(tx) = SignedQuaiTransaction::decode(bytes) {
        tx.hash()
    } else {
        SignedQiOperation::decode(bytes).and_then(|tx| tx.hash())
    }
}
