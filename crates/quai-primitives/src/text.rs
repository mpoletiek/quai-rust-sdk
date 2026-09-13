//! Explicit bounded Unicode conversion and caller-supplied UUID entropy.
use crate::{EncodingError, MAX_ENCODING_BYTES};
use unicode_normalization::UnicodeNormalization;

/// Unicode tables used by explicit normalization (independent of browser engine).
pub const UTF8_UNICODE_VERSION: (u8, u8, u8) = unicode_normalization::UNICODE_VERSION;
/// Explicit Unicode normalization. None at the call site preserves exact input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Utf8Normalization {
    /// Canonical composition.
    Nfc,
    /// Canonical decomposition.
    Nfd,
    /// Compatibility composition.
    Nfkc,
    /// Compatibility decomposition.
    Nfkd,
}
/// Encode valid Rust text as UTF-8, optionally normalizing explicitly. Both input
/// and output are limited to `MAX_ENCODING_BYTES`; expansion is checked as it is
/// streamed. No implicit normalization is applied to messages or addresses.
pub fn to_utf8_bytes(
    text: &str,
    form: Option<Utf8Normalization>,
) -> Result<Vec<u8>, EncodingError> {
    if text.len() > MAX_ENCODING_BYTES {
        return Err(EncodingError::TooLarge);
    }
    fn encode(chars: impl Iterator<Item = char>) -> Result<Vec<u8>, EncodingError> {
        let mut output = Vec::new();
        for ch in chars {
            let mut buffer = [0; 4];
            let encoded = ch.encode_utf8(&mut buffer).as_bytes();
            if output.len() + encoded.len() > MAX_ENCODING_BYTES {
                return Err(EncodingError::TooLarge);
            }
            output.extend_from_slice(encoded);
        }
        Ok(output)
    }
    match form {
        None => Ok(text.as_bytes().to_vec()),
        Some(Utf8Normalization::Nfc) => encode(text.nfc()),
        Some(Utf8Normalization::Nfd) => encode(text.nfd()),
        Some(Utf8Normalization::Nfkc) => encode(text.nfkc()),
        Some(Utf8Normalization::Nfkd) => encode(text.nfkd()),
    }
}
/// Return Unicode scalar values for optionally normalized text. Uses the same
/// input/normalized-byte bounds as `to_utf8_bytes`; at most `MAX_ENCODING_BYTES` scalars.
pub fn to_utf8_code_points(
    text: &str,
    form: Option<Utf8Normalization>,
) -> Result<Vec<u32>, EncodingError> {
    let encoded = to_utf8_bytes(text, form)?;
    Ok(to_utf8_string(&encoded)?.chars().map(u32::from).collect())
}
/// Borrow strictly valid bounded UTF-8 without replacement, skipping or loss.
/// Surrogates, overlong forms, invalid continuations and truncated sequences fail.
pub fn to_utf8_string(bytes: &[u8]) -> Result<&str, EncodingError> {
    if bytes.len() > MAX_ENCODING_BYTES {
        return Err(EncodingError::TooLarge);
    }
    std::str::from_utf8(bytes).map_err(|_| EncodingError::Invalid)
}
/// Format exactly 16 caller-supplied random bytes as a version-4 UUID, setting
/// version/variant bits in a copy. This function does not generate entropy or
/// mutate the input; use a cryptographic random source when uniqueness matters.
pub fn uuid_v4(random: &[u8; 16]) -> String {
    let mut bytes = *random;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let mut output = String::with_capacity(36);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for (i, byte) in bytes.iter().enumerate() {
        if [4, 6, 8, 10].contains(&i) {
            output.push('-');
        }
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 15) as usize] as char);
    }
    output
}
