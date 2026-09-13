//! Consistent read-only collection of explicitly selected browser wallet journals.
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

/// Errors never trigger an automatic retry, backup overwrite or storage mutation.
#[derive(Debug, thiserror::Error)]
pub enum BrowserBackupError {
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
