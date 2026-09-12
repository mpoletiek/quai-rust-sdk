use k256::Scalar;
use musig2::KeyAggContext;
use zeroize::Zeroizing;

use crate::{CryptoError, PublicKey, SchnorrPublicKey, SchnorrSignature, SecretKey};

/// Resource policy for one local ordered aggregation, not a consensus input limit.
pub const MAX_AGGREGATE_KEYS: usize = 1024;

/// Ordered, untweaked MuSig key aggregation for one process owning every key.
///
/// Duplicates and order are preserved, matching the pinned Quai node's
/// `AggregateKeys(keys, false)` path. A singleton uses ordinary Schnorr and is
/// intentionally rejected here. This context holds only public information;
/// it is not a distributed signing session or a nonce coordinator.
#[derive(Clone, Debug)]
pub struct OrderedKeyAggregate {
    context: KeyAggContext,
    public_keys: Vec<PublicKey>,
    aggregate: PublicKey,
}

impl OrderedKeyAggregate {
    /// Validate the count before calling the backend, preserving exact key order.
    pub fn new(keys: &[PublicKey]) -> Result<Self, CryptoError> {
        if !(2..=MAX_AGGREGATE_KEYS).contains(&keys.len()) {
            return Err(CryptoError::InvalidKeyCount);
        }
        let context = KeyAggContext::new(keys.iter().map(|key| key.0))
            .map_err(|_| CryptoError::InvalidKeyAggregation)?;
        let aggregate: k256::PublicKey = context.aggregated_pubkey();
        Ok(Self {
            context,
            public_keys: keys.to_vec(),
            aggregate: PublicKey(aggregate),
        })
    }

    /// The full, untweaked aggregate point. BIP340 verification uses its X coordinate.
    pub const fn public_key(&self) -> PublicKey {
        self.aggregate
    }

    /// Verify an exact prehash under this ordered aggregate, without hashing it again.
    pub fn verify_prehash(
        &self,
        digest: &[u8; 32],
        signature: &SchnorrSignature,
    ) -> Result<(), CryptoError> {
        let encoded = self.aggregate.to_compressed();
        let mut x = [0; 32];
        x.copy_from_slice(&encoded[1..]);
        SchnorrPublicKey::from_bytes(&x)?.verify(digest, signature)
    }

    /// Sign with every locally owned key in the context's exact order.
    ///
    /// This computes a local weighted aggregate secret using coefficients from
    /// the public MuSig library and constant-time k256 scalar operations. It then
    /// produces a fresh OS-auxiliary-randomized BIP340 signature. No aggregate
    /// secret, partial signature or nonce is exposed. It does not implement the
    /// distributed MuSig protocol, which has different coordination requirements.
    ///
    /// All named secret copies, products and the accumulator have zeroizing
    /// guards on every return path. Compiler/backend transient copies are subject
    /// to the same best-effort erasure limits documented by `SecretKey`.
    pub fn sign_local(
        &self,
        keys: &[&SecretKey],
        digest: &[u8; 32],
    ) -> Result<SchnorrSignature, CryptoError> {
        if keys.len() != self.public_keys.len()
            || keys
                .iter()
                .zip(&self.public_keys)
                .any(|(key, public)| key.public_key() != *public)
        {
            return Err(CryptoError::InvalidSecretKeySet);
        }
        let mut accumulator = Zeroizing::new(Scalar::ZERO);
        for (key, public) in keys.iter().zip(&self.public_keys) {
            // Coefficients are public. The library is not given private scalars:
            // its aggregated_seckey helper lacks zeroization of internal copies.
            let coefficient: Scalar = self
                .context
                .key_coefficient(public.0)
                .ok_or(CryptoError::InvalidKeyAggregation)?
                .into();
            let secret = key.guarded_scalar();
            let product = Zeroizing::new(*secret * coefficient);
            *accumulator += *product;
        }
        let bytes = Zeroizing::new(<[u8; 32]>::from(accumulator.to_bytes()));
        let aggregate =
            SecretKey::from_bytes(&bytes).map_err(|_| CryptoError::InvalidKeyAggregation)?;
        if aggregate.public_key() != self.aggregate {
            return Err(CryptoError::InvalidKeyAggregation);
        }
        let signature = aggregate.sign_schnorr(digest)?;
        self.verify_prehash(digest, &signature)?;
        Ok(signature)
    }
}
