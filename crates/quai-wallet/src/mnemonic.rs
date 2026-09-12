use crate::WalletError;
use core::fmt;
use unicode_normalization::UnicodeNormalization;
use zeroize::{Zeroize, Zeroizing};

/// All ten BIP39 languages supported by the pinned JavaScript reference.
pub use bip39::Language;

/// Explicitly exported secret text. Diagnostics redact the text; Drop zeroizes it.
pub struct SecretString(pub(crate) Zeroizing<String>);
impl SecretString {
    /// Borrow secret text deliberately. Caller-created copies are not erased here.
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString([REDACTED])")
    }
}

/// A 64-byte BIP39 seed whose owned buffer is zeroized on drop.
pub struct Seed(Zeroizing<[u8; 64]>);
impl Seed {
    /// Borrow seed bytes deliberately; avoid copying or logging them.
    pub fn expose(&self) -> &[u8; 64] {
        &self.0
    }
}
impl fmt::Debug for Seed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Seed([REDACTED])")
    }
}

/// A validated mnemonic stored as word indexes; no passphrase is retained.
pub struct Mnemonic(bip39::Mnemonic);
impl Mnemonic {
    /// Generate a native mnemonic from fresh OS entropy. Word count must be
    /// 12, 15, 18, 21 or 24. Entropy buffers are zeroized, with no fallback RNG.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn generate(language: Language, word_count: usize) -> Result<Self, WalletError> {
        if !matches!(word_count, 12 | 15 | 18 | 21 | 24) {
            return Err(WalletError::InvalidEntropy);
        }
        let mut entropy = Zeroizing::new([0u8; 32]);
        let length = word_count / 3 * 4;
        getrandom::fill(&mut entropy[..length]).map_err(|_| WalletError::RandomnessUnavailable)?;
        Self::from_entropy(language, &entropy[..length])
    }

    /// Parse in an explicit language, validating words and checksum with NFKD.
    /// Case is canonicalized as in the default quais.js wordlist parser.
    pub fn parse(language: Language, phrase: &str) -> Result<Self, WalletError> {
        if phrase.len() > 4096 {
            return Err(WalletError::InvalidMnemonic);
        }
        let normalized = Zeroizing::new(
            phrase
                .nfkd()
                .flat_map(char::to_lowercase)
                .collect::<String>(),
        );
        bip39::Mnemonic::parse_in_normalized(language, &normalized)
            .map(Self)
            .map_err(|_| WalletError::InvalidMnemonic)
    }

    /// Construct from caller-supplied cryptographic entropy (16,20,24,28,32 bytes).
    /// The caller is responsible for using a secure random source when generating it.
    pub fn from_entropy(language: Language, entropy: &[u8]) -> Result<Self, WalletError> {
        bip39::Mnemonic::from_entropy_in(language, entropy)
            .map(Self)
            .map_err(|_| WalletError::InvalidEntropy)
    }

    /// Explicit language needed to reconstruct the same canonical phrase.
    pub fn language(&self) -> Language {
        self.0.language()
    }

    /// Export the canonical phrase, using ideographic spaces for Japanese.
    pub fn phrase(&self) -> SecretString {
        let mut phrase = Zeroizing::new(String::new());
        let separator = if self.language() == Language::Japanese {
            "\u{3000}"
        } else {
            " "
        };
        for (index, word) in self.0.words().enumerate() {
            if index > 0 {
                phrase.push_str(separator);
            }
            phrase.push_str(word);
        }
        SecretString(phrase)
    }

    /// Derive the BIP39 seed with a caller-supplied passphrase and NFKD normalization.
    /// Every passphrase selects an identity; there is no "wrong password" checksum.
    pub fn to_seed(&self, passphrase: &str) -> Seed {
        let normalized = Zeroizing::new(passphrase.nfkd().collect::<String>());
        Seed(Zeroizing::new(self.0.to_seed_normalized(&normalized)))
    }
}
impl Drop for Mnemonic {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}
impl fmt::Debug for Mnemonic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Mnemonic([REDACTED])")
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod generation_tests {
    use super::*;
    #[test]
    fn native_generation_validates_and_redacts() {
        for count in [12, 15, 18, 21, 24] {
            let mnemonic = Mnemonic::generate(Language::English, count).unwrap();
            let phrase = mnemonic.phrase();
            assert_eq!(phrase.expose().split_whitespace().count(), count);
            assert!(Mnemonic::parse(Language::English, phrase.expose()).is_ok());
            assert_eq!(format!("{mnemonic:?}"), "Mnemonic([REDACTED])");
            assert_eq!(format!("{phrase:?}"), "SecretString([REDACTED])");
        }
        assert!(Mnemonic::generate(Language::English, 13).is_err());
    }
}
