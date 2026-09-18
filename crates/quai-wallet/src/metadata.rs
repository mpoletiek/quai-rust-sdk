//! Portable immutable public key origins, shared by native and browser wallet state.
use crate::{AccountPublic, CoinType};
use quai_crypto::PublicKey;
use quai_primitives::Address;
use thiserror::Error;
type Result<T> = std::result::Result<T, StorageError>;

/// Storage failures contain no SQL parameters or arbitrary database messages.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
#[non_exhaustive]
pub enum StorageError {
    /// SQLite I/O, locking, constraint or corruption failure; transaction rolled back.
    #[error("wallet database operation failed")]
    Database,
    /// Existing database belongs to another application or unsupported schema.
    #[error("foreign or unsupported wallet database")]
    Schema,
    /// Invalid public metadata, scope, bounds, or malformed stored record.
    #[error("invalid wallet storage data")]
    Invalid,
    /// Snapshot scope/generation is stale, or checkpoint rewinds without invalidation.
    #[error("stale or mismatched wallet snapshot")]
    StaleSnapshot,
    /// Immutable metadata, reservation ID or spend claim already exists.
    #[error("wallet metadata or reservation conflict")]
    Conflict,
    /// Bounded derivation allocation was cancelled; burned indexes remain consumed.
    #[error("fresh address allocation cancelled")]
    Cancelled,
    /// No matching address in the burned range; indexes remain consumed.
    #[error("fresh address allocation range exhausted")]
    DerivationExhausted,
    /// Requested transition could permit unsafe reuse or discard broadcast ambiguity.
    #[error("reservation transition is not permitted")]
    Transition,
    /// Generation or account nonce cannot increment without overflow.
    #[error("wallet storage counter exhausted")]
    Overflow,
}
/// Public origin; BIP44 ancestry is derived from a caller-trusted account xpub.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyOrigin {
    /// Public key imported without an HD ancestry claim.
    ImportedPublic,
    /// Exact BIP44 child, including indexes skipped during zone grinding.
    Bip44 {
        /// Coin type.
        coin: CoinType,
        /// Unhardened account number.
        account: u32,
        /// Internal/change branch.
        change: bool,
        /// Actual unhardened child index.
        index: u32,
    },
}
impl KeyOrigin {
    pub(crate) fn encode(self) -> Vec<u8> {
        match self {
            Self::ImportedPublic => vec![0],
            Self::Bip44 {
                coin,
                account,
                change,
                index,
            } => {
                let mut bytes = vec![1, u8::from(coin == CoinType::Qi), u8::from(change)];
                bytes.extend(account.to_be_bytes());
                bytes.extend(index.to_be_bytes());
                bytes
            }
        }
    }
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes == [0] {
            return Ok(Self::ImportedPublic);
        }
        if bytes.len() != 11 || bytes[0] != 1 || bytes[1] > 1 || bytes[2] > 1 {
            return Err(StorageError::Invalid);
        }
        let account =
            u32::from_be_bytes(bytes[3..7].try_into().map_err(|_| StorageError::Invalid)?);
        let index = u32::from_be_bytes(bytes[7..11].try_into().map_err(|_| StorageError::Invalid)?);
        if account >= 1 << 31 || index >= 1 << 31 {
            return Err(StorageError::Invalid);
        }
        Ok(Self::Bip44 {
            coin: if bytes[1] == 0 {
                CoinType::Quai
            } else {
                CoinType::Qi
            },
            account,
            change: bytes[2] == 1,
            index,
        })
    }
}
/// Immutable address/key-origin record. No seed, mnemonic, xprv or opaque payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicAddress {
    pub(crate) address: Address,
    pub(crate) public_key: [u8; 33],
    pub(crate) origin: KeyOrigin,
}
impl PublicAddress {
    pub(crate) fn from_backup_parts(
        address: Address,
        public_key: [u8; 33],
        origin: KeyOrigin,
    ) -> Result<Self> {
        let key = PublicKey::from_sec1_bytes(&public_key).map_err(|_| StorageError::Invalid)?;
        if key.address() != address || address.zone().is_err() {
            return Err(StorageError::Invalid);
        }
        if let KeyOrigin::Bip44 {
            coin,
            account,
            index,
            ..
        } = origin
            && (account >= 1 << 31 || index >= 1 << 31 || coin.ledger() != address.ledger())
        {
            return Err(StorageError::Invalid);
        }
        Ok(Self {
            address,
            public_key,
            origin,
        })
    }
    /// Record a validated public key without claiming HD ancestry.
    pub fn imported(key: &PublicKey) -> Result<Self> {
        let address = key.address();
        address.zone().map_err(|_| StorageError::Invalid)?;
        Ok(Self {
            address,
            public_key: key.to_compressed(),
            origin: KeyOrigin::ImportedPublic,
        })
    }
    /// Derive and record an exact nonhardened child from a trusted account xpub.
    pub fn derive(account: &AccountPublic, change: bool, index: u32) -> Result<Self> {
        let derived = account
            .derive_address(change, index)
            .map_err(|_| StorageError::Invalid)?;
        Ok(Self {
            address: derived.address,
            public_key: derived.public_key,
            origin: KeyOrigin::Bip44 {
                coin: derived.coin,
                account: derived.account,
                change,
                index,
            },
        })
    }
    /// Public address.
    pub fn address(&self) -> Address {
        self.address
    }
    /// Compressed SEC1 public key.
    pub fn public_key(&self) -> &[u8; 33] {
        &self.public_key
    }
    /// Public key ancestry metadata.
    pub fn origin(&self) -> KeyOrigin {
        self.origin
    }
}

impl PublicAddress {
    /// Export bounded public metadata, never a private key or proof of ancestry.
    /// The exact versioned encoding is 42 bytes for imported keys or 52 for BIP44.
    pub fn export_metadata(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(52);
        bytes.extend_from_slice(b"QADDR001");
        bytes.extend_from_slice(&self.public_key);
        bytes.extend_from_slice(&self.origin.encode());
        bytes
    }
    /// Validate exact canonical public metadata. HD ancestry remains caller asserted;
    /// a trusted account xpub or local resolver must prove ownership before use.
    pub fn from_metadata(bytes: &[u8]) -> Result<Self> {
        if !matches!(bytes.len(), 42 | 52) || &bytes[..8] != b"QADDR001" {
            return Err(StorageError::Invalid);
        }
        let public_key: [u8; 33] = bytes[8..41].try_into().map_err(|_| StorageError::Invalid)?;
        let key = PublicKey::from_sec1_bytes(&public_key).map_err(|_| StorageError::Invalid)?;
        let origin = KeyOrigin::decode(&bytes[41..])?;
        Self::from_backup_parts(key.address(), public_key, origin)
    }
}
