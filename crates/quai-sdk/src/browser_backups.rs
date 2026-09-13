//! Consistent collection and atomic recovery merge of selected browser journals.
use crate::browser_accounts::{BrowserAccountBook, BrowserAccountError};
use crate::browser_addresses::{BrowserAddressBook, BrowserAddressError};
use crate::browser_payments::{BrowserPaymentBook, BrowserPaymentError};
use crate::browser_qi::{BrowserQiBook, BrowserQiError};
use quai_payments::PrivatePaymentCode;
use quai_primitives::Hash32;
use quai_wallet::account_custody::AccountOperationBook;
use quai_wallet::discovery::NetworkScope;
use quai_wallet::full_backup::{
    AccountCustodyCapture, BackupOrigin, MAX_PORTABLE_CAPTURE_JOURNALS, PortableWalletCapture,
    WalletBackup, WalletBackupError,
};
use quai_wallet::metadata::{PublicAddress, StorageError};

/// Errors never trigger an automatic retry. Failed restore transactions commit no updates.
#[derive(Debug, thiserror::Error)]
pub enum BrowserBackupError {
    /// Atomic storage transaction failed or conflicted.
    #[error(transparent)]
    Browser(#[from] quai_browser::BrowserError),
    /// HD journal read/validation failed.
    #[error(transparent)]
    Address(#[from] BrowserAddressError),
    /// Account custody read/validation failed.
    #[error(transparent)]
    Account(#[from] BrowserAccountError),
    /// Qi custody read/validation failed.
    #[error(transparent)]
    Qi(#[from] BrowserQiError),
    /// Payment journal read/ownership validation failed.
    #[error(transparent)]
    Payment(#[from] BrowserPaymentError),
    /// Ownership-proved recovery capture rejected its inputs.
    #[error(transparent)]
    Backup(#[from] WalletBackupError),
    /// A bound was exceeded or a selected journal changed during collection.
    #[error(transparent)]
    State(#[from] StorageError),
}

/// Explicit initialized targets for a coordinated live recovery merge. Every
/// target must share one named IndexedDB database. Omitted stores are untouched;
/// enumerate all relevant wallet journals before resuming wallet operations.
#[derive(Default)]
pub struct BrowserWalletRestoreTargets<'a> {
    /// HD journals whose burned floors and retained request IDs are preserved.
    pub allocations: &'a [&'a BrowserAddressBook],
    /// Account journals whose compatible signed candidate families are united.
    pub accounts: &'a [&'a BrowserAccountBook],
    /// Qi journals whose input claims and compatible candidates are united.
    pub qi: &'a [&'a BrowserQiBook],
    /// Payment journals and exact private owners for exposure validation.
    pub payments: &'a [BrowserPaymentCapture<'a>],
}

/// Result of one committed recovery transaction. No network submission occurs.
#[derive(Debug)]
pub struct BrowserWalletRestore {
    /// New revisions in allocation, account, Qi, then payment input order.
    pub revisions: Vec<u64>,
    /// Pending HD/payment requests sealed as abandoned; IDs stay consumed.
    pub abandoned_allocations: usize,
    /// Per-account merge effects in account input order.
    pub accounts: Vec<quai_wallet::account_custody::AccountMergeReport>,
    /// Per-Qi merge effects in Qi input order.
    pub qi: Vec<quai_wallet::qi_custody::QiMergeReport>,
}

/// Merge an authenticated recovery backup into all selected live journals in one
/// revision-checked IndexedDB transaction. All ownership/state checks finish
/// before any write. Any concurrent change, duplicate namespace, cross-database
/// target or invalid merge aborts the entire restore; no automatic retry occurs.
///
/// Signed claims remain held, chain observations are discarded, and pending
/// allocation ranges become burned history. Retain the backup's prior inventory
/// when capturing successor backups. This does not discover omitted journals or
/// initialize missing/tombstoned stores. Cancellation after dispatch can commit
/// the whole transaction: re-read every target before deciding whether to retry.
pub async fn merge_wallet_backup(
    targets: BrowserWalletRestoreTargets<'_>,
    backup: &WalletBackup,
) -> Result<BrowserWalletRestore, BrowserBackupError> {
    use quai_browser::{BrowserSnapshotUpdate, compare_exchange_snapshots};
    let count = targets
        .allocations
        .len()
        .checked_add(targets.accounts.len())
        .and_then(|n| n.checked_add(targets.qi.len()))
        .and_then(|n| n.checked_add(targets.payments.len()))
        .ok_or(StorageError::Invalid)?;
    if count == 0 || count > MAX_PORTABLE_CAPTURE_JOURNALS {
        return Err(StorageError::Invalid.into());
    }
    let mut prepared = Vec::with_capacity(count);
    let mut total = 0;
    let mut report = BrowserWalletRestore {
        revisions: Vec::new(),
        abandoned_allocations: 0,
        accounts: Vec::new(),
        qi: Vec::new(),
    };
    for target in targets.allocations {
        let mut snapshot = target.snapshot().await?;
        report.abandoned_allocations += snapshot.book.merge_backup(backup)?;
        let bytes = snapshot.book.export_state();
        account_size(&mut total, bytes.len())?;
        prepared.push((&target.store, snapshot.revision, bytes));
    }
    for target in targets.accounts {
        let mut snapshot = target.snapshot().await?;
        report.accounts.push(snapshot.book.merge_backup(backup)?);
        let bytes = snapshot.book.export_state()?;
        account_size(&mut total, bytes.len())?;
        prepared.push((&target.store, snapshot.revision, bytes));
    }
    for target in targets.qi {
        let mut snapshot = target.snapshot().await?;
        report.qi.push(snapshot.book.merge_backup(backup)?);
        let bytes = snapshot.book.export_state()?;
        account_size(&mut total, bytes.len())?;
        prepared.push((&target.store, snapshot.revision, bytes));
    }
    for target in targets.payments {
        let mut snapshot = target.book.snapshot(target.owner).await?;
        report.abandoned_allocations += snapshot.book.merge_backup(target.owner, backup)?;
        let bytes = snapshot.book.export_state();
        account_size(&mut total, bytes.len())?;
        prepared.push((&target.book.store, snapshot.revision, bytes));
    }
    let updates: Vec<_> = prepared
        .iter()
        .map(|(store, revision, bytes)| BrowserSnapshotUpdate {
            store,
            expected: Some(*revision),
            bytes: Some(bytes),
        })
        .collect();
    report.revisions = compare_exchange_snapshots(&updates).await?;
    Ok(report)
}
/// Account store and its explicit private-origin descriptor.
#[derive(Clone, Copy)]
pub struct BrowserAccountCapture<'a> {
    /// Open current custody journal.
    pub book: &'a BrowserAccountBook,
    /// Exact owned public metadata; the capture proves its private derivation.
    pub address: &'a PublicAddress,
}
/// Payment store and guarded private owner used to validate completed exposures.
#[derive(Clone, Copy)]
pub struct BrowserPaymentCapture<'a> {
    /// Open current channel/direction journal.
    pub book: &'a BrowserPaymentBook,
    /// Borrowed local owner; never copied into public storage or logs.
    pub owner: &'a PrivatePaymentCode,
}
/// Explicit live stores to include in one consistent recovery capture. The caller
/// must enumerate every relevant journal; absent or omitted stores are not inferred.
#[derive(Default)]
pub struct BrowserWalletCaptureSources<'a> {
    /// Open HD account allocation journals.
    pub allocations: &'a [&'a BrowserAddressBook],
    /// Open account custody and owned public metadata.
    pub accounts: &'a [BrowserAccountCapture<'a>],
    /// Open Qi custody journals.
    pub qi: &'a [&'a BrowserQiBook],
    /// Open payment journals with exact private owners.
    pub payments: &'a [BrowserPaymentCapture<'a>],
    /// Authenticated earlier address/channel/exposure inventory. Current account
    /// and Qi stores must still supply every transaction custody record.
    pub previous_inventory: Option<&'a WalletBackup>,
    /// Additional frozen known owned addresses outside the selected journals.
    pub additional_addresses: &'a [(NetworkScope, PublicAddress)],
}
/// Journal category accompanying an observed revision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowserJournalKind {
    /// HD address allocation.
    Address,
    /// Quai account custody.
    Account,
    /// Qi input custody.
    Qi,
    /// Payment-code allocation.
    Payment,
}
/// Public identity/revision evidence for one included journal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BrowserCapturedRevision {
    /// Journal format/category.
    pub kind: BrowserJournalKind,
    /// Exact chain/genesis/zone.
    pub scope: NetworkScope,
    /// Public logical journal identity, not a database name or private key.
    pub identity: Hash32,
    /// Validated monotonic IndexedDB revision at the capture's consistent cut.
    pub revision: u64,
}
/// Ownership-proved backup plus the public revisions collected. Private material
/// remains guarded by WalletBackup; encrypt explicitly before persisting/exporting.
#[derive(Debug)]
pub struct BrowserWalletCapture {
    /// Plaintext guarded backup. Its Debug output redacts secret origins.
    pub backup: WalletBackup,
    /// Same order as selected addresses, accounts, Qi and payment stores.
    pub revisions: Vec<BrowserCapturedRevision>,
}
fn account_size(total: &mut usize, bytes: usize) -> Result<(), BrowserBackupError> {
    *total = total.checked_add(bytes).ok_or(StorageError::Invalid)?;
    if *total > 16 * 1024 * 1024 {
        return Err(StorageError::Invalid.into());
    }
    Ok(())
}
fn same_revision(before: u64, after: u64) -> Result<(), BrowserBackupError> {
    if before != after {
        return Err(StorageError::StaleSnapshot.into());
    }
    Ok(())
}
/// Read every selected journal, then re-read every revision. Equal monotonic
/// revisions establish a common point during collection for the selected public
/// states. Any mutation/tombstone/corruption rejects the attempt without retry.
/// This is not a chain snapshot, protection against malicious same-origin rollback,
/// or discovery of omitted stores. Writes after the cut need a later backup.
///
/// The operation is read-only and cancellation never changes a journal. At most
/// 128 journals and 16 MiB public state are collected; final portable backup limits
/// apply as well. Additional addresses and prior inventory must already be frozen.
/// Private origins remain guarded across awaits and are dropped on cancellation.
/// Run expensive ownership verification/encryption on an application worker.
pub async fn capture_wallet_backup(
    sources: BrowserWalletCaptureSources<'_>,
    origins: Vec<BackupOrigin>,
) -> Result<BrowserWalletCapture, BrowserBackupError> {
    let count = sources
        .allocations
        .len()
        .checked_add(sources.accounts.len())
        .and_then(|n| n.checked_add(sources.qi.len()))
        .and_then(|n| n.checked_add(sources.payments.len()))
        .ok_or(StorageError::Invalid)?;
    if count > MAX_PORTABLE_CAPTURE_JOURNALS {
        return Err(StorageError::Invalid.into());
    }
    let (mut addresses, mut accounts, mut qi, mut payments) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut bytes = 0;
    let mut revisions = Vec::new();
    for source in sources.allocations {
        let s = source.snapshot().await?;
        account_size(&mut bytes, s.book.export_state().len())?;
        revisions.push(BrowserCapturedRevision {
            kind: BrowserJournalKind::Address,
            scope: s.book.scope(),
            identity: s.book.identity(),
            revision: s.revision,
        });
        addresses.push(s);
    }
    for source in sources.accounts {
        let s = source.book.snapshot().await?;
        account_size(&mut bytes, s.book.export_state()?.len())?;
        revisions.push(BrowserCapturedRevision {
            kind: BrowserJournalKind::Account,
            scope: s.book.scope(),
            identity: AccountOperationBook::account_identity(s.book.owner()),
            revision: s.revision,
        });
        accounts.push(s);
    }
    for source in sources.qi {
        let s = source.snapshot().await?;
        account_size(&mut bytes, s.book.export_state()?.len())?;
        revisions.push(BrowserCapturedRevision {
            kind: BrowserJournalKind::Qi,
            scope: s.book.scope(),
            identity: s.book.identity(),
            revision: s.revision,
        });
        qi.push(s);
    }
    for source in sources.payments {
        let s = source.book.snapshot(source.owner).await?;
        account_size(&mut bytes, s.book.export_state().len())?;
        revisions.push(BrowserCapturedRevision {
            kind: BrowserJournalKind::Payment,
            scope: s.book.scope(),
            identity: s.book.identity(),
            revision: s.revision,
        });
        payments.push(s);
    }
    // These checks begin only after every first read completed. Successful
    // monotonically equal revisions have intersecting stable intervals.
    for (source, before) in sources.allocations.iter().zip(&addresses) {
        same_revision(before.revision, source.snapshot().await?.revision)?;
    }
    for (source, before) in sources.accounts.iter().zip(&accounts) {
        same_revision(before.revision, source.book.snapshot().await?.revision)?;
    }
    for (source, before) in sources.qi.iter().zip(&qi) {
        same_revision(before.revision, source.snapshot().await?.revision)?;
    }
    for (source, before) in sources.payments.iter().zip(&payments) {
        same_revision(
            before.revision,
            source.book.snapshot(source.owner).await?.revision,
        )?;
    }
    let backup = WalletBackup::capture_portable(
        PortableWalletCapture {
            allocations: &addresses.iter().map(|s| &s.book).collect::<Vec<_>>(),
            accounts: &accounts
                .iter()
                .zip(sources.accounts)
                .map(|(s, source)| AccountCustodyCapture {
                    book: &s.book,
                    address: source.address,
                })
                .collect::<Vec<_>>(),
            qi: &qi.iter().map(|s| &s.book).collect::<Vec<_>>(),
            payments: &payments.iter().map(|s| &s.book).collect::<Vec<_>>(),
            previous_inventory: sources.previous_inventory,
            additional_addresses: sources.additional_addresses,
        },
        origins,
    )?;
    Ok(BrowserWalletCapture { backup, revisions })
}
