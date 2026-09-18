//! Durable browser payment-code destination allocation. No private key is retained
//! by the adapter; callers supply their guarded owner for verification and search.
use quai_browser::{BrowserError, BrowserSnapshotStore, BrowserStorageScope};
use quai_payments::{
    PaymentCode, PaymentDirection, PaymentError, PaymentSearch, PrivatePaymentCode,
};
use quai_wallet::discovery::NetworkScope;
use quai_wallet::metadata::StorageError;
use quai_wallet::payment_allocation::{
    MAX_PAYMENT_ALLOCATION_BYTES, PaymentAddressRecord, PaymentAllocation, PaymentAllocationBook,
    PaymentAllocationId, PaymentAllocationStatus,
};

/// Errors never release a committed range. Inspect the retained caller ID after
/// a cancelled write before resuming; revision conflicts are not automatically retried.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BrowserPaymentError {
    /// IndexedDB I/O, revision conflict or ambiguous cancellation.
    #[error(transparent)]
    Browser(#[from] BrowserError),
    /// Invalid context/state, repeated ID, exhausted bounds or unsafe transition.
    #[error(transparent)]
    State(#[from] StorageError),
    /// Bounded search failed/cancelled. Its whole reserved range stays consumed.
    #[error(transparent)]
    Search(#[from] PaymentError),
    /// Never initialized or tombstoned. No implicit cursor reset is permitted.
    #[error("payment allocation journal is not initialized")]
    Uninitialized,
}
/// Detached validated inspection of a persisted public journal.
#[derive(Clone, Debug)]
pub struct BrowserPaymentSnapshot {
    /// Revision of this observation, not a finality or freshness assertion.
    pub revision: u64,
    /// Public allocation history; mutating this copy does not update storage.
    pub book: PaymentAllocationBook,
}
/// Atomic range and destination commits across tabs/workers, scoped to one owner
/// account, peer, direction and network/zone. No transaction or private-key custody.
#[derive(Clone)]
pub struct BrowserPaymentBook {
    pub(crate) store: BrowserSnapshotStore,
    scope: NetworkScope,
    local: PaymentCode,
    account: u32,
    peer: PaymentCode,
    direction: PaymentDirection,
}
impl BrowserPaymentBook {
    /// Open an exact namespace without initializing, searching or contacting nodes.
    /// The private owner is borrowed only to establish its public identity.
    pub async fn open(
        name: &str,
        scope: NetworkScope,
        owner: &PrivatePaymentCode,
        peer: PaymentCode,
        direction: PaymentDirection,
    ) -> Result<Self, BrowserPaymentError> {
        let store = BrowserSnapshotStore::open(
            name,
            BrowserStorageScope {
                chain_id: scope.chain_id,
                genesis: scope.genesis,
                zone: scope.zone,
                wallet: PaymentAllocationBook::channel_identity(
                    owner.public_code(),
                    owner.account(),
                    &peer,
                    direction,
                ),
            },
            MAX_PAYMENT_ALLOCATION_BYTES,
        )
        .await?;
        Ok(Self {
            store,
            scope,
            local: owner.public_code().clone(),
            account: owner.account(),
            peer,
            direction,
        })
    }
    fn check_owner(&self, owner: &PrivatePaymentCode) -> Result<(), BrowserPaymentError> {
        if self.local != *owner.public_code() || self.account != owner.account() {
            return Err(StorageError::Invalid.into());
        }
        Ok(())
    }
    /// Initialize only a never-written namespace using an explicit prior cursor.
    /// Include every exposed/burned index. Zero is valid only for an unused stream.
    /// Existing records and tombstones conflict, even when otherwise empty.
    pub async fn initialize(
        &self,
        owner: &PrivatePaymentCode,
        first_index: u32,
    ) -> Result<u64, BrowserPaymentError> {
        self.check_owner(owner)?;
        let book = PaymentAllocationBook::new(
            self.scope,
            owner,
            self.peer.clone(),
            self.direction,
            first_index,
        )?;
        Ok(self
            .store
            .compare_exchange(None, Some(&book.export_state()))
            .await?)
    }
    /// Initialize a never-written namespace from an authenticated backup's exact
    /// owner/peer/network cursor. This does not restore prior exposures or claims.
    #[cfg(feature = "backup")]
    pub async fn initialize_from_backup(
        &self,
        owner: &PrivatePaymentCode,
        backup: &quai_wallet::full_backup::WalletBackup,
    ) -> Result<u64, BrowserPaymentError> {
        self.check_owner(owner)?;
        let book = PaymentAllocationBook::from_backup(
            backup,
            self.scope,
            owner,
            self.peer.clone(),
            self.direction,
        )?;
        Ok(self
            .store
            .compare_exchange(None, Some(&book.export_state()))
            .await?)
    }
    /// Atomically merge authenticated channel floors, retaining IDs/exposures and
    /// abandoning pending searches. A CAS conflict rejects without automatic retry.
    #[cfg(feature = "backup")]
    pub async fn merge_backup(
        &self,
        owner: &PrivatePaymentCode,
        backup: &quai_wallet::full_backup::WalletBackup,
    ) -> Result<usize, BrowserPaymentError> {
        let mut snapshot = self.snapshot(owner).await?;
        let abandoned = snapshot.book.merge_backup(owner, backup)?;
        self.store
            .compare_exchange(Some(snapshot.revision), Some(&snapshot.book.export_state()))
            .await?;
        Ok(abandoned)
    }
    /// Validate every completed derivation against the explicit private owner.
    /// Run large journal reads/searches in an application worker. No keys are saved.
    pub async fn snapshot(
        &self,
        owner: &PrivatePaymentCode,
    ) -> Result<BrowserPaymentSnapshot, BrowserPaymentError> {
        self.check_owner(owner)?;
        let snapshot = self
            .store
            .read()
            .await?
            .ok_or(BrowserPaymentError::Uninitialized)?;
        let bytes = snapshot.bytes.ok_or(BrowserPaymentError::Uninitialized)?;
        Ok(BrowserPaymentSnapshot {
            revision: snapshot.revision,
            book: PaymentAllocationBook::from_state(
                &bytes,
                self.scope,
                owner,
                self.peer.clone(),
                self.direction,
            )?,
        })
    }
    /// Commit a consumed range before any search. Retain the ID even if this future
    /// is cancelled: the IndexedDB transaction may already have committed.
    pub async fn reserve(
        &self,
        owner: &PrivatePaymentCode,
        id: PaymentAllocationId,
        max_attempts: u32,
    ) -> Result<PaymentAllocation, BrowserPaymentError> {
        let mut snapshot = self.snapshot(owner).await?;
        snapshot.book.reserve(id, max_attempts)?;
        self.store
            .compare_exchange(Some(snapshot.revision), Some(&snapshot.book.export_state()))
            .await?;
        Ok(snapshot
            .book
            .allocation(id)
            .ok_or(StorageError::Invalid)?
            .clone())
    }
    /// Verify and commit an exact destination before returning it. The same ID/index
    /// is idempotent; a different index or abandoned request fails.
    pub async fn complete(
        &self,
        owner: &PrivatePaymentCode,
        id: PaymentAllocationId,
        index: u32,
    ) -> Result<PaymentAddressRecord, BrowserPaymentError> {
        let mut snapshot = self.snapshot(owner).await?;
        let completed = snapshot
            .book
            .allocation(id)
            .is_some_and(|a| matches!(a.status, PaymentAllocationStatus::Completed(_)));
        let record = snapshot.book.complete(owner, id, index)?;
        if !completed {
            self.store
                .compare_exchange(Some(snapshot.revision), Some(&snapshot.book.export_state()))
                .await?;
        }
        Ok(record)
    }
    /// Reserve, search and commit. Cancellation or exhausted search leaves the
    /// entire interval pending and consumed for explicit resume or abandonment.
    pub async fn allocate(
        &self,
        owner: &PrivatePaymentCode,
        id: PaymentAllocationId,
        max_attempts: u32,
        cancelled: impl FnMut() -> bool,
    ) -> Result<PaymentAddressRecord, BrowserPaymentError> {
        self.reserve(owner, id, max_attempts).await?;
        self.resume(owner, id, cancelled).await
    }
    /// Resume this request or return its exact committed destination. A new request
    /// must use a new ID. Send records do not confer locally spendable ownership.
    pub async fn resume(
        &self,
        owner: &PrivatePaymentCode,
        id: PaymentAllocationId,
        cancelled: impl FnMut() -> bool,
    ) -> Result<PaymentAddressRecord, BrowserPaymentError> {
        let snapshot = self.snapshot(owner).await?;
        let allocation = snapshot.book.allocation(id).ok_or(StorageError::Invalid)?;
        match &allocation.status {
            PaymentAllocationStatus::Completed(record) => Ok(record.clone()),
            PaymentAllocationStatus::Abandoned => Err(StorageError::Transition.into()),
            PaymentAllocationStatus::Pending => {
                let found = owner.search(
                    &self.peer,
                    self.direction,
                    PaymentSearch {
                        zone: self.scope.zone,
                        start_index: allocation.range.start,
                        max_attempts: allocation.range.end - allocation.range.start,
                    },
                    cancelled,
                )?;
                self.complete(owner, id, found.index).await
            }
        }
    }
    /// Abandon a pending request without releasing its range or ID.
    pub async fn abandon(
        &self,
        owner: &PrivatePaymentCode,
        id: PaymentAllocationId,
    ) -> Result<(), BrowserPaymentError> {
        let mut snapshot = self.snapshot(owner).await?;
        snapshot.book.abandon(id)?;
        self.store
            .compare_exchange(Some(snapshot.revision), Some(&snapshot.book.export_state()))
            .await?;
        Ok(())
    }
}
