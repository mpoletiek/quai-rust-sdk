//! Durable account custody across browser tabs/workers, with explicit CAS conflicts.
use quai_browser::{BrowserError, BrowserSnapshotStore, BrowserStorageScope};
use quai_consensus::{QuaiTransaction, SignedQuaiTransaction};
use quai_crypto::PublicKey;
use quai_primitives::Hash32;
use quai_signer::{Signer, SignerError};
use quai_wallet::account_custody::{
    AccountMergeReport, AccountOperationBook, MAX_ACCOUNT_CUSTODY_BYTES, ReservationId,
    ReservationState,
};
use quai_wallet::discovery::{Checkpoint, NetworkScope};
use quai_wallet::metadata::StorageError;

/// Failures never authorize releasing a signed nonce or retrying a network send.
#[derive(Debug, thiserror::Error)]
pub enum BrowserAccountError {
    /// Storage error, including a revision conflict or ambiguous cancelled write.
    #[error(transparent)]
    Browser(#[from] BrowserError),
    /// Scope, custody, bound or transition violation.
    #[error(transparent)]
    State(#[from] StorageError),
    /// Local signing rejected the request.
    #[error(transparent)]
    Signer(#[from] SignerError),
    /// Initialize a never-written namespace explicitly; tombstones require recovery.
    #[error("account custody journal is not initialized")]
    Uninitialized,
}
/// Complete validated public custody observation and its exact revision.
#[derive(Clone, Debug)]
pub struct BrowserAccountSnapshot {
    /// Capture before asynchronous canonical reads; supply it when committing observations.
    pub revision: u64,
    /// Detached inspection; mutations here do not change live storage.
    pub book: AccountOperationBook,
}
/// IndexedDB-backed nonce claims and immutable signed account candidate families.
/// Keys remain caller owned. No network request, fee estimation or automatic send
/// is performed; this layer can retain transfers, conversions and deployments.
#[derive(Clone)]
pub struct BrowserAccountBook {
    store: BrowserSnapshotStore,
    scope: NetworkScope,
    owner: PublicKey,
}
impl BrowserAccountBook {
    /// Open one exact account/network namespace; does not initialize or make RPC calls.
    pub async fn open(
        name: &str,
        scope: NetworkScope,
        owner: PublicKey,
    ) -> Result<Self, BrowserAccountError> {
        AccountOperationBook::new(scope, owner, 0)?;
        let store = BrowserSnapshotStore::open(
            name,
            BrowserStorageScope {
                chain_id: scope.chain_id,
                genesis: scope.genesis,
                zone: scope.zone,
                wallet: AccountOperationBook::account_identity(owner),
            },
            MAX_ACCOUNT_CUSTODY_BYTES,
        )
        .await?;
        Ok(Self {
            store,
            scope,
            owner,
        })
    }
    /// Initialize only a never-written namespace with an explicit retained nonce floor.
    pub async fn initialize(&self, first_nonce: u64) -> Result<u64, BrowserAccountError> {
        let book = AccountOperationBook::new(self.scope, self.owner, first_nonce)?;
        Ok(self
            .store
            .compare_exchange(None, Some(&book.export_state()?))
            .await?)
    }
    /// Initialize from authenticated backup custody. Existing data or tombstones
    /// conflict; this is not permission to overwrite newer operations with a backup.
    pub async fn initialize_from_backup(
        &self,
        backup: &quai_wallet::full_backup::WalletBackup,
    ) -> Result<u64, BrowserAccountError> {
        let book = AccountOperationBook::from_backup(backup, self.scope, self.owner)?;
        Ok(self
            .store
            .compare_exchange(None, Some(&book.export_state()?))
            .await?)
    }
    /// Union authenticated custody with live state under one CAS. A concurrent
    /// writer conflicts; no newer nonce/candidate is overwritten by a stale backup.
    /// Successful merge invalidates live inclusion observations for reconciliation.
    pub async fn merge_backup(
        &self,
        backup: &quai_wallet::full_backup::WalletBackup,
    ) -> Result<AccountMergeReport, BrowserAccountError> {
        let mut snapshot = self.snapshot().await?;
        let report = snapshot.book.merge_backup(backup)?;
        self.write(&snapshot).await?;
        Ok(report)
    }
    /// Revalidate all persisted signatures, claims and candidate edges on every read.
    pub async fn snapshot(&self) -> Result<BrowserAccountSnapshot, BrowserAccountError> {
        let snapshot = self
            .store
            .read()
            .await?
            .ok_or(BrowserAccountError::Uninitialized)?;
        let bytes = snapshot.bytes.ok_or(BrowserAccountError::Uninitialized)?;
        Ok(BrowserAccountSnapshot {
            revision: snapshot.revision,
            book: AccountOperationBook::from_state(&bytes, self.scope, self.owner)?,
        })
    }
    async fn write(&self, snapshot: &BrowserAccountSnapshot) -> Result<u64, BrowserAccountError> {
        Ok(self
            .store
            .compare_exchange(
                Some(snapshot.revision),
                Some(&snapshot.book.export_state()?),
            )
            .await?)
    }
    /// Commit the nonce and ID before returning. A cancelled write may have committed;
    /// inspect this same ID before resuming. Revision conflicts are never retried.
    pub async fn reserve_nonce(
        &self,
        id: ReservationId,
        remote_pending: u64,
    ) -> Result<u64, BrowserAccountError> {
        let mut snapshot = self.snapshot().await?;
        let nonce = snapshot.book.reserve_nonce(id, remote_pending)?;
        self.write(&snapshot).await?;
        Ok(nonce)
    }
    /// Persist a verified signed root before allowing the caller to expose it further.
    /// An already signed value may have escaped before this method was called; the
    /// caller must hold the original reservation until this write is resolved.
    pub async fn commit_signed(
        &self,
        id: ReservationId,
        signed: &SignedQuaiTransaction,
    ) -> Result<(), BrowserAccountError> {
        let mut snapshot = self.snapshot().await?;
        snapshot.book.commit_signed(id, signed)?;
        self.write(&snapshot).await?;
        Ok(())
    }
    /// Sign the exact approved transaction locally and return its bytes only after
    /// durable custody succeeds. Chain, owner and retained nonce are checked before
    /// invoking the signer; a lost write response is recovered under this same ID.
    pub async fn sign(
        &self,
        id: ReservationId,
        transaction: &QuaiTransaction,
        signer: &impl Signer,
    ) -> Result<SignedQuaiTransaction, BrowserAccountError> {
        let mut snapshot = self.snapshot().await?;
        let operation = snapshot
            .book
            .operation(id)
            .ok_or(StorageError::Transition)?;
        if operation.state != ReservationState::Reserved
            || transaction.chain_id != self.scope.chain_id
            || transaction.nonce != operation.nonce
            || signer.address() != snapshot.book.address().address()
            || signer.chain_id() != self.scope.chain_id
        {
            return Err(StorageError::Conflict.into());
        }
        let signed = signer.sign_quai(transaction)?;
        if signed.transaction() != transaction {
            return Err(SignerError::InvalidTransaction.into());
        }
        snapshot.book.commit_signed(id, &signed)?;
        self.write(&snapshot).await?;
        Ok(signed)
    }
    /// Persist an explicitly approved signed fee-only replacement before exposure.
    pub async fn commit_replacement(
        &self,
        id: ReservationId,
        parent: Hash32,
        signed: &SignedQuaiTransaction,
    ) -> Result<(), BrowserAccountError> {
        let mut snapshot = self.snapshot().await?;
        snapshot.book.commit_replacement(id, parent, signed)?;
        self.write(&snapshot).await?;
        Ok(())
    }
    /// Record an attempted send before dispatch; this makes no network request.
    pub async fn mark_submitted(&self, id: ReservationId) -> Result<(), BrowserAccountError> {
        let mut snapshot = self.snapshot().await?;
        snapshot.book.mark_submitted(id)?;
        self.write(&snapshot).await?;
        Ok(())
    }
    /// Release a never-signed nonce gap, retaining the cursor and original ID.
    pub async fn release_unsigned(&self, id: ReservationId) -> Result<(), BrowserAccountError> {
        let mut snapshot = self.snapshot().await?;
        snapshot.book.release_unsigned(id)?;
        self.write(&snapshot).await?;
        Ok(())
    }
    /// Reopen the exact unsigned gap for an explicitly approved repair transaction.
    pub async fn reopen_unsigned(&self, id: ReservationId) -> Result<(), BrowserAccountError> {
        let mut snapshot = self.snapshot().await?;
        snapshot.book.reopen_unsigned(id)?;
        self.write(&snapshot).await?;
        Ok(())
    }
    /// Persist canonical-read results only if the revision captured before those
    /// reads is still current. A concurrent invalidation, append or write conflicts.
    pub async fn observe_inclusion(
        &self,
        expected_revision: u64,
        id: ReservationId,
        hash: Hash32,
        checkpoint: Checkpoint,
    ) -> Result<(), BrowserAccountError> {
        let mut snapshot = self.snapshot().await?;
        if snapshot.revision != expected_revision {
            return Err(StorageError::StaleSnapshot.into());
        }
        snapshot.book.observe_inclusion(id, hash, checkpoint)?;
        self.write(&snapshot).await?;
        Ok(())
    }
    /// Invalidate one exact observation, fenced against the caller's prior revision.
    /// All signed bytes and nonces remain held for explicit recovery/rebroadcast.
    pub async fn invalidate_inclusion(
        &self,
        expected_revision: u64,
        id: ReservationId,
        expected: (Hash32, Checkpoint),
    ) -> Result<(), BrowserAccountError> {
        let mut snapshot = self.snapshot().await?;
        if snapshot.revision != expected_revision {
            return Err(StorageError::StaleSnapshot.into());
        }
        snapshot.book.invalidate_inclusion(id, expected)?;
        self.write(&snapshot).await?;
        Ok(())
    }
}
