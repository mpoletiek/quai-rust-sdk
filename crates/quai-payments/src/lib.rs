//! BIP47 payment-code and local channel foundations for Quai.
//!
//! Payment derivation uses the pinned Quai coin-969 account path. Notification
//! transaction construction, network discovery and automatic sends are separate.
mod channel;
mod code;
pub use channel::{
    CHANNEL_SCHEME, MAX_CHANNEL_BYTES, MAX_SEARCH_ATTEMPTS, PaymentChannel, PaymentDirection,
    PaymentSearch, PaymentSearchResult,
};
pub use code::{
    HARDENED_INDEX, PAYMENT_CODE_PREFIX, PaymentCode, PrivatePaymentCode, QUAI_PAYMENT_COIN,
};

/// Payment errors omit secret inputs and backend diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PaymentError {
    /// Seed must contain 16 through 64 bytes.
    #[error("invalid payment seed length")]
    InvalidSeed,
    /// Invalid code length, prefix, checksum, version, features, reserved bytes or point.
    #[error("invalid BIP47 payment code")]
    InvalidCode,
    /// Account/child index would be hardened or overflow the supported domain.
    #[error("invalid payment derivation index")]
    InvalidIndex,
    /// A BIP32 child, shared-secret scalar or additive key result is invalid.
    #[error("payment key derivation failed")]
    Derivation,
    /// Channel metadata is not owned by the supplied private account.
    #[error("payment channel owner mismatch")]
    OwnerMismatch,
    /// Unrecognized, duplicate or malformed channel metadata.
    #[error("invalid payment channel metadata")]
    InvalidChannel,
    /// Cancellation preserves the first unexamined candidate for safe resumption.
    #[error("payment search cancelled after {attempts} candidates")]
    SearchCancelled {
        /// Number of candidates already examined.
        attempts: u32,
        /// First unexamined candidate, or None if the index space is exhausted.
        next_index: Option<u32>,
    },
    /// Search budget or nonhardened index space was exhausted without a match.
    #[error("payment search exhausted after {attempts} candidates")]
    SearchExhausted {
        /// Number of candidates already examined.
        attempts: u32,
        /// First unexamined candidate, or None if the index space is exhausted.
        next_index: Option<u32>,
    },
    /// A local resource policy was exceeded.
    #[error("payment resource limit exceeded")]
    Limit,
}
