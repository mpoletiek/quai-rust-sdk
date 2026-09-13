use k256::ecdsa::{RecoveryId, Signature, VerifyingKey, signature::hazmat::PrehashVerifier};

use crate::{CryptoError, PublicKey};

/// A validated low-S ECDSA signature paired with its full recovery identifier.
///
/// The mathematical recovery ID is zero through three. Quai's parity-only wire
/// representation can encode only zero and one; conversion checks that boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecoverableSignature {
    signature: Signature,
    recovery_id: RecoveryId,
}
impl RecoverableSignature {
    /// Validate compact `r || s` bytes and a full recovery ID, rejecting high S.
    pub fn from_compact(bytes: &[u8; 64], recovery_id: u8) -> Result<Self, CryptoError> {
        let recovery_id =
            RecoveryId::from_byte(recovery_id).ok_or(CryptoError::InvalidRecoveryId)?;
        let signature = Signature::from_slice(bytes).map_err(|_| CryptoError::InvalidSignature)?;
        if signature.normalize_s().is_some() {
            return Err(CryptoError::HighS);
        }
        Ok(Self {
            signature,
            recovery_id,
        })
    }

    /// Parse EIP-2098 `r || yParityAndS`. Validates nonzero in-range scalars and
    /// low S after extracting parity; this is distinct from raw `r || s` bytes.
    pub fn from_eip2098(bytes: &[u8; 64]) -> Result<Self, CryptoError> {
        let mut compact = *bytes;
        let parity = compact[32] >> 7;
        compact[32] &= 0x7f;
        Self::from_compact(&compact, parity)
    }

    /// Encode EIP-2098 `r || yParityAndS`; full recovery IDs 2/3 cannot fit.
    pub fn to_eip2098(self) -> Result<[u8; 64], CryptoError> {
        if self.recovery_id() > 1 {
            return Err(CryptoError::UnsupportedRecoveryId);
        }
        let mut bytes = self.to_compact();
        bytes[32] |= self.recovery_id() << 7;
        Ok(bytes)
    }

    /// Return compact `r || s` bytes, without a recovery byte.
    pub fn to_compact(self) -> [u8; 64] {
        self.signature.to_bytes().into()
    }

    /// Return the full mathematical recovery ID, zero through three.
    pub fn recovery_id(self) -> u8 {
        self.recovery_id.to_byte()
    }

    /// Return the big-endian R scalar.
    pub fn r(self) -> [u8; 32] {
        self.signature.r().to_bytes().into()
    }

    /// Return the big-endian S scalar, always in the low half of the group order.
    pub fn s(self) -> [u8; 32] {
        self.signature.s().to_bytes().into()
    }

    /// Parse quais.js's 65-byte `r || s || v` representation.
    ///
    /// Accepts parity 0/1 or normalized 27/28. Chain-encoded EIP155 values are
    /// rejected because this signature type has no transaction/chain context.
    pub fn from_quais_bytes(bytes: &[u8; 65]) -> Result<Self, CryptoError> {
        let id = match bytes[64] {
            0 | 1 => bytes[64],
            27 | 28 => bytes[64] - 27,
            _ => return Err(CryptoError::UnsupportedRecoveryId),
        };
        let mut compact = [0; 64];
        compact.copy_from_slice(&bytes[..64]);
        Self::from_compact(&compact, id)
    }

    /// Serialize to quais.js's 65-byte representation with V equal to 27 or 28.
    pub fn to_quais_bytes(self) -> Result<[u8; 65], CryptoError> {
        if self.recovery_id.to_byte() > 1 {
            return Err(CryptoError::UnsupportedRecoveryId);
        }
        let mut bytes = [0; 65];
        bytes[..64].copy_from_slice(&self.to_compact());
        bytes[64] = 27 + self.recovery_id.to_byte();
        Ok(bytes)
    }

    /// Recover the public key for an exact 32-byte prehash.
    pub fn recover_prehash(&self, digest: &[u8; 32]) -> Result<PublicKey, CryptoError> {
        let key = VerifyingKey::recover_from_prehash(digest, &self.signature, self.recovery_id)
            .map_err(|_| CryptoError::VerificationFailed)?;
        PublicKey::from_sec1_bytes(key.to_encoded_point(true).as_bytes())
    }

    /// Verify ECDSA and require the recovery ID to select the expected public key.
    pub fn verify_prehash(
        &self,
        digest: &[u8; 32],
        public_key: &PublicKey,
    ) -> Result<(), CryptoError> {
        VerifyingKey::from(public_key.0)
            .verify_prehash(digest, &self.signature)
            .map_err(|_| CryptoError::VerificationFailed)?;
        if self.recover_prehash(digest)? != *public_key {
            return Err(CryptoError::VerificationFailed);
        }
        Ok(())
    }
}

/// A validated BIP340 x-only public key; its implied Y coordinate is even.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchnorrPublicKey(k256::schnorr::VerifyingKey);
impl SchnorrPublicKey {
    /// Validate an exact 32-byte big-endian X coordinate.
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self, CryptoError> {
        k256::schnorr::VerifyingKey::from_bytes(bytes)
            .map(Self)
            .map_err(|_| CryptoError::InvalidPublicKey)
    }
    pub(crate) const fn from_backend(key: k256::schnorr::VerifyingKey) -> Self {
        Self(key)
    }
    /// Return the X-only public key bytes.
    pub fn to_bytes(self) -> [u8; 32] {
        self.0.to_bytes().into()
    }
    /// Verify a BIP340 signature of exact bytes, with no implicit hashing step.
    pub fn verify(&self, message: &[u8], signature: &SchnorrSignature) -> Result<(), CryptoError> {
        self.0
            .verify_raw(message, &signature.0)
            .map_err(|_| CryptoError::VerificationFailed)
    }
}

/// A parsed 64-byte BIP340 signature. Parsing alone does not verify a message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchnorrSignature(k256::schnorr::Signature);
impl SchnorrSignature {
    /// Parse and validate scalar encodings in an exact 64-byte signature.
    pub fn from_bytes(bytes: &[u8; 64]) -> Result<Self, CryptoError> {
        k256::schnorr::Signature::try_from(bytes.as_slice())
            .map(Self)
            .map_err(|_| CryptoError::InvalidSignature)
    }
    pub(crate) const fn from_backend(signature: k256::schnorr::Signature) -> Self {
        Self(signature)
    }
    /// Serialize as the 64-byte BIP340 signature.
    pub fn to_bytes(self) -> [u8; 64] {
        self.0.to_bytes()
    }
}
