//! BIP39/BIP32 derivation, encrypted seed backups, selection and optional native storage.
//! Bounded watch-only discovery relies on explicitly tagged node observations.
//! No verified chain-proof recovery or complete reorg engine is implemented.
//!
//! Secret-owning wrappers redact diagnostics and do not implement `Clone`,
//! `Display`, or generic serialization. Secret exports require explicit access.
//! Zeroization is best effort; it cannot erase caller copies or guarantee that
//! cryptographic dependencies/compiler temporaries never retain copies.

mod backup;
pub mod discovery;
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
pub mod full_backup;
mod hd;
pub mod metadata;
pub mod qi_keys;
mod selection;
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
pub mod storage;
pub use selection::{
    CandidateCoin, CoinSelection, SelectionError, SelectionRequest, SweepMode,
    preserves_denominations, select_fewest, select_sweep, select_with_fee,
};
mod mnemonic;

pub use backup::{BackupError, BackupKdf, EncryptedSeedBackup, SeedBackup};

pub use hd::{
    AccountPublic, CoinType, DerivedAddress, ExtendedKeyMetadata, ExtendedPrivateKey,
    ExtendedPublicKey, HdWallet, Search, SearchResult,
};
pub use mnemonic::{Language, Mnemonic, MnemonicEntropy, SecretString, Seed};
use thiserror::Error;

/// Wallet errors never echo phrases, passphrases, seeds or extended private keys.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum WalletError {
    /// Invalid word count, word, language or checksum.
    #[error("invalid mnemonic")]
    InvalidMnemonic,
    /// Entropy must contain 128–256 bits in 32-bit increments.
    #[error("invalid mnemonic entropy length")]
    InvalidEntropy,
    /// Operating-system entropy was unavailable; no fallback randomness is used.
    #[error("secure operating-system randomness unavailable")]
    RandomnessUnavailable,
    /// BIP32 seed length must be 16 through 64 bytes.
    #[error("invalid BIP32 seed length")]
    InvalidSeed,
    /// Invalid extended-key checksum, prefix, metadata or curve point/scalar.
    #[error("invalid extended key")]
    InvalidExtendedKey,
    /// Account and child indexes must be below 2^31 before hardening.
    #[error("index must be below 2^31")]
    InvalidIndex,
    /// Hardened public derivation is impossible.
    #[error("cannot derive a hardened child from an extended public key")]
    HardenedPublicChild,
    /// Path must be valid and absolute derivation must begin at a master node.
    #[error("invalid derivation path or non-master starting node")]
    InvalidPath,
    /// Cryptographic derivation failed or the maximum depth was reached.
    #[error("HD key derivation failed")]
    Derivation,
    /// Imported account xpub metadata does not match the declared origin.
    #[error("account xpub must have depth three and the declared hardened account index")]
    InvalidAccountOrigin,
    /// Exact-index derivation does not select the required ledger and a known zone.
    #[error("derived address does not match the wallet ledger or a known zone")]
    InvalidDerivedAddress,
    /// A search must specify 1 through 10,000,000 attempts.
    #[error("invalid address search limit")]
    InvalidSearchLimit,
    /// Search stopped before deriving the next candidate.
    #[error("address search cancelled after {attempts} attempts")]
    Cancelled {
        /// Number of candidates already examined.
        attempts: u32,
        /// Candidate index at which a later search can resume.
        next_index: u32,
    },
    /// All permitted candidates were examined without a match.
    #[error("address search exhausted after {attempts} attempts")]
    SearchExhausted {
        /// Number of candidates examined.
        attempts: u32,
        /// Next nonhardened candidate; None means the index space ended.
        next_index: Option<u32>,
    },
}
