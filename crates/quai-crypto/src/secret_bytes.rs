use core::{fmt, ops::Deref};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

/// Explicitly exposed fixed-size secret material with redacted diagnostics.
/// The default size is 32 bytes; full shared SEC1 points use 65 bytes.
///
/// Owned bytes zeroize on drop. This wrapper has no Clone, Copy, Display or serde
/// implementation. Explicitly borrowed bytes can still be copied or logged by
/// the caller; compiler/backend transient copies cannot be guaranteed erased.
pub struct SecretBytes<const N: usize = 32>(Zeroizing<[u8; N]>);
impl<const N: usize> SecretBytes<N> {
    pub(crate) fn new(bytes: Zeroizing<[u8; N]>) -> Self {
        Self(bytes)
    }
    /// Explicitly borrow the sensitive bytes for encryption or key import.
    pub fn as_bytes(&self) -> &[u8; N] {
        &self.0
    }
}
impl<const N: usize> fmt::Debug for SecretBytes<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretBytes([REDACTED])")
    }
}
impl<const N: usize> Deref for SecretBytes<N> {
    type Target = [u8; N];
    fn deref(&self) -> &Self::Target {
        self.as_bytes()
    }
}
impl<const N: usize> AsRef<[u8]> for SecretBytes<N> {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}
impl<const N: usize> Zeroize for SecretBytes<N> {
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}
impl<const N: usize> ZeroizeOnDrop for SecretBytes<N> {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_size_secret_buffers_zeroize_and_keep_drop_guards() {
        fn guarded<T: ZeroizeOnDrop>() {}
        guarded::<SecretBytes>();
        guarded::<SecretBytes<65>>();
        let mut point = SecretBytes::new(Zeroizing::new([7; 65]));
        point.zeroize();
        assert_eq!(point.as_bytes(), &[0; 65]);
        assert_eq!(format!("{point:?}"), "SecretBytes([REDACTED])");
    }
}
