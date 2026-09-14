//! Portable authenticated wallet-state backups, with optional native capture/restore.
//! Old QUAISEED envelopes remain separate.
//! Plaintext serialization is explicit, bounded and zeroized; no wallet serde is exposed.
use crate::discovery::{Checkpoint, NetworkScope};
use crate::metadata::{KeyOrigin, PublicAddress, StorageError};
use crate::state::{DerivationState, NonceState, OperationState, PublicWalletState, ScopeState};
pub use crate::state::{
    QuaiReplacement, ReplacementCandidate, Reservation, ReservationId, ReservationState,
};
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
use crate::storage::SqliteStore;
use crate::{AccountPublic, BackupKdf, CoinType, ExtendedPrivateKey};
use chacha20poly1305::{AeadInOut, KeyInit, Tag, XChaCha20Poly1305, XNonce};
use quai_consensus::{MAX_TRANSACTION_BYTES, OutPoint, U256};
use quai_crypto::{SecretBytes, SecretKey};
use quai_primitives::{Address, Hash32, QiAddress, QuaiAddress, Zone};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};
use thiserror::Error;
use zeroize::Zeroizing;

pub mod legacy;
mod payment;
mod portable;
pub use portable::{AccountCustodyCapture, MAX_PORTABLE_CAPTURE_JOURNALS, PortableWalletCapture};
mod views;
pub use crate::state::payment::PaymentAddressRecord;
pub use views::{
    BackupDerivationCursor, BackupOperation, BackupPaymentChannel, BackupPaymentExposure,
    BackupScope,
};

const MAGIC: &[u8; 8] = b"QUAIWALT";
const HEADER: usize = 68;
const MAX_PLAINTEXT: usize = 16 * 1024 * 1024;
const MAX_RECORDS: usize = 100_000;
const MAX_ORIGINS: usize = 16;

/// Full-backup failures never echo secret origins or decrypted bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum WalletBackupError {
    /// Invalid input, bound, password or internally inconsistent public state.
    #[error("invalid wallet backup input")]
    InvalidInput,
    /// Unsupported private origin, extension/channel or database schema.
    #[error("unsupported wallet backup origin or extension")]
    Unsupported,
    /// Supplied secrets do not prove every persisted public origin.
    #[error("wallet backup ownership verification failed")]
    Ownership,
    /// Wrong password, corruption, malformed content or unsupported encrypted version.
    #[error("unable to unlock wallet backup")]
    UnlockFailed,
    /// OS entropy or bounded cryptographic resources unavailable.
    #[error("wallet backup cryptographic resources unavailable")]
    Resources,
    /// Atomic database operation failed; existing state remains intact.
    #[error("wallet backup storage operation failed: {0}")]
    Storage(#[from] StorageError),
}
type Result<T> = std::result::Result<T, WalletBackupError>;

/// Supported explicit secret origin kind; public-only ownership is excluded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackupOriginKind {
    /// Original 16..64-byte seed, including an already-applied BIP39 passphrase.
    Seed,
    /// Depth-zero BIP32 private master, preserving its chain code.
    MasterXprv,
    /// Standalone secp256k1 scalar, with no claimed HD ancestry.
    ImportedPrivateKey,
    /// A depth-three BIP47 account with explicit asserted m/47'/969' ancestry.
    PaymentAccountXprv,
}
enum OriginMaterial {
    Seed(Zeroizing<Vec<u8>>),
    Master(ExtendedPrivateKey),
    Imported(SecretBytes),
    PaymentAccount {
        account: u32,
        key: ExtendedPrivateKey,
    },
}
/// Owned, zeroizing/redacted secret origin. No Clone, Display or implicit serialization.
pub struct BackupOrigin(OriginMaterial);
impl fmt::Debug for BackupOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BackupOrigin([REDACTED])")
    }
}
impl BackupOrigin {
    /// Preserve exact original/effective seed bytes. Mnemonic text and passphrase need
    /// not be retained to preserve the same derived identity.
    pub fn from_seed(seed: &[u8]) -> Result<Self> {
        if !(16..=64).contains(&seed.len()) {
            return Err(WalletBackupError::InvalidInput);
        }
        ExtendedPrivateKey::from_seed(seed).map_err(|_| WalletBackupError::InvalidInput)?;
        Ok(Self(OriginMaterial::Seed(Zeroizing::new(seed.to_vec()))))
    }
    /// Preserve the effective BIP39 seed after language and passphrase normalization.
    /// The original phrase, wordlist label and passphrase are not retained.
    pub fn from_mnemonic(mnemonic: &crate::Mnemonic, passphrase: &str) -> Result<Self> {
        let seed = mnemonic.to_seed(passphrase);
        Self::from_seed(seed.expose())
    }
    /// Explicit guarded export of the BIP32 master xprv for either HD origin.
    /// Imported standalone keys have no chain code and return Unsupported.
    pub fn export_master_xprv(&self) -> Result<crate::SecretString> {
        match &self.0 {
            OriginMaterial::Seed(seed) => ExtendedPrivateKey::from_seed(seed)
                .and_then(|master| master.export())
                .map_err(|_| WalletBackupError::Resources),
            OriginMaterial::Master(master) => {
                master.export().map_err(|_| WalletBackupError::Resources)
            }
            OriginMaterial::Imported(_) | OriginMaterial::PaymentAccount { .. } => {
                Err(WalletBackupError::Unsupported)
            }
        }
    }
    /// Import a master xprv. Non-master xprvs are explicitly unsupported in v1.
    pub fn from_master_xprv(encoded: &str) -> Result<Self> {
        let key =
            ExtendedPrivateKey::import(encoded).map_err(|_| WalletBackupError::InvalidInput)?;
        if key.depth() != 0 {
            return Err(WalletBackupError::Unsupported);
        }
        Ok(Self(OriginMaterial::Master(key)))
    }
    /// Preserve an imported depth-three BIP47 payment account in QUAIWALT v3.
    /// Validates the hardened account index but cannot prove omitted ancestors;
    /// the caller asserts m/47'/969'. Never treated as a BIP44 master origin.
    pub fn from_payment_account_xprv(encoded: &str, account: u32) -> Result<Self> {
        quai_payments::PrivatePaymentCode::from_account_xprv(encoded, account)
            .map_err(|_| WalletBackupError::InvalidInput)?;
        let key =
            ExtendedPrivateKey::import(encoded).map_err(|_| WalletBackupError::InvalidInput)?;
        Ok(Self(OriginMaterial::PaymentAccount { account, key }))
    }
    /// Export this exact payment account with its asserted account index. Other
    /// origin types return Unsupported instead of silently narrowing a master.
    pub fn export_payment_account_xprv(&self) -> Result<(u32, crate::SecretString)> {
        match &self.0 {
            OriginMaterial::PaymentAccount { account, key } => Ok((
                *account,
                key.export().map_err(|_| WalletBackupError::Resources)?,
            )),
            _ => Err(WalletBackupError::Unsupported),
        }
    }
    /// Explicitly copy/export a standalone key into guarded backup ownership.
    pub fn from_private_key(key: &SecretKey) -> Self {
        Self(OriginMaterial::Imported(key.export_bytes()))
    }
    /// Public origin category.
    pub fn kind(&self) -> BackupOriginKind {
        match &self.0 {
            OriginMaterial::Seed(_) => BackupOriginKind::Seed,
            OriginMaterial::Master(_) => BackupOriginKind::MasterXprv,
            OriginMaterial::Imported(_) => BackupOriginKind::ImportedPrivateKey,
            OriginMaterial::PaymentAccount { .. } => BackupOriginKind::PaymentAccountXprv,
        }
    }
    /// Explicitly inspect exact preserved seed bytes; other origins return None.
    pub fn expose_seed(&self) -> Option<&[u8]> {
        match &self.0 {
            OriginMaterial::Seed(seed) => Some(seed),
            _ => None,
        }
    }
    fn account_key(&self, coin: CoinType, account: u32) -> Result<ExtendedPrivateKey> {
        if account >= 1 << 31 {
            return Err(WalletBackupError::InvalidInput);
        }
        let purpose = match &self.0 {
            OriginMaterial::Seed(seed) => {
                ExtendedPrivateKey::from_seed(seed).and_then(|master| master.derive_child(44, true))
            }
            OriginMaterial::Master(master) => master.derive_child(44, true),
            OriginMaterial::Imported(_) | OriginMaterial::PaymentAccount { .. } => {
                return Err(WalletBackupError::Ownership);
            }
        }
        .map_err(|_| WalletBackupError::Ownership)?;
        purpose
            .derive_child(coin.number(), true)
            .and_then(|key| key.derive_child(account, true))
            .map_err(|_| WalletBackupError::Ownership)
    }
    /// Recreate the exact public account for either supported coin type.
    pub fn account_public(&self, coin: CoinType, account: u32) -> Result<AccountPublic> {
        let key = self.account_key(coin, account)?;
        AccountPublic::import(&key.public_key().export(), coin, account)
            .map_err(|_| WalletBackupError::Ownership)
    }
    /// Recover a standalone imported key into its usual guarded signing wrapper.
    pub fn imported_key(&self) -> Result<SecretKey> {
        match &self.0 {
            OriginMaterial::Imported(bytes) => {
                SecretKey::from_bytes(bytes).map_err(|_| WalletBackupError::Ownership)
            }
            _ => Err(WalletBackupError::Unsupported),
        }
    }
    /// Recover an HD child only for an origin that contains its master secret.
    pub fn derive_key(
        &self,
        coin: CoinType,
        account: u32,
        change: bool,
        index: u32,
    ) -> Result<SecretKey> {
        self.account_key(coin, account)?
            .derive_child(u32::from(change), false)
            .and_then(|key| key.derive_child(index, false))
            .and_then(|key| key.secret_key())
            .map_err(|_| WalletBackupError::Ownership)
    }
}
/// Captured public state and its explicit secret owners. Native capture includes
/// every network/zone scope in the selected database, not just its bound scope;
/// portable account capture includes only the explicitly selected account journal.
/// UTXO snapshots/checkpoints are intentionally invalidated during restore.
pub struct WalletBackup {
    origins: Vec<BackupOrigin>,
    state: PublicWalletState,
}
impl fmt::Debug for WalletBackup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WalletBackup([REDACTED])")
    }
}
/// Required work after restore; no inclusion observation is promoted to finality.
#[derive(Clone, Debug)]
pub struct RestoreReport {
    /// Restored scopes and new invalidated snapshot generations.
    pub invalidated_scopes: Vec<(NetworkScope, u64)>,
    /// Signed/submitted/confirmed operations from this backup whose claims remain held.
    pub retained_signed_operations: usize,
    /// Held signed operations lacking canonical payload bytes for rebroadcast.
    pub hash_only_operations: usize,
    /// Every restored scope requires a new qualified scan.
    pub rescan_required: bool,
    /// Restored signed operations need verified reconciliation. Existing target operations
    /// remain unchanged and may have their own outstanding reconciliation requirements.
    pub reconciliation_required: bool,
}
impl WalletBackup {
    /// Capture one portable account custody journal and prove its exact public
    /// metadata against explicit private origins. This is an account-only backup:
    /// it does not capture HD/payment allocation journals, other accounts, Qi
    /// claims or browser database state. Back up those independently until using
    /// a complete wallet capture. Inclusion is omitted for fresh reconciliation.
    pub fn capture_account_custody(
        book: &crate::account_custody::AccountOperationBook,
        address: PublicAddress,
        origins: Vec<BackupOrigin>,
    ) -> Result<Self> {
        if address.address() != book.address().address()
            || *address.public_key() != book.owner().to_compressed()
        {
            return Err(WalletBackupError::Ownership);
        }
        let operations = book
            .operations()
            .map(|op| OperationState {
                record: Reservation {
                    id: op.id,
                    state: if op.state == ReservationState::Confirmed {
                        ReservationState::Submitted
                    } else {
                        op.state
                    },
                    transaction: op.transaction,
                    inclusion: None,
                },
                kind: 1,
                qi: vec![],
                nonce: Some((book.address(), op.nonce)),
                payload: op.payload.clone(),
                replacements: op.replacements.clone(),
            })
            .collect();
        let state = PublicWalletState {
            scopes: vec![ScopeState {
                scope: book.scope(),
                addresses: vec![address],
                derivation: vec![],
                nonces: vec![NonceState {
                    address: book.address(),
                    next_nonce: book.next_nonce(),
                }],
                operations,
            }],
            channels: vec![],
            exposures: vec![],
        };
        let backup = Self { origins, state };
        backup.validate()?;
        Ok(backup)
    }
    /// Capture one portable Qi custody journal with explicit secret origins which
    /// prove every retained public address. Current UTXOs and chain observations
    /// are omitted. This does not capture address/payment allocation journals,
    /// account custody or other browser namespaces.
    pub fn capture_qi_custody(
        book: &crate::qi_custody::QiOperationBook,
        origins: Vec<BackupOrigin>,
    ) -> Result<Self> {
        let operations = book
            .operations()
            .map(|op| OperationState {
                record: Reservation {
                    id: op.id,
                    state: if op.state == ReservationState::Confirmed {
                        ReservationState::Submitted
                    } else {
                        op.state
                    },
                    transaction: op.transaction,
                    inclusion: None,
                },
                kind: 0,
                qi: op
                    .claims
                    .iter()
                    .map(|c| (c.outpoint, c.owner.address()))
                    .collect(),
                nonce: None,
                payload: op.payload.clone(),
                replacements: op.replacements.clone(),
            })
            .collect();
        let state = PublicWalletState {
            scopes: vec![ScopeState {
                scope: book.scope(),
                addresses: book.addresses().cloned().collect(),
                derivation: vec![],
                nonces: vec![],
                operations,
            }],
            channels: vec![],
            exposures: vec![],
        };
        let backup = Self { origins, state };
        backup.validate()?;
        Ok(backup)
    }
    /// Capture all database scopes atomically. Every HD/imported address and bound
    /// account xpub must be proved by one of the explicit supplied secret origins.
    #[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
    pub fn capture(store: &mut SqliteStore, origins: Vec<BackupOrigin>) -> Result<Self> {
        let state = store.capture_public_state()?;
        let backup = Self { origins, state };
        backup.validate()?;
        Ok(backup)
    }
    /// Explicit guarded secret origins, retaining their original representations.
    pub fn origins(&self) -> &[BackupOrigin] {
        &self.origins
    }
    /// Public network/zone identities contained in this backup.
    pub fn scopes(&self) -> Vec<NetworkScope> {
        self.state.scopes.iter().map(|state| state.scope).collect()
    }
    /// Validate all secret/public derivations before the atomic database merge.
    /// Existing cursors never decrease and conflicting operation records fail the
    /// entire restore. No existing claim is released or erased by backup import.
    #[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
    pub fn restore(&self, store: &mut SqliteStore) -> Result<RestoreReport> {
        let payment_owners = self.validate()?;
        let mut signed = 0;
        let mut hash_only = 0;
        for operation in self.state.scopes.iter().flat_map(|scope| &scope.operations) {
            if matches!(
                operation.record.state,
                ReservationState::Signed
                    | ReservationState::Submitted
                    | ReservationState::Confirmed
            ) {
                signed += 1;
                if operation.payload.is_none() {
                    hash_only += 1;
                }
            }
        }
        let invalidated_scopes = store.restore_public_state(&self.state, &payment_owners)?;
        Ok(RestoreReport {
            invalidated_scopes,
            retained_signed_operations: signed,
            hash_only_operations: hash_only,
            rescan_required: true,
            reconciliation_required: signed > 0,
        })
    }
    /// Encrypt with fresh OS salt/nonce, memory-hard KDF, authenticated format header
    /// and a bounded zeroizing plaintext buffer. Run on a caller-managed CPU worker.
    pub fn encrypt(&self, password: &[u8], kdf: BackupKdf) -> Result<EncryptedWalletBackup> {
        let mut salt = [0; 16];
        let mut nonce = [0; 24];
        getrandom::fill(&mut salt).map_err(|_| WalletBackupError::Resources)?;
        getrandom::fill(&mut nonce).map_err(|_| WalletBackupError::Resources)?;
        self.encrypt_with_randomness(password, kdf, salt, nonce)
    }
    fn encrypt_with_randomness(
        &self,
        password: &[u8],
        kdf: BackupKdf,
        salt: [u8; 16],
        nonce: [u8; 24],
    ) -> Result<EncryptedWalletBackup> {
        password_valid(password)?;
        self.validate()?;
        let mut plaintext = self.encode()?;
        let mut header = [0u8; HEADER];
        header[..8].copy_from_slice(MAGIC);
        header[8..12].copy_from_slice(&[self.version(), 1, 1, 0]);
        header[12..16].copy_from_slice(&kdf.memory_kib().to_be_bytes());
        header[16..20].copy_from_slice(&kdf.iterations().to_be_bytes());
        header[20..24].copy_from_slice(&kdf.lanes().to_be_bytes());
        header[24..40].copy_from_slice(&salt);
        header[40..64].copy_from_slice(&nonce);
        header[64..68].copy_from_slice(&(plaintext.len() as u32).to_be_bytes());
        let key = crate::backup::derive_key(password, &salt, kdf)
            .map_err(|_| WalletBackupError::Resources)?;
        let cipher = XChaCha20Poly1305::new_from_slice(&key[..])
            .map_err(|_| WalletBackupError::Resources)?;
        let tag = cipher
            .encrypt_inout_detached(&XNonce::from(nonce), &header, (&mut plaintext[..]).into())
            .map_err(|_| WalletBackupError::Resources)?;
        let mut encoded = Vec::new();
        encoded
            .try_reserve_exact(HEADER + plaintext.len() + 16)
            .map_err(|_| WalletBackupError::Resources)?;
        encoded.extend(header);
        encoded.extend_from_slice(&plaintext);
        encoded.extend_from_slice(&tag);
        Ok(EncryptedWalletBackup(encoded))
    }
    fn validate(&self) -> Result<Vec<quai_payments::PrivatePaymentCode>> {
        if self.origins.is_empty()
            || self.origins.len() > MAX_ORIGINS
            || self.state.scopes.is_empty()
            || self.state.scopes.len() > 64
        {
            return Err(WalletBackupError::InvalidInput);
        }
        let mut accounts = BTreeSet::new();
        let mut scope_keys = BTreeSet::new();
        let mut total = self.state.channels.len() + self.state.exposures.len();
        for scope in &self.state.scopes {
            if scope.scope.genesis.bytes() == &[0; 32] || !scope_keys.insert(scope.scope.key()) {
                return Err(WalletBackupError::InvalidInput);
            }
            total = total
                .checked_add(
                    scope.addresses.len()
                        + scope.derivation.len()
                        + scope.nonces.len()
                        + scope.operations.len(),
                )
                .ok_or(WalletBackupError::InvalidInput)?;
            for address in &scope.addresses {
                if let KeyOrigin::Bip44 { coin, account, .. } = address.origin() {
                    accounts.insert((coin.number(), account));
                }
            }
            for cursor in &scope.derivation {
                accounts.insert((cursor.coin.number(), cursor.account));
            }
        }
        if total > MAX_RECORDS || accounts.len() > 4096 {
            return Err(WalletBackupError::InvalidInput);
        }
        if self.origins.len().saturating_mul(accounts.len()) > 8192 {
            return Err(WalletBackupError::InvalidInput);
        }
        let (payment_owners, payment_owned) = self.payment_proof()?;
        let mut derivation_checks = 0usize;
        let mut public_accounts = BTreeMap::<(u32, u32), Vec<AccountPublic>>::new();
        let mut imported = BTreeSet::new();
        for origin in &self.origins {
            if let OriginMaterial::Imported(bytes) = &origin.0 {
                imported.insert(
                    SecretKey::from_bytes(bytes)
                        .map_err(|_| WalletBackupError::Ownership)?
                        .public_key()
                        .to_compressed(),
                );
                continue;
            }
            if matches!(&origin.0, OriginMaterial::PaymentAccount { .. }) {
                continue;
            }
            for &(coin, account) in &accounts {
                public_accounts
                    .entry((coin, account))
                    .or_default()
                    .push(origin.account_public(coin_type(coin as u16)?, account)?);
            }
        }
        for scope in &self.state.scopes {
            let mut addresses = BTreeMap::new();
            let mut origins = BTreeSet::new();
            let mut maximum_indexes = BTreeMap::<(u32, u32, bool), u32>::new();
            for address in &scope.addresses {
                if address
                    .address()
                    .zone()
                    .map_err(|_| WalletBackupError::InvalidInput)?
                    != scope.scope.zone
                    || addresses.insert(address.address(), address).is_some()
                {
                    return Err(WalletBackupError::InvalidInput);
                }
                let owned = match address.origin() {
                    KeyOrigin::ImportedPublic => {
                        imported.contains(address.public_key())
                            || payment_owned.contains(&(scope.scope.key(), *address.public_key()))
                    }
                    KeyOrigin::Bip44 {
                        coin,
                        account,
                        change,
                        index,
                    } => {
                        if index >= 1 << 31
                            || account >= 1 << 31
                            || !origins.insert((coin.number(), account, change, index))
                        {
                            return Err(WalletBackupError::InvalidInput);
                        }
                        maximum_indexes
                            .entry((coin.number(), account, change))
                            .and_modify(|value| *value = (*value).max(index))
                            .or_insert(index);
                        let mut owned = false;
                        if let Some(nodes) = public_accounts.get(&(coin.number(), account)) {
                            for node in nodes {
                                derivation_checks += 1;
                                if derivation_checks > 200_000 {
                                    return Err(WalletBackupError::InvalidInput);
                                }
                                if node.derive_address(change, index).is_ok_and(|derived| {
                                    derived.address == address.address()
                                        && &derived.public_key == address.public_key()
                                }) {
                                    owned = true;
                                    break;
                                }
                            }
                        }
                        owned
                    }
                };
                if !owned {
                    return Err(WalletBackupError::Ownership);
                }
            }
            let mut cursors = BTreeSet::new();
            let mut bound_accounts = BTreeMap::new();
            for cursor in &scope.derivation {
                if cursor.account >= 1 << 31
                    || cursor.next_index > 1 << 31
                    || !cursors.insert((cursor.coin.number(), cursor.account, cursor.change))
                {
                    return Err(WalletBackupError::InvalidInput);
                }
                if bound_accounts
                    .insert((cursor.coin.number(), cursor.account), &cursor.xpub)
                    .is_some_and(|old| old != &cursor.xpub)
                {
                    return Err(WalletBackupError::Ownership);
                }
                if !public_accounts
                    .get(&(cursor.coin.number(), cursor.account))
                    .is_some_and(|nodes| nodes.iter().any(|node| node.export() == cursor.xpub))
                {
                    return Err(WalletBackupError::Ownership);
                }
                if maximum_indexes
                    .get(&(cursor.coin.number(), cursor.account, cursor.change))
                    .is_some_and(|index| *index >= cursor.next_index)
                {
                    return Err(WalletBackupError::InvalidInput);
                }
            }
            let mut nonces = BTreeMap::new();
            for nonce in &scope.nonces {
                if nonce.address.zone() != scope.scope.zone
                    || !addresses.contains_key(&nonce.address.address())
                    || nonces.insert(nonce.address, nonce.next_nonce).is_some()
                {
                    return Err(WalletBackupError::InvalidInput);
                }
            }
            let mut ids = BTreeSet::new();
            let mut outpoints = BTreeSet::new();
            let mut nonce_claims = BTreeSet::new();
            for operation in &scope.operations {
                total = total
                    .checked_add(operation.qi.len())
                    .ok_or(WalletBackupError::InvalidInput)?;
                if total > MAX_RECORDS
                    || !ids.insert(operation.record.id.0)
                    || operation.kind > 1
                    || operation.qi.len() > 4096
                {
                    return Err(WalletBackupError::InvalidInput);
                }
                if !operation.replacements.is_empty() {
                    crate::state::replacements::validate_family(
                        operation
                            .payload
                            .as_deref()
                            .ok_or(WalletBackupError::InvalidInput)?,
                        &operation.replacements,
                    )
                    .map_err(|_| WalletBackupError::InvalidInput)?;
                }
                let record = &operation.record;
                let unsigned = matches!(
                    record.state,
                    ReservationState::Reserved | ReservationState::Released
                );
                if unsigned != record.transaction.is_none()
                    || (record.state == ReservationState::Confirmed) != record.inclusion.is_some()
                    || unsigned && operation.payload.is_some()
                {
                    return Err(WalletBackupError::InvalidInput);
                }
                if operation.kind == 0 {
                    if operation.nonce.is_some()
                        || (record.state == ReservationState::Released) != operation.qi.is_empty()
                    {
                        return Err(WalletBackupError::InvalidInput);
                    }
                    for (outpoint, address) in &operation.qi {
                        let owner = QiAddress::try_from(*address)
                            .map_err(|_| WalletBackupError::InvalidInput)?;
                        if owner.zone() != scope.scope.zone
                            || !addresses.contains_key(address)
                            || outpoint.transaction_hash.bytes()[2] != scope.scope.zone.byte()
                            || outpoint.transaction_hash == Hash32::ZERO
                            || !outpoints.insert(*outpoint)
                        {
                            return Err(WalletBackupError::InvalidInput);
                        }
                    }
                } else {
                    if !operation.qi.is_empty() {
                        return Err(WalletBackupError::InvalidInput);
                    }
                    let (address, nonce) =
                        operation.nonce.ok_or(WalletBackupError::InvalidInput)?;
                    if !nonces.get(&address).is_some_and(|next| *next > nonce)
                        || !nonce_claims.insert((address, nonce))
                    {
                        return Err(WalletBackupError::InvalidInput);
                    }
                }
                if let Some(payload) = &operation.payload {
                    if payload.is_empty() || payload.len() > MAX_TRANSACTION_BYTES {
                        return Err(WalletBackupError::InvalidInput);
                    }
                    if operation.kind == 1 {
                        let signed = quai_consensus::SignedQuaiTransaction::decode(payload)
                            .map_err(|_| WalletBackupError::InvalidInput)?;
                        if signed.transaction().chain_id != scope.scope.chain_id
                            || signed.from().zone() != scope.scope.zone
                            || Some((signed.from(), signed.transaction().nonce)) != operation.nonce
                            || signed.hash().ok() != record.transaction
                        {
                            return Err(WalletBackupError::InvalidInput);
                        }
                    } else {
                        let signed = quai_consensus::SignedQiOperation::decode(payload)
                            .map_err(|_| WalletBackupError::InvalidInput)?;
                        let inputs: BTreeSet<_> = signed
                            .transaction()
                            .inputs
                            .iter()
                            .map(|input| (input.previous_output, input.public_key.address()))
                            .collect();
                        if signed.transaction().chain_id != scope.scope.chain_id
                            || inputs != operation.qi.iter().copied().collect()
                            || signed.hash().ok() != record.transaction
                        {
                            return Err(WalletBackupError::InvalidInput);
                        }
                    }
                }
            }
        }
        Ok(payment_owners)
    }
    fn encode(&self) -> Result<Zeroizing<Vec<u8>>> {
        let mut writer = Writer::new()?;
        writer.u32(u32::from(self.version() >= 2))?; // mandatory extension bitmap
        writer.u16(self.origins.len() as u16)?;
        for origin in &self.origins {
            match &origin.0 {
                OriginMaterial::Seed(seed) => {
                    writer.u8(1)?;
                    writer.short(seed)?;
                }
                OriginMaterial::Master(master) => {
                    writer.u8(2)?;
                    let encoded = master.export().map_err(|_| WalletBackupError::Resources)?;
                    writer.short(encoded.expose().as_bytes())?;
                }
                OriginMaterial::Imported(bytes) => {
                    writer.u8(3)?;
                    writer.short(&bytes[..])?;
                }
                OriginMaterial::PaymentAccount { account, key } => {
                    writer.u8(4)?;
                    let encoded = key.export().map_err(|_| WalletBackupError::Resources)?;
                    let mut payload = Zeroizing::new(account.to_be_bytes().to_vec());
                    payload.extend_from_slice(encoded.expose().as_bytes());
                    writer.short(&payload)?;
                }
            }
        }
        writer.u16(self.state.scopes.len() as u16)?;
        for scope in &self.state.scopes {
            writer.put(&scope.scope.key())?;
            writer.u32(scope.addresses.len() as u32)?;
            for address in &scope.addresses {
                writer.put(address.address().bytes())?;
                writer.put(address.public_key())?;
                match address.origin() {
                    KeyOrigin::ImportedPublic => writer.u8(0)?,
                    KeyOrigin::Bip44 {
                        coin,
                        account,
                        change,
                        index,
                    } => {
                        writer.u8(1)?;
                        writer.u16(coin.number() as u16)?;
                        writer.u32(account)?;
                        writer.u8(u8::from(change))?;
                        writer.u32(index)?;
                    }
                }
            }
            writer.u32(scope.derivation.len() as u32)?;
            for cursor in &scope.derivation {
                writer.u16(cursor.coin.number() as u16)?;
                writer.u32(cursor.account)?;
                writer.u8(u8::from(cursor.change))?;
                writer.short(cursor.xpub.as_bytes())?;
                writer.u32(cursor.next_index)?;
            }
            writer.u32(scope.nonces.len() as u32)?;
            for nonce in &scope.nonces {
                writer.put(nonce.address.bytes())?;
                writer.u64(nonce.next_nonce)?;
            }
            writer.u32(scope.operations.len() as u32)?;
            for operation in &scope.operations {
                writer.put(&operation.record.id.0)?;
                writer.u8(operation.kind)?;
                writer.u8(operation.record.state as u8)?;
                match operation.record.transaction {
                    Some(hash) => {
                        writer.u8(1)?;
                        writer.put(hash.bytes())?;
                    }
                    None => writer.u8(0)?,
                }
                match operation.record.inclusion {
                    Some(block) => {
                        writer.u8(1)?;
                        writer.put(block.hash.bytes())?;
                        writer.put(&block.height.to_be_bytes::<32>())?;
                    }
                    None => writer.u8(0)?,
                }
                writer.u16(operation.qi.len() as u16)?;
                for (outpoint, address) in &operation.qi {
                    writer.put(outpoint.transaction_hash.bytes())?;
                    writer.u16(outpoint.index)?;
                    writer.put(address.bytes())?;
                }
                match operation.nonce {
                    Some((address, nonce)) => {
                        writer.u8(1)?;
                        writer.put(address.bytes())?;
                        writer.u64(nonce)?;
                    }
                    None => writer.u8(0)?,
                }
                let payload = operation.payload.as_deref().unwrap_or(&[]);
                writer.u32(payload.len() as u32)?;
                writer.put(payload)?;
                if self.version() >= 4 {
                    writer.u8(operation.replacements.len() as u8)?;
                    for variant in &operation.replacements {
                        writer.put(variant.parent.bytes())?;
                        writer.u32(variant.payload.len() as u32)?;
                        writer.put(&variant.payload)?;
                    }
                }
            }
        }
        self.encode_payments(&mut writer)?;
        Ok(writer.0)
    }
    #[cfg(test)]
    fn decode(plaintext: &[u8]) -> Result<Self> {
        Self::decode_version(plaintext, 1)
    }
    fn decode_version(plaintext: &[u8], version: u8) -> Result<Self> {
        let mut reader = Reader(plaintext, MAX_RECORDS);
        if reader.u32()? != u32::from(version >= 2) || !(1..=5).contains(&version) {
            return Err(WalletBackupError::Unsupported);
        }
        let count = usize::from(reader.u16()?);
        if count == 0 || count > MAX_ORIGINS {
            return Err(WalletBackupError::InvalidInput);
        }
        let mut origins = Vec::new();
        for _ in 0..count {
            let kind = reader.u8()?;
            let bytes = reader.short(116)?;
            origins.push(match kind {
                1 => BackupOrigin::from_seed(bytes)?,
                2 => BackupOrigin::from_master_xprv(
                    std::str::from_utf8(bytes).map_err(|_| WalletBackupError::InvalidInput)?,
                )?,
                3 => {
                    let raw = Zeroizing::new(fixed::<32>(bytes)?);
                    let key =
                        SecretKey::from_bytes(&raw).map_err(|_| WalletBackupError::InvalidInput)?;
                    BackupOrigin::from_private_key(&key)
                }
                4 if version >= 3 && bytes.len() >= 4 => BackupOrigin::from_payment_account_xprv(
                    std::str::from_utf8(&bytes[4..])
                        .map_err(|_| WalletBackupError::InvalidInput)?,
                    u32::from_be_bytes(fixed(&bytes[..4])?),
                )?,
                _ => return Err(WalletBackupError::Unsupported),
            });
        }
        let count = usize::from(reader.u16()?);
        if count == 0 || count > 64 {
            return Err(WalletBackupError::InvalidInput);
        }
        let mut scopes = Vec::new();
        for _ in 0..count {
            let chain_id = U256::from_be_bytes(reader.array::<32>()?);
            let genesis = Hash32::from_bytes(reader.array()?);
            let zone =
                Zone::from_byte(reader.u8()?).map_err(|_| WalletBackupError::InvalidInput)?;
            let mut scope = ScopeState {
                scope: NetworkScope {
                    chain_id,
                    genesis,
                    zone,
                },
                addresses: vec![],
                derivation: vec![],
                nonces: vec![],
                operations: vec![],
            };
            let count = reader.count(54)?;
            for _ in 0..count {
                let address = Address::from_bytes(reader.array()?);
                let public_key = reader.array()?;
                let origin = match reader.u8()? {
                    0 => KeyOrigin::ImportedPublic,
                    1 => KeyOrigin::Bip44 {
                        coin: coin_type(reader.u16()?)?,
                        account: reader.u32()?,
                        change: reader.boolean()?,
                        index: reader.u32()?,
                    },
                    _ => return Err(WalletBackupError::Unsupported),
                };
                scope.addresses.push(PublicAddress::from_backup_parts(
                    address, public_key, origin,
                )?);
            }
            let count = reader.count(13)?;
            for _ in 0..count {
                let coin = coin_type(reader.u16()?)?;
                let account = reader.u32()?;
                let change = reader.boolean()?;
                let xpub = std::str::from_utf8(reader.short(112)?)
                    .map_err(|_| WalletBackupError::InvalidInput)?;
                AccountPublic::import(xpub, coin, account)
                    .map_err(|_| WalletBackupError::InvalidInput)?;
                scope.derivation.push(DerivationState {
                    coin,
                    account,
                    change,
                    xpub: xpub.to_owned(),
                    next_index: reader.u32()?,
                });
            }
            let count = reader.count(28)?;
            for _ in 0..count {
                scope.nonces.push(NonceState {
                    address: QuaiAddress::try_from(reader.array::<20>()?)
                        .map_err(|_| WalletBackupError::InvalidInput)?,
                    next_nonce: reader.u64()?,
                });
            }
            let count = reader.count(26)?;
            for _ in 0..count {
                let id = ReservationId(reader.array()?);
                let kind = reader.u8()?;
                let state = match reader.u8()? {
                    0 => ReservationState::Reserved,
                    1 => ReservationState::Signed,
                    2 => ReservationState::Submitted,
                    3 => ReservationState::Confirmed,
                    4 => ReservationState::Released,
                    _ => return Err(WalletBackupError::InvalidInput),
                };
                let transaction = if reader.boolean()? {
                    Some(Hash32::from_bytes(reader.array()?))
                } else {
                    None
                };
                let inclusion = if reader.boolean()? {
                    Some(Checkpoint {
                        hash: Hash32::from_bytes(reader.array()?),
                        height: U256::from_be_bytes(reader.array::<32>()?),
                    })
                } else {
                    None
                };
                let count = usize::from(reader.u16()?);
                if count > 4096 || count > reader.0.len() / 54 || count > reader.1 {
                    return Err(WalletBackupError::InvalidInput);
                }
                reader.1 -= count;
                let mut qi = Vec::new();
                for _ in 0..count {
                    qi.push((
                        OutPoint {
                            transaction_hash: Hash32::from_bytes(reader.array()?),
                            index: reader.u16()?,
                        },
                        Address::from_bytes(reader.array()?),
                    ));
                }
                let nonce = if reader.boolean()? {
                    Some((
                        QuaiAddress::try_from(reader.array::<20>()?)
                            .map_err(|_| WalletBackupError::InvalidInput)?,
                        reader.u64()?,
                    ))
                } else {
                    None
                };
                let length = reader.u32()? as usize;
                if length > MAX_TRANSACTION_BYTES {
                    return Err(WalletBackupError::InvalidInput);
                }
                let payload = if length > 0 {
                    Some(reader.take(length)?.to_vec())
                } else {
                    None
                };
                let mut replacements = Vec::new();
                if version >= 4 {
                    let count = reader.u8()? as usize;
                    if count > 32 {
                        return Err(WalletBackupError::InvalidInput);
                    }
                    for _ in 0..count {
                        let parent = Hash32::from_bytes(reader.array()?);
                        let length = reader.u32()? as usize;
                        if length == 0 || length > MAX_TRANSACTION_BYTES {
                            return Err(WalletBackupError::InvalidInput);
                        }
                        replacements.push(crate::state::QuaiReplacement {
                            parent,
                            payload: reader.take(length)?.to_vec(),
                        });
                    }
                }
                if version < 5 && kind == 0 && !replacements.is_empty() {
                    return Err(WalletBackupError::Unsupported);
                }
                scope.operations.push(OperationState {
                    record: Reservation {
                        id,
                        state,
                        transaction,
                        inclusion,
                    },
                    kind,
                    qi,
                    nonce,
                    payload,
                    replacements,
                });
            }
            scopes.push(scope);
        }
        let (channels, exposures) = payment::decode_payments(&mut reader, version)?;
        if !reader.0.is_empty() {
            return Err(WalletBackupError::Unsupported);
        }
        let backup = Self {
            origins,
            state: PublicWalletState {
                scopes,
                channels,
                exposures,
            },
        };
        backup.validate()?;
        Ok(backup)
    }
}
/// Authenticated wallet ciphertext, safe for caller-managed persistence.
#[derive(Clone)]
pub struct EncryptedWalletBackup(Vec<u8>);
impl fmt::Debug for EncryptedWalletBackup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EncryptedWalletBackup([REDACTED])")
    }
}
impl EncryptedWalletBackup {
    /// Structural/KDF/length validation before copying an untrusted envelope.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        header(bytes)?;
        Ok(Self(bytes.to_vec()))
    }
    /// Explicit ciphertext bytes for persistence.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    /// Authenticate, decode bounded plaintext and verify secret/public ownership.
    /// Wrong passwords and all malformed decrypted content share one unlock error.
    pub fn decrypt(&self, password: &[u8]) -> Result<WalletBackup> {
        password_valid(password).map_err(|_| WalletBackupError::UnlockFailed)?;
        let (kdf, length) = header(&self.0)?;
        let salt = fixed::<16>(&self.0[24..40])?;
        let nonce = fixed::<24>(&self.0[40..64])?;
        let key = crate::backup::derive_key(password, &salt, kdf)
            .map_err(|_| WalletBackupError::Resources)?;
        let cipher = XChaCha20Poly1305::new_from_slice(&key[..])
            .map_err(|_| WalletBackupError::Resources)?;
        let mut plaintext = Zeroizing::new(self.0[HEADER..HEADER + length].to_vec());
        let tag = Tag::from(fixed::<16>(&self.0[HEADER + length..])?);
        cipher
            .decrypt_inout_detached(
                &XNonce::from(nonce),
                &self.0[..HEADER],
                (&mut plaintext[..]).into(),
                &tag,
            )
            .map_err(|_| WalletBackupError::UnlockFailed)?;
        WalletBackup::decode_version(&plaintext, self.0[8])
            .map_err(|_| WalletBackupError::UnlockFailed)
    }
}
fn password_valid(password: &[u8]) -> Result<()> {
    if password.is_empty() || password.len() > 1024 {
        Err(WalletBackupError::InvalidInput)
    } else {
        Ok(())
    }
}
fn header(bytes: &[u8]) -> Result<(BackupKdf, usize)> {
    if bytes.len() < HEADER + 16
        || bytes.len() > HEADER + MAX_PLAINTEXT + 16
        || &bytes[..8] != MAGIC
        || !(1..=5).contains(&bytes[8])
        || bytes[9..12] != [1, 1, 0]
    {
        return Err(WalletBackupError::UnlockFailed);
    }
    let value = |index| fixed::<4>(&bytes[index..index + 4]).map(u32::from_be_bytes);
    let kdf = BackupKdf::new(value(12)?, value(16)?, value(20)?)
        .map_err(|_| WalletBackupError::UnlockFailed)?;
    let length = value(64)? as usize;
    if length == 0 || length > MAX_PLAINTEXT || HEADER + length + 16 != bytes.len() {
        return Err(WalletBackupError::UnlockFailed);
    }
    Ok((kdf, length))
}
fn fixed<const N: usize>(bytes: &[u8]) -> Result<[u8; N]> {
    bytes
        .try_into()
        .map_err(|_| WalletBackupError::InvalidInput)
}
fn coin_type(value: u16) -> Result<CoinType> {
    match value {
        994 => Ok(CoinType::Quai),
        969 => Ok(CoinType::Qi),
        _ => Err(WalletBackupError::Unsupported),
    }
}
struct Writer(Zeroizing<Vec<u8>>);
impl Writer {
    fn new() -> Result<Self> {
        let mut bytes = Zeroizing::new(Vec::new());
        bytes
            .try_reserve_exact(MAX_PLAINTEXT)
            .map_err(|_| WalletBackupError::Resources)?;
        Ok(Self(bytes))
    }
    fn put(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.len() > MAX_PLAINTEXT - self.0.len() {
            return Err(WalletBackupError::InvalidInput);
        }
        self.0.extend_from_slice(bytes);
        Ok(())
    }
    fn u8(&mut self, value: u8) -> Result<()> {
        self.put(&[value])
    }
    fn u16(&mut self, value: u16) -> Result<()> {
        self.put(&value.to_be_bytes())
    }
    fn u32(&mut self, value: u32) -> Result<()> {
        self.put(&value.to_be_bytes())
    }
    fn u64(&mut self, value: u64) -> Result<()> {
        self.put(&value.to_be_bytes())
    }
    fn short(&mut self, value: &[u8]) -> Result<()> {
        self.u16(u16::try_from(value.len()).map_err(|_| WalletBackupError::InvalidInput)?)?;
        self.put(value)
    }
}
struct Reader<'a>(&'a [u8], usize);
impl<'a> Reader<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        if length > self.0.len() {
            return Err(WalletBackupError::InvalidInput);
        }
        let (head, tail) = self.0.split_at(length);
        self.0 = tail;
        Ok(head)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        fixed(self.take(N)?)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.array()?))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.array()?))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.array()?))
    }
    fn boolean(&mut self) -> Result<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(WalletBackupError::InvalidInput),
        }
    }
    fn short(&mut self, max: usize) -> Result<&'a [u8]> {
        let length = usize::from(self.u16()?);
        if length > max {
            return Err(WalletBackupError::InvalidInput);
        }
        self.take(length)
    }
    fn count(&mut self, min_size: usize) -> Result<usize> {
        let count = self.u32()? as usize;
        if count > self.1 || count > self.0.len() / min_size {
            return Err(WalletBackupError::InvalidInput);
        }
        self.1 -= count;
        Ok(count)
    }
}

#[cfg(all(test, feature = "sqlite", not(target_arch = "wasm32")))]
mod tests;
