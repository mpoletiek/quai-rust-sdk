//! Explicit local Qi key resolution across HD, imported and payment origins.
use crate::metadata::{KeyOrigin, PublicAddress, StorageError};
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
use crate::storage::SqliteStore;
use crate::{CoinType, HdWallet};
use quai_crypto::SecretKey;
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
use quai_payments::PaymentDirection;
#[cfg(feature = "payments")]
use quai_payments::{PaymentCode, PrivatePaymentCode};
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
    /// native or browser public state. Conflicting/duplicate keys are rejected.
    pub fn import(&mut self, key: SecretKey) -> Result<PublicAddress, StorageError> {
        let address = PublicAddress::imported(&key.public_key())?;
        QiAddress::try_from(address.address()).map_err(|_| StorageError::Invalid)?;
        if self.imported.len() >= 4096 || self.imported.contains_key(&address.address()) {
            return Err(StorageError::Invalid);
        }
        self.imported.insert(address.address(), key);
        Ok(address)
    }
    /// Import one explicitly selected BIP47 receive key on native or browser targets.
    /// The caller must persist its channel exposure and burned range before exposing
    /// the address. This verifies key ownership; it does not allocate or save state.
    #[cfg(feature = "payments")]
    pub fn import_payment_receive(
        &mut self,
        owner: &PrivatePaymentCode,
        peer: &PaymentCode,
        index: u32,
    ) -> Result<PublicAddress, StorageError> {
        let key = owner
            .receive_key(peer, index)
            .map_err(|_| StorageError::Invalid)?;
        self.import(key)
    }
    /// Load registered receive exposures for a verified channel in this store's
    /// zone. Derive and verify every key before atomically extending the keyring.
    /// Send destinations are never imported as locally owned keys.
    #[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
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
    /// Load receive exposures for every channel registered to `owner` in this store,
    /// so coin selection cannot choose a channel output whose key was never loaded.
    /// Each channel is verified and loaded atomically; on error, channels loaded
    /// earlier remain (they are verified keys) and the rest are not loaded.
    #[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
    pub fn load_payment_channels(
        &mut self,
        store: &SqliteStore,
        owner: &PrivatePaymentCode,
    ) -> Result<usize, StorageError> {
        let mut count = 0;
        for channel in store.payment_channels(owner)? {
            count +=
                self.load_payment_channel(store, owner, channel.channel.counterparty_code())?;
        }
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
