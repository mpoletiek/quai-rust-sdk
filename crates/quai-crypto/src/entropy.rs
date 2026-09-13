use crate::CryptoError;
use zeroize::Zeroize;
/// Maximum entropy request, shared by native and Web Crypto callers.
pub const MAX_RANDOM_BYTES: usize = 65_536;
/// Fill caller-owned bytes using the operating system or browser Web Crypto.
/// No fallback PRNG is used. Oversize requests fail before touching the buffer;
/// a backend failure erases the destination so partial entropy cannot be used.
/// Callers own the secrecy and erasure of successfully filled bytes.
pub fn fill_random(destination: &mut [u8]) -> Result<(), CryptoError> {
    fill_with(destination, |bytes| getrandom::fill(bytes).map_err(|_| ()))
}
fn fill_with(
    destination: &mut [u8],
    backend: impl FnOnce(&mut [u8]) -> Result<(), ()>,
) -> Result<(), CryptoError> {
    if destination.len() > MAX_RANDOM_BYTES {
        return Err(CryptoError::RandomRequestTooLarge);
    }
    if backend(destination).is_err() {
        destination.zeroize();
        return Err(CryptoError::RandomnessUnavailable);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn entropy_failure_clears_partial_output_and_oversize_never_calls_backend() {
        let mut bytes = [17; 32];
        assert_eq!(
            fill_with(&mut bytes, |bytes| {
                bytes[..4].fill(42);
                Err(())
            }),
            Err(CryptoError::RandomnessUnavailable)
        );
        assert!(bytes.iter().all(|b| *b == 0));
        let mut large = vec![17; MAX_RANDOM_BYTES + 1];
        assert_eq!(
            fill_with(&mut large, |_| panic!("must not call backend")),
            Err(CryptoError::RandomRequestTooLarge)
        );
        assert!(large.iter().all(|b| *b == 17));
    }
}
