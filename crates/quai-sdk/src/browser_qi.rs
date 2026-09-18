//! Durable browser Qi claims with explicit discovery, signing and persistence boundaries.
use crate::discovery::CurrentQiDiscovery;
use quai_browser::{BrowserError, BrowserSnapshotStore, BrowserStorageScope};
use quai_consensus::{
    OutPoint, QiConversionTransaction, QiTransaction, QiWrappingTransaction, SignedQiOperation,
    TransactionError, U256,
};
use quai_primitives::{Hash32, QiAddress};
use quai_wallet::discovery::{CanonicalStatus, Checkpoint, NetworkScope};
use quai_wallet::metadata::{PublicAddress, StorageError};
use quai_wallet::qi_custody::{
    MAX_QI_CUSTODY_BYTES, QiMergeReport, QiOperationBook, ReservationId, ReservationState,
};
use quai_wallet::qi_keys::QiKeyResolver;
use quai_wallet::{AccountPublic, CandidateCoin, CoinType};
use std::collections::{BTreeMap, BTreeSet};
/// Errors and cancellation never authorize dropping signed claims or retrying a send.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BrowserQiError {
    /// Failed or conflicting storage write; cancelled writes may have committed.
    #[error(transparent)]
    Browser(#[from] BrowserError),
    /// Invalid/stale source, metadata, bounds, conflicting claims or unsafe transition.
    #[error(transparent)]
    State(#[from] StorageError),
    /// Canonical transaction construction, signature or protocol-shape failure.
    #[error(transparent)]
    Transaction(#[from] TransactionError),
    /// Initialize a never-written namespace explicitly; tombstones require recovery.
    #[error("Qi custody journal is not initialized")]
    Uninitialized,
}
/// Detached complete custody state and its exact IndexedDB revision.
#[derive(Clone, Debug)]
pub struct BrowserQiSnapshot {
    /// Capture before discovery/canonical reads and pass to dependent writes.
    pub revision: u64,
    /// Public origins, claims and candidates. Detached mutations do not persist.
    pub book: QiOperationBook,
}
/// Scoped input custody across tabs and workers. Keys remain caller owned.
/// No network, fee quote, address allocation or implicit transaction send occurs.
#[derive(Clone)]
pub struct BrowserQiBook {
    pub(crate) store: BrowserSnapshotStore,
    scope: NetworkScope,
    identity: Hash32,
}
impl BrowserQiBook {
    /// Open the same database name and stable wallet identity in every writer.
    /// Different databases/namespaces cannot prevent one another from spending.
    pub async fn open(
        name: &str,
        scope: NetworkScope,
        identity: Hash32,
    ) -> Result<Self, BrowserQiError> {
        QiOperationBook::new(scope, identity)?;
        let store = BrowserSnapshotStore::open(
            name,
            BrowserStorageScope {
                chain_id: scope.chain_id,
                genesis: scope.genesis,
                zone: scope.zone,
                wallet: QiOperationBook::storage_identity(identity),
            },
            MAX_QI_CUSTODY_BYTES,
        )
        .await?;
        Ok(Self {
            store,
            scope,
            identity,
        })
    }
    /// Initialize only genuinely fresh custody. Empty current UTXOs do not prove
    /// that previously signed transactions or imported claims are absent.
    pub async fn initialize(&self) -> Result<u64, BrowserQiError> {
        let book = QiOperationBook::new(self.scope, self.identity)?;
        Ok(self
            .store
            .compare_exchange(None, Some(&book.export_state()?))
            .await?)
    }
    /// Initialize a never-written namespace from authenticated Qi custody. Does
    /// not restore current UTXO observations, history or address allocation journals.
    pub async fn initialize_from_backup(
        &self,
        backup: &quai_wallet::full_backup::WalletBackup,
    ) -> Result<u64, BrowserQiError> {
        let book = QiOperationBook::from_backup(backup, self.scope, self.identity)?;
        Ok(self
            .store
            .compare_exchange(None, Some(&book.export_state()?))
            .await?)
    }
    /// Atomically union authenticated Qi custody into an existing journal. Keeps
    /// all live claims and candidates, rejects conflicts, and invalidates source
    /// and inclusion observations. Concurrent writes reject without automatic retry.
    pub async fn merge_backup(
        &self,
        backup: &quai_wallet::full_backup::WalletBackup,
    ) -> Result<QiMergeReport, BrowserQiError> {
        let mut snapshot = self.snapshot().await?;
        let report = snapshot.book.merge_backup(backup)?;
        self.write(&snapshot).await?;
        Ok(report)
    }
    /// Revalidate every public origin, input claim, signed payload and candidate.
    pub async fn snapshot(&self) -> Result<BrowserQiSnapshot, BrowserQiError> {
        let s = self
            .store
            .read()
            .await?
            .ok_or(BrowserQiError::Uninitialized)?;
        let bytes = s.bytes.ok_or(BrowserQiError::Uninitialized)?;
        Ok(BrowserQiSnapshot {
            revision: s.revision,
            book: QiOperationBook::from_state(&bytes, self.scope, self.identity)?,
        })
    }
    async fn write(&self, snapshot: &BrowserQiSnapshot) -> Result<u64, BrowserQiError> {
        Ok(self
            .store
            .compare_exchange(
                Some(snapshot.revision),
                Some(&snapshot.book.export_state()?),
            )
            .await?)
    }
    async fn at_revision(&self, expected: u64) -> Result<BrowserQiSnapshot, BrowserQiError> {
        let s = self.snapshot().await?;
        if s.revision != expected {
            return Err(StorageError::StaleSnapshot.into());
        }
        Ok(s)
    }
    /// Claim exact observed outputs under the journal revision captured before
    /// source reads. Supplied observations must belong to this configured network.
    /// Lock/expiry checks do not turn latest-only reads into an atomic chain snapshot.
    pub async fn reserve(
        &self,
        expected_revision: u64,
        id: ReservationId,
        checkpoint: Checkpoint,
        candidate_height: U256,
        coins: &[CandidateCoin],
        owners: &[PublicAddress],
    ) -> Result<(), BrowserQiError> {
        let mut s = self.at_revision(expected_revision).await?;
        s.book
            .reserve(id, checkpoint, candidate_height, coins, owners)?;
        self.write(&s).await?;
        Ok(())
    }
    /// Reserve selected points from the portable gap-50/deep discovery result.
    /// Re-derives exact selected HD origins from the caller's trusted account xpub.
    /// `Matches` is a source recheck, not historical recovery or atomic unspentness.
    pub async fn reserve_discovery(
        &self,
        expected_revision: u64,
        id: ReservationId,
        report: &CurrentQiDiscovery,
        account: &AccountPublic,
        candidate_height: U256,
        selected: &[OutPoint],
    ) -> Result<(), BrowserQiError> {
        if report.scope != self.scope
            || report.canonical != CanonicalStatus::Matches
            || account.coin_type() != CoinType::Qi
            || report.addresses.len() > 100_000
            || selected.is_empty()
            || selected.len() > 4096
        {
            return Err(StorageError::Invalid.into());
        }
        let total = report
            .addresses
            .iter()
            .try_fold(0usize, |n, a| n.checked_add(a.outputs.len()))
            .ok_or(StorageError::Invalid)?;
        if total > 100_000 {
            return Err(StorageError::Invalid.into());
        }
        let mut s = self.at_revision(expected_revision).await?;
        let wanted: BTreeSet<_> = selected.iter().copied().collect();
        if wanted.len() != selected.len() {
            return Err(StorageError::Invalid.into());
        }
        let mut seen = BTreeSet::new();
        let mut owners = BTreeMap::new();
        let mut coins = Vec::new();
        for address in &report.addresses {
            let outputs: Vec<_> = address
                .outputs
                .iter()
                .filter(|o| wanted.contains(&o.outpoint))
                .collect();
            if outputs.is_empty() {
                continue;
            }
            let derived = &address.derived;
            let public = PublicAddress::derive(account, derived.change, derived.index)?;
            if derived.coin != CoinType::Qi
                || derived.account != account.account_index()
                || derived.zone != self.scope.zone
                || public.address() != derived.address
                || *public.public_key() != derived.public_key
            {
                return Err(StorageError::Invalid.into());
            }
            let owner = QiAddress::try_from(public.address()).map_err(|_| StorageError::Invalid)?;
            if owners.get(&owner).is_some_and(|old| old != &public) {
                return Err(StorageError::Conflict.into());
            }
            owners.insert(owner, public);
            for output in outputs {
                if !seen.insert(output.outpoint) {
                    return Err(StorageError::Invalid.into());
                }
                coins.push(CandidateCoin {
                    outpoint: output.outpoint,
                    address: owner,
                    denomination: output.denomination,
                    unlock_height: output.unlock_height,
                    expires_at: None,
                    reserved: false,
                });
            }
        }
        if seen != wanted {
            return Err(StorageError::Invalid.into());
        }
        s.book.reserve(
            id,
            report.checkpoint,
            candidate_height,
            &coins,
            &owners.into_values().collect::<Vec<_>>(),
        )?;
        self.write(&s).await?;
        Ok(())
    }
    /// Resolve exact HD/imported/BIP47 input keys locally and return a signature
    /// only after canonical bytes are durably stored. The complete approved raw
    /// transaction, including conversion/wrapping data, is preserved without retries.
    pub async fn sign(
        &self,
        id: ReservationId,
        transaction: &QiTransaction,
        resolver: &(impl QiKeyResolver + ?Sized),
    ) -> Result<SignedQiOperation, BrowserQiError> {
        let mut s = self.snapshot().await?;
        let op = s.book.operation(id).ok_or(StorageError::Transition)?;
        if op.state != ReservationState::Reserved {
            return Err(StorageError::Transition.into());
        }
        s.book.check_inputs(id, transaction)?;
        let signed = {
            let mut keys = Vec::with_capacity(transaction.inputs.len());
            for input in &transaction.inputs {
                let owner = QiAddress::try_from(input.public_key.address())
                    .map_err(|_| StorageError::Invalid)?;
                let public = s.book.address(owner).ok_or(StorageError::Invalid)?;
                let key = resolver.resolve(public)?;
                if key.public_key() != input.public_key {
                    return Err(StorageError::Conflict.into());
                }
                keys.push(key);
            }
            let keys: Vec<_> = keys.iter().collect();
            match transaction.data.len() {
                0 => SignedQiOperation::Transfer(transaction.sign_local(&keys)?),
                20 => SignedQiOperation::Wrapping(
                    QiWrappingTransaction::from_transaction(transaction.clone())?
                        .sign_local(&keys)?,
                ),
                22 => SignedQiOperation::Conversion(
                    QiConversionTransaction::from_transaction(transaction.clone())?
                        .sign_local(&keys)?,
                ),
                _ => return Err(StorageError::Invalid.into()),
            }
        };
        if signed.transaction() != transaction {
            return Err(StorageError::Conflict.into());
        }
        s.book.commit_signed(id, &signed)?;
        self.write(&s).await?;
        Ok(signed)
    }
    /// Persist independently signed bytes before any further exposure. The caller
    /// remains responsible for bytes which may have escaped before this call.
    pub async fn commit_signed(
        &self,
        id: ReservationId,
        signed: &SignedQiOperation,
    ) -> Result<(), BrowserQiError> {
        let mut s = self.snapshot().await?;
        s.book.commit_signed(id, signed)?;
        self.write(&s).await?;
        Ok(())
    }
    /// Retain an explicitly approved same-input/data replacement before exposure.
    pub async fn commit_replacement(
        &self,
        id: ReservationId,
        parent: Hash32,
        signed: &SignedQiOperation,
    ) -> Result<(), BrowserQiError> {
        let mut s = self.snapshot().await?;
        s.book.commit_replacement(id, parent, signed)?;
        self.write(&s).await?;
        Ok(())
    }
    /// Record a broadcast attempt before explicit typed provider submission.
    pub async fn mark_submitted(&self, id: ReservationId) -> Result<(), BrowserQiError> {
        let mut s = self.snapshot().await?;
        s.book.mark_submitted(id)?;
        self.write(&s).await?;
        Ok(())
    }
    /// Cancel only a never-signed operation, retaining its ID and public origins.
    pub async fn release_unsigned(&self, id: ReservationId) -> Result<(), BrowserQiError> {
        let mut s = self.snapshot().await?;
        s.book.release_unsigned(id)?;
        self.write(&s).await?;
        Ok(())
    }
    /// Persist independent canonical observations under the revision captured before
    /// reading receipts/headers. No stale result can undo a concurrent invalidation.
    pub async fn observe_inclusion(
        &self,
        expected_revision: u64,
        id: ReservationId,
        hash: Hash32,
        block: Checkpoint,
    ) -> Result<(), BrowserQiError> {
        let mut s = self.at_revision(expected_revision).await?;
        s.book.observe_inclusion(id, hash, block)?;
        self.write(&s).await?;
        Ok(())
    }
    /// Remove one exact lost-canonicality observation without releasing signed inputs.
    pub async fn invalidate_inclusion(
        &self,
        expected_revision: u64,
        id: ReservationId,
        expected: (Hash32, Checkpoint),
    ) -> Result<(), BrowserQiError> {
        let mut s = self.at_revision(expected_revision).await?;
        s.book.invalidate_inclusion(id, expected)?;
        self.write(&s).await?;
        Ok(())
    }
}
