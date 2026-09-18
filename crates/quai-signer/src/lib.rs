//! Local signing binds a key to an explicit chain; no network or automatic sends.
pub use quai_abi::TypedData;
use quai_consensus::{
    QiTransaction, QuaiTransaction, SignedQiTransaction, SignedQuaiTransaction, U256,
};
use quai_crypto::{
    PublicKey, RecoverableSignature, SchnorrPublicKey, SchnorrSignature, SecretKey, hash_message,
    keccak256,
};
use quai_primitives::{Address, QiAddress};
use std::fmt;
use thiserror::Error;

/// Explicit policy for typed-data domains that omit a chain identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DomainPolicy {
    /// Require the domain's chain ID to equal the signer's configured chain.
    RequireChainId,
    /// Permit an absent/null chain ID; any present chain ID must still match.
    /// Such signatures may be usable on multiple chains and require explicit review.
    AllowUnbound,
}

impl DomainPolicy {
    /// Check a validated document against an explicit nonzero chain before any signing prompt.
    pub fn validate(self, data: &TypedData, chain_id: U256) -> Result<(), SignerError> {
        if chain_id == U256::ZERO {
            return Err(SignerError::ChainMismatch);
        }
        match data.domain().get("chainId").filter(|v| !v.is_null()) {
            None if self == DomainPolicy::RequireChainId => {
                return Err(SignerError::ChainMismatch);
            }
            None => (),
            Some(value) => {
                let chain = match value {
                    serde_json::Value::Number(n) => n
                        .as_u64()
                        .map(U256::from)
                        .ok_or(SignerError::ChainMismatch)?,
                    serde_json::Value::String(text) => {
                        let (digits, radix) = text
                            .strip_prefix("0x")
                            .or_else(|| text.strip_prefix("0X"))
                            .map_or((text.as_str(), 10), |digits| (digits, 16));
                        U256::from_str_radix(digits, radix)
                            .map_err(|_| SignerError::ChainMismatch)?
                    }
                    _ => return Err(SignerError::ChainMismatch),
                };
                if chain != chain_id {
                    return Err(SignerError::ChainMismatch);
                }
            }
        }
        Ok(())
    }
}

/// Signing failures contain no key, payload or backend diagnostic strings.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SignerError {
    /// The configured chain must be nonzero and equal the transaction chain.
    #[error("signer chain ID mismatch or invalid chain ID")]
    ChainMismatch,
    /// The key/address must select a supported zone.
    #[error("signer address does not select a supported zone")]
    InvalidAddress,
    /// A watch-only signer cannot authorize transactions or messages.
    #[error("watch-only signer cannot sign")]
    WatchOnly,
    /// Invalid ledger, transaction or signature.
    #[error("transaction signing failed validation")]
    InvalidTransaction,
    /// Message signature creation failed.
    #[error("message signing failed")]
    MessageSigning,
    /// The Qi message would also be a valid Qi transaction signature.
    #[error("refusing to sign a Qi transaction as a message")]
    QiTransactionMessage,
    /// This signer has not implemented the distinct Qi message format.
    #[error("Qi message signing is unsupported by this signer")]
    QiMessageUnsupported,
    /// This adapter has not implemented typed-data signing.
    #[error("typed-data signing is unsupported by this signer")]
    TypedDataUnsupported,
}

/// Synchronous local-signing contract; remote/browser signing adapters are separate.
pub trait Signer {
    /// Public signing address.
    fn address(&self) -> Address;
    /// The chain explicitly bound to this signer.
    fn chain_id(&self) -> U256;
    /// Sign the exact Quai payload after checking the configured chain.
    fn sign_quai(
        &self,
        transaction: &QuaiTransaction,
    ) -> Result<SignedQuaiTransaction, SignerError>;
    /// Sign an ordinary single-input Qi transfer after checking chain and key ownership.
    fn sign_qi_single(
        &self,
        transaction: &QiTransaction,
    ) -> Result<SignedQiTransaction, SignerError>;
    /// Personal-message ECDSA signature. This format does not bind chain ID.
    fn sign_message(&self, message: &[u8]) -> Result<RecoverableSignature, SignerError>;
    /// Qi BIP340 signature over Keccak(message), with no prefix or chain binding.
    /// String callers must explicitly supply UTF-8 bytes. Requires a Qi key.
    fn sign_qi_message(&self, _message: &[u8]) -> Result<SchnorrSignature, SignerError> {
        Err(SignerError::QiMessageUnsupported)
    }
    /// Sign an immutable validated typed-data document with explicit domain policy.
    fn sign_typed_data(
        &self,
        _data: &TypedData,
        _policy: DomainPolicy,
    ) -> Result<RecoverableSignature, SignerError> {
        Err(SignerError::TypedDataUnsupported)
    }
}

/// A non-cloneable zeroizing local key bound to a chain identity.
pub struct LocalSigner {
    key: SecretKey,
    address: Address,
    chain_id: U256,
}
impl LocalSigner {
    /// Full public key, required to bind a Qi message signature to its address.
    pub fn public_key(&self) -> PublicKey {
        self.key.public_key()
    }
    /// Bind a validated key to a nonzero chain and known address zone.
    pub fn new(key: SecretKey, chain_id: U256) -> Result<Self, SignerError> {
        if chain_id == U256::ZERO {
            return Err(SignerError::ChainMismatch);
        }
        let address = key.public_key().address();
        address.zone().map_err(|_| SignerError::InvalidAddress)?;
        Ok(Self {
            key,
            address,
            chain_id,
        })
    }
}
impl fmt::Debug for LocalSigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalSigner")
            .field("address", &self.address)
            .field("chain_id", &self.chain_id)
            .finish_non_exhaustive()
    }
}
impl Signer for LocalSigner {
    fn address(&self) -> Address {
        self.address
    }
    fn chain_id(&self) -> U256 {
        self.chain_id
    }
    fn sign_quai(&self, tx: &QuaiTransaction) -> Result<SignedQuaiTransaction, SignerError> {
        if tx.chain_id != self.chain_id {
            return Err(SignerError::ChainMismatch);
        }
        tx.sign(&self.key)
            .map_err(|_| SignerError::InvalidTransaction)
    }
    fn sign_qi_single(&self, tx: &QiTransaction) -> Result<SignedQiTransaction, SignerError> {
        if tx.chain_id != self.chain_id {
            return Err(SignerError::ChainMismatch);
        }
        tx.sign_single(&self.key)
            .map_err(|_| SignerError::InvalidTransaction)
    }
    fn sign_message(&self, message: &[u8]) -> Result<RecoverableSignature, SignerError> {
        self.key
            .sign_prehash(&hash_message(message))
            .map_err(|_| SignerError::MessageSigning)
    }
    fn sign_qi_message(&self, message: &[u8]) -> Result<SchnorrSignature, SignerError> {
        sign_qi_message(&self.key, message)
    }
    fn sign_typed_data(
        &self,
        data: &TypedData,
        policy: DomainPolicy,
    ) -> Result<RecoverableSignature, SignerError> {
        policy.validate(data, self.chain_id)?;
        self.key
            .sign_prehash(data.signing_hash().bytes())
            .map_err(|_| SignerError::MessageSigning)
    }
}

/// Public identity with an explicit inability to sign, suitable for watch-only applications.
#[derive(Clone, Copy, Debug)]
pub struct WatchOnlySigner {
    address: Address,
    chain_id: U256,
}
impl WatchOnlySigner {
    /// Bind a known-zone public address to a nonzero chain.
    pub fn new(address: Address, chain_id: U256) -> Result<Self, SignerError> {
        if chain_id == U256::ZERO {
            return Err(SignerError::ChainMismatch);
        }
        address.zone().map_err(|_| SignerError::InvalidAddress)?;
        Ok(Self { address, chain_id })
    }
}
impl Signer for WatchOnlySigner {
    fn address(&self) -> Address {
        self.address
    }
    fn chain_id(&self) -> U256 {
        self.chain_id
    }
    fn sign_quai(&self, _: &QuaiTransaction) -> Result<SignedQuaiTransaction, SignerError> {
        Err(SignerError::WatchOnly)
    }
    fn sign_qi_single(&self, _: &QiTransaction) -> Result<SignedQiTransaction, SignerError> {
        Err(SignerError::WatchOnly)
    }
    fn sign_message(&self, _: &[u8]) -> Result<RecoverableSignature, SignerError> {
        Err(SignerError::WatchOnly)
    }
    fn sign_qi_message(&self, _: &[u8]) -> Result<SchnorrSignature, SignerError> {
        Err(SignerError::WatchOnly)
    }
    fn sign_typed_data(
        &self,
        _: &TypedData,
        _: DomainPolicy,
    ) -> Result<RecoverableSignature, SignerError> {
        Err(SignerError::WatchOnly)
    }
}

/// Sign the published Qi wallet message format: BIP340 over Keccak(raw bytes).
/// No prefix, length, network or application domain is inserted. Callers own the
/// exact message semantics. Fresh auxiliary entropy is required on each call.
///
/// Every Qi spend signs BIP340 over Keccak of its unsigned protobuf, so without
/// a domain prefix a message whose bytes are an unsigned transaction yields a
/// valid spend signature: a requester could present a transfer of the signer's
/// output as a "message". The format is fixed by the published wallets, so the
/// prefix cannot be added. Instead, any message that decodes as a transaction
/// with inputs, which every Qi spend has, is refused with
/// [`SignerError::QiTransactionMessage`].
pub fn sign_qi_message(key: &SecretKey, message: &[u8]) -> Result<SchnorrSignature, SignerError> {
    QiAddress::try_from(key.public_key().address()).map_err(|_| SignerError::InvalidAddress)?;
    if quai_consensus::decode_proto_transaction(message).is_ok_and(|tx| tx.tx_ins.is_some()) {
        return Err(SignerError::QiTransactionMessage);
    }
    key.sign_schnorr(&keccak256(message))
        .map_err(|_| SignerError::MessageSigning)
}

/// Verify the Qi message format against an explicit full public key and address.
/// BIP340 has no recovery byte; an x-only key cannot select the address's Y parity.
pub fn verify_qi_message(
    address: QiAddress,
    public_key: &PublicKey,
    message: &[u8],
    signature: &SchnorrSignature,
) -> Result<(), SignerError> {
    if public_key.address() != address.address() {
        return Err(SignerError::InvalidAddress);
    }
    let compressed = public_key.to_compressed();
    let mut x = [0; 32];
    x.copy_from_slice(&compressed[1..]);
    SchnorrPublicKey::from_bytes(&x)
        .and_then(|key| key.verify(&keccak256(message), signature))
        .map_err(|_| SignerError::MessageSigning)
}
