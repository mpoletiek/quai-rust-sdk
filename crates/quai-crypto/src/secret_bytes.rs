use core::{fmt, ops::Deref};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

/// Explicitly exposed 32-byte secret material with redacted diagnostics.
///
/// Owned bytes zeroize on drop. This wrapper has no Clone, Copy, Display or serde
/// implementation. Explicitly borrowed bytes can still be copied or logged by
/// the caller; compiler/backend transient copies cannot be guaranteed erased.
pub struct SecretBytes(Zeroizing<[u8; 32]>);
impl SecretBytes {
    pub(crate) fn new(bytes: Zeroizing<[u8; 32]>) -> Self {
        Self(bytes)
    }
    /// Explicitly borrow the sensitive bytes for encryption or key import.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}
impl fmt::Debug for SecretBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretBytes([REDACTED])")
    }
}
impl Deref for SecretBytes {
    type Target = [u8; 32];
    fn deref(&self) -> &Self::Target {
        self.as_bytes()
    }
}
impl AsRef<[u8]> for SecretBytes {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}
impl Zeroize for SecretBytes {
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}
impl ZeroizeOnDrop for SecretBytes {}
