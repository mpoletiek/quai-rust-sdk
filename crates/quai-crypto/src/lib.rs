//! Cryptographic building blocks for Quai applications.
//!
//! This crate has no wallet storage or transaction wire-format assumptions.
//! Secret keys redact diagnostics and zeroize on drop. Imported input buffers
//! remain the caller's responsibility. Browser entropy uses Web Crypto and is
//! exercised in the SDK's actual Chromium worker tests.

mod aggregate;
pub mod curve;
mod entropy;
mod hash;
pub use entropy::{MAX_RANDOM_BYTES, fill_random};
mod keys;
mod secret_bytes;
mod signature_metadata;
mod signatures;
/// Exact unsigned integer used by legacy signature metadata helpers.
pub use ruint::aliases::U256;
pub use secret_bytes::SecretBytes;
pub use signature_metadata::{SignatureMetadata, legacy_chain_id, legacy_chain_v, normalized_v};

pub use aggregate::{MAX_AGGREGATE_KEYS, OrderedKeyAggregate};
pub use hash::{
    MESSAGE_PREFIX, hash_message, hmac_sha256, hmac_sha512, keccak256, ripemd160, sha256, sha512,
    verify_hmac_sha256, verify_hmac_sha512,
};
pub use keys::{PublicKey, SecretKey};
pub use signatures::{RecoverableSignature, SchnorrPublicKey, SchnorrSignature};

/// Recover the address signing exact personal-message bytes. Text callers pass
/// UTF-8 bytes; hex-looking text is not decoded. Recovery does not authenticate
/// an expected signer: compare the returned address with the authorized address.
pub fn recover_message_signer(
    message: &[u8],
    signature: &RecoverableSignature,
) -> Result<quai_primitives::Address, CryptoError> {
    signature
        .recover_prehash(&hash_message(message))
        .map(PublicKey::address)
}

/// Errors never include secret input or backend diagnostic strings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CryptoError {
    /// A scalar encoding is outside the secp256k1 group order.
    InvalidScalar,
    /// A multipart hash exceeds its explicit part, tag or byte limit.
    HashLimit,
    /// A secret scalar is zero or outside the secp256k1 group order.
    InvalidSecretKey,
    /// Ordered key aggregation requires two through the configured maximum keys.
    InvalidKeyCount,
    /// Public aggregation produced an invalid or infinite aggregate.
    InvalidKeyAggregation,
    /// Owned secret keys do not match the context's exact ordered public key list.
    InvalidSecretKeySet,
    /// A public key has invalid SEC1 encoding or is not a nonidentity curve point.
    InvalidPublicKey,
    /// A signature is malformed, noncanonical or has invalid scalar values.
    InvalidSignature,
    /// A signature has high S; the caller must not silently rewrite wire intent.
    HighS,
    /// The recovery identifier is not in the mathematical range zero through three.
    InvalidRecoveryId,
    /// The signature uses a recovery bit not representable in the Quai JS parity form.
    UnsupportedRecoveryId,
    /// Signature creation failed.
    SigningFailed,
    /// Signature verification or public-key recovery failed.
    VerificationFailed,
    /// The operating system could not provide usable cryptographic randomness.
    RandomnessUnavailable,
    /// Requested random output exceeds the explicit shared native/browser bound.
    RandomRequestTooLarge,
}

impl core::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::InvalidScalar => "invalid secp256k1 scalar",
            Self::HashLimit => "hash input exceeds resource limit",
            Self::InvalidSecretKey => "invalid secp256k1 secret key",
            Self::InvalidKeyCount => "invalid ordered aggregation key count",
            Self::InvalidKeyAggregation => "invalid ordered public key aggregate",
            Self::InvalidSecretKeySet => "secret keys do not match ordered aggregation context",
            Self::InvalidPublicKey => "invalid secp256k1 public key",
            Self::InvalidSignature => "invalid signature",
            Self::HighS => "noncanonical high-S signature",
            Self::InvalidRecoveryId => "invalid recovery identifier",
            Self::UnsupportedRecoveryId => {
                "recovery identifier cannot be represented by Quai parity"
            }
            Self::SigningFailed => "signature generation failed",
            Self::VerificationFailed => "signature verification failed",
            Self::RandomnessUnavailable => "cryptographic randomness unavailable",
            Self::RandomRequestTooLarge => "random output exceeds resource limit",
        })
    }
}
impl std::error::Error for CryptoError {}
