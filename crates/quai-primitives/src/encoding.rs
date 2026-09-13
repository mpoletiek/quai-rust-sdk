//! Bounded byte encodings and explicit 256-bit signed representations.
use crate::SignedUnits;
use base64::Engine;
use ruint::aliases::{U256, U512};
use thiserror::Error;
/// Maximum general byte utility input/output, excluding hexadecimal expansion.
pub const MAX_ENCODING_BYTES: usize = 1_048_576;
/// Base58 uses quadratic radix conversion and therefore has a smaller input cap.
pub const MAX_BASE58_BYTES: usize = 4096;
/// Invalid encoding, width or arithmetic input; errors never retain input content.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum EncodingError {
    /// Input violates the canonical encoding or strict UTF-8 requirements.
    #[error("invalid byte encoding")]
    Invalid,
    /// Requested padding/slice/width does not fit the input or integer.
    #[error("invalid byte or integer bounds")]
    Bounds,
    /// Input or output exceeds its documented resource bound.
    #[error("encoding resource limit exceeded")]
    TooLarge,
}
fn bounded(length: usize) -> Result<(), EncodingError> {
    if length > MAX_ENCODING_BYTES {
        Err(EncodingError::TooLarge)
    } else {
        Ok(())
    }
}
/// Lowercase even-width hexadecimal with a `0x` prefix, preserving leading zeros.
pub fn hexlify(bytes: &[u8]) -> Result<String, EncodingError> {
    bounded(bytes.len())?;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(2 + bytes.len() * 2);
    text.push_str("0x");
    for byte in bytes {
        text.push(HEX[(byte >> 4) as usize] as char);
        text.push(HEX[(byte & 15) as usize] as char);
    }
    Ok(text)
}
/// Decode strict prefixed, even-width hexadecimal into owned bytes. A `0X`
/// prefix, odd nibbles, whitespace and invalid digits are rejected.
pub fn get_bytes(text: &str) -> Result<Vec<u8>, EncodingError> {
    if text.len() > MAX_ENCODING_BYTES * 2 + 2 {
        return Err(EncodingError::TooLarge);
    }
    let digits = text.strip_prefix("0x").ok_or(EncodingError::Invalid)?;
    if !digits.len().is_multiple_of(2) || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(EncodingError::Invalid);
    }
    let nibble = |b: u8| {
        if b.is_ascii_digit() {
            b - b'0'
        } else {
            b.to_ascii_lowercase() - b'a' + 10
        }
    };
    Ok(digits
        .as_bytes()
        .chunks_exact(2)
        .map(|b| nibble(b[0]) * 16 + nibble(b[1]))
        .collect())
}
/// Concatenate byte slices after bounding their total size before allocation.
pub fn concat_bytes(parts: &[&[u8]]) -> Result<Vec<u8>, EncodingError> {
    if parts.len() > 4096 {
        return Err(EncodingError::TooLarge);
    }
    let size = parts
        .iter()
        .try_fold(0usize, |n, p| n.checked_add(p.len()))
        .ok_or(EncodingError::TooLarge)?;
    bounded(size)?;
    let mut bytes = Vec::with_capacity(size);
    for part in parts {
        bytes.extend_from_slice(part);
    }
    Ok(bytes)
}
/// Checked borrowed byte slice; negative/JS-coerced indexes are not accepted.
pub fn data_slice(bytes: &[u8], start: usize, end: usize) -> Result<&[u8], EncodingError> {
    bytes.get(start..end).ok_or(EncodingError::Bounds)
}
/// Borrow the bytes after all leading zero bytes. All-zero input returns empty.
pub fn strip_zeros_left(bytes: &[u8]) -> &[u8] {
    &bytes[bytes.iter().position(|b| *b != 0).unwrap_or(bytes.len())..]
}
fn pad(bytes: &[u8], length: usize, left: bool) -> Result<Vec<u8>, EncodingError> {
    bounded(length)?;
    if bytes.len() > length {
        return Err(EncodingError::Bounds);
    }
    let mut result = vec![0; length];
    let start = if left { length - bytes.len() } else { 0 };
    result[start..start + bytes.len()].copy_from_slice(bytes);
    Ok(result)
}
/// Left-pad an integer byte representation with zero bytes, without truncation.
pub fn zero_pad_value(bytes: &[u8], length: usize) -> Result<Vec<u8>, EncodingError> {
    pad(bytes, length, true)
}
/// Right-pad a fixed-byte representation with zero bytes, without truncation.
pub fn zero_pad_bytes(bytes: &[u8], length: usize) -> Result<Vec<u8>, EncodingError> {
    pad(bytes, length, false)
}
/// Standard padded RFC 4648 Base64 encoding.
pub fn encode_base64(bytes: &[u8]) -> Result<String, EncodingError> {
    bounded(bytes.len())?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}
/// Strict standard padded Base64; URL alphabet, whitespace, missing padding and
/// nonzero trailing bits are rejected rather than accepting Node Buffer coercions.
pub fn decode_base64(text: &str) -> Result<Vec<u8>, EncodingError> {
    if text.len() > MAX_ENCODING_BYTES.div_ceil(3) * 4 {
        return Err(EncodingError::TooLarge);
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text)
        .map_err(|_| EncodingError::Invalid)?;
    bounded(bytes.len())?;
    Ok(bytes)
}
/// Bitcoin-alphabet Base58, preserving each leading zero byte as `1`.
pub fn encode_base58(bytes: &[u8]) -> Result<String, EncodingError> {
    if bytes.len() > MAX_BASE58_BYTES {
        return Err(EncodingError::TooLarge);
    }
    Ok(bs58::encode(bytes).into_string())
}
/// Base58 to bytes, preserving leading zeros. This is preferable for key material.
pub fn decode_base58_bytes(text: &str) -> Result<Vec<u8>, EncodingError> {
    if text.len() > MAX_BASE58_BYTES * 138 / 100 + 1 {
        return Err(EncodingError::TooLarge);
    }
    // Bound decoded allocation independently of leading-zero expansion.
    let mut output = vec![0; MAX_BASE58_BYTES];
    let size = bs58::decode(text)
        .onto(output.as_mut_slice())
        .map_err(|e| match e {
            bs58::decode::Error::BufferTooSmall => EncodingError::TooLarge,
            _ => EncodingError::Invalid,
        })?;
    output.truncate(size);
    Ok(output)
}
/// Numeric Base58 decode, matching JS zero stripping within an explicit U256 range.
pub fn decode_base58(text: &str) -> Result<U256, EncodingError> {
    let bytes = decode_base58_bytes(text)?;
    let bytes = strip_zeros_left(&bytes);
    if bytes.len() > 32 {
        return Err(EncodingError::Bounds);
    }
    Ok(U256::from_be_slice(bytes))
}
/// Encode at most 31 UTF-8 bytes plus null termination into an ABI bytes32 word.
pub fn encode_bytes32(text: &str) -> Result<[u8; 32], EncodingError> {
    if text.len() > 31 {
        return Err(EncodingError::Bounds);
    }
    let mut bytes = [0; 32];
    bytes[..text.len()].copy_from_slice(text.as_bytes());
    Ok(bytes)
}
/// Decode strict UTF-8 after stripping trailing zero padding. Internal null bytes
/// are preserved; the final word byte must be zero, matching the pinned JS helper.
pub fn decode_bytes32(bytes: &[u8; 32]) -> Result<&str, EncodingError> {
    if bytes[31] != 0 {
        return Err(EncodingError::Invalid);
    }
    let end = bytes.iter().rposition(|b| *b != 0).map_or(0, |i| i + 1);
    std::str::from_utf8(&bytes[..end]).map_err(|_| EncodingError::Invalid)
}
/// Keep the lowest `bits` bits, with the full supported range 0..=256.
pub fn mask(value: U256, bits: u16) -> Result<U256, EncodingError> {
    match bits {
        0 => Ok(U256::ZERO),
        256 => Ok(value),
        1..=255 => Ok(value & ((U256::from(1) << bits as usize) - U256::from(1))),
        _ => Err(EncodingError::Bounds),
    }
}
/// Convert a checked signed value to its two's-complement representation at 1..=256 bits.
pub fn to_twos(value: SignedUnits, bits: u16) -> Result<U256, EncodingError> {
    if !(1..=256).contains(&bits) {
        return Err(EncodingError::Bounds);
    }
    let magnitude = U256::checked_from_limbs_slice(value.magnitude().as_limbs())
        .ok_or(EncodingError::Bounds)?;
    let limit = U256::from(1) << (bits as usize - 1);
    if magnitude > limit || (!value.is_negative() && magnitude == limit) {
        return Err(EncodingError::Bounds);
    }
    if value.is_negative() {
        mask((!magnitude).wrapping_add(U256::from(1)), bits)
    } else {
        Ok(magnitude)
    }
}
/// Interpret an unsigned bit pattern as signed two's complement at 1..=256 bits.
/// Bits outside the selected width are rejected instead of silently truncated.
pub fn from_twos(value: U256, bits: u16) -> Result<SignedUnits, EncodingError> {
    if !(1..=256).contains(&bits) || mask(value, bits)? != value {
        return Err(EncodingError::Bounds);
    }
    let negative = value.bit(bits as usize - 1);
    let magnitude = if negative {
        mask((!value).wrapping_add(U256::from(1)), bits)?
    } else {
        value
    };
    SignedUnits::new(negative, U512::from(magnitude)).map_err(|_| EncodingError::Bounds)
}
