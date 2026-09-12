use core::{fmt, str::FromStr};

/// Exactly 32 bytes, used for RPC hashes and storage words; no ledger is implied.
#[derive(Clone, Copy, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Hash32([u8; 32]);

/// A hash must contain exactly 64 hexadecimal digits after lowercase `0x`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Hash32Error;

impl fmt::Display for Hash32Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("expected 0x-prefixed 32-byte hexadecimal value")
    }
}
impl std::error::Error for Hash32Error {}

impl Hash32 {
    /// The all-zero value.
    pub const ZERO: Self = Self([0; 32]);
    /// Construct from an exact-size byte array.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
    /// Borrow all 32 bytes.
    pub const fn bytes(&self) -> &[u8; 32] {
        &self.0
    }
    /// Consume into the exact-size byte array.
    pub const fn into_bytes(self) -> [u8; 32] {
        self.0
    }
}

impl From<[u8; 32]> for Hash32 {
    fn from(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}
impl From<Hash32> for [u8; 32] {
    fn from(hash: Hash32) -> Self {
        hash.0
    }
}
impl AsRef<[u8]> for Hash32 {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}
impl FromStr for Hash32 {
    type Err = Hash32Error;
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        if input.len() != 66 || !input.starts_with("0x") {
            return Err(Hash32Error);
        }
        fn nibble(byte: u8) -> Result<u8, Hash32Error> {
            match byte {
                b'0'..=b'9' => Ok(byte - b'0'),
                b'a'..=b'f' => Ok(byte - b'a' + 10),
                b'A'..=b'F' => Ok(byte - b'A' + 10),
                _ => Err(Hash32Error),
            }
        }
        let mut bytes = [0; 32];
        for (byte, pair) in bytes.iter_mut().zip(input.as_bytes()[2..].chunks_exact(2)) {
            *byte = nibble(pair[0])? * 16 + nibble(pair[1])?;
        }
        Ok(Self(bytes))
    }
}
impl fmt::Display for Hash32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("0x")?;
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}
impl fmt::Debug for Hash32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_width_roundtrip_and_rejection() {
        let hash = Hash32::from_bytes(core::array::from_fn(|i| i as u8));
        assert_eq!(hash.to_string().parse(), Ok(hash));
        assert_eq!(
            format!("0x{}", "AB".repeat(32)).parse(),
            Ok(Hash32::from_bytes([0xab; 32]))
        );
        for input in [
            "0x".into(),
            "00".repeat(32),
            format!("0X{}", "00".repeat(32)),
            format!("0x{}", "00".repeat(31)),
            format!("0x{}", "00".repeat(33)),
            format!("0x{}", "gg".repeat(32)),
            format!("0x{}", "é".repeat(32)),
        ] {
            assert!(input.parse::<Hash32>().is_err());
        }
    }
}
