use crate::WalletError;
use core::fmt;
use unicode_normalization::UnicodeNormalization;
use zeroize::{Zeroize, Zeroizing};

/// All ten BIP39 languages supported by the pinned JavaScript reference.
pub use bip39::Language;

/// Capacity reserved for the canonical phrase a wallet UI displays.
///
/// Ten words of the longest published wordlist with separators stay far inside
/// this, so the export path allocates once at a fixed size rather than growing.
const MAX_NORMALIZED_BYTES: usize = 16 * 1024;

/// Copy a normalized or case-folded secret into a guard sized in a counting pass.
///
/// A reserved multiple of the input length is a guess, and a wrong guess is a
/// security bug rather than a slow path: `String` growth frees each earlier
/// buffer without wiping it, leaving progressively longer prefixes of the
/// secret on the heap for a core dump, swap file or same-user read to recover.
/// NFKD expansion has no small bound — U+FDFA is three bytes in and thirty-three
/// out — so the size is measured instead of assumed. The counting pass iterates
/// the caller's borrowed text and copies nothing, so it creates no new exposure.
pub(crate) fn normalized_guard<I: Iterator<Item = char>>(
    source: impl Fn() -> I,
) -> Zeroizing<String> {
    let mut guard = Zeroizing::new(String::with_capacity(
        source().map(char::len_utf8).sum::<usize>(),
    ));
    guard.extend(source());
    debug_assert_eq!(guard.len(), guard.capacity(), "guard grew while filling");
    guard
}

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
pub struct Seed(pub(crate) Zeroizing<[u8; 64]>);
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

/// Explicitly exported BIP39 entropy, containing 16, 20, 24, 28 or 32 bytes.
/// The owned buffer zeroizes on drop; diagnostics never expose its contents.
pub struct MnemonicEntropy {
    bytes: Zeroizing<[u8; 33]>,
    length: usize,
}
impl MnemonicEntropy {
    // The backend's to_entropy_array re-detects language and can panic for
    // explicitly validated phrases shared by both Chinese wordlists. Stored
    // indices already define the entropy; no language inference is needed.
    pub(crate) fn from_backend(mnemonic: &bip39::Mnemonic) -> Self {
        let mut bytes = Zeroizing::new([0u8; 33]);
        let mut position = 0usize;
        for index in mnemonic.word_indices() {
            for bit in (0..11).rev() {
                if index & (1 << bit) != 0 {
                    bytes[position / 8] |= 1 << (7 - position % 8);
                }
                position += 1;
            }
        }
        let length = mnemonic.word_count() / 3 * 4;
        bytes[length..].fill(0);
        Self { bytes, length }
    }
    /// Borrow only the original entropy, excluding BIP39 checksum bits.
    /// The caller owns and must protect any copies it creates.
    pub fn expose(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}
impl fmt::Debug for MnemonicEntropy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MnemonicEntropy([REDACTED])")
    }
}

/// A validated mnemonic stored as word indexes; no passphrase is retained.
pub struct Mnemonic(bip39::Mnemonic);
impl Mnemonic {
    /// Generate a mnemonic from fresh OS entropy (Web Crypto in browser wasm).
    /// Word count must be
    /// 12, 15, 18, 21 or 24. Entropy buffers are zeroized, with no fallback RNG.
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
        // Allocate the final guard before copying any secret. Zeroizing erases
        // only the buffer it owns at drop, so growing a String from too small a
        // capacity frees each earlier allocation unwiped, leaving a trail of
        // progressively longer prefixes of the phrase on the heap.
        let normalized = normalized_guard(|| phrase.nfkd().flat_map(char::to_lowercase));
        // Published Chinese wordlists also accept unseparated ideographs.
        let normalized = if matches!(
            language,
            Language::SimplifiedChinese | Language::TraditionalChinese
        ) {
            // Re-spacing keeps every non-whitespace character and inserts one
            // separator between them, so the exact size is known before any
            // character is copied.
            let kept = || normalized.chars().filter(|c| !c.is_whitespace());
            let mut spaced = Zeroizing::new(String::with_capacity(
                kept().map(char::len_utf8).sum::<usize>() + kept().count().saturating_sub(1),
            ));
            for c in normalized.chars().filter(|c| !c.is_whitespace()) {
                if !spaced.is_empty() {
                    spaced.push(' ');
                }
                spaced.push(c);
            }
            spaced
        } else {
            normalized
        };
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

    /// Explicitly export the original BIP39 entropy, with zeroizing custody.
    /// This excludes the checksum and does not retain or encode any passphrase.
    pub fn entropy(&self) -> MnemonicEntropy {
        MnemonicEntropy::from_backend(&self.0)
    }

    /// Export the canonical phrase, using ideographic spaces for Japanese.
    ///
    /// This is the path a wallet UI calls to display the phrase, so it is the
    /// one whose intermediate copies matter most: the guard is sized to the
    /// bounded maximum before any word is copied, rather than grown.
    pub fn phrase(&self) -> SecretString {
        let mut phrase = Zeroizing::new(String::with_capacity(MAX_NORMALIZED_BYTES));
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
        // Same reasoning as `parse`: size the guard before copying the secret,
        // so normalization does not scatter unwiped prefixes of the passphrase.
        // This path takes no length bound, so a reserved multiple of the input
        // would be a guess: NFKD expands U+FDFA from three bytes to thirty-three,
        // eleven times, which any small fixed factor under-reserves.
        let normalized = normalized_guard(|| passphrase.nfkd());
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

    #[test]
    fn secret_text_guards_are_sized_before_any_secret_is_copied() {
        // Zeroizing erases only the buffer it owns at drop. A String grown from
        // zero capacity frees each earlier allocation unwiped, leaving a trail
        // of progressively longer prefixes of the secret on the heap. Every
        // guard that touches phrase or passphrase text must therefore reserve
        // its bound up front. This mirrors the keystore's equivalent check.
        let phrase = Mnemonic::generate(Language::English, 24).unwrap();

        // The export path a wallet UI calls to display the phrase.
        let exported = phrase.phrase();
        assert!(
            exported.0.capacity() >= MAX_NORMALIZED_BYTES,
            "phrase() grew its guard: capacity {}",
            exported.0.capacity()
        );
        assert!(!exported.expose().is_empty());

        // Parsing normalizes the caller's text into a guard of its own.
        let reparsed = Mnemonic::parse(Language::English, exported.expose()).unwrap();
        assert_eq!(reparsed.phrase().expose(), exported.expose());

        // A passphrase is normalized on the seed path; NFKD can expand it.
        let seed = reparsed.to_seed("\u{fdfa}");
        assert_eq!(seed.0.len(), 64);
    }

    #[test]
    fn a_normalized_guard_is_exact_for_the_worst_expansion_this_crate_can_see() {
        // U+FDFA is three bytes in and thirty-three out under NFKD, an eleven
        // times expansion. Any reserve of a small multiple of the input length
        // under-reserves it, and the guard then grows and frees an unwiped
        // prefix of the secret. Assert the size is measured, not assumed: a
        // guard whose length equals its capacity never reallocated while
        // filling, which is the property that keeps the secret in one buffer.
        let guard = normalized_guard(|| "\u{fdfa}".nfkd());
        assert_eq!(guard.len(), 33);
        assert_eq!(guard.capacity(), 33, "guard was not sized before filling");

        // The same holds once the expansion is repeated past any fixed ceiling
        // a previous bound would have clamped to, and for the case-folding
        // variant the parse path uses.
        let long = "\u{fdfa}".repeat(1024);
        let guard = normalized_guard(|| long.nfkd());
        assert_eq!(guard.len(), 33 * 1024);
        assert_eq!(guard.capacity(), guard.len());
        let folded = normalized_guard(|| "\u{130}ABC".nfkd().flat_map(char::to_lowercase));
        assert_eq!(folded.capacity(), folded.len());
        assert!(folded.ends_with("abc"));

        // An empty secret must not allocate a growing buffer either.
        let empty = normalized_guard(|| "".nfkd());
        assert_eq!(empty.len(), 0);
    }
}
