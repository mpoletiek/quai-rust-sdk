use crate::{CryptoError, RecoverableSignature};
use ruint::aliases::U256;

/// Normalize parity 0/1, legacy 27/28 or EIP-155 V >= 35 to 27/28.
/// This parses metadata only and does not establish a transaction chain.
pub fn normalized_v(v: U256) -> Result<u8, CryptoError> {
    if v == U256::ZERO || v == U256::from(27) {
        return Ok(27);
    }
    if v == U256::from(1) || v == U256::from(28) {
        return Ok(28);
    }
    if v < U256::from(35) {
        return Err(CryptoError::UnsupportedRecoveryId);
    }
    Ok(if v.bit(0) { 27 } else { 28 })
}
/// Extract an EIP-155 chain ID from V. Legacy 27/28 returns zero (unspecified);
/// parity-only 0/1 and other values below 35 reject, matching the published helper.
pub fn legacy_chain_id(v: U256) -> Result<U256, CryptoError> {
    if v == U256::from(27) || v == U256::from(28) {
        return Ok(U256::ZERO);
    }
    v.checked_sub(U256::from(35))
        .map(|v| v / U256::from(2))
        .ok_or(CryptoError::UnsupportedRecoveryId)
}
/// Compute EIP-155 V = 2*chain_id + 35 + parity from normalized 27/28.
/// Invalid normalized V and U256 overflow reject. No active signer policy changes.
pub fn legacy_chain_v(chain_id: U256, v: u8) -> Result<U256, CryptoError> {
    if !matches!(v, 27 | 28) {
        return Err(CryptoError::UnsupportedRecoveryId);
    }
    chain_id
        .checked_mul(U256::from(2))
        .and_then(|n| n.checked_add(U256::from(35 + v - 27)))
        .ok_or(CryptoError::UnsupportedRecoveryId)
}

/// Validated parity signature with optional retained legacy EIP-155 metadata.
/// Quai protobuf signatures use explicit chain IDs separately; this wrapper
/// never changes their wire format or infers authorization from a legacy V.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignatureMetadata {
    signature: RecoverableSignature,
    network_v: Option<U256>,
}
impl SignatureMetadata {
    /// Construct from exact R/S and an explicit parity, normalized or EIP-155 V.
    /// Scalars must be nonzero, in range and low S. V >= 35 is retained separately.
    pub fn from_rs_v(r: [u8; 32], s: [u8; 32], v: U256) -> Result<Self, CryptoError> {
        let parity = normalized_v(v)? - 27;
        let mut bytes = [0; 64];
        bytes[..32].copy_from_slice(&r);
        bytes[32..].copy_from_slice(&s);
        let signature = RecoverableSignature::from_compact(&bytes, parity)?;
        Ok(Self {
            signature,
            network_v: if v >= U256::from(35) { Some(v) } else { None },
        })
    }
    /// Wrap a parity-representable signature without legacy metadata.
    pub fn from_signature(signature: RecoverableSignature) -> Result<Self, CryptoError> {
        if signature.recovery_id() > 1 {
            return Err(CryptoError::UnsupportedRecoveryId);
        }
        Ok(Self {
            signature,
            network_v: None,
        })
    }
    /// The validated signature. Serialization normalizes to parity or 27/28 and
    /// cannot encode the separately retained full legacy V.
    pub const fn signature(self) -> RecoverableSignature {
        self.signature
    }
    /// Retained V for legacy metadata, absent on ordinary Quai/parity signatures.
    pub const fn network_v(self) -> Option<U256> {
        self.network_v
    }
    /// Optional EIP-155 chain ID; Some(0) remains distinct from absent metadata.
    pub fn legacy_chain_id(self) -> Option<U256> {
        self.network_v.map(|v| (v - U256::from(35)) / U256::from(2))
    }
    /// Normalized V, always 27 or 28.
    pub fn v(self) -> u8 {
        27 + self.signature.recovery_id()
    }
    /// Serialize a published-compatible signature JSON object with exact decimal
    /// legacy V. Signature bytes are public; no secret key or message is included.
    pub fn to_json(self) -> String {
        let hex = |bytes: [u8; 32]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let network = self.network_v.map_or("null".into(), |v| format!("\"{v}\""));
        format!(
            "{{\"_type\":\"signature\",\"networkV\":{network},\"r\":\"0x{}\",\"s\":\"0x{}\",\"v\":{}}}",
            hex(self.signature.r()),
            hex(self.signature.s()),
            self.v()
        )
    }
}
