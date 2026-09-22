use crate::PaymentError;
use bip32::{ChildNumber, ExtendedKey, ExtendedKeyAttrs, Prefix, XPrv};
use core::{fmt, str::FromStr};
use quai_crypto::{PublicKey, SecretKey, hmac_sha512, sha256};
use zeroize::Zeroizing;

/// Payment-code Base58Check prefix, shared with BIP47 version-one codes.
pub const PAYMENT_CODE_PREFIX: u8 = 0x47;
/// Quai/Qi's BIP47 coin type, from the pinned implementation's executed path.
pub const QUAI_PAYMENT_COIN: u32 = 969;
/// First hardened BIP32 index; payment children must be strictly below this.
pub const HARDENED_INDEX: u32 = 1 << 31;

/// A validated public version-one, feature-zero BIP47 payment code.
///
/// It contains an extended public key and reveals linkable wallet metadata.
/// Diagnostics are redacted; publishing/exporting it is explicit.
#[derive(Clone, PartialEq, Eq)]
pub struct PaymentCode {
    bytes: [u8; 80],
    public_key: PublicKey,
    notification: PublicKey,
}
impl fmt::Debug for PaymentCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PaymentCode([REDACTED])")
    }
}
impl PaymentCode {
    /// Validate the exact 80-byte payload, including reserved bits and curve point.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, PaymentError> {
        let bytes: [u8; 80] = bytes.try_into().map_err(|_| PaymentError::InvalidCode)?;
        if bytes[0] != 1 || bytes[1] != 0 || bytes[67..].iter().any(|b| *b != 0) {
            return Err(PaymentError::InvalidCode);
        }
        let public_key =
            PublicKey::from_sec1_bytes(&bytes[2..35]).map_err(|_| PaymentError::InvalidCode)?;
        let notification = public_key
            .add_tweak(&child_tweak(&bytes, 0)?)
            .map_err(|_| PaymentError::Derivation)?;
        Ok(Self {
            bytes,
            public_key,
            notification,
        })
    }
    /// Validate bounded Base58Check text, version prefix, checksum and full payload.
    pub fn from_base58(text: &str) -> Result<Self, PaymentError> {
        if text.len() > 120 || text.is_empty() {
            return Err(PaymentError::InvalidCode);
        }
        let mut decoded = [0; 85];
        let n = bs58::decode(text)
            .with_check(Some(PAYMENT_CODE_PREFIX))
            .onto(&mut decoded)
            .map_err(|_| PaymentError::InvalidCode)?;
        if n != 81 {
            return Err(PaymentError::InvalidCode);
        }
        Self::from_bytes(&decoded[1..81])
    }
    /// Export the public payload; reserved bytes are always canonical zeros.
    pub const fn to_bytes(&self) -> [u8; 80] {
        self.bytes
    }
    /// Export the public code with its version prefix and double-SHA256 checksum.
    pub fn to_base58(&self) -> String {
        let mut framed = [0; 81];
        framed[0] = PAYMENT_CODE_PREFIX;
        framed[1..].copy_from_slice(&self.bytes);
        bs58::encode(framed).with_check().into_string()
    }
    /// The account's root public key, before notification/payment child derivation.
    pub const fn public_key(&self) -> PublicKey {
        self.public_key
    }
    /// The public BIP32 chain code. Sharing it reduces wallet privacy.
    pub fn chain_code(&self) -> [u8; 32] {
        let mut chain_code = [0; 32];
        chain_code.copy_from_slice(&self.bytes[35..67]);
        chain_code
    }
    /// Cached public notification child at index zero.
    pub const fn notification_public_key(&self) -> PublicKey {
        self.notification
    }
    /// Derive one exact nonhardened payment child, without silently skipping errors.
    ///
    /// One HMAC and one generator multiplication from the payload's own key and
    /// chain code. `bip32` pays a generic multiplication, which never consults
    /// the precomputed generator table, plus a parent fingerprint this never
    /// reads; payment grinding repeats this for roughly 512 candidates per
    /// usable Qi address. The result is bit-identical to `bip32`, which the
    /// tests assert differentially.
    pub fn child_public_key(&self, index: u32) -> Result<PublicKey, PaymentError> {
        self.public_key
            .add_tweak(&child_tweak(&self.bytes, index)?)
            .map_err(|_| PaymentError::Derivation)
    }
}

/// The BIP32 CKDpub tweak `I_L` for a nonhardened child of a payment code.
///
/// `I = HMAC-SHA512(chain code, ser_P(K) || ser32(index))`. Both halves of the
/// payload are public, but `I_L` of a receive child is added to the root
/// secret, so it stays in guarded buffers. `bip32` rejects an `I_L` at or above
/// the order, as this does; the chance is below 1 in 2^127.
fn child_tweak(bytes: &[u8; 80], index: u32) -> Result<SecretKey, PaymentError> {
    child(index)?;
    let mut data = [0u8; 37];
    data[..33].copy_from_slice(&bytes[2..35]);
    data[33..].copy_from_slice(&index.to_be_bytes());
    let hash = Zeroizing::new(hmac_sha512(&bytes[35..67], &data));
    let mut tweak = Zeroizing::new([0u8; 32]);
    tweak.copy_from_slice(&hash[..32]);
    SecretKey::from_bytes(&tweak).map_err(|_| PaymentError::Derivation)
}
impl FromStr for PaymentCode {
    type Err = PaymentError;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::from_base58(text)
    }
}

/// Secret BIP47 account state with redacted diagnostics and backend zeroization.
///
/// The account path is `m/47'/969'/account'`. Notification uses `/0` and payment
/// receive derivation uses `/index` directly, without an extra change branch.
/// Seeds and keys have no implicit serialization or ordinary cloning interface.
pub struct PrivatePaymentCode {
    /// The account root scalar, so a receive child is one scalar addition.
    root_key: SecretKey,
    code: PaymentCode,
    account: u32,
    notification: SecretKey,
}
impl fmt::Debug for PrivatePaymentCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PrivatePaymentCode([REDACTED])")
    }
}
impl PrivatePaymentCode {
    /// Derive the exact Quai payment account from 16 through 64 seed bytes.
    /// The caller retains responsibility for its seed buffer.
    pub fn from_seed(seed: &[u8], account: u32) -> Result<Self, PaymentError> {
        Self::from_seed_coin(seed, QUAI_PAYMENT_COIN, account)
    }
    fn from_seed_coin(seed: &[u8], coin: u32, account: u32) -> Result<Self, PaymentError> {
        if !(16..=64).contains(&seed.len()) {
            return Err(PaymentError::InvalidSeed);
        }
        if account >= HARDENED_INDEX || coin >= HARDENED_INDEX {
            return Err(PaymentError::InvalidIndex);
        }
        // The backend constructor only accepts 16/32/64 bytes; standard BIP32 and
        // pinned quais.js accept every length in 16..=64. Guard the master HMAC.
        let master = Zeroizing::new(hmac_sha512(b"Bitcoin seed", seed));
        let mut material = ExtendedKey {
            prefix: Prefix::XPRV,
            attrs: ExtendedKeyAttrs {
                depth: 0,
                parent_fingerprint: [0; 4],
                child_number: ChildNumber(0),
                chain_code: [0; 32],
            },
            key_bytes: [0; 33],
        };
        material.key_bytes[1..].copy_from_slice(&master[..32]);
        material.attrs.chain_code.copy_from_slice(&master[32..]);
        let mut root = XPrv::try_from(material).map_err(|_| PaymentError::Derivation)?;
        for index in [47, coin, account] {
            root = root
                .derive_child(
                    ChildNumber::new(index, true).map_err(|_| PaymentError::InvalidIndex)?,
                )
                .map_err(|_| PaymentError::Derivation)?;
        }
        Self::from_account_root(root, account)
    }
    /// Import a canonical depth-zero xprv and derive m/47'/969'/account'.
    /// The caller owns and must protect the input string; errors redact it.
    pub fn from_master_xprv(encoded: &str, account: u32) -> Result<Self, PaymentError> {
        let mut root = parse_xprv(encoded)?;
        let attrs = root.attrs();
        if attrs.depth != 0
            || attrs.child_number.0 != 0
            || attrs.parent_fingerprint != [0; 4]
            || account >= HARDENED_INDEX
        {
            return Err(PaymentError::InvalidIndex);
        }
        for index in [47, QUAI_PAYMENT_COIN, account] {
            root = root
                .derive_child(
                    ChildNumber::new(index, true).map_err(|_| PaymentError::InvalidIndex)?,
                )
                .map_err(|_| PaymentError::Derivation)?;
        }
        Self::from_account_root(root, account)
    }
    /// Import a depth-three hardened payment-account xprv. The serialized key
    /// cannot prove its ancestors: the caller explicitly asserts m/47'/969'.
    /// Preserve this account origin separately; full wallet backups accept seed
    /// and master-xprv payment origins, not standalone account-xprv origins.
    pub fn from_account_xprv(encoded: &str, account: u32) -> Result<Self, PaymentError> {
        let root = parse_xprv(encoded)?;
        if account >= HARDENED_INDEX
            || root.attrs().depth != 3
            || root.attrs().child_number.0 != account + HARDENED_INDEX
        {
            return Err(PaymentError::InvalidIndex);
        }
        Self::from_account_root(root, account)
    }
    fn from_account_root(root: XPrv, account: u32) -> Result<Self, PaymentError> {
        let public = root.public_key();
        let mut bytes = [0; 80];
        bytes[0] = 1;
        bytes[2..35].copy_from_slice(&public.to_bytes());
        bytes[35..67].copy_from_slice(&public.attrs().chain_code);
        let code = PaymentCode::from_bytes(&bytes)?;
        let notification = secret(
            &root
                .derive_child(child(0)?)
                .map_err(|_| PaymentError::Derivation)?,
        )?;
        if notification.public_key() != code.notification_public_key() {
            return Err(PaymentError::Derivation);
        }
        Ok(Self {
            root_key: secret(&root)?,
            code,
            account,
            notification,
        })
    }
    /// Public code for this exact secret account; immutable ownership binding.
    pub const fn public_code(&self) -> &PaymentCode {
        &self.code
    }
    /// Hardened account index before applying its hardening bit.
    pub const fn account(&self) -> u32 {
        self.account
    }
    /// Explicitly derive a secret notification child for a protocol using it.
    ///
    /// This key enters the shared secret of every payment this code sends,
    /// and its ECDH with a peer's notification key is the pair's index-0
    /// payment secret. Do not use it, or that ECDH, for messaging or any other
    /// protocol. See `docs/PAYMENT_CODES_BEYOND_PAYMENTS.md` in the repository.
    pub fn notification_key(&self) -> Result<SecretKey, PaymentError> {
        SecretKey::from_bytes(&self.notification.export_bytes())
            .map_err(|_| PaymentError::Derivation)
    }
    /// Compute a recipient's payment public key using this sender's notification key.
    pub fn send_public_key(
        &self,
        recipient: &PaymentCode,
        index: u32,
    ) -> Result<PublicKey, PaymentError> {
        let receiver = recipient.child_public_key(index)?;
        let shared = self.notification.ecdh_shared_x(&receiver);
        let hash = Zeroizing::new(sha256(shared.as_slice()));
        let tweak = SecretKey::from_bytes(&hash).map_err(|_| PaymentError::Derivation)?;
        receiver
            .add_tweak(&tweak)
            .map_err(|_| PaymentError::Derivation)
    }
    /// Derive a locally spendable payment secret from a sender's public code.
    pub fn receive_key(&self, sender: &PaymentCode, index: u32) -> Result<SecretKey, PaymentError> {
        let (receiver, receiver_public) = self.receive_child(index)?;
        let tweak = payment_tweak(&receiver, sender)?;
        let result = receiver
            .add_tweak(&tweak)
            .map_err(|_| PaymentError::Derivation)?;
        // This key signs spends, so its scalar and point arithmetic are checked
        // against each other before it is returned.
        let expected = receiver_public
            .add_tweak(&tweak)
            .map_err(|_| PaymentError::Derivation)?;
        if result.public_key() != expected {
            return Err(PaymentError::Derivation);
        }
        Ok(result)
    }
    /// Compute a receiving public key without exposing its temporary secret.
    ///
    /// The grinding path: its point is the child point plus the payment tweak
    /// times the generator, so no secret result is formed and no scalar is
    /// converted back to a point. [`Self::receive_key`] returns the matching
    /// secret with a cross-check, for signing.
    pub fn receive_public_key(
        &self,
        sender: &PaymentCode,
        index: u32,
    ) -> Result<PublicKey, PaymentError> {
        let (receiver, receiver_public) = self.receive_child(index)?;
        receiver_public
            .add_tweak(&payment_tweak(&receiver, sender)?)
            .map_err(|_| PaymentError::Derivation)
    }
    /// The receive child's secret and point, from one shared CKD tweak.
    fn receive_child(&self, index: u32) -> Result<(SecretKey, PublicKey), PaymentError> {
        let tweak = child_tweak(&self.code.bytes, index)?;
        let secret = self
            .root_key
            .add_tweak(&tweak)
            .map_err(|_| PaymentError::Derivation)?;
        let public = self
            .code
            .public_key
            .add_tweak(&tweak)
            .map_err(|_| PaymentError::Derivation)?;
        Ok((secret, public))
    }
}
/// `SHA256(ECDH_x(receiver, sender notification))` as a scalar.
fn payment_tweak(receiver: &SecretKey, sender: &PaymentCode) -> Result<SecretKey, PaymentError> {
    let shared = receiver.ecdh_shared_x(&sender.notification_public_key());
    let hash = Zeroizing::new(sha256(shared.as_slice()));
    SecretKey::from_bytes(&hash).map_err(|_| PaymentError::Derivation)
}
fn child(index: u32) -> Result<ChildNumber, PaymentError> {
    ChildNumber::new(index, false).map_err(|_| PaymentError::InvalidIndex)
}
fn secret(node: &XPrv) -> Result<SecretKey, PaymentError> {
    let bytes = Zeroizing::new(node.to_bytes());
    SecretKey::from_bytes(&bytes).map_err(|_| PaymentError::Derivation)
}

fn parse_xprv(encoded: &str) -> Result<XPrv, PaymentError> {
    if encoded.len() > ExtendedKey::MAX_BASE58_SIZE {
        return Err(PaymentError::Derivation);
    }
    let extended = ExtendedKey::from_str(encoded).map_err(|_| PaymentError::Derivation)?;
    if extended.prefix != Prefix::XPRV {
        return Err(PaymentError::Derivation);
    }
    XPrv::try_from(extended).map_err(|_| PaymentError::Derivation)
}

#[cfg(test)]
mod fast_path_tests {
    use super::*;
    use bip32::XPub;

    /// The pre-optimization derivations, through `bip32`, as the oracle.
    fn bip32_root(seed: &[u8; 32]) -> XPrv {
        let mut root = XPrv::new(seed).unwrap();
        for index in [47, QUAI_PAYMENT_COIN, 0] {
            root = root
                .derive_child(ChildNumber::new(index, true).unwrap())
                .unwrap();
        }
        root
    }
    fn bip32_child_public(code: &PaymentCode, index: u32) -> PublicKey {
        let root = XPub::try_from(ExtendedKey {
            prefix: Prefix::XPUB,
            attrs: ExtendedKeyAttrs {
                depth: 0,
                parent_fingerprint: [0; 4],
                child_number: ChildNumber(0),
                chain_code: code.chain_code(),
            },
            key_bytes: code.public_key().to_compressed(),
        })
        .unwrap();
        let node = root.derive_child(child(index).unwrap()).unwrap();
        PublicKey::from_sec1_bytes(&node.to_bytes()).unwrap()
    }
    fn bip32_receive(root: &XPrv, sender: &PaymentCode, index: u32) -> PublicKey {
        let receiver = secret(&root.derive_child(child(index).unwrap()).unwrap()).unwrap();
        let shared = receiver.ecdh_shared_x(&sender.notification_public_key());
        let tweak = SecretKey::from_bytes(&sha256(shared.as_slice())).unwrap();
        receiver.add_tweak(&tweak).unwrap().public_key()
    }

    #[test]
    fn fast_derivation_matches_bip32_for_children_sends_and_receives() {
        let (alice_seed, bob_seed) = ([0x11; 32], [0x22; 32]);
        let alice = PrivatePaymentCode::from_seed(&alice_seed, 0).unwrap();
        let bob = PrivatePaymentCode::from_seed(&bob_seed, 0).unwrap();
        let bob_root = bip32_root(&bob_seed);
        assert_eq!(
            secret(&bob_root).unwrap().public_key(),
            bob.public_code().public_key()
        );
        for index in (0..300).chain([HARDENED_INDEX - 1]) {
            for code in [alice.public_code(), bob.public_code()] {
                assert_eq!(
                    code.child_public_key(index).unwrap(),
                    bip32_child_public(code, index)
                );
            }
            let expected = bip32_receive(&bob_root, alice.public_code(), index);
            assert_eq!(
                bob.receive_public_key(alice.public_code(), index).unwrap(),
                expected
            );
            assert_eq!(
                bob.receive_key(alice.public_code(), index)
                    .unwrap()
                    .public_key(),
                expected
            );
            assert_eq!(
                alice.send_public_key(bob.public_code(), index).unwrap(),
                expected
            );
        }
        assert_eq!(
            bob.public_code().child_public_key(HARDENED_INDEX),
            Err(PaymentError::InvalidIndex)
        );
        assert_eq!(
            bob.public_code().notification_public_key(),
            bip32_child_public(bob.public_code(), 0)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ripemd::{Digest, Ripemd160};
    use serde_json::Value;
    fn decode(s: &str) -> Vec<u8> {
        s.as_bytes()
            .chunks_exact(2)
            .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
            .collect()
    }
    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
    #[test]
    fn published_bip47_alice_bob_codes_shared_secrets_and_ten_destinations() {
        let f: Value =
            serde_json::from_str(include_str!("../tests/fixtures/bip47-published.json")).unwrap();
        // Test-only coin-zero construction verifies the public BIP47 vectors.
        // The production constructor always uses Quai coin969.
        let alice =
            PrivatePaymentCode::from_seed_coin(&decode(f["alice"]["seed"].as_str().unwrap()), 0, 0)
                .unwrap();
        let bob =
            PrivatePaymentCode::from_seed_coin(&decode(f["bob"]["seed"].as_str().unwrap()), 0, 0)
                .unwrap();
        for (name, owner) in [("alice", &alice), ("bob", &bob)] {
            assert_eq!(owner.public_code().to_base58(), f[name]["code"]);
            assert_eq!(
                hex(owner.notification_key().unwrap().export_bytes().as_bytes()),
                f[name]["notificationSecret"]
            );
            assert_eq!(
                hex(&owner
                    .public_code()
                    .notification_public_key()
                    .to_compressed()),
                f[name]["notificationPublic"]
            );
        }
        for index in 0..10u32 {
            let child = bob.public_code().child_public_key(index).unwrap();
            let shared = alice.notification.ecdh_shared_x(&child);
            assert_eq!(hex(shared.as_bytes()), f["sharedX"][index as usize]);
            let public = alice.send_public_key(bob.public_code(), index).unwrap();
            assert_eq!(
                public,
                bob.receive_key(alice.public_code(), index)
                    .unwrap()
                    .public_key()
            );
            let hash = Ripemd160::digest(sha256(&public.to_compressed()));
            let mut framed = vec![0];
            framed.extend(hash);
            assert_eq!(
                bs58::encode(framed).with_check().into_string(),
                f["bitcoinP2pkh"][index as usize]
            );
        }
    }
}
