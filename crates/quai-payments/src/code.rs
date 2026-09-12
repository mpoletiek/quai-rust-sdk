use crate::PaymentError;
use bip32::{ChildNumber, ExtendedKey, ExtendedKeyAttrs, Prefix, XPrv, XPub};
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
    root: XPub,
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
        let root = XPub::try_from(ExtendedKey {
            prefix: Prefix::XPUB,
            attrs: ExtendedKeyAttrs {
                depth: 0,
                parent_fingerprint: [0; 4],
                child_number: ChildNumber(0),
                chain_code: bytes[35..67]
                    .try_into()
                    .map_err(|_| PaymentError::InvalidCode)?,
            },
            key_bytes: bytes[2..35]
                .try_into()
                .map_err(|_| PaymentError::InvalidCode)?,
        })
        .map_err(|_| PaymentError::InvalidCode)?;
        let notification = root
            .derive_child(child(0)?)
            .map_err(|_| PaymentError::Derivation)?;
        let notification = PublicKey::from_sec1_bytes(&notification.to_bytes())
            .map_err(|_| PaymentError::Derivation)?;
        Ok(Self {
            bytes,
            root,
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
        self.root.attrs().chain_code
    }
    /// Cached public notification child at index zero.
    pub const fn notification_public_key(&self) -> PublicKey {
        self.notification
    }
    /// Derive one exact nonhardened payment child, without silently skipping errors.
    pub fn child_public_key(&self, index: u32) -> Result<PublicKey, PaymentError> {
        let node = self
            .root
            .derive_child(child(index)?)
            .map_err(|_| PaymentError::Derivation)?;
        PublicKey::from_sec1_bytes(&node.to_bytes()).map_err(|_| PaymentError::Derivation)
    }
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
    root: XPrv,
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
            root,
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
        let node = self
            .root
            .derive_child(child(index)?)
            .map_err(|_| PaymentError::Derivation)?;
        let receiver = secret(&node)?;
        let shared = receiver.ecdh_shared_x(&sender.notification_public_key());
        let hash = Zeroizing::new(sha256(shared.as_slice()));
        let tweak = SecretKey::from_bytes(&hash).map_err(|_| PaymentError::Derivation)?;
        let result = receiver
            .add_tweak(&tweak)
            .map_err(|_| PaymentError::Derivation)?;
        let expected = receiver
            .public_key()
            .add_tweak(&tweak)
            .map_err(|_| PaymentError::Derivation)?;
        if result.public_key() != expected {
            return Err(PaymentError::Derivation);
        }
        Ok(result)
    }
    /// Compute a receiving public key without exposing its temporary secret.
    pub fn receive_public_key(
        &self,
        sender: &PaymentCode,
        index: u32,
    ) -> Result<PublicKey, PaymentError> {
        Ok(self.receive_key(sender, index)?.public_key())
    }
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
