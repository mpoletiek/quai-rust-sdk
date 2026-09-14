//! Exact bounded integer interchange. Floating-point bridges are explicit and
//! limited to JavaScript's safe integer range; chain quantities remain U256.
use crate::{EncodingError, SignedUnits, hexlify, strip_zeros_left, zero_pad_value};
use ruint::aliases::{U256, U512};

/// Largest magnitude that can be transported as a JavaScript safe integer.
pub const MAX_SAFE_INTEGER: u64 = (1u64 << 53) - 1;
/// Maximum integer text, including sign and radix prefix.
pub const MAX_INTEGER_TEXT: usize = 1024;

/// Parse a signed integer with decimal or lowercase `0x`, `0o`, `0b` prefix.
/// Digits may have leading zeros or uppercase hex digits. Optional leading minus
/// is supported for every radix; plus signs, whitespace, underscores, fractions
/// and exponent notation are rejected. Range is -2^511 through 2^511-1.
pub fn parse_integer(text: &str) -> Result<SignedUnits, EncodingError> {
    if text.len() > MAX_INTEGER_TEXT {
        return Err(EncodingError::TooLarge);
    }
    let (negative, digits) = text.strip_prefix('-').map_or((false, text), |s| (true, s));
    let (radix, digits) = if let Some(s) = digits.strip_prefix("0x") {
        (16u64, s)
    } else if let Some(s) = digits.strip_prefix("0o") {
        (8, s)
    } else if let Some(s) = digits.strip_prefix("0b") {
        (2, s)
    } else {
        (10, digits)
    };
    if digits.is_empty() {
        return Err(EncodingError::Invalid);
    }
    let mut magnitude = U512::ZERO;
    for byte in digits.bytes() {
        let digit = match byte {
            b'0'..=b'9' => u64::from(byte - b'0'),
            b'a'..=b'f' => u64::from(byte - b'a' + 10),
            b'A'..=b'F' => u64::from(byte - b'A' + 10),
            _ => return Err(EncodingError::Invalid),
        };
        if digit >= radix {
            return Err(EncodingError::Invalid);
        }
        magnitude = magnitude
            .checked_mul(U512::from(radix))
            .and_then(|n| n.checked_add(U512::from(digit)))
            .ok_or(EncodingError::Bounds)?;
    }
    SignedUnits::new(negative, magnitude).map_err(|_| EncodingError::Bounds)
}

/// Parse an unsigned 256-bit integer using parse_integer's explicit grammar.
pub fn parse_uint(text: &str) -> Result<U256, EncodingError> {
    parse_integer(text)?
        .try_u256()
        .map_err(|_| EncodingError::Bounds)
}

/// Decode bounded big-endian bytes into U256, permitting leading zero padding.
/// Empty/all-zero input is zero. Nonzero magnitude must fit 32 bytes.
pub fn uint_from_be_bytes(bytes: &[u8]) -> Result<U256, EncodingError> {
    if bytes.len() > crate::MAX_ENCODING_BYTES {
        return Err(EncodingError::TooLarge);
    }
    let magnitude = strip_zeros_left(bytes);
    if magnitude.len() > 32 {
        return Err(EncodingError::Bounds);
    }
    Ok(U256::from_be_slice(magnitude))
}

/// Minimal big-endian bytes; zero is empty, matching the reference toBeArray.
pub fn uint_to_be_array(value: U256) -> Vec<u8> {
    strip_zeros_left(&value.to_be_bytes::<32>()).to_vec()
}

/// Even hexadecimal with optional exact byte width. Minimal zero is `0x00`;
/// width zero rejects even zero, matching toBeHex. Padding is bounded by
/// MAX_ENCODING_BYTES, and values are never truncated to fit.
pub fn uint_to_be_hex(value: U256, width: Option<usize>) -> Result<String, EncodingError> {
    let mut bytes = uint_to_be_array(value);
    if bytes.is_empty() {
        bytes.push(0);
    }
    if let Some(width) = width {
        bytes = zero_pad_value(&bytes, width)?;
    }
    hexlify(&bytes)
}

/// Minimal JSON-RPC unsigned quantity; zero is `0x0` and there are no leading zeros.
pub fn uint_to_quantity(value: U256) -> String {
    format!("{value:#x}")
}

/// Explicit f64-to-integer bridge for JavaScript interoperability. Reject
/// fractions, infinities, NaN and magnitudes above 2^53-1; normalize negative zero.
pub fn integer_from_safe_number(value: f64) -> Result<SignedUnits, EncodingError> {
    if !value.is_finite() || value.fract() != 0.0 || value.abs() > MAX_SAFE_INTEGER as f64 {
        return Err(EncodingError::Bounds);
    }
    SignedUnits::new(value.is_sign_negative(), U512::from(value.abs() as u64))
        .map_err(|_| EncodingError::Bounds)
}

/// Explicit integer-to-f64 bridge restricted to exact JavaScript safe integers.
pub fn integer_to_safe_number(value: SignedUnits) -> Result<f64, EncodingError> {
    if value.magnitude() > U512::from(MAX_SAFE_INTEGER) {
        return Err(EncodingError::Bounds);
    }
    let number = value.magnitude().to::<u64>() as f64;
    Ok(if value.is_negative() { -number } else { number })
}

/// Hexadecimal validation intent; general hexadecimal permits odd nibbles.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HexFormat {
    /// Any number of hex digits, including zero or an odd digit count.
    Any,
    /// An even number of hex digits representing complete bytes.
    Bytes,
    /// Exactly this many bytes.
    Exact(usize),
}
/// Validate lowercase `0x` prefix and ASCII hex digits without allocation. Input
/// is bounded to twice MAX_ENCODING_BYTES plus the prefix. This does not decode
/// bytes or validate an address checksum.
pub fn is_hex_string(text: &str, format: HexFormat) -> bool {
    if text.len() > crate::MAX_ENCODING_BYTES * 2 + 2 {
        return false;
    }
    let Some(digits) = text.strip_prefix("0x") else {
        return false;
    };
    let shape = match format {
        HexFormat::Any => true,
        HexFormat::Bytes => digits.len().is_multiple_of(2),
        HexFormat::Exact(count) => count.checked_mul(2) == Some(digits.len()),
    };
    shape && digits.bytes().all(|b| b.is_ascii_hexdigit())
}
