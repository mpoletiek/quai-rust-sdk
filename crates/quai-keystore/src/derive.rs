//! Standalone bounded PBKDF2/scrypt utilities with guarded derived output.
use crate::{Kdf, KdfLimits, KeystoreError};
use std::fmt;
use zeroize::Zeroizing;
/// Hash selected for PBKDF2-HMAC. No implicit password normalization is performed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pbkdf2Hash {
    /// SHA-256, 32-byte output blocks.
    Sha256,
    /// SHA-512, 64-byte output blocks.
    Sha512,
}
/// Explicit standalone key derivation parameters, independent of keystore format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeriveParams {
    /// Scrypt with N = 2^log_n and explicit block/lane costs.
    Scrypt {
        /// Base-two cost exponent.
        log_n: u8,
        /// Block size parameter.
        r: u32,
        /// Parallelization/lane count.
        p: u32,
    },
    /// PBKDF2 with a positive per-output-block iteration count.
    Pbkdf2 {
        /// Iterations per block.
        rounds: u32,
        /// HMAC hash choice.
        hash: Pbkdf2Hash,
    },
}
/// Resource policy, including the output-length multiplier for PBKDF2 work.
#[derive(Clone, Copy, Debug)]
pub struct DeriveLimits {
    /// Existing bounded KDF memory/parameter policy.
    pub kdf: KdfLimits,
    /// Maximum derived bytes, at most 1024; default 64.
    pub max_output_bytes: usize,
    /// Maximum PBKDF2 rounds times output blocks; hard ceiling 2^28.
    pub max_pbkdf2_work: u64,
}
impl Default for DeriveLimits {
    fn default() -> Self {
        Self {
            kdf: KdfLimits::default(),
            max_output_bytes: 64,
            max_pbkdf2_work: 1 << 24,
        }
    }
}
/// Fixed diagnostics never retain the password, salt or derived key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DeriveError {
    /// Invalid algorithm parameters or zero output length.
    #[error("invalid key derivation parameters")]
    Invalid,
    /// Password/salt/output/work/memory bound exceeded before work.
    #[error("key derivation limit exceeded")]
    Limit,
    /// Caller declined the started/completed lifecycle checkpoint.
    #[error("key derivation cancelled at checkpoint")]
    Cancelled,
}
/// Coarse lifecycle progress, suitable for a caller-owned worker notification.
/// RustCrypto's inner KDF loop does not expose progress or interruption hooks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeriveProgress {
    /// Parameters passed validation; expensive work has not begun.
    Started,
    /// Output is ready but has not been released to the caller.
    Completed,
}
impl DeriveProgress {
    /// Fraction 0 or 1. No estimated intermediate percentage is fabricated.
    pub fn fraction(self) -> f64 {
        match self {
            Self::Started => 0.0,
            Self::Completed => 1.0,
        }
    }
}
/// Owned derived bytes, zeroized on drop and redacted in diagnostics. No Clone or
/// general serialization exposes secrets; explicit borrowing remains possible.
pub struct DerivedKey(Zeroizing<Vec<u8>>);
impl fmt::Debug for DerivedKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DerivedKey([REDACTED])")
    }
}
impl DerivedKey {
    /// Explicit borrowed access. Caller copies and input buffers are its responsibility.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}
impl DeriveParams {
    fn kdf(self) -> Kdf {
        match self {
            Self::Scrypt { log_n, r, p } => Kdf::Scrypt { log_n, r, p },
            Self::Pbkdf2 { rounds, hash } => Kdf::Pbkdf2 {
                rounds,
                sha512: hash == Pbkdf2Hash::Sha512,
            },
        }
    }
    /// Validate parameters/output budgets without inspecting or deriving a password.
    pub fn validate(self, length: usize, limits: DeriveLimits) -> Result<(), DeriveError> {
        if length == 0 || matches!(self, Self::Scrypt { log_n: 0, .. }) {
            return Err(DeriveError::Invalid);
        }
        if let Self::Scrypt { log_n, r, .. } = self
            && u64::from(log_n) >= u64::from(r) * 16
        {
            return Err(DeriveError::Invalid);
        }
        if limits.max_output_bytes == 0
            || limits.max_output_bytes > 1024
            || length > limits.max_output_bytes
            || limits.max_pbkdf2_work == 0
            || limits.max_pbkdf2_work > 1 << 28
        {
            return Err(DeriveError::Limit);
        }
        self.kdf().validate(limits.kdf).map_err(|e| {
            if e == KeystoreError::Limit {
                DeriveError::Limit
            } else {
                DeriveError::Invalid
            }
        })?;
        if let Self::Pbkdf2 { rounds, hash } = self {
            let width = if hash == Pbkdf2Hash::Sha512 { 64 } else { 32 };
            let work = u64::from(rounds) * (length.div_ceil(width) as u64);
            if work > limits.max_pbkdf2_work {
                return Err(DeriveError::Limit);
            }
        }
        Ok(())
    }
}
/// Derive exact raw password/salt bytes, each at most 1024 bytes. Empty inputs are
/// permitted for interoperability. This is synchronous CPU work; execute on a
/// bounded native blocking worker or dedicated browser worker, not a UI/event loop.
/// Scrypt's upstream scratch workspace wiping limitation remains unchanged.
pub fn derive_key(
    password: &[u8],
    salt: &[u8],
    params: DeriveParams,
    length: usize,
    limits: DeriveLimits,
) -> Result<DerivedKey, DeriveError> {
    derive_key_with_progress(password, salt, params, length, limits, |_| true)
}
/// Derive with before/after lifecycle callbacks. False cancels at that checkpoint;
/// completed cancellation drops/zeroizes the output. No mid-loop cancellation or
/// fine-grained progress is claimed, and callbacks must not block a UI thread.
pub fn derive_key_with_progress(
    password: &[u8],
    salt: &[u8],
    params: DeriveParams,
    length: usize,
    limits: DeriveLimits,
    mut progress: impl FnMut(DeriveProgress) -> bool,
) -> Result<DerivedKey, DeriveError> {
    if password.len() > 1024 || salt.len() > 1024 {
        return Err(DeriveError::Limit);
    }
    params.validate(length, limits)?;
    if !progress(DeriveProgress::Started) {
        return Err(DeriveError::Cancelled);
    }
    let mut output = Zeroizing::new(vec![0; length]);
    match params {
        DeriveParams::Scrypt { log_n, r, p } => scrypt::scrypt(
            password,
            salt,
            &scrypt::Params::new(log_n, r, p).map_err(|_| DeriveError::Invalid)?,
            &mut output,
        )
        .map_err(|_| DeriveError::Invalid)?,
        DeriveParams::Pbkdf2 {
            rounds,
            hash: Pbkdf2Hash::Sha256,
        } => pbkdf2::pbkdf2_hmac::<pbkdf2::sha2::Sha256>(password, salt, rounds, &mut output),
        DeriveParams::Pbkdf2 {
            rounds,
            hash: Pbkdf2Hash::Sha512,
        } => pbkdf2::pbkdf2_hmac::<pbkdf2::sha2::Sha512>(password, salt, rounds, &mut output),
    }
    if !progress(DeriveProgress::Completed) {
        return Err(DeriveError::Cancelled);
    }
    Ok(DerivedKey(output))
}
