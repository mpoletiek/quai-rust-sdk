//! Bounded wordlist utilities and published OWL/OWL-A dictionary decoding.
//! The compression format is documented by quais.js's MIT-licensed wordlist sources.
use crate::{Language, SecretString};
use quai_primitives::Hash32;
use std::{collections::BTreeSet, fmt};
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

/// Maximum wordlist entry count; BIP39 mnemonic conversion requires exactly 2,048.
pub const MAX_WORDLIST_WORDS: usize = 2048;
/// Maximum encoded dictionary/diacritic input bytes individually.
pub const MAX_WORDLIST_INPUT_BYTES: usize = 65_536;
/// Maximum expanded intermediate dictionary bytes.
pub const MAX_WORDLIST_BYTES: usize = 1 << 20;
/// Maximum phrase bytes before or after splitting/joining.
pub const MAX_PHRASE_BYTES: usize = 4096;

/// Wordlist validation failures never include supplied phrase contents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum WordlistError {
    /// Input, expansion or output exceeds the fixed resource policy.
    #[error("wordlist resource limit exceeded")]
    Limit,
    /// Unsupported or malformed compressed dictionary/accent representation.
    #[error("invalid compressed wordlist")]
    Encoding,
    /// Expanded words do not match the caller-supplied Keccak checksum.
    #[error("wordlist checksum mismatch")]
    Checksum,
    /// Locale, word count, duplicate or noncanonical word is invalid.
    #[error("invalid wordlist")]
    Words,
    /// Mnemonic count, words, checksum or entropy is invalid.
    #[error("invalid mnemonic")]
    Mnemonic,
}
/// Explicit phrase splitting/joining policy; custom locales do not select it implicitly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WordlistStyle {
    /// Lowercase and split on ECMAScript whitespace; join with ASCII spaces.
    General,
    /// Split on ASCII/ideographic spaces; join with ideographic spaces.
    Japanese,
    /// Remove ASCII/ideographic spaces and split Unicode scalars; join with spaces.
    Chinese,
}
/// Explicitly exposed phrase words. Owned strings zeroize on drop and diagnostics
/// redact them. Caller-created copies remain the caller's responsibility.
pub struct SecretWords(Zeroizing<Vec<String>>);
impl SecretWords {
    /// Deliberately borrow the ordered sensitive words, including empty split fields.
    pub fn expose(&self) -> &[String] {
        &self.0
    }
}
impl fmt::Debug for SecretWords {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretWords([REDACTED])")
    }
}
#[derive(Clone, Debug)]
enum Words {
    Builtin(&'static [&'static str; 2048]),
    Owned(Vec<String>),
}
/// Validated immutable public dictionary with explicit phrase conventions.
/// A checksum validates supplied bytes against an expected value, not their source.
#[derive(Clone, Debug)]
pub struct Wordlist {
    locale: String,
    style: WordlistStyle,
    words: Words,
    owl: Option<String>,
    accents: Option<String>,
}
impl Wordlist {
    /// Borrow a built-in BIP39 dictionary without allocating its words.
    pub fn builtin(language: Language) -> Self {
        let (locale, style) = match language {
            Language::English => ("en", WordlistStyle::General),
            Language::Spanish => ("es", WordlistStyle::General),
            Language::French => ("fr", WordlistStyle::General),
            Language::Italian => ("it", WordlistStyle::General),
            Language::Czech => ("cz", WordlistStyle::General),
            Language::Portuguese => ("pt", WordlistStyle::General),
            Language::Korean => ("ko", WordlistStyle::General),
            Language::Japanese => ("ja", WordlistStyle::Japanese),
            Language::SimplifiedChinese => ("zh_cn", WordlistStyle::Chinese),
            Language::TraditionalChinese => ("zh_tw", WordlistStyle::Chinese),
        };
        Self {
            locale: locale.into(),
            style,
            words: Words::Builtin(language.word_list()),
            owl: None,
            accents: None,
        }
    }
    /// Construct 1–2,048 unique, nonempty NFKD words of at most 128 bytes each.
    /// Words may not contain whitespace/control characters. General-style words
    /// must be lowercase; Chinese-style words must be single Unicode scalars, so
    /// canonical phrases remain recoverable under the chosen split policy. Locale is a bounded
    /// ASCII tag; it neither authenticates the list nor selects phrase rules.
    pub fn from_words(
        locale: &str,
        words: Vec<String>,
        style: WordlistStyle,
    ) -> Result<Self, WordlistError> {
        if locale.is_empty()
            || locale.len() > 32
            || !locale
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        {
            return Err(WordlistError::Words);
        }
        if words.is_empty() {
            return Err(WordlistError::Words);
        }
        if words.len() > MAX_WORDLIST_WORDS {
            return Err(WordlistError::Limit);
        }
        let mut unique = BTreeSet::new();
        let mut total = 0usize;
        for word in &words {
            total = total.checked_add(word.len()).ok_or(WordlistError::Limit)?;
            if total > MAX_WORDLIST_BYTES || word.len() > 128 {
                return Err(WordlistError::Limit);
            }
            if word.is_empty()
                || word
                    .chars()
                    .any(|c| c.is_whitespace() || c.is_control() || c == '\u{feff}')
                || word.nfkd().ne(word.chars())
                || (style == WordlistStyle::General
                    && word.chars().flat_map(char::to_lowercase).ne(word.chars()))
                || (style == WordlistStyle::Chinese && word.chars().count() != 1)
                || !unique.insert(word.as_str())
            {
                return Err(WordlistError::Words);
            }
        }
        Ok(Self {
            locale: locale.into(),
            style,
            words: Words::Owned(words),
            owl: None,
            accents: None,
        })
    }
    /// Decode published OWL or OWL-A data and eagerly verify the supplied
    /// Keccak(words joined with newlines plus a final newline) checksum.
    /// Expansions, accent bit widths and output words are strictly bounded.
    pub fn from_owl(
        locale: &str,
        data: &str,
        accents: Option<&str>,
        checksum: Hash32,
    ) -> Result<Self, WordlistError> {
        if data.len() > MAX_WORDLIST_INPUT_BYTES
            || accents.is_some_and(|a| a.len() > MAX_WORDLIST_INPUT_BYTES)
        {
            return Err(WordlistError::Limit);
        }
        let mut words = decode_owl(data)?;
        if let Some(accents) = accents {
            apply_accents(&mut words, accents)?;
        }
        let mut result = Self::from_words(locale, words, WordlistStyle::General)?;
        if result.checksum() != checksum {
            return Err(WordlistError::Checksum);
        }
        result.owl = Some(data.into());
        result.accents = accents.map(str::to_owned);
        Ok(result)
    }
    /// Declared public locale tag.
    pub fn locale(&self) -> &str {
        &self.locale
    }
    /// Explicit phrase policy.
    pub const fn style(&self) -> WordlistStyle {
        self.style
    }
    /// Number of public dictionary entries.
    pub fn len(&self) -> usize {
        match &self.words {
            Words::Builtin(w) => w.len(),
            Words::Owned(w) => w.len(),
        }
    }
    /// Validated dictionaries are never empty.
    pub fn is_empty(&self) -> bool {
        false
    }
    /// Checked lookup; no negative index conversion or numeric coercion.
    pub fn get_word(&self, index: usize) -> Option<&str> {
        match &self.words {
            Words::Builtin(w) => w.get(index).copied(),
            Words::Owned(w) => w.get(index).map(String::as_str),
        }
    }
    /// Exact lookup. Case/NFKD normalization is explicit at phrase conversion.
    pub fn get_word_index(&self, word: &str) -> Option<u16> {
        (0..self.len())
            .find(|&i| self.get_word(i) == Some(word))
            .map(|i| i as u16)
    }
    /// Iterate all words in their encoded BIP39 index order.
    pub fn words(&self) -> impl ExactSizeIterator<Item = &str> {
        (0..self.len()).map(|i| self.get_word(i).unwrap())
    }
    /// Public dictionary checksum used by published compressed wordlists.
    pub fn checksum(&self) -> Hash32 {
        let mut bytes = Vec::new();
        for word in self.words() {
            bytes.extend_from_slice(word.as_bytes());
            bytes.push(b'\n');
        }
        quai_crypto::keccak256(&bytes).into()
    }
    /// Original compressed data if constructed through from_owl.
    pub fn owl_data(&self) -> Option<&str> {
        self.owl.as_deref()
    }
    /// Original accent overlay if constructed through from_owl with one.
    pub fn accent_data(&self) -> Option<&str> {
        self.accents.as_deref()
    }
    /// Split a bounded phrase using this list's explicit conventions. General and
    /// Japanese modes preserve edge empty fields like JavaScript regex split.
    /// Chinese mode uses Unicode scalars, never unpaired UTF-16 surrogates.
    pub fn split(&self, phrase: &str) -> Result<SecretWords, WordlistError> {
        if phrase.len() > MAX_PHRASE_BYTES {
            return Err(WordlistError::Limit);
        }
        // Deliberately `str::to_lowercase`, not a per-character fold. Only the
        // whole-string form implements Unicode `Final_Sigma`, so "ΟΔΟΣ" folds to
        // "οδος" here and to "οδοσ" character by character. The reference's
        // `toLowerCase` has the same rule, and a custom wordlist holding a
        // final-sigma word would otherwise stop matching an uppercase phrase —
        // a wallet that restored under an earlier release would not restore.
        //
        // The cost is that this grows its own buffer internally and can leave
        // unwiped fragments of the phrase on the heap, which the guards
        // elsewhere in this file avoid. Correctness of restore wins; sizing this
        // exactly needs a final-sigma-aware pass, which is a separate change.
        let text = if self.style == WordlistStyle::General {
            Zeroizing::new(phrase.to_lowercase())
        } else {
            // `to_owned` on a `&str` allocates once at the exact length.
            Zeroizing::new(phrase.to_owned())
        };
        if text.len() > MAX_PHRASE_BYTES {
            return Err(WordlistError::Limit);
        }
        let mut words = Zeroizing::new(Vec::new());
        if self.style == WordlistStyle::Chinese {
            for c in text.chars().filter(|c| !matches!(c, ' ' | '\u{3000}')) {
                words.push(c.to_string());
            }
        } else {
            let mut start = 0;
            let mut separator = false;
            for (i, c) in text.char_indices() {
                let is_separator = if self.style == WordlistStyle::General {
                    ecma_space(c)
                } else {
                    matches!(c, ' ' | '\u{3000}')
                };
                if is_separator {
                    if !separator {
                        words.push(text[start..i].to_owned());
                    }
                    start = i + c.len_utf8();
                }
                separator = is_separator;
            }
            words.push(text[start..].to_owned());
        }
        if words.len() > MAX_WORDLIST_WORDS {
            return Err(WordlistError::Limit);
        }
        Ok(SecretWords(words))
    }
    /// Join explicit words into a guarded phrase, bounded to 4,096 UTF-8 bytes.
    /// Joining does not validate mnemonic checksum or dictionary membership.
    pub fn join(&self, words: &[&str]) -> Result<SecretString, WordlistError> {
        if words.len() > MAX_WORDLIST_WORDS {
            return Err(WordlistError::Limit);
        }
        let separator = if self.style == WordlistStyle::Japanese {
            "\u{3000}"
        } else {
            " "
        };
        let size = words
            .iter()
            .try_fold(words.len().saturating_sub(1) * separator.len(), |n, w| {
                n.checked_add(w.len()).ok_or(WordlistError::Limit)
            })?;
        if size > MAX_PHRASE_BYTES {
            return Err(WordlistError::Limit);
        }
        let mut value = Zeroizing::new(String::with_capacity(size));
        for (i, word) in words.iter().enumerate() {
            if i > 0 {
                value.push_str(separator);
            }
            value.push_str(word);
        }
        Ok(SecretString(value))
    }
}
fn ecma_space(c: char) -> bool {
    matches!(c,'\u{0009}'..='\u{000d}'|'\u{0020}'|'\u{00a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}')
}
const SUBS: &[u8] = b" !#$%&'()*+,-./<=>?@[]^_`{|}~";
fn push(out: &mut String, text: &str) -> Result<(), WordlistError> {
    if out.len().saturating_add(text.len()) > MAX_WORDLIST_BYTES {
        return Err(WordlistError::Limit);
    }
    out.push_str(text);
    Ok(())
}
fn decode_owl(data: &str) -> Result<Vec<String>, WordlistError> {
    let offset = 1 + 2 * SUBS.len();
    if !data.is_ascii() || !data.starts_with('0') || data.len() < offset {
        return Err(WordlistError::Encoding);
    }
    let substitutions = &data.as_bytes()[1..offset];
    let mut expanded = data[offset..].to_owned();
    for i in (0..SUBS.len()).rev() {
        let mut next = String::new();
        for b in expanded.bytes() {
            if b == SUBS[i] {
                push(
                    &mut next,
                    std::str::from_utf8(&substitutions[2 * i..2 * i + 2]).unwrap(),
                )?;
            } else {
                push(&mut next, std::str::from_utf8(&[b]).unwrap())?;
            }
        }
        expanded = next;
    }
    let mut clumps = Vec::new();
    let bytes = expanded.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if clumps.len() > MAX_WORDLIST_WORDS * 4 {
            return Err(WordlistError::Limit);
        }
        match bytes[i] {
            b':' => {
                clumps.push(":".into());
                i += 1;
            }
            b'0'..=b'9' => {
                for _ in 0..=bytes[i] - b'0' {
                    clumps.push(";".into());
                }
                i += 1;
            }
            b'A'..=b'Z' => {
                let start = i;
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_lowercase() {
                    i += 1;
                }
                clumps.push(expanded[start..i].to_ascii_lowercase());
            }
            _ => return Err(WordlistError::Encoding),
        }
    }
    fn unfold(words: Vec<String>, separator: &str) -> Result<Vec<String>, WordlistError> {
        let mut initial = b'a';
        let mut result = Vec::new();
        let mut total = 0usize;
        for word in words {
            if word == separator {
                initial = initial.checked_add(1).ok_or(WordlistError::Encoding)?;
            } else if word.bytes().all(|b| b.is_ascii_alphabetic()) {
                if initial > b'z' {
                    return Err(WordlistError::Encoding);
                }
                let mut value = String::new();
                value.push(char::from(initial));
                push(&mut value, &word)?;
                total = total.checked_add(value.len()).ok_or(WordlistError::Limit)?;
                result.push(value);
            } else {
                initial = b'a';
                total = total.checked_add(word.len()).ok_or(WordlistError::Limit)?;
                result.push(word);
            }
            if total > MAX_WORDLIST_BYTES || result.len() > MAX_WORDLIST_WORDS * 4 {
                return Err(WordlistError::Limit);
            }
        }
        Ok(result)
    }
    unfold(unfold(clumps, ";")?, ":")
}
fn decode_bits(width: u32, data: &str) -> Result<Vec<usize>, WordlistError> {
    const ALPHABET: &[u8] = b")!@#$%^&*(ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz-_";
    if !(1..=9).contains(&width) {
        return Err(WordlistError::Encoding);
    }
    let max = (1u32 << width) - 1;
    let (mut accum, mut bits, mut flood) = (0u32, 0u32, 0usize);
    let mut values = Vec::new();
    for b in data.bytes() {
        let value = ALPHABET
            .iter()
            .position(|v| *v == b)
            .ok_or(WordlistError::Encoding)? as u32;
        accum = (accum << 6) | value;
        bits += 6;
        while bits >= width {
            bits -= width;
            let value = accum >> bits;
            accum &= (1 << bits) - 1;
            if value == 0 {
                flood = flood
                    .checked_add(max as usize)
                    .ok_or(WordlistError::Limit)?;
                if flood > MAX_WORDLIST_BYTES {
                    return Err(WordlistError::Limit);
                }
            } else {
                values.push(value as usize + flood);
                flood = 0;
                if values.len() > MAX_WORDLIST_WORDS {
                    return Err(WordlistError::Limit);
                }
            }
        }
    }
    Ok(values)
}
fn apply_accents(words: &mut Vec<String>, accents: &str) -> Result<(), WordlistError> {
    if !accents.is_ascii() {
        return Err(WordlistError::Encoding);
    }
    let mut text = words.join(",");
    if text.len() > MAX_WORDLIST_BYTES {
        return Err(WordlistError::Limit);
    }
    let overlays: Vec<_> = accents.split(',').collect();
    if overlays.len() > 32 {
        return Err(WordlistError::Limit);
    }
    for overlay in overlays {
        let bytes = overlay.as_bytes();
        let mut p = 0;
        while p < bytes.len() && bytes[p].is_ascii_lowercase() {
            p += 1;
        }
        let letters = &bytes[..p];
        let start = p;
        while p < bytes.len() && bytes[p].is_ascii_digit() {
            p += 1;
        }
        if letters.is_empty() || p - start < 2 {
            return Err(WordlistError::Encoding);
        }
        let code = overlay[start..p - 1]
            .parse::<u32>()
            .map_err(|_| WordlistError::Encoding)?;
        let mark = char::from_u32(code)
            .filter(|_| code <= 0xffff)
            .ok_or(WordlistError::Encoding)?;
        if !(0x0300..=0x036f).contains(&code) {
            return Err(WordlistError::Encoding);
        }
        let positions = decode_bits(u32::from(bytes[p - 1] - b'0'), &overlay[p..])?;
        let mut pos = 0;
        let mut remaining = positions.first().copied();
        let mut output = String::new();
        for c in text.chars() {
            let mut encoded = [0; 4];
            push(&mut output, c.encode_utf8(&mut encoded))?;
            if c.is_ascii()
                && letters.contains(&(c as u8))
                && let Some(n) = remaining
            {
                if n == 1 {
                    let mut encoded = [0; 4];
                    push(&mut output, mark.encode_utf8(&mut encoded))?;
                    pos += 1;
                    remaining = positions.get(pos).copied();
                } else {
                    remaining = Some(n - 1);
                }
            }
        }
        if pos != positions.len() {
            return Err(WordlistError::Encoding);
        }
        text = output;
    }
    *words = text.split(',').map(str::to_owned).collect();
    Ok(())
}

/// BIP39 mnemonic using an explicit immutable 2,048-word dictionary. The
/// dictionary is borrowed; phrase/entropy/seed exports require deliberate access.
/// No locale is guessed, and no passphrase is retained.
pub struct CustomMnemonic<'a> {
    inner: bip39::Mnemonic,
    wordlist: &'a Wordlist,
}
impl<'a> CustomMnemonic<'a> {
    /// Encode validated BIP39 entropy (16, 20, 24, 28 or 32 bytes) with this list.
    pub fn from_entropy(wordlist: &'a Wordlist, entropy: &[u8]) -> Result<Self, WordlistError> {
        if wordlist.len() != 2048 {
            return Err(WordlistError::Words);
        }
        let inner = bip39::Mnemonic::from_entropy_in(Language::English, entropy)
            .map_err(|_| WordlistError::Mnemonic)?;
        Ok(Self { inner, wordlist })
    }
    /// Parse this dictionary's phrase conventions, normalize each word with NFKD
    /// and verify its BIP39 checksum. Edge empty fields reject, as in the published
    /// custom-wordlist mnemonic conversion; dictionary indexes never change.
    pub fn parse(wordlist: &'a Wordlist, phrase: &str) -> Result<Self, WordlistError> {
        if wordlist.len() != 2048 {
            return Err(WordlistError::Words);
        }
        let split = wordlist.split(phrase)?;
        if !matches!(split.expose().len(), 12 | 15 | 18 | 21 | 24) {
            return Err(WordlistError::Mnemonic);
        }
        // The English rendering is as secret as the input phrase, so it is
        // reserved once. No BIP39 English word exceeds eight bytes, and one
        // separator follows all but the last.
        let mut english = Zeroizing::new(String::with_capacity(split.expose().len() * 9));
        for word in split.expose() {
            let normalized = crate::mnemonic::normalized_guard(|| word.nfkd());
            let index = wordlist
                .get_word_index(&normalized)
                .ok_or(WordlistError::Mnemonic)?;
            if !english.is_empty() {
                english.push(' ');
            }
            english.push_str(Language::English.word_list()[usize::from(index)]);
        }
        let inner = bip39::Mnemonic::parse_in_normalized(Language::English, &english)
            .map_err(|_| WordlistError::Mnemonic)?;
        Ok(Self { inner, wordlist })
    }
    /// Borrow the exact dictionary defining this mnemonic's identity.
    pub fn wordlist(&self) -> &Wordlist {
        self.wordlist
    }
    /// Canonical phrase in a redacted zeroizing string.
    pub fn phrase(&self) -> Result<SecretString, WordlistError> {
        let words: Vec<_> = self
            .inner
            .word_indices()
            .map(|i| self.wordlist.get_word(i).unwrap())
            .collect();
        self.wordlist.join(&words)
    }
    /// Original entropy without checksum bits, with guarded owned bytes.
    pub fn entropy(&self) -> crate::MnemonicEntropy {
        crate::MnemonicEntropy::from_backend(&self.inner)
    }
    /// Derive the exact BIP39 64-byte seed with an explicit passphrase. Uses
    /// PBKDF2-HMAC-SHA512, 2,048 rounds, and NFKD phrase/password normalization.
    /// Passphrases are capped at 4,096 bytes before/after normalization. Feed the
    /// returned seed to HdWallet::from_seed or BackupOrigin::from_seed explicitly.
    pub fn to_seed(&self, passphrase: &str) -> Result<crate::Seed, WordlistError> {
        if passphrase.len() > MAX_PHRASE_BYTES {
            return Err(WordlistError::Limit);
        }
        let phrase = self.phrase()?;
        let password = crate::mnemonic::normalized_guard(|| phrase.expose().nfkd());
        let passphrase = crate::mnemonic::normalized_guard(|| passphrase.nfkd());
        if password.len() > MAX_PHRASE_BYTES || passphrase.len() > MAX_PHRASE_BYTES {
            return Err(WordlistError::Limit);
        }
        let mut salt = Zeroizing::new(Vec::with_capacity(8 + passphrase.len() + 4));
        salt.extend_from_slice(b"mnemonic");
        salt.extend_from_slice(passphrase.as_bytes());
        salt.extend_from_slice(&[0, 0, 0, 1]);
        // SHA512's output is exactly the requested seed length: PBKDF2 block 1.
        let mut previous = Zeroizing::new(quai_crypto::hmac_sha512(password.as_bytes(), &salt));
        let mut seed = Zeroizing::new(*previous);
        for _ in 1..2048 {
            let next = Zeroizing::new(quai_crypto::hmac_sha512(password.as_bytes(), &*previous));
            for (out, byte) in seed.iter_mut().zip(next.iter()) {
                *out ^= *byte;
            }
            previous = next;
        }
        Ok(crate::Seed(seed))
    }
}
impl Drop for CustomMnemonic<'_> {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.inner.zeroize();
    }
}
impl fmt::Debug for CustomMnemonic<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CustomMnemonic([REDACTED])")
    }
}
