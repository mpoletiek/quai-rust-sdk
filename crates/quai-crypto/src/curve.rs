//! Validated secp256k1 arithmetic for protocol integrations.
//!
//! These primitives do not provide a distributed signing session, nonce protocol
//! or rogue-key protection. Use the protocol's specified key aggregation and
//! nonce rules. Scalar operations use k256; public field helpers use variable-time
//! integer arithmetic and must not be used with secrets.

use k256::elliptic_curve::{PrimeField, bigint::U256 as BackendUint, ops::Reduce};
use sha2::{Digest, Sha256};
use zeroize::{ZeroizeOnDrop, Zeroizing};

use crate::{CryptoError, PublicKey, SecretBytes, U256};

/// secp256k1 scalar group order (the reference SDK's `N`).
pub const CURVE_ORDER: U256 = U256::from_limbs([
    0xbfd25e8cd0364141,
    0xbaaedce6af48a03b,
    0xfffffffffffffffe,
    0xffffffffffffffff,
]);
/// secp256k1 coordinate field prime.
pub const FIELD_PRIME: U256 = U256::from_limbs([
    0xfffffffefffffc2f,
    0xffffffffffffffff,
    0xffffffffffffffff,
    0xffffffffffffffff,
]);

/// A scalar in `[0, n)`, including zero, with redacted diagnostics and guarded
/// storage. No implicit copies or serialization. Caller-owned inputs and backend
/// transient copies are outside the zeroization guarantee.
pub struct CurveScalar(Zeroizing<k256::Scalar>);

impl CurveScalar {
    /// Reject noncanonical values at or above the order; allow zero.
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self, CryptoError> {
        Option::<k256::Scalar>::from(k256::Scalar::from_repr((*bytes).into()))
            .map(|scalar| Self(Zeroizing::new(scalar)))
            .ok_or(CryptoError::InvalidScalar)
    }

    /// Reduce exactly 32 bytes modulo the order. This is not unbiased random
    /// secret-key generation; use SecretKey::generate for that purpose.
    pub fn reduce_bytes(bytes: &[u8; 32]) -> Self {
        Self(Zeroizing::new(
            <k256::Scalar as Reduce<BackendUint>>::reduce_bytes(&(*bytes).into()),
        ))
    }

    /// Explicit guarded big-endian export.
    pub fn export_bytes(&self) -> SecretBytes {
        let bytes = Zeroizing::new(self.0.to_bytes());
        SecretBytes::new(Zeroizing::new((*bytes).into()))
    }

    /// Test whether this scalar is zero. Reveals this one bit to the caller.
    pub fn is_zero(&self) -> bool {
        bool::from(self.0.is_zero())
    }
    /// Add modulo the group order.
    pub fn add(&self, other: &Self) -> Self {
        Self(Zeroizing::new(*self.0 + *other.0))
    }
    /// Multiply modulo the group order.
    pub fn multiply(&self, other: &Self) -> Self {
        Self(Zeroizing::new(*self.0 * *other.0))
    }
    /// Negate modulo the group order; zero stays zero.
    pub fn negate(&self) -> Self {
        Self(Zeroizing::new(-*self.0))
    }
}
impl core::fmt::Debug for CurveScalar {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("CurveScalar([REDACTED])")
    }
}
impl ZeroizeOnDrop for CurveScalar {}

impl PublicKey {
    /// Negate a validated point.
    pub fn negate(self) -> Self {
        Self(
            k256::PublicKey::from_affine(-*self.0.as_affine())
                .expect("negating a validated nonidentity point remains nonidentity"),
        )
    }
    /// Return the canonical X coordinate.
    pub fn x_coordinate(self) -> [u8; 32] {
        let mut x = [0; 32];
        x.copy_from_slice(&self.to_compressed()[1..]);
        x
    }
    /// Whether the canonical Y coordinate is even.
    pub fn has_even_y(self) -> bool {
        self.to_compressed()[0] == 2
    }
    /// Lift an X coordinate to its even-Y point, rejecting nonresidues and values
    /// outside the field. Unlike the reference's misnamed liftX this accepts X-only input.
    pub fn lift_x(x: &[u8; 32]) -> Result<Self, CryptoError> {
        let mut encoded = [0; 33];
        encoded[0] = 2;
        encoded[1..].copy_from_slice(x);
        Self::from_sec1_bytes(&encoded)
    }
    /// Multiply with the backend's constant-time scalar multiplication. Reject
    /// the point at infinity (including multiplication by zero).
    pub fn multiply(&self, scalar: &CurveScalar) -> Result<Self, CryptoError> {
        let point = Zeroizing::new(self.0.to_projective() * *scalar.0);
        k256::PublicKey::from_affine(point.to_affine())
            .map(Self)
            .map_err(|_| CryptoError::InvalidPublicKey)
    }
    /// Compute `self * scalar + other`, rejecting infinity. Zero is permitted and
    /// yields other. Uses ordinary backend operations, not the source's unsafe API.
    pub fn multiply_add(&self, scalar: &CurveScalar, other: &Self) -> Result<Self, CryptoError> {
        let product = Zeroizing::new(self.0.to_projective() * *scalar.0);
        let point = Zeroizing::new(*product + other.0.to_projective());
        k256::PublicKey::from_affine(point.to_affine())
            .map(Self)
            .map_err(|_| CryptoError::InvalidPublicKey)
    }
}

/// Compute `x³ + 7 mod p` for a public coordinate, reducing any U256 input.
/// Variable time; never use this helper for secret arithmetic.
pub fn public_curve_rhs(x: U256) -> U256 {
    x.mul_mod(x, FIELD_PRIME)
        .mul_mod(x, FIELD_PRIME)
        .add_mod(U256::from(7), FIELD_PRIME)
}

/// Quadratic residue symbol modulo the prime: -1, 0 or 1. Inputs are reduced;
/// multiples of the prime return zero without the reference's zero-loop hazard.
/// Uses a bounded 256-bit exponentiation; variable time, public inputs only.
pub fn public_jacobi_symbol(value: U256) -> i8 {
    let value = value % FIELD_PRIME;
    if value == U256::ZERO {
        return 0;
    }
    let exponent = (FIELD_PRIME - U256::from(1)) >> 1;
    if value.pow_mod(exponent, FIELD_PRIME) == U256::from(1) {
        1
    } else {
        -1
    }
}

/// Maximum parts in one multipart hash, including empty parts.
pub const MAX_HASH_PARTS: usize = 1024;
/// Maximum combined message bytes in one multipart hash.
pub const MAX_HASH_BYTES: usize = 1024 * 1024;

fn check_parts(parts: &[&[u8]]) -> Result<(), CryptoError> {
    if parts.len() > MAX_HASH_PARTS {
        return Err(CryptoError::HashLimit);
    }
    let mut total = 0usize;
    for part in parts {
        total = total
            .checked_add(part.len())
            .ok_or(CryptoError::HashLimit)?;
        if total > MAX_HASH_BYTES {
            return Err(CryptoError::HashLimit);
        }
    }
    Ok(())
}

/// Hash concatenated parts without allocating their concatenation.
pub fn sha256_parts(parts: &[&[u8]]) -> Result<[u8; 32], CryptoError> {
    check_parts(parts)?;
    let mut hash = Sha256::new();
    for part in parts {
        hash.update(part);
    }
    Ok(hash.finalize().into())
}

/// BIP340 tagged hash: SHA256(SHA256(tag) || SHA256(tag) || messages).
/// Tag is exact UTF-8 (no normalization), at most 1024 bytes. No global tag cache.
pub fn tagged_sha256(tag: &str, parts: &[&[u8]]) -> Result<[u8; 32], CryptoError> {
    tagged_hash_bytes(tag.as_bytes(), parts)
}

/// Reproduce the pinned quais.js adapter's tag encoding: one truncated first
/// UTF-16 code unit per Unicode code point. This differs from UTF-8 for non-ASCII
/// tags; use only for interoperability with an existing protocol using that
/// adapter. Standard ASCII BIP340/MuSig tags produce the same hashes as
/// tagged_sha256. The original UTF-8 tag is limited to 1024 bytes.
pub fn quais_tagged_sha256(tag: &str, parts: &[&[u8]]) -> Result<[u8; 32], CryptoError> {
    if tag.len() > 1024 {
        return Err(CryptoError::HashLimit);
    }
    let bytes: Vec<u8> = tag
        .chars()
        .map(|c| {
            let code = u32::from(c);
            let first = if code >= 0x10000 {
                0xd800 + ((code - 0x10000) >> 10)
            } else {
                code
            };
            (first & 0xff) as u8
        })
        .collect();
    tagged_hash_bytes(&bytes, parts)
}

fn tagged_hash_bytes(tag: &[u8], parts: &[&[u8]]) -> Result<[u8; 32], CryptoError> {
    if tag.len() > 1024 {
        return Err(CryptoError::HashLimit);
    }
    check_parts(parts)?;
    let tag_hash = Sha256::digest(tag);
    let mut hash = Sha256::new();
    hash.update(tag_hash);
    hash.update(tag_hash);
    for part in parts {
        hash.update(part);
    }
    Ok(hash.finalize().into())
}
