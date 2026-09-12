use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256, Sha512};
use tiny_keccak::{Hasher, Keccak};

/// The actual personal-message prefix in pinned quais.js, despite its Quai name.
pub const MESSAGE_PREFIX: &[u8] = b"\x19Ethereum Signed Message:\n";

/// Compute Keccak-256, which differs from standardized SHA3-256.
pub fn keccak256(data: &[u8]) -> [u8; 32] {
    let mut hash = Keccak::v256();
    hash.update(data);
    let mut output = [0; 32];
    hash.finalize(&mut output);
    output
}

/// Hash exact message bytes with the pinned quais.js personal-sign prefix.
///
/// Use `text.as_bytes()` for UTF-8 strings. Hex-looking text is not decoded.
/// The decimal length counts bytes, not Unicode characters, and no extra
/// SHA-256 hash is applied. Input is streamed without copying the message.
pub fn hash_message(message: &[u8]) -> [u8; 32] {
    // usize has at most 39 decimal digits on a hypothetical 128-bit target.
    let mut decimal = [0_u8; 40];
    let mut offset = decimal.len();
    let mut length = message.len();
    loop {
        offset -= 1;
        decimal[offset] = b'0' + (length % 10) as u8;
        length /= 10;
        if length == 0 {
            break;
        }
    }
    let mut hash = Keccak::v256();
    hash.update(MESSAGE_PREFIX);
    hash.update(&decimal[offset..]);
    hash.update(message);
    let mut output = [0; 32];
    hash.finalize(&mut output);
    output
}

/// Compute SHA-256.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// Compute SHA-512.
pub fn sha512(data: &[u8]) -> [u8; 64] {
    Sha512::digest(data).into()
}

/// Compute HMAC-SHA256. Treat the returned bytes as secret when used as key material.
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    // HMAC accepts keys of every length, including zero.
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC supports every key length");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// Compute HMAC-SHA512, used in BIP32 derivation. Derived key bytes are caller-owned secrets.
pub fn hmac_sha512(key: &[u8], data: &[u8]) -> [u8; 64] {
    let mut mac = Hmac::<Sha512>::new_from_slice(key).expect("HMAC supports every key length");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// Verify a full HMAC-SHA256 tag using the backend's constant-time comparison.
pub fn verify_hmac_sha256(
    key: &[u8],
    data: &[u8],
    tag: &[u8; 32],
) -> Result<(), crate::CryptoError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC supports every key length");
    mac.update(data);
    mac.verify_slice(tag)
        .map_err(|_| crate::CryptoError::VerificationFailed)
}

/// Verify a full HMAC-SHA512 tag using the backend's constant-time comparison.
pub fn verify_hmac_sha512(
    key: &[u8],
    data: &[u8],
    tag: &[u8; 64],
) -> Result<(), crate::CryptoError> {
    let mut mac = Hmac::<Sha512>::new_from_slice(key).expect("HMAC supports every key length");
    mac.update(data);
    mac.verify_slice(tag)
        .map_err(|_| crate::CryptoError::VerificationFailed)
}
