//! Explicit local Qi key resolution across HD, imported and payment origins.
use crate::storage::{KeyOrigin, PublicAddress, SqliteStore, StorageError};
use crate::{CoinType, HdWallet};
use quai_crypto::SecretKey;
use quai_payments::{PaymentCode, PaymentDirection, PrivatePaymentCode};
use quai_primitives::{Address, QiAddress};
use std::collections::BTreeMap;

/// Resolve a local signing key for exact public ownership metadata.
/// Callers must independently verify the returned public key before signing.
pub trait QiKeyResolver: Sync {
    /// Return a guarded key or fail without exposing secret material.
    fn resolve(&self, address: &PublicAddress) -> Result<SecretKey, StorageError>;
}

impl QiKeyResolver for HdWallet {
    fn resolve(&self, address: &PublicAddress) -> Result<SecretKey, StorageError> {
        let KeyOrigin::Bip44 {
            coin: CoinType::Qi,
            account,
            change,
            index,
        } = address.origin()
        else {
            return Err(StorageError::Invalid);
        };
        if self.coin_type() != CoinType::Qi {
            return Err(StorageError::Invalid);
        }
        let key = self
            .derive_key(account, change, index)
            .and_then(|key| key.secret_key())
            .map_err(|_| StorageError::Invalid)?;
        verify(address, key)
    }
}

fn verify(address: &PublicAddress, key: SecretKey) -> Result<SecretKey, StorageError> {
    if key.public_key().to_compressed() != *address.public_key()
        || key.public_key().address() != address.address()
        || QiAddress::try_from(address.address()).is_err()
    {
        return Err(StorageError::Invalid);
    }
    Ok(key)
}

/// Bounded, memory-only local keyring. Keys zeroize on drop and are never serialized.
/// Reconstruct it explicitly from authenticated backup material after restart.
/// Public imported metadata alone never grants signing authority.
pub struct QiKeyring<'a> {
    hd: Option<&'a HdWallet>,
    imported: BTreeMap<Address, SecretKey>,
}
impl<'a> QiKeyring<'a> {
    /// Create a keyring with an optional Qi HD origin; no network or storage mutation.
    pub fn new(hd: Option<&'a HdWallet>) -> Result<Self, StorageError> {
        if hd.is_some_and(|wallet| wallet.coin_type() != CoinType::Qi) {
            return Err(StorageError::Invalid);
        }
        Ok(Self {
            hd,
            imported: BTreeMap::new(),
        })
    }
    /// Take ownership of an explicitly imported key. Returns public metadata for
    /// `SqliteStore::import_metadata`. Conflicting/duplicate keys are rejected.
    pub fn import(&mut self, key: SecretKey) -> Result<PublicAddress, StorageError> {
        let address = PublicAddress::imported(&key.public_key())?;
        QiAddress::try_from(address.address()).map_err(|_| StorageError::Invalid)?;
        if self.imported.len() >= 4096 || self.imported.contains_key(&address.address()) {
            return Err(StorageError::Invalid);
        }
        self.imported.insert(address.address(), key);
        Ok(address)
    }
    /// Load registered receive exposures for a verified channel in this store's
    /// zone. Derive and verify every key before atomically extending the keyring.
    /// Send destinations are never imported as locally owned keys.
    pub fn load_payment_channel(
        &mut self,
        store: &SqliteStore,
        owner: &PrivatePaymentCode,
        peer: &PaymentCode,
    ) -> Result<usize, StorageError> {
        if store.payment_channel(owner, peer)?.is_none() {
            return Err(StorageError::Invalid);
        }
        let records = store.payment_addresses(owner, peer, PaymentDirection::Receive)?;
        let mut staged = BTreeMap::new();
        for record in records
            .iter()
            .filter(|record| record.zone == store.scope().zone)
        {
            if self.imported.contains_key(&record.address.address()) {
                continue;
            }
            if self.imported.len() + staged.len() >= 4096 {
                return Err(StorageError::Invalid);
            }
            let key = owner
                .receive_key(peer, record.index)
                .map_err(|_| StorageError::Invalid)?;
            if key.public_key() != record.public_key
                || key.public_key().address() != record.address.address()
            {
                return Err(StorageError::Invalid);
            }
            staged.insert(record.address.address(), key);
        }
        let count = staged.len();
        self.imported.extend(staged);
        Ok(count)
    }
}
impl QiKeyResolver for QiKeyring<'_> {
    fn resolve(&self, address: &PublicAddress) -> Result<SecretKey, StorageError> {
        match address.origin() {
            KeyOrigin::Bip44 { .. } => self.hd.ok_or(StorageError::Invalid)?.resolve(address),
            KeyOrigin::ImportedPublic => {
                let key = self
                    .imported
                    .get(&address.address())
                    .ok_or(StorageError::Invalid)?;
                let bytes = key.export_bytes();
                let key =
                    SecretKey::from_bytes(bytes.as_bytes()).map_err(|_| StorageError::Invalid)?;
                verify(address, key)
            }
        }
    }
}
