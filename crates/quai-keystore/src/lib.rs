//! Bounded legacy Web3 v3 / x-quais keystore interchange.
//!
//! Native authenticated full-wallet backups belong to `quai-wallet`. This format
//! only protects the private-key ciphertext with its legacy MAC. Optional mnemonic
//! metadata is accepted only when its derived key matches the decrypted key.
//! KDFs are synchronous CPU work: use a bounded worker, never an async executor/UI thread.
use aes::cipher::{KeyIvInit, StreamCipher};
use quai_crypto::{SecretKey, keccak256};
use quai_primitives::Address;
use quai_wallet::{ExtendedPrivateKey, Language, Mnemonic};
use serde_json::Value;
use std::fmt;
use subtle::ConstantTimeEq;
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;
/// Standalone guarded key derivation, independent of encrypted document parsing.
pub mod derive;
mod json;

/// Sanitized failures never include passwords, keys or mnemonic metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum KeystoreError {
    /// Unsupported or ambiguous document/cipher/KDF/extension.
    #[error("invalid or unsupported legacy keystore")]
    Format,
    /// Resource policy exceeded before expensive work.
    #[error("legacy keystore exceeds resource limits")]
    Limit,
    /// MAC, key/address or mnemonic derivation does not match.
    #[error("legacy keystore authentication or ownership check failed")]
    Authentication,
    /// OS or Web Crypto entropy failed.
    #[error("secure randomness unavailable")]
    Randomness,
    /// The document's KDF parameters are below the accepted work or salt floor.
    ///
    /// Distinct from [`KeystoreError::Limit`], which reports a ceiling, so a
    /// caller can deliberately accept a weak document by lowering the floor
    /// rather than by disabling the bound in both directions.
    #[error("legacy keystore key derivation is weaker than the accepted minimum")]
    WeakParameters,
}
/// Explicit text normalization versus exact password bytes, matching quais.js.
pub enum Password<'a> {
    /// UTF-8 after NFKC normalization (not BIP39's NFKD convention).
    Text(&'a str),
    /// Exact bytes without Unicode normalization.
    Bytes(&'a [u8]),
}
impl fmt::Debug for Password<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password([REDACTED])")
    }
}
impl Password<'_> {
    fn guarded(&self) -> Result<Zeroizing<Vec<u8>>, KeystoreError> {
        let bytes = match self {
            Self::Text(s) if s.len() <= 1024 => {
                // Allocate the final bounded guard before copying any normalized
                // secret; incremental growth must not leave prior heap copies.
                let mut normalized = Zeroizing::new(Vec::with_capacity(4096));
                let mut utf8 = Zeroizing::new([0u8; 4]);
                for character in s.nfkc() {
                    let encoded = character.encode_utf8(&mut utf8[..]).as_bytes();
                    if normalized.len() + encoded.len() > 4096 {
                        return Err(KeystoreError::Limit);
                    }
                    normalized.extend_from_slice(encoded);
                }
                normalized
            }
            Self::Bytes(b) if b.len() <= 1024 => Zeroizing::new(b.to_vec()),
            _ => return Err(KeystoreError::Limit),
        };
        if bytes.len() > 4096 {
            return Err(KeystoreError::Limit);
        }
        Ok(bytes)
    }
}
/// Fixed hostile-input KDF limits; all checked before deriving or allocating work memory.
#[derive(Clone, Copy, Debug)]
pub struct KdfLimits {
    /// Maximum scrypt V/B/T memory even with parallel feature unification; default 256 MiB.
    pub max_memory_bytes: u64,
    /// Maximum N*r*p work units; default 2^24.
    pub max_scrypt_work: u64,
    /// Maximum PBKDF2 rounds; default 2,000,000.
    pub max_pbkdf2_rounds: u32,
    /// Minimum N*r*p work units; default 2^20.
    ///
    /// Ceilings alone bound only what the document can cost *us*. A floor bounds
    /// what it costs an attacker: nothing stops a hostile file declaring
    /// `n=2, r=1, p=1`, which decrypts correctly and turns an import flow into
    /// an offline oracle against the password the user just typed, at roughly
    /// one hash per guess. Set to zero to accept any work factor deliberately.
    pub min_scrypt_work: u64,
    /// Minimum PBKDF2 rounds; default 100,000. Zero accepts any count.
    pub min_pbkdf2_rounds: u32,
    /// Minimum salt length in bytes; default 16.
    ///
    /// A short salt makes precomputation across victims viable; the format
    /// otherwise permits a single byte.
    pub min_salt_bytes: usize,
}
impl Default for KdfLimits {
    fn default() -> Self {
        Self {
            max_memory_bytes: 256 * 1024 * 1024,
            max_scrypt_work: 1 << 24,
            max_pbkdf2_rounds: 2_000_000,
            min_scrypt_work: 1 << 20,
            min_pbkdf2_rounds: 100_000,
            min_salt_bytes: 16,
        }
    }
}
#[derive(Clone, Copy)]
enum Kdf {
    Scrypt { log_n: u8, r: u32, p: u32 },
    Pbkdf2 { rounds: u32, sha512: bool },
}
impl Kdf {
    fn validate(self, limits: KdfLimits) -> Result<(), KeystoreError> {
        // Hard ceilings also bound callers who accidentally pass unbounded policies.
        if limits.max_memory_bytes > 1024 * 1024 * 1024
            || limits.max_scrypt_work > 1 << 28
            || limits.max_pbkdf2_rounds > 10_000_000
        {
            return Err(KeystoreError::Limit);
        }
        match self {
            Self::Scrypt { log_n, r, p } => {
                let n = 1u64.checked_shl(log_n.into()).ok_or(KeystoreError::Limit)?;
                // Cargo features unify: another dependency can enable scrypt/parallel.
                // Bound B plus a V/T workspace for every lane, not just one worker.
                let memory = n
                    .checked_add(2)
                    .and_then(|x| x.checked_mul(u64::from(p)))
                    .and_then(|x| x.checked_mul(u64::from(r)))
                    .and_then(|x| x.checked_mul(128))
                    .ok_or(KeystoreError::Limit)?;
                let work = n
                    .checked_mul(u64::from(r))
                    .and_then(|x| x.checked_mul(u64::from(p)))
                    .ok_or(KeystoreError::Limit)?;
                if memory > limits.max_memory_bytes || work > limits.max_scrypt_work {
                    return Err(KeystoreError::Limit);
                }
                if work < limits.min_scrypt_work {
                    return Err(KeystoreError::WeakParameters);
                }
                scrypt::Params::new(log_n, r, p).map_err(|_| KeystoreError::Format)?;
            }
            Self::Pbkdf2 { rounds, .. } => {
                if rounds == 0 || rounds > limits.max_pbkdf2_rounds {
                    return Err(KeystoreError::Limit);
                }
                if rounds < limits.min_pbkdf2_rounds {
                    return Err(KeystoreError::WeakParameters);
                }
            }
        }
        Ok(())
    }
    fn derive(
        self,
        password: &[u8],
        salt: &[u8],
        limits: KdfLimits,
    ) -> Result<Zeroizing<[u8; 64]>, KeystoreError> {
        self.validate(limits)?;
        // Checked here rather than at parse time so the floor applies wherever a
        // derivation actually happens, including the mnemonic section.
        if salt.len() < limits.min_salt_bytes {
            return Err(KeystoreError::WeakParameters);
        }
        let mut key = Zeroizing::new([0; 64]);
        match self {
            Self::Scrypt { log_n, r, p } => scrypt::scrypt(
                password,
                salt,
                &scrypt::Params::new(log_n, r, p).map_err(|_| KeystoreError::Format)?,
                key.as_mut(),
            )
            .map_err(|_| KeystoreError::Format)?,
            Self::Pbkdf2 {
                rounds,
                sha512: false,
            } => {
                pbkdf2::pbkdf2_hmac::<pbkdf2::sha2::Sha256>(password, salt, rounds, &mut key[..32])
            }
            Self::Pbkdf2 {
                rounds,
                sha512: true,
            } => {
                pbkdf2::pbkdf2_hmac::<pbkdf2::sha2::Sha512>(password, salt, rounds, &mut key[..32])
            }
        }
        Ok(key)
    }
}
struct MnemonicData {
    ciphertext: Vec<u8>,
    iv: [u8; 16],
    language: Language,
    path: String,
}
/// Validated, encrypted document. Parsing does no KDF or entropy work.
pub struct Keystore {
    address: Address,
    iv: [u8; 16],
    ciphertext: [u8; 32],
    mac: [u8; 32],
    salt: Vec<u8>,
    kdf: Kdf,
    mnemonic: Option<MnemonicData>,
}
impl fmt::Debug for Keystore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Keystore([ENCRYPTED])")
    }
}
/// Decrypted key and optional mnemonic whose derivation was checked against it.
pub struct KeystoreAccount {
    key: SecretKey,
    mnemonic: Option<VerifiedMnemonic>,
}
impl fmt::Debug for KeystoreAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("KeystoreAccount([REDACTED])")
    }
}
impl KeystoreAccount {
    /// Public account derived from the actual key.
    pub fn address(&self) -> Address {
        self.key.public_key().address()
    }
    /// Borrow the guarded signing key explicitly.
    pub fn secret_key(&self) -> &SecretKey {
        &self.key
    }
    /// Mnemonic present only after empty-passphrase derivation ownership verification.
    pub fn mnemonic(&self) -> Option<&VerifiedMnemonic> {
        self.mnemonic.as_ref()
    }
    /// Transfer key ownership; any mnemonic held in this object is dropped and zeroized.
    pub fn into_secret_key(self) -> SecretKey {
        self.key
    }
}
/// Derivation-verified mnemonic. Legacy format cannot represent a BIP39 passphrase.
pub struct VerifiedMnemonic {
    mnemonic: Mnemonic,
    path: String,
}
impl fmt::Debug for VerifiedMnemonic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("VerifiedMnemonic([REDACTED])")
    }
}
impl VerifiedMnemonic {
    /// Explicit secret mnemonic access.
    pub fn mnemonic(&self) -> &Mnemonic {
        &self.mnemonic
    }
    /// Exact absolute derivation path validated against the private key.
    pub fn path(&self) -> &str {
        &self.path
    }
}
fn string(v: &Value) -> Result<&str, KeystoreError> {
    v.as_str().ok_or(KeystoreError::Format)
}
fn uint(v: &Value) -> Result<u64, KeystoreError> {
    v.as_u64().ok_or(KeystoreError::Format)
}
fn bytes(v: &Value, min: usize, max: usize) -> Result<Vec<u8>, KeystoreError> {
    let text = string(v)?;
    let text = text.strip_prefix("0x").unwrap_or(text);
    if text.len() % 2 != 0
        || text.len() / 2 < min
        || text.len() / 2 > max
        || !text.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(KeystoreError::Format);
    }
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            u8::from_str_radix(
                std::str::from_utf8(pair).map_err(|_| KeystoreError::Format)?,
                16,
            )
            .map_err(|_| KeystoreError::Format)
        })
        .collect()
}
fn fixed<const N: usize>(v: &Value) -> Result<[u8; N], KeystoreError> {
    bytes(v, N, N)?
        .try_into()
        .map_err(|_| KeystoreError::Format)
}
fn allowed(v: &Value, keys: &[&str]) -> Result<(), KeystoreError> {
    let map = v.as_object().ok_or(KeystoreError::Format)?;
    if map.keys().any(|k| !keys.contains(&k.as_str())) {
        return Err(KeystoreError::Format);
    }
    Ok(())
}
fn language(locale: &str) -> Result<Language, KeystoreError> {
    match locale {
        "en" => Ok(Language::English),
        "es" => Ok(Language::Spanish),
        "fr" => Ok(Language::French),
        "it" => Ok(Language::Italian),
        "ja" => Ok(Language::Japanese),
        "ko" => Ok(Language::Korean),
        "zh_cn" => Ok(Language::SimplifiedChinese),
        "zh_tw" => Ok(Language::TraditionalChinese),
        "cz" => Ok(Language::Czech),
        "pt" => Ok(Language::Portuguese),
        _ => Err(KeystoreError::Format),
    }
}
impl Keystore {
    /// Parse strict v3 AES-128-CTR with scrypt or PBKDF2-HMAC-SHA256/SHA512.
    /// Case-insensitive field aliases are accepted only if unique. Unknown extensions
    /// fail explicitly. An address is mandatory to detect un-MACed IV tampering.
    pub fn from_json(document: &[u8], limits: KdfLimits) -> Result<Self, KeystoreError> {
        let v = json::parse(document)?;
        allowed(&v, &["address", "id", "version", "crypto", "x-quais"])?;
        if uint(&v["version"])? != 3 {
            return Err(KeystoreError::Format);
        }
        let address = Address::from_bytes(fixed::<20>(&v["address"])?);
        let c = &v["crypto"];
        allowed(
            c,
            &[
                "cipher",
                "cipherparams",
                "ciphertext",
                "kdf",
                "kdfparams",
                "mac",
            ],
        )?;
        allowed(&c["cipherparams"], &["iv"])?;
        if string(&c["cipher"])? != "aes-128-ctr" {
            return Err(KeystoreError::Format);
        }
        let p = &c["kdfparams"];
        if uint(&p["dklen"])? != 32 {
            return Err(KeystoreError::Format);
        }
        let kdf = match string(&c["kdf"])? {
            "scrypt" => {
                allowed(p, &["salt", "n", "r", "p", "dklen"])?;
                let n = uint(&p["n"])?;
                if !n.is_power_of_two() || n < 2 {
                    return Err(KeystoreError::Format);
                }
                Kdf::Scrypt {
                    log_n: n.trailing_zeros() as u8,
                    r: uint(&p["r"])?
                        .try_into()
                        .map_err(|_| KeystoreError::Limit)?,
                    p: uint(&p["p"])?
                        .try_into()
                        .map_err(|_| KeystoreError::Limit)?,
                }
            }
            "pbkdf2" => {
                allowed(p, &["salt", "c", "prf", "dklen"])?;
                let sha512 = match string(&p["prf"])? {
                    "hmac-sha256" => false,
                    "hmac-sha512" => true,
                    _ => return Err(KeystoreError::Format),
                };
                Kdf::Pbkdf2 {
                    rounds: uint(&p["c"])?
                        .try_into()
                        .map_err(|_| KeystoreError::Limit)?,
                    sha512,
                }
            }
            _ => return Err(KeystoreError::Format),
        };
        kdf.validate(limits)?;
        let mnemonic = if let Some(m) = v.get("x-quais") {
            allowed(
                m,
                &[
                    "version",
                    "mnemonicciphertext",
                    "mnemoniccounter",
                    "path",
                    "locale",
                    "client",
                    "gethfilename",
                ],
            )?;
            if !matches!(kdf, Kdf::Scrypt { .. }) || string(&m["version"])? != "0.1" {
                return Err(KeystoreError::Format);
            }
            let ciphertext = bytes(&m["mnemonicciphertext"], 16, 32)?;
            if ciphertext.len() % 4 != 0 {
                return Err(KeystoreError::Format);
            }
            let path = m
                .get("path")
                .map(string)
                .transpose()?
                .unwrap_or("m/44'/994'/0'/0/0");
            if path.is_empty() || path.len() > 1024 {
                return Err(KeystoreError::Limit);
            }
            Some(MnemonicData {
                ciphertext,
                iv: fixed(&m["mnemoniccounter"])?,
                path: path.to_owned(),
                language: language(m.get("locale").map(string).transpose()?.unwrap_or("en"))?,
            })
        } else {
            None
        };
        Ok(Self {
            address,
            iv: fixed(&c["cipherparams"]["iv"])?,
            ciphertext: fixed(&c["ciphertext"])?,
            mac: fixed(&c["mac"])?,
            salt: bytes(&p["salt"], 1, 64)?,
            kdf,
            mnemonic,
        })
    }
    /// Synchronously derive, verify MAC, decrypt and validate all available ownership.
    /// KDF limits are rechecked here so parsing with a larger limit cannot bypass policy.
    pub fn decrypt(
        &self,
        password: Password<'_>,
        limits: KdfLimits,
    ) -> Result<KeystoreAccount, KeystoreError> {
        let password = password.guarded()?;
        let derived = self.kdf.derive(&password, &self.salt, limits)?;
        let mut mac_data = Zeroizing::new([0u8; 48]);
        mac_data[..16].copy_from_slice(&derived[16..32]);
        mac_data[16..].copy_from_slice(&self.ciphertext);
        if !bool::from(keccak256(&mac_data[..]).ct_eq(&self.mac)) {
            return Err(KeystoreError::Authentication);
        }
        let mut plain = Zeroizing::new(self.ciphertext);
        ctr::Ctr128BE::<aes::Aes128>::new_from_slices(&derived[..16], &self.iv)
            .map_err(|_| KeystoreError::Format)?
            .try_apply_keystream(&mut plain[..])
            .map_err(|_| KeystoreError::Format)?;
        let key = SecretKey::from_bytes(&plain).map_err(|_| KeystoreError::Authentication)?;
        if key.public_key().address() != self.address {
            return Err(KeystoreError::Authentication);
        }
        let mnemonic = if let Some(m) = &self.mnemonic {
            let mut entropy = Zeroizing::new(m.ciphertext.clone());
            ctr::Ctr128BE::<aes::Aes256>::new_from_slices(&derived[32..], &m.iv)
                .map_err(|_| KeystoreError::Format)?
                .try_apply_keystream(&mut entropy)
                .map_err(|_| KeystoreError::Format)?;
            Some(verify_mnemonic(&key, &entropy, m.language, &m.path)?)
        } else {
            None
        };
        Ok(KeystoreAccount { key, mnemonic })
    }
}
fn verify_mnemonic(
    key: &SecretKey,
    entropy: &[u8],
    language: Language,
    path: &str,
) -> Result<VerifiedMnemonic, KeystoreError> {
    let mnemonic =
        Mnemonic::from_entropy(language, entropy).map_err(|_| KeystoreError::Authentication)?;
    let seed = mnemonic.to_seed("");
    let derived = ExtendedPrivateKey::from_seed(seed.expose())
        .and_then(|root| root.derive_path(path))
        .and_then(|node| node.secret_key())
        .map_err(|_| KeystoreError::Authentication)?;
    if !bool::from(
        derived
            .export_bytes()
            .as_bytes()
            .ct_eq(key.export_bytes().as_bytes()),
    ) {
        return Err(KeystoreError::Authentication);
    }
    Ok(VerifiedMnemonic {
        mnemonic,
        path: path.to_owned(),
    })
}

/// Encrypted legacy JSON; diagnostics omit ciphertext, identifiers and metadata.
pub struct EncryptedKeystore(String);
impl fmt::Debug for EncryptedKeystore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EncryptedKeystore([ENCRYPTED])")
    }
}
impl EncryptedKeystore {
    /// Explicit export of encrypted JSON for legacy interchange.
    pub fn as_json(&self) -> &str {
        &self.0
    }
}
/// Export a single key using fresh OS/Web Crypto salt, IV and UUID and pinned-JS scrypt defaults.
/// This synchronous operation uses about 128 MiB of scrypt working memory.
pub fn encrypt(
    key: &SecretKey,
    password: Password<'_>,
) -> Result<EncryptedKeystore, KeystoreError> {
    encrypt_randomized(key, password, None)
}
/// Export a derivation-verified mnemonic with a legacy key. Only the empty BIP39
/// passphrase is representable; mismatched entropy/language/path fails before KDF.
/// Metadata is not authenticated by this legacy format; import always rederives it.
pub fn encrypt_with_mnemonic(
    key: &SecretKey,
    password: Password<'_>,
    entropy: &[u8],
    language: Language,
    path: &str,
) -> Result<EncryptedKeystore, KeystoreError> {
    if path.len() > 1024 {
        return Err(KeystoreError::Limit);
    }
    verify_mnemonic(key, entropy, language, path)?;
    encrypt_randomized(key, password, Some((entropy, language, path)))
}
fn encrypt_randomized(
    key: &SecretKey,
    password: Password<'_>,
    mnemonic: Option<(&[u8], Language, &str)>,
) -> Result<EncryptedKeystore, KeystoreError> {
    let password = password.guarded()?;
    let mut random = [0; 80];
    quai_crypto::fill_random(&mut random).map_err(|_| KeystoreError::Randomness)?;
    seal(
        key,
        &password,
        mnemonic,
        Kdf::Scrypt {
            log_n: 17,
            r: 8,
            p: 1,
        },
        &random,
        KdfLimits::default(),
    )
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn locale(language: Language) -> &'static str {
    match language {
        Language::English => "en",
        Language::Spanish => "es",
        Language::French => "fr",
        Language::Italian => "it",
        Language::Japanese => "ja",
        Language::Korean => "ko",
        Language::SimplifiedChinese => "zh_cn",
        Language::TraditionalChinese => "zh_tw",
        Language::Czech => "cz",
        Language::Portuguese => "pt",
    }
}
/// `limits` governs the derivation this export performs. Production callers pass
/// the defaults; the deterministic JavaScript-parity tests lower the floor so
/// they can reproduce the reference's deliberately cheap fixture parameters.
fn seal(
    key: &SecretKey,
    password: &[u8],
    mnemonic: Option<(&[u8], Language, &str)>,
    kdf: Kdf,
    random: &[u8; 80],
    limits: KdfLimits,
) -> Result<EncryptedKeystore, KeystoreError> {
    let Kdf::Scrypt { log_n, r, p } = kdf else {
        return Err(KeystoreError::Format);
    };
    let derived = kdf.derive(password, &random[..32], limits)?;
    let mut ciphertext = Zeroizing::new(*key.export_bytes().as_bytes());
    ctr::Ctr128BE::<aes::Aes128>::new_from_slices(&derived[..16], &random[32..48])
        .map_err(|_| KeystoreError::Format)?
        .try_apply_keystream(&mut ciphertext[..])
        .map_err(|_| KeystoreError::Format)?;
    let mut mac_data = Zeroizing::new([0; 48]);
    mac_data[..16].copy_from_slice(&derived[16..32]);
    mac_data[16..].copy_from_slice(&ciphertext[..]);
    let mut uuid: [u8; 16] = random[48..64]
        .try_into()
        .map_err(|_| KeystoreError::Format)?;
    uuid[6] = (uuid[6] & 15) | 64;
    uuid[8] = (uuid[8] & 63) | 128;
    let id = hex(&uuid);
    let id = format!(
        "{}-{}-{}-{}-{}",
        &id[..8],
        &id[8..12],
        &id[12..16],
        &id[16..20],
        &id[20..]
    );
    let mut document = serde_json::json!({"address":hex(key.public_key().address().bytes()),"id":id,"version":3,"Crypto":{"cipher":"aes-128-ctr","cipherparams":{"iv":hex(&random[32..48])},"ciphertext":hex(&ciphertext[..]),"kdf":"scrypt","kdfparams":{"salt":hex(&random[..32]),"n":1u64<<log_n,"r":r,"p":p,"dklen":32},"mac":hex(&keccak256(&mac_data[..]))}});
    if let Some((entropy, language, path)) = mnemonic {
        let mut encrypted = Zeroizing::new(entropy.to_vec());
        ctr::Ctr128BE::<aes::Aes256>::new_from_slices(&derived[32..], &random[64..])
            .map_err(|_| KeystoreError::Format)?
            .try_apply_keystream(&mut encrypted)
            .map_err(|_| KeystoreError::Format)?;
        document["x-quais"] = serde_json::json!({"version":"0.1","client":"quai-rust-sdk","path":path,"locale":locale(language),"mnemonicCounter":hex(&random[64..]),"mnemonicCiphertext":hex(&encrypted)});
    }
    Ok(EncryptedKeystore(
        serde_json::to_string(&document).map_err(|_| KeystoreError::Format)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Limits that accept the reference fixtures' deliberately cheap KDF parameters.
    ///
    /// The pinned JavaScript vectors use `n=16, r=1, p=1` so the suite runs fast.
    /// Production defaults reject that as `WeakParameters`; importing a fixture is
    /// the intended "I know this is weak" case, so it lowers the floor explicitly
    /// rather than the suite disabling the bound globally.
    fn fixture_limits() -> KdfLimits {
        KdfLimits {
            min_scrypt_work: 0,
            min_pbkdf2_rounds: 0,
            min_salt_bytes: 0,
            ..KdfLimits::default()
        }
    }

    #[test]
    fn normalized_password_growth_is_bounded_before_secret_reallocation() {
        // U+FDFA expands to a 33-byte Arabic phrase under NFKC.
        let near_limit = "\u{fdfa}".repeat(124);
        let guarded = Password::Text(&near_limit).guarded().unwrap();
        assert_eq!(guarded.len(), 4092);
        assert_eq!(guarded.capacity(), 4096);
        let over_limit = "\u{fdfa}".repeat(125);
        assert!(matches!(
            Password::Text(&over_limit).guarded(),
            Err(KeystoreError::Limit)
        ));
        assert!(Password::Text("").guarded().unwrap().is_empty());
    }

    #[test]
    fn deterministic_rust_encryption_matches_javascript_ciphertext_mac_and_uuid() {
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/keystores.json")).unwrap();
        let v = &fixture["vectors"][0];
        let key_bytes = fixed::<32>(&v["expected"]["privateKey"]).unwrap();
        let key = SecretKey::from_bytes(&key_bytes).unwrap();
        let mut random = [0; 80];
        random[..32].fill(0x11);
        random[32..48].fill(0x22);
        random[48..64].fill(0x33);
        random[64..].fill(0x44);
        let encrypted = seal(
            &key,
            b"PUBLIC password",
            None,
            Kdf::Scrypt {
                log_n: 4,
                r: 1,
                p: 1,
            },
            &random,
            fixture_limits(),
        )
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(encrypted.as_json()).unwrap(),
            v["json"]
        );
        for row in fixture["vectors"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["expected"].get("mnemonic").is_some())
        {
            let key =
                SecretKey::from_bytes(&fixed(&row["expected"]["privateKey"]).unwrap()).unwrap();
            let m = &row["expected"]["mnemonic"];
            let entropy = bytes(&m["entropy"], 16, 32).unwrap();
            let language = language(string(&m["locale"]).unwrap()).unwrap();
            let path = string(&m["path"]).unwrap();
            let encrypted = seal(
                &key,
                b"PUBLIC mnemonic",
                Some((&entropy, language, path)),
                Kdf::Scrypt {
                    log_n: 4,
                    r: 1,
                    p: 1,
                },
                &random,
                fixture_limits(),
            )
            .unwrap();
            let result: Value = serde_json::from_str(encrypted.as_json()).unwrap();
            assert_eq!(result["Crypto"], row["json"]["Crypto"]);
            for field in ["mnemonicCiphertext", "mnemonicCounter", "path", "locale"] {
                assert_eq!(result["x-quais"][field], row["json"]["x-quais"][field]);
            }
            Keystore::from_json(encrypted.as_json().as_bytes(), fixture_limits())
                .unwrap()
                .decrypt(Password::Text("PUBLIC mnemonic"), fixture_limits())
                .unwrap();
        }
    }
}
