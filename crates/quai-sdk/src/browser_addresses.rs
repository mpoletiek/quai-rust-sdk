//! Durable browser HD allocation with range-before-search and address-before-return commits.
use quai_browser::{BrowserError, BrowserSnapshotStore, BrowserStorageScope};
use quai_wallet::allocation::{
    AddressAllocation, AddressAllocationBook, AddressAllocationId, AddressAllocationStatus,
    MAX_ADDRESS_ALLOCATION_BYTES,
};
use quai_wallet::discovery::NetworkScope;
use quai_wallet::metadata::{PublicAddress, StorageError};
use quai_wallet::{AccountPublic, Search, WalletError};

/// Allocation errors do not undo committed ranges. After cancellation or a failed
/// write, inspect the caller-supplied allocation ID before deciding how to resume.
#[derive(Debug, thiserror::Error)]
pub enum BrowserAddressError {
    /// Browser storage failure or revision conflict, including ambiguous cancellation.
    #[error(transparent)]
    Browser(#[from] BrowserError),
    /// Invalid scope/state, reused ID, exhausted bounds or forbidden transition.
    #[error(transparent)]
    State(#[from] StorageError),
    /// Search cancelled/exhausted; its entire reserved range remains consumed.
    #[error(transparent)]
    Search(#[from] WalletError),
    /// Initialize with explicit prior cursor floors before making allocations.
    /// A tombstone also requires explicit recovery, never silent reinitialization.
    #[error("address allocation journal is not initialized")]
    Uninitialized,
}
/// A validated public journal and its IndexedDB revision, for read-only inspection.
#[derive(Clone, Debug)]
pub struct BrowserAddressSnapshot {
    /// Exact revision to identify this observation; no finality/freshness assertion.
    pub revision: u64,
    /// Immutable inspection of cursors and retained allocations.
    pub book: AddressAllocationBook,
}
/// Scoped compare-and-exchange allocation across browser tabs and workers.
/// Public account metadata only; run expensive derivation in an application worker.
/// No nonce, UTXO, secret backup, payment-channel or transaction state is held here.
#[derive(Clone)]
pub struct BrowserAddressBook {
    store: BrowserSnapshotStore,
    scope: NetworkScope,
    account: AccountPublic,
}
impl BrowserAddressBook {
    /// Open the namespace for an independently trusted account and network.
    /// Opening does not initialize cursors, search, prompt or make network requests.
    pub async fn open(
        name: &str,
        scope: NetworkScope,
        account: AccountPublic,
    ) -> Result<Self, BrowserAddressError> {
        let store = BrowserSnapshotStore::open(
            name,
            BrowserStorageScope {
                chain_id: scope.chain_id,
                genesis: scope.genesis,
                zone: scope.zone,
                wallet: AddressAllocationBook::account_identity(&account),
            },
            MAX_ADDRESS_ALLOCATION_BYTES,
        )
        .await?;
        Ok(Self {
            store,
            scope,
            account,
        })
    }
    /// Initialize only a never-written namespace with explicit prior raw floors.
    /// Existing records and tombstones conflict. Empty current UTXOs do not justify
    /// zero floors; use an authenticated backup or retained allocation history.
    pub async fn initialize(
        &self,
        first_receive: u32,
        first_change: u32,
    ) -> Result<u64, BrowserAddressError> {
        let book = AddressAllocationBook::new(
            self.scope,
            self.account.clone(),
            first_receive,
            first_change,
        )?;
        Ok(self
            .store
            .compare_exchange(None, Some(&book.export_state()))
            .await?)
    }
    /// Initialize a never-written namespace from authenticated native/portable backup
    /// floors, checking this exact account. This does not restore transaction claims.
    #[cfg(feature = "backup")]
    pub async fn initialize_from_backup(
        &self,
        backup: &quai_wallet::full_backup::WalletBackup,
    ) -> Result<u64, BrowserAddressError> {
        let book = AddressAllocationBook::from_backup(backup, self.scope, self.account.clone())?;
        Ok(self
            .store
            .compare_exchange(None, Some(&book.export_state()))
            .await?)
    }
    /// Atomically merge authenticated floors, retaining IDs/completions and
    /// abandoning pending searches. A CAS conflict rejects without automatic retry.
    #[cfg(feature = "backup")]
    pub async fn merge_backup(
        &self,
        backup: &quai_wallet::full_backup::WalletBackup,
    ) -> Result<usize, BrowserAddressError> {
        let mut snapshot = self.snapshot().await?;
        let abandoned = snapshot.book.merge_backup(backup)?;
        self.store
            .compare_exchange(Some(snapshot.revision), Some(&snapshot.book.export_state()))
            .await?;
        Ok(abandoned)
    }
    /// Read and validate the complete bounded journal against the configured scope
    /// and account. A tombstone never silently creates an empty wallet branch.
    pub async fn snapshot(&self) -> Result<BrowserAddressSnapshot, BrowserAddressError> {
        let snapshot = self
            .store
            .read()
            .await?
            .ok_or(BrowserAddressError::Uninitialized)?;
        let bytes = snapshot.bytes.ok_or(BrowserAddressError::Uninitialized)?;
        Ok(BrowserAddressSnapshot {
            revision: snapshot.revision,
            book: AddressAllocationBook::from_state(&bytes, self.scope, self.account.clone())?,
        })
    }
    /// Durably consume a raw range before any address is selected. IDs never become
    /// reusable; callers retain their ID even if awaiting this write is cancelled.
    pub async fn reserve(
        &self,
        id: AddressAllocationId,
        change: bool,
        max_attempts: u32,
    ) -> Result<AddressAllocation, BrowserAddressError> {
        let mut snapshot = self.snapshot().await?;
        snapshot.book.reserve(id, change, max_attempts)?;
        self.store
            .compare_exchange(Some(snapshot.revision), Some(&snapshot.book.export_state()))
            .await?;
        Ok(snapshot
            .book
            .allocation(id)
            .ok_or(StorageError::Invalid)?
            .clone())
    }
    /// Persist an exact completed address before returning it. Repeating the same
    /// allocation/index returns its existing address; a different index conflicts.
    pub async fn complete(
        &self,
        id: AddressAllocationId,
        index: u32,
    ) -> Result<PublicAddress, BrowserAddressError> {
        let mut snapshot = self.snapshot().await?;
        let completed = snapshot
            .book
            .allocation(id)
            .is_some_and(|a| matches!(a.status, AddressAllocationStatus::Completed(_)));
        let address = snapshot.book.complete(id, index)?;
        if !completed {
            self.store
                .compare_exchange(Some(snapshot.revision), Some(&snapshot.book.export_state()))
                .await?;
        }
        Ok(address)
    }
    /// Reserve, search and commit one new allocation. Cancellation/exhaustion after
    /// reservation keeps the range pending and consumed for explicit resume/abandon.
    pub async fn allocate(
        &self,
        id: AddressAllocationId,
        change: bool,
        max_attempts: u32,
        cancelled: impl FnMut() -> bool,
    ) -> Result<PublicAddress, BrowserAddressError> {
        self.reserve(id, change, max_attempts).await?;
        self.resume(id, cancelled).await
    }
    /// Resume this exact pending request, or return its already committed address.
    /// A new request must use a new ID. Revision conflicts are surfaced, not retried.
    pub async fn resume(
        &self,
        id: AddressAllocationId,
        cancelled: impl FnMut() -> bool,
    ) -> Result<PublicAddress, BrowserAddressError> {
        let snapshot = self.snapshot().await?;
        let allocation = snapshot.book.allocation(id).ok_or(StorageError::Invalid)?;
        match &allocation.status {
            AddressAllocationStatus::Completed(address) => Ok(address.clone()),
            AddressAllocationStatus::Abandoned => Err(StorageError::Transition.into()),
            AddressAllocationStatus::Pending => {
                let found = self.account.search(
                    allocation.change,
                    Search {
                        zone: self.scope.zone,
                        start_index: allocation.range.start,
                        max_attempts: allocation.range.end - allocation.range.start,
                    },
                    cancelled,
                )?;
                self.complete(id, found.address.index).await
            }
        }
    }
    /// Durably abandon a pending request while retaining its ID and burned range.
    /// A completed address cannot be released or made fresh again.
    pub async fn abandon(&self, id: AddressAllocationId) -> Result<(), BrowserAddressError> {
        let mut snapshot = self.snapshot().await?;
        snapshot.book.abandon(id)?;
        self.store
            .compare_exchange(Some(snapshot.revision), Some(&snapshot.book.export_state()))
            .await?;
        Ok(())
    }
}
