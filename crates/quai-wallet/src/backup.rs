//! Version-one authenticated seed envelope. This is not a full wallet snapshot.
use crate::{CoinType, HdWallet, WalletError};
use argon2::{Algorithm, Argon2, Block, Params, Version};
use chacha20poly1305::{AeadInOut, KeyInit, Tag, XChaCha20Poly1305, XNonce};
use core::fmt;
use thiserror::Error;
use zeroize::Zeroizing;

const MAGIC: &[u8; 8] = b"QUAISEED";
const HEADER_LENGTH: usize = 64;
const PAYLOAD_LENGTH: usize = 71;
const ENVELOPE_LENGTH: usize = HEADER_LENGTH + PAYLOAD_LENGTH + 16;
const MAX_PASSWORD_LENGTH: usize = 1024;

/// Backup errors do not reveal input secrets or distinguish corruption from a wrong password.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
#[non_exhaustive]
pub enum BackupError {
    /// Input seed length or account index is invalid.
    #[error("invalid seed backup input")]
    InvalidSeed,
    /// Encryption requires a nonempty password of at most 1024 bytes.
    #[error("backup password must contain 1 through 1024 bytes")]
    InvalidPassword,
    /// Parameters exceed the format's accepted cost range.
    #[error("invalid backup KDF parameters")]
    InvalidParameters,
    /// Wrong password, tampered ciphertext/header, or unsupported/malformed format.
    #[error("unable to unlock seed backup")]
    UnlockFailed,
    /// The operating system could not supply secure random salt/nonce bytes.
    #[error("secure randomness unavailable")]
    RandomnessUnavailable,
    /// Bounded memory allocation or cryptographic processing could not complete.
    #[error("backup cryptographic resources unavailable")]
    ResourcesUnavailable,
}

/// Explicit Argon2id v1.3 cost parameters, bounded before any KDF allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackupKdf {
    memory_kib: u32,
    iterations: u32,
    lanes: u32,
}
impl Default for BackupKdf {
    /// RFC 9106's second recommended profile: 64 MiB, three passes, four lanes.
    fn default() -> Self {
        Self {
            memory_kib: 65536,
            iterations: 3,
            lanes: 4,
        }
    }
}
impl BackupKdf {
    /// Accept 64–256 MiB, 3–6 passes, 1–4 lanes, and at most 768 MiB-passes.
    /// The limit bounds one operation; callers must also bound concurrent operations.
    pub fn new(memory_kib: u32, iterations: u32, lanes: u32) -> Result<Self, BackupError> {
        let result = Self {
            memory_kib,
            iterations,
            lanes,
        };
        result.validate()?;
        Ok(result)
    }
    fn validate(self) -> Result<(), BackupError> {
        if !(65536..=262144).contains(&self.memory_kib)
            || !(3..=6).contains(&self.iterations)
            || !(1..=4).contains(&self.lanes)
            || u64::from(self.memory_kib) * u64::from(self.iterations) > 786432
        {
            return Err(BackupError::InvalidParameters);
        }
        Ok(())
    }
    /// Memory cost in KiB.
    pub fn memory_kib(self) -> u32 {
        self.memory_kib
    }
    /// Argon2 pass count.
    pub fn iterations(self) -> u32 {
        self.iterations
    }
    /// Argon2 lane count; this does not spawn application threads.
    pub fn lanes(self) -> u32 {
        self.lanes
    }
}

/// Original seed bytes plus explicit coin and preferred account metadata.
///
/// This retains seed identity, including an already-applied BIP39 passphrase.
/// It does not retain a mnemonic, imported private keys, payment channels,
/// issued address indexes, outpoints, or synchronization state.
pub struct SeedBackup {
    seed: Zeroizing<[u8; 64]>,
    length: u8,
    coin: CoinType,
    account: u32,
}
impl fmt::Debug for SeedBackup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SeedBackup([REDACTED])")
    }
}
impl SeedBackup {
    /// Copy an original 16..=64-byte seed into zeroizing storage.
    /// The caller remains responsible for its input buffer and password strength.
    pub fn new(seed: &[u8], coin: CoinType, account: u32) -> Result<Self, BackupError> {
        if !(16..=64).contains(&seed.len()) || account >= (1 << 31) {
            return Err(BackupError::InvalidSeed);
        }
        let mut buffer = Zeroizing::new([0u8; 64]);
        buffer[..seed.len()].copy_from_slice(seed);
        Ok(Self {
            seed: buffer,
            length: seed.len() as u8,
            coin,
            account,
        })
    }
    /// Explicitly borrow original seed bytes; the unused padded area is not exposed.
    pub fn expose_seed(&self) -> &[u8] {
        &self.seed[..usize::from(self.length)]
    }
    /// Coin selected when the backup was created.
    pub fn coin_type(&self) -> CoinType {
        self.coin
    }
    /// Preferred account metadata; the seed still derives all accounts.
    pub fn account_index(&self) -> u32 {
        self.account
    }
    /// Reconstruct the same coin-level HD identity from the preserved seed.
    pub fn wallet(&self) -> Result<HdWallet, WalletError> {
        HdWallet::from_seed(self.expose_seed(), self.coin)
    }

    /// Encrypt with fresh operating-system salt/nonce bytes and exact password bytes.
    /// This synchronous memory-hard operation belongs on a caller-managed CPU worker.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn encrypt(
        &self,
        password: &[u8],
        kdf: BackupKdf,
    ) -> Result<EncryptedSeedBackup, BackupError> {
        validate_password(password)?;
        kdf.validate()?;
        let mut salt = [0u8; 16];
        let mut nonce = [0u8; 24];
        getrandom::fill(&mut salt).map_err(|_| BackupError::RandomnessUnavailable)?;
        getrandom::fill(&mut nonce).map_err(|_| BackupError::RandomnessUnavailable)?;
        self.encrypt_with_randomness(password, kdf, salt, nonce)
    }

    // Private implementation permits reproducible unit vectors. No public API
    // allows a caller to reuse a nonce or replace the OS random source.
    #[cfg(any(not(target_arch = "wasm32"), test))]
    fn encrypt_with_randomness(
        &self,
        password: &[u8],
        kdf: BackupKdf,
        salt: [u8; 16],
        nonce: [u8; 24],
    ) -> Result<EncryptedSeedBackup, BackupError> {
        validate_password(password)?;
        kdf.validate()?;
        let mut envelope = [0u8; ENVELOPE_LENGTH];
        envelope[..8].copy_from_slice(MAGIC);
        envelope[8..12].copy_from_slice(&[1, 1, 1, 0]); // format, Argon2id-v19, XChaCha, reserved
        envelope[12..16].copy_from_slice(&kdf.memory_kib.to_be_bytes());
        envelope[16..20].copy_from_slice(&kdf.iterations.to_be_bytes());
        envelope[20..24].copy_from_slice(&kdf.lanes.to_be_bytes());
        envelope[24..40].copy_from_slice(&salt);
        envelope[40..64].copy_from_slice(&nonce);
        let mut plaintext = Zeroizing::new([0u8; PAYLOAD_LENGTH]);
        plaintext[0] = self.length;
        plaintext[1..3].copy_from_slice(&(self.coin.number() as u16).to_be_bytes());
        plaintext[3..7].copy_from_slice(&self.account.to_be_bytes());
        plaintext[7..].copy_from_slice(&self.seed[..]);
        let key = derive_key(password, &salt, kdf)?;
        let cipher = XChaCha20Poly1305::new_from_slice(&key[..])
            .map_err(|_| BackupError::ResourcesUnavailable)?;
        let tag = cipher
            .encrypt_inout_detached(
                &XNonce::from(nonce),
                &envelope[..HEADER_LENGTH],
                (&mut plaintext[..]).into(),
            )
            .map_err(|_| BackupError::ResourcesUnavailable)?;
        envelope[HEADER_LENGTH..HEADER_LENGTH + PAYLOAD_LENGTH].copy_from_slice(&plaintext[..]);
        envelope[HEADER_LENGTH + PAYLOAD_LENGTH..].copy_from_slice(&tag);
        Ok(EncryptedSeedBackup(envelope))
    }
}

/// Fixed-size authenticated encrypted seed backup; safe to persist as ciphertext.
/// No file I/O or generic wallet serialization is performed by this type.
#[derive(Clone)]
pub struct EncryptedSeedBackup([u8; ENVELOPE_LENGTH]);
impl fmt::Debug for EncryptedSeedBackup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EncryptedSeedBackup([REDACTED])")
    }
}
impl EncryptedSeedBackup {
    /// Exact version-one binary envelope size; readers reject every other length.
    pub const ENCODED_LENGTH: usize = ENVELOPE_LENGTH;

    /// Parse bounded structural metadata only. Authentication occurs during decrypt.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, BackupError> {
        if bytes.len() != ENVELOPE_LENGTH {
            return Err(BackupError::UnlockFailed);
        }
        read_header(bytes)?;
        let mut envelope = [0u8; ENVELOPE_LENGTH];
        envelope.copy_from_slice(bytes);
        Ok(Self(envelope))
    }
    /// Borrow encrypted bytes for caller-managed persistence.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Authenticate and decrypt; wrong password and malformed/tampered data share one error.
    pub fn decrypt(&self, password: &[u8]) -> Result<SeedBackup, BackupError> {
        validate_password(password).map_err(|_| BackupError::UnlockFailed)?;
        let kdf = read_header(&self.0)?;
        let salt: &[u8; 16] = self.0[24..40]
            .try_into()
            .map_err(|_| BackupError::UnlockFailed)?;
        let nonce: [u8; 24] = self.0[40..64]
            .try_into()
            .map_err(|_| BackupError::UnlockFailed)?;
        let key = derive_key(password, salt, kdf)?;
        let cipher = XChaCha20Poly1305::new_from_slice(&key[..])
            .map_err(|_| BackupError::ResourcesUnavailable)?;
        let mut plaintext = Zeroizing::new([0u8; PAYLOAD_LENGTH]);
        plaintext.copy_from_slice(&self.0[HEADER_LENGTH..HEADER_LENGTH + PAYLOAD_LENGTH]);
        let tag: [u8; 16] = self.0[HEADER_LENGTH + PAYLOAD_LENGTH..]
            .try_into()
            .map_err(|_| BackupError::UnlockFailed)?;
        cipher
            .decrypt_inout_detached(
                &XNonce::from(nonce),
                &self.0[..HEADER_LENGTH],
                (&mut plaintext[..]).into(),
                &Tag::from(tag),
            )
            .map_err(|_| BackupError::UnlockFailed)?;
        let length = usize::from(plaintext[0]);
        if !(16..=64).contains(&length) || plaintext[7 + length..].iter().any(|byte| *byte != 0) {
            return Err(BackupError::UnlockFailed);
        }
        let coin = match u16::from_be_bytes([plaintext[1], plaintext[2]]) {
            994 => CoinType::Quai,
            969 => CoinType::Qi,
            _ => return Err(BackupError::UnlockFailed),
        };
        let account = u32::from_be_bytes(
            plaintext[3..7]
                .try_into()
                .map_err(|_| BackupError::UnlockFailed)?,
        );
        SeedBackup::new(&plaintext[7..7 + length], coin, account)
            .map_err(|_| BackupError::UnlockFailed)
    }
}

fn validate_password(password: &[u8]) -> Result<(), BackupError> {
    if password.is_empty() || password.len() > MAX_PASSWORD_LENGTH {
        return Err(BackupError::InvalidPassword);
    }
    Ok(())
}
fn read_header(bytes: &[u8]) -> Result<BackupKdf, BackupError> {
    if bytes.len() != ENVELOPE_LENGTH || &bytes[..8] != MAGIC || bytes[8..12] != [1, 1, 1, 0] {
        return Err(BackupError::UnlockFailed);
    }
    let read = |offset| -> Result<u32, BackupError> {
        Ok(u32::from_be_bytes(
            bytes[offset..offset + 4]
                .try_into()
                .map_err(|_| BackupError::UnlockFailed)?,
        ))
    };
    BackupKdf::new(read(12)?, read(16)?, read(20)?).map_err(|_| BackupError::UnlockFailed)
}

pub(crate) fn derive_key(
    password: &[u8],
    salt: &[u8; 16],
    kdf: BackupKdf,
) -> Result<Zeroizing<[u8; 32]>, BackupError> {
    // Revalidate immediately before allocation, including callers in this module.
    kdf.validate()?;
    let params = Params::new(kdf.memory_kib, kdf.iterations, kdf.lanes, Some(32))
        .map_err(|_| BackupError::ResourcesUnavailable)?;
    // argon2's convenience allocation does not erase its working arena on drop.
    // Own a fallibly allocated Zeroizing arena so both success and failure wipe it.
    let mut arena = Zeroizing::new(Vec::<Block>::new());
    arena
        .try_reserve_exact(params.block_count())
        .map_err(|_| BackupError::ResourcesUnavailable)?;
    arena.resize(params.block_count(), Block::new());
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into_with_memory(password, salt, &mut key[..], &mut arena[..])
        .map_err(|_| BackupError::ResourcesUnavailable)?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn unhex(text: &str) -> Vec<u8> {
        text.as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(core::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }
    fn vector() -> serde_json::Value {
        serde_json::from_str(include_str!("../tests/backup-vector.json")).unwrap()
    }
    fn fixture() -> EncryptedSeedBackup {
        EncryptedSeedBackup::from_bytes(&unhex(vector()["envelope"].as_str().unwrap())).unwrap()
    }
    const PASSWORD: &[u8] = b"public-backup-vector-password";

    #[test]
    fn independent_argon2_and_libsodium_vector_matches() {
        let vector = vector();
        let seed = unhex(vector["seed"].as_str().unwrap());
        let salt: [u8; 16] = unhex(vector["salt"].as_str().unwrap()).try_into().unwrap();
        let nonce: [u8; 24] = unhex(vector["nonce"].as_str().unwrap()).try_into().unwrap();
        let backup = SeedBackup::new(&seed, CoinType::Qi, 7).unwrap();
        let encrypted = backup
            .encrypt_with_randomness(PASSWORD, BackupKdf::default(), salt, nonce)
            .unwrap();
        assert_eq!(encrypted.as_bytes(), fixture().as_bytes());
        let restored = encrypted.decrypt(PASSWORD).unwrap();
        assert_eq!(restored.expose_seed(), seed);
        assert_eq!(restored.coin_type(), CoinType::Qi);
        assert_eq!(restored.account_index(), 7);
        assert_eq!(
            backup.wallet().unwrap().root_public_key().export(),
            restored.wallet().unwrap().root_public_key().export()
        );
    }

    #[test]
    fn encrypted_roundtrip_preserves_short_intermediate_and_full_seeds() {
        for (length, coin, account) in [
            (16, CoinType::Quai, 0),
            (31, CoinType::Qi, 19),
            (64, CoinType::Quai, (1 << 31) - 1),
        ] {
            let seed: Vec<u8> = (0..length).map(|value| value as u8).collect();
            let backup = SeedBackup::new(&seed, coin, account).unwrap();
            let encrypted = backup.encrypt(PASSWORD, BackupKdf::default()).unwrap();
            assert_eq!(
                encrypted.as_bytes().len(),
                EncryptedSeedBackup::ENCODED_LENGTH
            );
            let restored = EncryptedSeedBackup::from_bytes(encrypted.as_bytes())
                .unwrap()
                .decrypt(PASSWORD)
                .unwrap();
            assert_eq!(restored.expose_seed(), seed);
            assert_eq!(restored.coin_type(), coin);
            assert_eq!(restored.account_index(), account);
        }
    }

    #[test]
    fn production_encryption_uses_fresh_salt_and_nonce() {
        let backup = SeedBackup::new(&[7; 16], CoinType::Quai, 0).unwrap();
        let first = backup.encrypt(PASSWORD, BackupKdf::default()).unwrap();
        let second = backup.encrypt(PASSWORD, BackupKdf::default()).unwrap();
        assert_ne!(&first.as_bytes()[24..40], &second.as_bytes()[24..40]);
        assert_ne!(&first.as_bytes()[40..64], &second.as_bytes()[40..64]);
        assert_ne!(first.as_bytes(), second.as_bytes());
    }

    #[test]
    fn corruption_and_wrong_password_share_unlock_error() {
        let encrypted = fixture();
        assert_eq!(
            encrypted.decrypt(b"wrong public password").unwrap_err(),
            BackupError::UnlockFailed
        );
        // Authentic-looking KDF lane change, salt, nonce, ciphertext and tag.
        for offset in [23, 24, 40, HEADER_LENGTH, ENVELOPE_LENGTH - 1] {
            let mut bytes = encrypted.0;
            if offset == 23 {
                bytes[offset] = 3;
            } else {
                bytes[offset] ^= 1;
            }
            let error = match EncryptedSeedBackup::from_bytes(&bytes) {
                Ok(parsed) => parsed.decrypt(PASSWORD).unwrap_err(),
                Err(error) => error,
            };
            assert_eq!(error, BackupError::UnlockFailed);
        }
    }

    #[test]
    fn authenticated_invalid_seed_length_is_rejected_without_slicing_panic() {
        // An independently tagged malformed payload exercises validation after
        // authentication, not merely tag rejection of modified ciphertext.
        let vector = vector();
        let key = Zeroizing::new(unhex(vector["key"].as_str().unwrap()));
        let cipher = XChaCha20Poly1305::new_from_slice(&key).unwrap();
        let mut envelope = fixture().0;
        let nonce: [u8; 24] = envelope[40..64].try_into().unwrap();
        let mut payload = Zeroizing::new([0u8; PAYLOAD_LENGTH]);
        payload[0] = 255;
        let tag = cipher
            .encrypt_inout_detached(
                &XNonce::from(nonce),
                &envelope[..HEADER_LENGTH],
                (&mut payload[..]).into(),
            )
            .unwrap();
        envelope[HEADER_LENGTH..HEADER_LENGTH + PAYLOAD_LENGTH].copy_from_slice(&payload[..]);
        envelope[HEADER_LENGTH + PAYLOAD_LENGTH..].copy_from_slice(&tag);
        assert_eq!(
            EncryptedSeedBackup::from_bytes(&envelope)
                .unwrap()
                .decrypt(PASSWORD)
                .unwrap_err(),
            BackupError::UnlockFailed
        );
    }

    #[test]
    fn untrusted_sizes_and_costs_reject_before_kdf() {
        for length in [0, 64, ENVELOPE_LENGTH - 1, ENVELOPE_LENGTH + 1, 4096] {
            assert_eq!(
                EncryptedSeedBackup::from_bytes(&vec![0; length]).unwrap_err(),
                BackupError::UnlockFailed
            );
        }
        let encrypted = fixture();
        for offset in [0, 8, 9, 10, 11] {
            let mut bytes = encrypted.0;
            bytes[offset] ^= 1;
            assert_eq!(
                EncryptedSeedBackup::from_bytes(&bytes).unwrap_err(),
                BackupError::UnlockFailed
            );
        }
        for offset in [12, 16, 20] {
            for value in [0, u32::MAX] {
                let mut bytes = encrypted.0;
                bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
                assert_eq!(
                    EncryptedSeedBackup::from_bytes(&bytes).unwrap_err(),
                    BackupError::UnlockFailed
                );
            }
        }
        for password in [Vec::new(), vec![b'a'; 1025]] {
            assert_eq!(
                encrypted.decrypt(&password).unwrap_err(),
                BackupError::UnlockFailed
            );
        }
        for parameters in [(8, 1, 1), (65536, 2, 4), (262144, 6, 4), (65536, 3, 5)] {
            assert_eq!(
                BackupKdf::new(parameters.0, parameters.1, parameters.2).unwrap_err(),
                BackupError::InvalidParameters
            );
        }
        assert!(BackupKdf::new(262144, 3, 4).is_ok());
    }

    #[test]
    fn seed_and_password_validation_and_redaction() {
        for length in 16..=64 {
            let seed = vec![length as u8; length];
            assert_eq!(
                SeedBackup::new(&seed, CoinType::Qi, 0)
                    .unwrap()
                    .expose_seed(),
                seed
            );
        }
        for length in [0, 15, 65] {
            assert!(SeedBackup::new(&vec![0; length], CoinType::Qi, 0).is_err());
        }
        assert!(SeedBackup::new(&[0; 16], CoinType::Qi, 1 << 31).is_err());
        let backup = SeedBackup::new(&[7; 16], CoinType::Quai, 0).unwrap();
        assert_eq!(
            backup.encrypt(b"", BackupKdf::default()).unwrap_err(),
            BackupError::InvalidPassword
        );
        assert_eq!(format!("{backup:?}"), "SeedBackup([REDACTED])");
        assert_eq!(
            format!("{:?}", fixture()),
            "EncryptedSeedBackup([REDACTED])"
        );
    }
}
