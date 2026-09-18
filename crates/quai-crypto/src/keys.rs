use core::fmt;
use k256::elliptic_curve::{
    bigint::U256,
    ops::{MulByGenerator, Reduce},
    sec1::ToEncodedPoint,
};
use quai_primitives::Address;
use zeroize::{ZeroizeOnDrop, Zeroizing};

use crate::{CryptoError, RecoverableSignature, SchnorrPublicKey, SchnorrSignature, keccak256};

/// A validated secret scalar, zeroized on drop and redacted in diagnostics.
///
/// This deliberately implements neither `Clone`, `Copy`, `Display` nor secret
/// serialization. The backend's stored scalar and temporary signing keys
/// zeroize on drop. This cannot erase prior caller-owned copies, registers,
/// compiler spills, crash dumps or copies inside every cryptographic operation.
pub struct SecretKey(k256::SecretKey);

impl SecretKey {
    /// Validate a 32-byte big-endian scalar. The caller retains its input buffer.
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self, CryptoError> {
        k256::SecretKey::from_slice(bytes)
            .map(Self)
            .map_err(|_| CryptoError::InvalidSecretKey)
    }

    /// Explicitly export a guarded copy for encrypted backups or key import.
    ///
    /// The returned buffer zeroizes on drop and redacts Debug. Explicitly borrowing
    /// its bytes exposes the secret; callers must avoid logs and unguarded copies. The key
    /// itself still has no implicit serialization, Display, Clone or Copy.
    pub fn export_bytes(&self) -> crate::SecretBytes {
        let bytes = Zeroizing::new(self.0.to_bytes());
        crate::SecretBytes::new(Zeroizing::new((*bytes).into()))
    }

    /// Generate a scalar from the OS CSPRNG, failing rather than degrading randomness.
    ///
    /// Rejection sampling is bounded to 128 attempts; invalid scalar samples are
    /// never reduced modulo the order. Browser entropy uses Web Crypto and is exercised in Chromium workers.
    pub fn generate() -> Result<Self, CryptoError> {
        let mut bytes = Zeroizing::new([0; 32]);
        for _ in 0..128 {
            getrandom::fill(bytes.as_mut()).map_err(|_| CryptoError::RandomnessUnavailable)?;
            if let Ok(key) = Self::from_bytes(&bytes) {
                return Ok(key);
            }
        }
        Err(CryptoError::RandomnessUnavailable)
    }

    pub(crate) fn guarded_scalar(&self) -> Zeroizing<k256::Scalar> {
        let scalar = Zeroizing::new(self.0.to_nonzero_scalar());
        Zeroizing::new(**scalar)
    }

    /// Compute the raw secp256k1 ECDH shared X coordinate into a zeroizing buffer.
    ///
    /// This is shared secret material, not an encryption key. The calling protocol
    /// must apply its specified KDF/hash and protect any copies it creates.
    pub fn ecdh_shared_x(&self, peer: &PublicKey) -> crate::SecretBytes {
        let scalar = Zeroizing::new(self.0.to_nonzero_scalar());
        let shared = k256::ecdh::diffie_hellman(&*scalar, peer.0.as_affine());
        crate::SecretBytes::new(Zeroizing::new((*shared.raw_secret_bytes()).into()))
    }

    /// Compute the full uncompressed 65-byte SEC1 ECDH shared point into a
    /// zeroizing buffer, matching the published SigningKey shared-secret form.
    /// This is secret material, not an encryption key; apply the protocol KDF.
    /// Named scalar/point/encoding intermediates are guarded. Backend/compiler
    /// transient copies cannot be guaranteed erased.
    pub fn ecdh_shared_point(&self, peer: &PublicKey) -> crate::SecretBytes<65> {
        let scalar = self.guarded_scalar();
        let point = Zeroizing::new(peer.0.to_projective() * *scalar);
        let encoded = Zeroizing::new(point.to_encoded_point(false));
        let mut bytes = Zeroizing::new([0u8; 65]);
        bytes.copy_from_slice(encoded.as_bytes());
        crate::SecretBytes::new(bytes)
    }

    /// Add another nonzero scalar modulo the curve order; reject a zero result.
    /// All named private arithmetic intermediates have zeroizing guards.
    pub fn add_tweak(&self, tweak: &SecretKey) -> Result<Self, CryptoError> {
        let own = self.guarded_scalar();
        let tweak = tweak.guarded_scalar();
        let sum = Zeroizing::new(*own + *tweak);
        let bytes = Zeroizing::new(<[u8; 32]>::from(sum.to_bytes()));
        Self::from_bytes(&bytes)
    }

    /// Derive the corresponding full curve public key.
    pub fn public_key(&self) -> PublicKey {
        PublicKey(self.0.public_key())
    }

    /// Derive the BIP340 x-only public key (implicitly lifted to even Y).
    pub fn schnorr_public_key(&self) -> SchnorrPublicKey {
        let signing = k256::schnorr::SigningKey::from(&self.0);
        SchnorrPublicKey::from_backend(*signing.verifying_key())
    }

    /// Sign an already computed 32-byte digest with deterministic RFC6979 ECDSA.
    ///
    /// The backend returns low-S signatures and adjusts the recovery ID together.
    /// This method does not hash again or infer a transaction signing preimage.
    pub fn sign_prehash(&self, digest: &[u8; 32]) -> Result<RecoverableSignature, CryptoError> {
        let signing = k256::ecdsa::SigningKey::from(&self.0);
        // RFC6979 bits2octets reduces the message representative modulo n.
        // ecdsa 0.16 passes its prehash straight to RFC6979; reducing here
        // matches noble/quais for digests >= n without changing ECDSA z.
        let representative =
            <k256::Scalar as Reduce<U256>>::reduce_bytes(&(*digest).into()).to_bytes();
        let (signature, recovery_id) = signing
            .sign_prehash_recoverable(&representative)
            .map_err(|_| CryptoError::SigningFailed)?;
        RecoverableSignature::from_compact(&signature.to_bytes().into(), recovery_id.to_byte())
    }

    /// Sign exact bytes with BIP340, obtaining fresh auxiliary entropy from the OS.
    ///
    /// No implicit SHA-256 pass is added. For transaction use, pass the exact
    /// protocol-specified digest. The consensus crate supplies transaction-bound
    /// signing; this primitive does not validate transaction semantics.
    pub fn sign_schnorr(&self, message: &[u8]) -> Result<SchnorrSignature, CryptoError> {
        let mut auxiliary = Zeroizing::new([0; 32]);
        getrandom::fill(auxiliary.as_mut()).map_err(|_| CryptoError::RandomnessUnavailable)?;
        self.sign_schnorr_with_aux(message, &auxiliary)
    }

    pub(crate) fn sign_schnorr_with_aux(
        &self,
        message: &[u8],
        auxiliary: &[u8; 32],
    ) -> Result<SchnorrSignature, CryptoError> {
        let signing = k256::schnorr::SigningKey::from(&self.0);
        let signature = signing
            .sign_raw(message, auxiliary)
            .map_err(|_| CryptoError::SigningFailed)?;
        // Verify before returning, including in optimized builds.
        signing
            .verifying_key()
            .verify_raw(message, &signature)
            .map_err(|_| CryptoError::SigningFailed)?;
        Ok(SchnorrSignature::from_backend(signature))
    }
}
impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretKey([REDACTED])")
    }
}
// The wrapped k256::SecretKey implements Drop and ZeroizeOnDrop.
impl ZeroizeOnDrop for SecretKey {}

/// A validated nonidentity secp256k1 point, represented independently of its encoding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PublicKey(pub(crate) k256::PublicKey);

impl PublicKey {
    /// Parse exactly a compressed (33-byte) or uncompressed (65-byte) SEC1 key.
    ///
    /// X-only, raw 64-byte coordinates, hybrid encodings, infinity and private
    /// scalar inputs are deliberately not guessed from their length.
    pub fn from_sec1_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        if !matches!(
            (bytes.len(), bytes.first()),
            (33, Some(2 | 3)) | (65, Some(4))
        ) {
            return Err(CryptoError::InvalidPublicKey);
        }
        k256::PublicKey::from_sec1_bytes(bytes)
            .map(Self)
            .map_err(|_| CryptoError::InvalidPublicKey)
    }

    /// Add two validated public points; reject the identity result. Point order
    /// does not affect addition. This does not implement an aggregate signing
    /// protocol or protect against rogue-key attacks; use OrderedKeyAggregate
    /// when the application requires the supported MuSig aggregation protocol.
    pub fn add_point(self, other: Self) -> Result<Self, CryptoError> {
        let point = self.0.to_projective() + other.0.to_projective();
        k256::PublicKey::from_affine(point.to_affine())
            .map(Self)
            .map_err(|_| CryptoError::InvalidPublicKey)
    }

    /// Add a nonzero secret scalar times the generator to this point.
    /// Uses the backend's curve operations and rejects the identity result.
    pub fn add_tweak(&self, tweak: &SecretKey) -> Result<Self, CryptoError> {
        let scalar = tweak.guarded_scalar();
        // `mul_by_generator` consults k256's precomputed generator table; the
        // generic `GENERATOR * scalar` runs a full 256-bit ladder and ignores it.
        // Same group element either way, and the table selects are constant-time.
        let point = self.0.to_projective() + k256::ProjectivePoint::mul_by_generator(&scalar);
        k256::PublicKey::from_affine(point.to_affine())
            .map(Self)
            .map_err(|_| CryptoError::InvalidPublicKey)
    }

    /// [`Self::add_tweak`] for several tweaks, in order, sharing one field
    /// inversion.
    ///
    /// Each result is exactly what `add_tweak` returns for its tweak, an
    /// identity point included. Converting a projective point to affine costs a
    /// field inversion, about a fifth of a tweak's cost; normalizing the batch
    /// together costs one inversion plus a few multiplications per point.
    /// Everything inverted is a public point, so batching reveals nothing.
    pub fn add_tweaks(&self, tweaks: &[SecretKey]) -> Vec<Result<Self, CryptoError>> {
        use k256::elliptic_curve::{group::Group, point::BatchNormalize};
        // An empty batch makes the generic batch inversion report failure,
        // which batch_normalize unwraps into a panic.
        if tweaks.is_empty() {
            return Vec::new();
        }
        let base = self.0.to_projective();
        let points: Vec<_> = tweaks
            .iter()
            .map(|tweak| base + k256::ProjectivePoint::mul_by_generator(&*tweak.guarded_scalar()))
            .collect();
        // k256 0.13.4's batch_normalize substitutes a dummy denominator only
        // when z compares equal to zero, but an identity reached by addition
        // can hold an unreduced zero z, and the batch inversion then panics.
        // `is_identity` normalizes first, so identities are swapped for the
        // generator here and rejected in their own slots below. These points
        // are public, so branching on them reveals nothing.
        let identity: Vec<bool> = points.iter().map(|p| bool::from(p.is_identity())).collect();
        let safe: Vec<_> = points
            .iter()
            .zip(&identity)
            .map(|(point, &identity)| {
                if identity {
                    k256::ProjectivePoint::GENERATOR
                } else {
                    *point
                }
            })
            .collect();
        <k256::ProjectivePoint as BatchNormalize<[k256::ProjectivePoint]>>::batch_normalize(
            safe.as_slice(),
        )
        .into_iter()
        .zip(identity)
        .map(|(point, identity)| {
            if identity {
                return Err(CryptoError::InvalidPublicKey);
            }
            k256::PublicKey::from_affine(point)
                .map(Self)
                .map_err(|_| CryptoError::InvalidPublicKey)
        })
        .collect()
    }

    /// Return canonical compressed SEC1 bytes.
    pub fn to_compressed(self) -> [u8; 33] {
        let mut bytes = [0; 33];
        bytes.copy_from_slice(self.0.to_encoded_point(true).as_bytes());
        bytes
    }

    /// Return canonical uncompressed SEC1 bytes, including the `0x04` marker.
    pub fn to_uncompressed(self) -> [u8; 65] {
        let mut bytes = [0; 65];
        bytes.copy_from_slice(self.0.to_encoded_point(false).as_bytes());
        bytes
    }

    /// Derive the raw address from the last 20 bytes of Keccak(uncompressed X || Y).
    ///
    /// No zone grinding is performed. The resulting zone can be unknown and the
    /// ledger may be Quai or Qi; use a typed address conversion to validate intent.
    pub fn address(self) -> Address {
        let hash = keccak256(&self.to_uncompressed()[1..]);
        let mut bytes = [0; 20];
        bytes.copy_from_slice(&hash[12..]);
        Address::from_bytes(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(input: &str) -> Vec<u8> {
        (0..input.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&input[i..i + 2], 16).unwrap())
            .collect()
    }
    fn fixed<const N: usize>(input: &str) -> [u8; N] {
        decode(input).try_into().unwrap()
    }

    #[test]
    fn official_bip340_auxiliary_randomness_signing_vectors() {
        let mut count = 0;
        for line in include_str!("../tests/fixtures/bip340.csv").lines().skip(1) {
            let fields: Vec<_> = line.split(',').collect();
            if fields[1].is_empty() {
                continue;
            }
            let key = SecretKey::from_bytes(&fixed(fields[1])).unwrap();
            assert_eq!(key.schnorr_public_key().to_bytes(), fixed::<32>(fields[2]));
            let signature = key
                .sign_schnorr_with_aux(&decode(fields[4]), &fixed(fields[3]))
                .unwrap();
            assert_eq!(
                signature.to_bytes(),
                fixed::<64>(fields[5]),
                "BIP340 index {}",
                fields[0]
            );
            count += 1;
        }
        assert_eq!(count, 8);
    }
}

#[cfg(test)]
mod add_tweaks_tests {
    use super::*;

    #[test]
    fn batched_tweaks_equal_single_tweaks_including_the_identity() {
        let secret = SecretKey::from_bytes(&[7; 32]).unwrap();
        let base = secret.public_key();
        let mut tweaks: Vec<SecretKey> = (1u8..=40)
            .map(|n| SecretKey::from_bytes(&[n; 32]).unwrap())
            .collect();
        // base + (-secret)·G is the identity, which both paths must reject.
        let negated = -*secret.guarded_scalar();
        tweaks.insert(
            17,
            SecretKey::from_bytes(&negated.to_bytes().into()).unwrap(),
        );
        let batched = base.add_tweaks(&tweaks);
        assert_eq!(batched.len(), tweaks.len());
        for (tweak, batched) in tweaks.iter().zip(batched) {
            assert_eq!(batched, base.add_tweak(tweak));
        }
        assert!(base.add_tweaks(&tweaks)[17].is_err());
        assert!(base.add_tweaks(&[]).is_empty());
    }
}
