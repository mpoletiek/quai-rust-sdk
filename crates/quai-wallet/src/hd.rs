use crate::{Mnemonic, SecretString, WalletError};
use bip32::{ChildNumber, DerivationPath, ExtendedKey, ExtendedKeyAttrs, Prefix, XPrv, XPub};
use core::fmt;
use hmac::{Hmac, Mac};
use quai_crypto::{PublicKey, SecretKey};
use quai_primitives::{Address, Ledger, Zone};
use sha2::Sha512;
use zeroize::{Zeroize, Zeroizing};

const HARDENED: u32 = 1 << 31;

/// Candidates per thread in one parallel grind chunk. See `search_parallel`.
#[cfg(all(feature = "rayon", not(target_arch = "wasm32")))]
const CHUNK_PER_THREAD: usize = 8;
/// Lower bound, so a single-threaded pool still batches enough to amortize.
#[cfg(all(feature = "rayon", not(target_arch = "wasm32")))]
const MIN_CHUNK: u32 = 32;
/// Upper bound on wasted derivation past a match within one chunk.
#[cfg(all(feature = "rayon", not(target_arch = "wasm32")))]
const MAX_CHUNK: u32 = 512;

/// Public BIP32 derivation metadata. Sharing chain information reduces wallet privacy.
/// A four-byte fingerprint is a routing hint, not proof of key ownership.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ExtendedKeyMetadata {
    /// Master depth is zero.
    pub depth: u8,
    /// Serialized child number, including the high hardened bit.
    pub child_number: u32,
    /// First four bytes of HASH160 of this node's compressed public key.
    pub fingerprint: [u8; 4],
    /// Parent fingerprint; zero for the master node.
    pub parent_fingerprint: [u8; 4],
    /// BIP32 public derivation chain code.
    pub chain_code: [u8; 32],
}
impl ExtendedKeyMetadata {
    /// Child index with the hardened bit removed.
    pub const fn child_index(&self) -> u32 {
        self.child_number & !HARDENED
    }
    /// Whether this node was derived as a hardened child.
    pub const fn is_hardened(&self) -> bool {
        self.child_number & HARDENED != 0
    }
}
impl fmt::Debug for ExtendedKeyMetadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ExtendedKeyMetadata([REDACTED])")
    }
}

/// BIP44 coin types from the pinned Quai SDK.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoinType {
    /// Account-based Quai: m/44'/994'.
    Quai,
    /// UTXO-based Qi: m/44'/969'.
    Qi,
}
impl CoinType {
    /// SLIP44 coin number.
    pub const fn number(self) -> u32 {
        match self {
            Self::Quai => 994,
            Self::Qi => 969,
        }
    }
    /// Required address ledger.
    pub const fn ledger(self) -> Ledger {
        match self {
            Self::Quai => Ledger::Quai,
            Self::Qi => Ledger::Qi,
        }
    }
}

fn child(index: u32, hardened: bool) -> Result<ChildNumber, WalletError> {
    ChildNumber::new(index, hardened).map_err(|_| WalletError::InvalidIndex)
}
fn master_path(depth: u8, path: &str) -> Result<DerivationPath, WalletError> {
    if depth != 0 || path.len() > 4096 {
        return Err(WalletError::InvalidPath);
    }
    let path: DerivationPath = path.parse().map_err(|_| WalletError::InvalidPath)?;
    if path.iter().count() > 255 {
        return Err(WalletError::InvalidPath);
    }
    Ok(path)
}
fn relative_path(depth: u8, path: &str) -> Result<DerivationPath, WalletError> {
    if path.is_empty() || path.len() > 4094 || path.starts_with(['m', '/']) {
        return Err(WalletError::InvalidPath);
    }
    let path = master_path(0, &format!("m/{path}"))?;
    if path.iter().count() + usize::from(depth) > 255 {
        return Err(WalletError::InvalidPath);
    }
    Ok(path)
}
fn validate_root(attrs: &ExtendedKeyAttrs) -> Result<(), WalletError> {
    if attrs.depth == 0 && (attrs.parent_fingerprint != [0; 4] || attrs.child_number.0 != 0) {
        return Err(WalletError::InvalidExtendedKey);
    }
    Ok(())
}

fn decode_extended(encoded: &str, prefix: Prefix) -> Result<ExtendedKey, WalletError> {
    if encoded.len() > 112 || !encoded.starts_with(prefix.as_str()) {
        return Err(WalletError::InvalidExtendedKey);
    }
    // Keep decoded private input zeroizing even on checksum/metadata errors.
    let mut bytes = Zeroizing::new([0u8; 82]);
    let length = bs58::decode(encoded)
        .with_check(None)
        .onto(&mut bytes[..])
        .map_err(|_| WalletError::InvalidExtendedKey)?;
    if length != 78 || bytes[..4] != prefix.to_bytes() {
        return Err(WalletError::InvalidExtendedKey);
    }
    let mut key = ExtendedKey {
        prefix,
        attrs: ExtendedKeyAttrs {
            depth: bytes[4],
            parent_fingerprint: [0; 4],
            child_number: ChildNumber(0),
            chain_code: [0; 32],
        },
        key_bytes: [0; 33],
    };
    key.attrs.parent_fingerprint.copy_from_slice(&bytes[5..9]);
    key.attrs.child_number = ChildNumber(u32::from_be_bytes(
        bytes[9..13]
            .try_into()
            .map_err(|_| WalletError::InvalidExtendedKey)?,
    ));
    key.attrs.chain_code.copy_from_slice(&bytes[13..45]);
    key.key_bytes.copy_from_slice(&bytes[45..78]);
    validate_root(&key.attrs)?;
    Ok(key)
}

/// BIP32 private extended key with redacted diagnostics and a zeroizing scalar.
pub struct ExtendedPrivateKey(XPrv);
impl fmt::Debug for ExtendedPrivateKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ExtendedPrivateKey([REDACTED])")
    }
}
impl ExtendedPrivateKey {
    /// Create a BIP32 master key from any seed of 16 through 64 bytes.
    pub fn from_seed(seed: &[u8]) -> Result<Self, WalletError> {
        if !(16..=64).contains(&seed.len()) {
            return Err(WalletError::InvalidSeed);
        }
        // bip32 0.5 accepts only 16/32/64-byte seeds. BIP32/quais.js allow the
        // complete 16..=64 range, so perform the standard master HMAC here and
        // delegate scalar validation and all child arithmetic to the library.
        let mut mac =
            Hmac::<Sha512>::new_from_slice(b"Bitcoin seed").map_err(|_| WalletError::Derivation)?;
        mac.update(seed);
        let mut output = mac.finalize().into_bytes();
        let mut extended = ExtendedKey {
            prefix: Prefix::XPRV,
            attrs: ExtendedKeyAttrs {
                depth: 0,
                parent_fingerprint: [0; 4],
                child_number: ChildNumber(0),
                chain_code: [0; 32],
            },
            key_bytes: [0; 33],
        };
        extended.key_bytes[1..].copy_from_slice(&output[..32]);
        extended.attrs.chain_code.copy_from_slice(&output[32..]);
        output[..].zeroize();
        XPrv::try_from(extended)
            .map(Self)
            .map_err(|_| WalletError::Derivation)
    }

    /// Import a Base58Check xprv. Other version prefixes are deliberately rejected.
    pub fn import(encoded: &str) -> Result<Self, WalletError> {
        let extended = decode_extended(encoded, Prefix::XPRV)?;
        XPrv::try_from(extended)
            .map(Self)
            .map_err(|_| WalletError::InvalidExtendedKey)
    }

    /// Export a private extended key into explicitly accessed, redacted secret text.
    pub fn export(&self) -> Result<SecretString, WalletError> {
        let extended = self.0.to_extended_key(Prefix::XPRV);
        let mut buffer = Zeroizing::new([0u8; ExtendedKey::MAX_BASE58_SIZE]);
        let text = extended
            .write_base58(&mut buffer)
            .map_err(|_| WalletError::InvalidExtendedKey)?;
        Ok(SecretString(Zeroizing::new(text.to_owned())))
    }

    /// Strip private key material, retaining the public BIP32 chain information.
    pub fn public_key(&self) -> ExtendedPublicKey {
        ExtendedPublicKey(self.0.public_key())
    }

    /// Export a zeroizing secret scalar wrapper for signing in the crypto layer.
    pub fn secret_key(&self) -> Result<SecretKey, WalletError> {
        let bytes = Zeroizing::new(self.0.to_bytes());
        SecretKey::from_bytes(&bytes).map_err(|_| WalletError::Derivation)
    }

    /// Derive one exact child; rare invalid BIP32 children return an error without silently changing indexes.
    pub fn derive_child(&self, index: u32, hardened: bool) -> Result<Self, WalletError> {
        self.0
            .derive_child(child(index, hardened)?)
            .map(Self)
            .map_err(|_| WalletError::Derivation)
    }

    /// Derive an absolute path from a master key; subtree nodes must use derive_child.
    pub fn derive_path(&self, path: &str) -> Result<Self, WalletError> {
        let path = master_path(self.depth(), path)?;
        let mut node = Self(self.0.clone());
        for index in path.iter() {
            node = node.derive_child(index.index(), index.is_hardened())?;
        }
        Ok(node)
    }

    /// Derive a nonempty relative path (for example `0/7`) from any node.
    /// Absolute prefixes are rejected; the entire path/depth is checked first.
    pub fn derive_relative_path(&self, path: &str) -> Result<Self, WalletError> {
        let path = relative_path(self.depth(), path)?;
        let mut node = Self(self.0.clone());
        for index in path.iter() {
            node = node.derive_child(index.index(), index.is_hardened())?;
        }
        Ok(node)
    }

    /// BIP32 derivation depth (master is zero).
    pub fn depth(&self) -> u8 {
        self.0.attrs().depth
    }

    /// Public node metadata, identical before and after stripping the private scalar.
    pub fn metadata(&self) -> ExtendedKeyMetadata {
        self.public_key().metadata()
    }
}

/// Public BIP32 key for nonhardened watch-only derivation.
/// Extended public keys reveal wallet activity; diagnostics omit their value.
#[derive(Clone)]
pub struct ExtendedPublicKey(XPub);
impl fmt::Debug for ExtendedPublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ExtendedPublicKey([REDACTED])")
    }
}
impl ExtendedPublicKey {
    /// Construct a synthetic depth-zero public derivation root from a validated
    /// point and chain code. It does not prove master ancestry: use from_components
    /// when actual depth/parent/child metadata is known. No private key is needed.
    pub fn from_public_key_chain_code(
        public_key: PublicKey,
        chain_code: [u8; 32],
    ) -> Result<Self, WalletError> {
        let key = ExtendedKey {
            prefix: Prefix::XPUB,
            attrs: ExtendedKeyAttrs {
                depth: 0,
                parent_fingerprint: [0; 4],
                child_number: ChildNumber(0),
                chain_code,
            },
            key_bytes: public_key.to_compressed(),
        };
        XPub::try_from(key)
            .map(Self)
            .map_err(|_| WalletError::InvalidExtendedKey)
    }
    /// Construct a public node with explicit BIP32 metadata. Master metadata and
    /// this point's fingerprint are checked; parent identity/ancestry are still
    /// caller claims, not authenticated by a four-byte fingerprint.
    pub fn from_components(
        public_key: PublicKey,
        metadata: ExtendedKeyMetadata,
    ) -> Result<Self, WalletError> {
        let attrs = ExtendedKeyAttrs {
            depth: metadata.depth,
            parent_fingerprint: metadata.parent_fingerprint,
            child_number: ChildNumber(metadata.child_number),
            chain_code: metadata.chain_code,
        };
        validate_root(&attrs)?;
        let key = ExtendedKey {
            prefix: Prefix::XPUB,
            attrs,
            key_bytes: public_key.to_compressed(),
        };
        let node = XPub::try_from(key).map_err(|_| WalletError::InvalidExtendedKey)?;
        if node.fingerprint() != metadata.fingerprint {
            return Err(WalletError::InvalidExtendedKey);
        }
        Ok(Self(node))
    }
    /// Import an xpub, rejecting xprv input even though the underlying library accepts it.
    pub fn import(encoded: &str) -> Result<Self, WalletError> {
        let extended = decode_extended(encoded, Prefix::XPUB)?;
        XPub::try_from(extended)
            .map(Self)
            .map_err(|_| WalletError::InvalidExtendedKey)
    }
    /// Export public chain information. Sharing it reduces wallet privacy.
    pub fn export(&self) -> String {
        self.0.to_string(Prefix::XPUB)
    }
    /// Public curve point represented by this extended key.
    pub fn public_key(&self) -> Result<PublicKey, WalletError> {
        PublicKey::from_sec1_bytes(&self.0.to_bytes()).map_err(|_| WalletError::Derivation)
    }
    /// Derive a nonhardened child. Passing true is an explicit error.
    pub fn derive_child(&self, index: u32, hardened: bool) -> Result<Self, WalletError> {
        if hardened {
            return Err(WalletError::HardenedPublicChild);
        }
        self.0
            .derive_child(child(index, false)?)
            .map(Self)
            .map_err(|_| WalletError::Derivation)
    }
    /// Derive a nonhardened absolute path from a master xpub.
    pub fn derive_path(&self, path: &str) -> Result<Self, WalletError> {
        let path = master_path(self.depth(), path)?;
        let mut node = self.clone();
        for index in path.iter() {
            node = node.derive_child(index.index(), index.is_hardened())?;
        }
        Ok(node)
    }

    /// Derive a nonempty relative path from a subtree xpub; hardened steps fail.
    pub fn derive_relative_path(&self, path: &str) -> Result<Self, WalletError> {
        let path = relative_path(self.depth(), path)?;
        if path.iter().any(|index| index.is_hardened()) {
            return Err(WalletError::HardenedPublicChild);
        }
        let mut node = self.clone();
        for index in path.iter() {
            node = node.derive_child(index.index(), false)?;
        }
        Ok(node)
    }
    /// BIP32 derivation depth.
    pub fn depth(&self) -> u8 {
        self.0.attrs().depth
    }

    /// Public metadata retained by xpub export/import. Full ancestry paths are not encoded.
    pub fn metadata(&self) -> ExtendedKeyMetadata {
        let attrs = self.0.attrs();
        ExtendedKeyMetadata {
            depth: attrs.depth,
            child_number: attrs.child_number.0,
            fingerprint: self.0.fingerprint(),
            parent_fingerprint: attrs.parent_fingerprint,
            chain_code: attrs.chain_code,
        }
    }
}

/// Wallet root at m/44'/coin'. Derivation is stateless and stores no addresses or mnemonic.
pub struct HdWallet {
    root: ExtendedPrivateKey,
    coin: CoinType,
}
impl fmt::Debug for HdWallet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HdWallet")
            .field("coin", &self.coin)
            .finish_non_exhaustive()
    }
}
impl HdWallet {
    /// Coin type selected at construction.
    pub fn coin_type(&self) -> CoinType {
        self.coin
    }
    /// Construct from a validated mnemonic and explicit passphrase without retaining either.
    pub fn from_mnemonic(
        mnemonic: &Mnemonic,
        passphrase: &str,
        coin: CoinType,
    ) -> Result<Self, WalletError> {
        let seed = mnemonic.to_seed(passphrase);
        Self::from_seed(seed.expose(), coin)
    }
    /// Construct from a BIP32 seed, retaining only the coin-level root.
    pub fn from_seed(seed: &[u8], coin: CoinType) -> Result<Self, WalletError> {
        let root = ExtendedPrivateKey::from_seed(seed)?
            .derive_child(44, true)?
            .derive_child(coin.number(), true)?;
        Ok(Self { root, coin })
    }
    /// Restore BIP44 derivation from a canonical depth-zero master xprv.
    /// Caller-owned encoded secret buffers remain the caller's responsibility.
    pub fn from_master_xprv(encoded: &str, coin: CoinType) -> Result<Self, WalletError> {
        let master = ExtendedPrivateKey::import(encoded)?;
        if master.0.attrs().depth != 0
            || master.0.attrs().child_number.0 != 0
            || master.0.attrs().parent_fingerprint != [0; 4]
        {
            return Err(WalletError::InvalidExtendedKey);
        }
        let root = master
            .derive_child(44, true)?
            .derive_child(coin.number(), true)?;
        Ok(Self { root, coin })
    }
    /// Coin-level public xpub; cannot derive hardened accounts. The pinned quais.js
    /// misnamed xPub() returns a private extended key; this API never does.
    pub fn root_public_key(&self) -> ExtendedPublicKey {
        self.root.public_key()
    }
    /// Derive a watch-only account root at m/44'/coin'/account'.
    pub fn account_public(&self, account: u32) -> Result<AccountPublic, WalletError> {
        Ok(AccountPublic {
            key: self.root.derive_child(account, true)?.public_key(),
            coin: self.coin,
            account,
        })
    }
    /// Derive a private node at an exact BIP44 index without zone grinding.
    pub fn derive_key(
        &self,
        account: u32,
        change: bool,
        index: u32,
    ) -> Result<ExtendedPrivateKey, WalletError> {
        self.root
            .derive_child(account, true)?
            .derive_child(u32::from(change), false)?
            .derive_child(index, false)
    }
    /// Search using public child derivation; no global wallet index is mutated.
    pub fn search(
        &self,
        account: u32,
        change: bool,
        search: Search,
        cancelled: impl FnMut() -> bool,
    ) -> Result<SearchResult, WalletError> {
        self.account_public(account)?
            .search(change, search, cancelled)
    }
}

/// A branch node prepared for repeated nonhardened child derivation.
///
/// This exists because address grinding is the SDK's dominant CPU cost. Quai
/// encodes the zone in address byte 0 and the ledger in bit 7 of byte 1, and
/// both come from the Keccak hash of the derived point, so a usable address
/// cannot be chosen -- it is ground for, at roughly one in 256 * 2 candidates.
/// A scan therefore performs hundreds of times more derivation than a standard
/// BIP44 gap scan, which is what makes per-candidate work worth removing.
///
/// Against `bip32::ExtendedPublicKey::derive_child` this drops three costs the
/// grind does not need:
///
/// 1. `bip32` multiplies with k256's generic `Mul` (`public_key.rs`, unchanged
///    on its current main branch), which never consults the precomputed
///    generator table. `PublicKey::add_tweak` uses `mul_by_generator`, which
///    does. That is the dominant term.
/// 2. `derive_child` computes a RIPEMD160(SHA256(..)) parent fingerprint for the
///    child's metadata. A scan reads the point and nothing else.
/// 3. The caller then went through `to_bytes` and `from_sec1_bytes`, compressing
///    a point only to decompress it again: a modular square root per candidate.
///
/// The output is bit-identical to `bip32`, including its zero/overflow
/// rejection, which `tests/hd_ckdpub.rs` asserts differentially. `bip32` remains
/// the only path for extended-key import, export and serialization; this is used
/// solely where a scan needs a child point.
struct ScanBranch {
    public_key: PublicKey,
    /// Parent chain code. Sensitive: it permits deriving every sibling.
    chain_code: Zeroizing<[u8; 32]>,
    /// Cached compressed parent, the constant 33-byte HMAC prefix.
    compressed: [u8; 33],
}

impl ScanBranch {
    fn new(node: &ExtendedPublicKey) -> Result<Self, WalletError> {
        let public_key = node.public_key()?;
        Ok(Self {
            public_key,
            chain_code: Zeroizing::new(node.0.attrs().chain_code),
            compressed: public_key.to_compressed(),
        })
    }

    /// One CKDpub step, returning only the child point.
    ///
    /// `I = HMAC-SHA512(c_par, ser_P(K_par) || ser32(i))`, child point
    /// `K_i = point(I_L) + K_par`. `I_R` is the child chain code, which a leaf
    /// never uses, so it is not returned; the hash still computes it and the
    /// guard still erases it.
    fn child_public_key(&self, index: u32) -> Result<PublicKey, WalletError> {
        if index >= HARDENED {
            return Err(WalletError::HardenedPublicChild);
        }
        let mut data = [0u8; 37];
        data[..33].copy_from_slice(&self.compressed);
        data[33..].copy_from_slice(&index.to_be_bytes());
        let hash = Zeroizing::new(quai_crypto::hmac_sha512(&*self.chain_code, &data));
        let mut tweak = Zeroizing::new([0u8; 32]);
        tweak.copy_from_slice(&hash[..32]);
        // BIP32 says to skip an index whose tweak is zero or >= n. `bip32`
        // returns an error instead, noting the probability is below 1 in 2^127;
        // match that exactly rather than diverging on a case neither will meet.
        let scalar = SecretKey::from_bytes(&tweak).map_err(|_| WalletError::Derivation)?;
        self.public_key
            .add_tweak(&scalar)
            .map_err(|_| WalletError::Derivation)
    }
}

/// An account xpub and explicit origin metadata, suitable for watch-only use.
#[derive(Clone, Debug)]
pub struct AccountPublic {
    key: ExtendedPublicKey,
    coin: CoinType,
    account: u32,
}
impl AccountPublic {
    /// Declared coin origin; imported xpub ancestry must be trusted separately.
    pub fn coin_type(&self) -> CoinType {
        self.coin
    }
    /// Hardened account index before the hardening bit.
    pub fn account_index(&self) -> u32 {
        self.account
    }
    /// Import an account xpub with declared origin. Depth/account are verified;
    /// coin ancestry cannot be proved by xpub bytes and must come from a trusted source.
    pub fn import(encoded: &str, coin: CoinType, account: u32) -> Result<Self, WalletError> {
        let expected = child(account, true)?;
        let key = ExtendedPublicKey::import(encoded)?;
        if key.0.attrs().depth != 3 || key.0.attrs().child_number != expected {
            return Err(WalletError::InvalidAccountOrigin);
        }
        Ok(Self { key, coin, account })
    }
    /// Export the account xpub; store coin and account origin alongside it.
    pub fn export(&self) -> String {
        self.key.export()
    }
    /// Derive and validate one exact index's zone and ledger.
    pub fn derive_address(&self, change: bool, index: u32) -> Result<DerivedAddress, WalletError> {
        let branch = ScanBranch::new(&self.key.derive_child(u32::from(change), false)?)?;
        self.address_from_key(branch.child_public_key(index)?, change, index)
    }
    /// Validate one derived point's zone and ledger.
    ///
    /// Takes a point already in hand, so the grind path never compresses a point
    /// only to decompress it again, which costs a modular square root per
    /// candidate.
    fn address_from_key(
        &self,
        public: PublicKey,
        change: bool,
        index: u32,
    ) -> Result<DerivedAddress, WalletError> {
        let address = public.address();
        let zone = address
            .zone()
            .map_err(|_| WalletError::InvalidDerivedAddress)?;
        if address.ledger() != self.coin.ledger() {
            return Err(WalletError::InvalidDerivedAddress);
        }
        Ok(DerivedAddress {
            address,
            public_key: public.to_compressed(),
            coin: self.coin,
            account: self.account,
            change,
            index,
            zone,
        })
    }

    /// Parallel equivalent of [`Self::search`], returning an identical result.
    ///
    /// Grinding is embarrassingly parallel: each candidate is an independent
    /// public derivation, and roughly 511 of every 512 are discarded. This
    /// splits the range into chunks, derives each chunk across the rayon pool,
    /// and takes the match with the **lowest index**.
    ///
    /// Reducing on lowest index rather than first-to-finish is what makes the
    /// result identical to the sequential search rather than merely valid: a
    /// wallet that resumed from a different match would derive a different
    /// address set, and `next_index` is persisted monotonically, so a
    /// nondeterministic search could skip an address permanently. `attempts`
    /// likewise reports what the sequential search would have examined, not how
    /// many candidates the pool derived, so the two agree on every field.
    ///
    /// Two deliberate differences from [`Self::search`]:
    ///
    /// - Cancellation is checked once per chunk rather than once per candidate,
    ///   so it is coarser. On cancellation the reported `next_index` is the
    ///   start of the unexamined remainder, so resuming never skips a candidate.
    /// - The pool may derive past a match within its chunk, so it performs
    ///   more total work for less wall clock. On a
    ///   battery or CPU budget, prefer the sequential search.
    ///
    /// Only public derivation runs here; no secret scalar is shared with the
    /// pool.
    #[cfg(all(feature = "rayon", not(target_arch = "wasm32")))]
    pub fn search_parallel(
        &self,
        change: bool,
        search: Search,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<SearchResult, WalletError> {
        use rayon::prelude::*;

        child(search.start_index, false)?;
        if search.max_attempts == 0 || search.max_attempts > 10_000_000 {
            return Err(WalletError::InvalidSearchLimit);
        }
        let branch = ScanBranch::new(&self.key.derive_child(u32::from(change), false)?)?;

        // Sized to amortize the pool's per-chunk overhead, NOT to the hit rate.
        // A chunk wide enough to usually contain a match derives far more
        // candidates than the sequential search would examine, and that wasted
        // work cancels the parallelism: at 2048 this measured slower than
        // sequential. Keeping the chunk near the thread count bounds the
        // overshoot to roughly one chunk while still filling every core.
        let chunk = u32::try_from(rayon::current_num_threads().saturating_mul(CHUNK_PER_THREAD))
            .unwrap_or(u32::MAX)
            .clamp(MIN_CHUNK, MAX_CHUNK);

        let mut examined = 0u32;
        while examined < search.max_attempts {
            if cancelled() {
                return Err(WalletError::Cancelled {
                    attempts: examined,
                    next_index: search
                        .start_index
                        .checked_add(examined)
                        .filter(|next| *next < HARDENED)
                        .ok_or(WalletError::SearchExhausted {
                            attempts: examined,
                            next_index: None,
                        })?,
                });
            }
            let remaining = search.max_attempts - examined;
            let width = chunk.min(remaining);
            let base =
                search
                    .start_index
                    .checked_add(examined)
                    .ok_or(WalletError::SearchExhausted {
                        attempts: examined,
                        next_index: None,
                    })?;
            // Stop the chunk at the hardened boundary rather than wrapping.
            let width = width.min(HARDENED.saturating_sub(base));
            if width == 0 {
                return Err(WalletError::SearchExhausted {
                    attempts: examined,
                    next_index: None,
                });
            }

            // The first match or hard error in index order, exactly what the
            // sequential search would reach first. `find_map_first` resolves
            // by position, not by which thread finishes first.
            let first = (0..width).into_par_iter().find_map_first(|offset| {
                let index = base + offset;
                match branch
                    .child_public_key(index)
                    .and_then(|point| self.address_from_key(point, change, index))
                {
                    Ok(address) if address.zone == search.zone => Some((offset, Ok(address))),
                    Ok(_) | Err(WalletError::InvalidDerivedAddress) => None,
                    Err(error) => Some((offset, Err(error))),
                }
            });
            match first {
                Some((_, Err(error))) => return Err(error),
                Some((offset, Ok(address))) => {
                    let index = base + offset;
                    return Ok(SearchResult {
                        address,
                        attempts: examined + offset + 1,
                        next_index: index.checked_add(1).filter(|next| *next < HARDENED),
                    });
                }
                None => examined += width,
            }
        }
        Err(WalletError::SearchExhausted {
            attempts: examined,
            next_index: search
                .start_index
                .checked_add(examined)
                .filter(|next| *next < HARDENED),
        })
    }

    /// Bounded synchronous search with a cancellation check before each candidate.
    /// Offload long searches through the caller's chosen runtime; no hidden threads are spawned.
    pub fn search(
        &self,
        change: bool,
        search: Search,
        cancelled: impl FnMut() -> bool,
    ) -> Result<SearchResult, WalletError> {
        let mut window = self.search_window(change, search, 1, cancelled)?;
        match (window.stop, window.addresses.pop()) {
            (WindowStop::Filled, Some(address)) => Ok(SearchResult {
                address,
                attempts: window.attempts,
                next_index: window.next_index,
            }),
            (WindowStop::Cancelled, _) => Err(WalletError::Cancelled {
                attempts: window.attempts,
                next_index: window.next_index.ok_or(WalletError::InvalidSearchLimit)?,
            }),
            _ => Err(WalletError::SearchExhausted {
                attempts: window.attempts,
                next_index: window.next_index,
            }),
        }
    }

    /// Up to `count` consecutive matching addresses, deriving the branch once.
    ///
    /// Scanners read a window of addresses per round trip. Calling
    /// [`Self::search`] once per address re-derived the branch node through
    /// `bip32` each time; this derives it once for the whole window. Candidates
    /// are examined in the same order with the same cancellation check before
    /// each one, so the addresses are exactly what `count` successive searches
    /// would return. `max_attempts` bounds the candidates across the window.
    ///
    /// Running out of candidates or being cancelled is a [`WindowStop`], not an
    /// error, because the addresses already found remain valid.
    pub fn search_window(
        &self,
        change: bool,
        search: Search,
        count: usize,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<SearchWindow, WalletError> {
        child(search.start_index, false)?;
        if count == 0 || search.max_attempts == 0 || search.max_attempts > 10_000_000 {
            return Err(WalletError::InvalidSearchLimit);
        }
        // Hoisted once per window; the per-candidate step avoids the generic
        // scalar multiply, the unused parent fingerprint and the compress
        // round trip that `ExtendedPublicKey::derive_child` performs.
        let branch = ScanBranch::new(&self.key.derive_child(u32::from(change), false)?)?;
        let mut window = SearchWindow {
            addresses: Vec::with_capacity(count.min(64)),
            attempts: 0,
            next_index: Some(search.start_index),
            stop: WindowStop::Exhausted,
        };
        while window.attempts < search.max_attempts {
            let Some(index) = window.next_index else {
                break;
            };
            if cancelled() {
                window.stop = WindowStop::Cancelled;
                return Ok(window);
            }
            let candidate = branch.child_public_key(index)?;
            window.attempts += 1;
            window.next_index = index.checked_add(1).filter(|next| *next < HARDENED);
            match self.address_from_key(candidate, change, index) {
                Ok(address) if address.zone == search.zone => {
                    window.addresses.push(address);
                    if window.addresses.len() == count {
                        window.stop = WindowStop::Filled;
                        return Ok(window);
                    }
                }
                Ok(_) | Err(WalletError::InvalidDerivedAddress) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(window)
    }
}

/// Matching addresses from one [`AccountPublic::search_window`] call.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SearchWindow {
    /// Matches in derivation-index order.
    pub addresses: Vec<DerivedAddress>,
    /// Candidates examined, including matches.
    pub attempts: u32,
    /// First unexamined candidate, or None past the last nonhardened index.
    pub next_index: Option<u32>,
    /// Why the window ended.
    pub stop: WindowStop,
}

/// Why a [`SearchWindow`] ended.
///
/// Exhaustive on purpose: `Cancelled` means the window's addresses must be
/// discarded, and a new stop reason should fail to compile, not fall into a
/// wildcard arm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowStop {
    /// The requested number of addresses was found.
    Filled,
    /// `max_attempts` or the nonhardened index space ran out first.
    Exhausted,
    /// Cancellation was observed before examining `next_index`.
    Cancelled,
}

/// Explicit search range; returned continuation metadata supports caller-owned checkpoints.
#[derive(Clone, Copy, Debug)]
pub struct Search {
    /// Required zone.
    pub zone: Zone,
    /// First nonhardened index to examine.
    pub start_index: u32,
    /// Maximum candidates to examine, from 1 through 10,000,000.
    pub max_attempts: u32,
}

/// Public derivation metadata; contains no secret key, mnemonic, seed or chain code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivedAddress {
    /// Valid address on the wallet's ledger and a known zone.
    pub address: Address,
    /// Compressed SEC1 public key.
    pub public_key: [u8; 33],
    /// Wallet coin type.
    pub coin: CoinType,
    /// Hardened BIP44 account index before the hardening bit.
    pub account: u32,
    /// False for external addresses, true for change.
    pub change: bool,
    /// Actual child index, including all skipped nonmatching candidates.
    pub index: u32,
    /// Zone encoded in the address.
    pub zone: Zone,
}
impl DerivedAddress {
    /// Full BIP44 path containing the actual ground index.
    pub fn path(&self) -> String {
        format!(
            "m/44'/{}'/{}'/{}/{}",
            self.coin.number(),
            self.account,
            u8::from(self.change),
            self.index
        )
    }
}

/// Successful search and safe continuation information.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchResult {
    /// First matching address in the requested range.
    pub address: DerivedAddress,
    /// Number of candidates examined, including the match.
    pub attempts: u32,
    /// Next nonhardened candidate, or None if the range ended.
    pub next_index: Option<u32>,
}
