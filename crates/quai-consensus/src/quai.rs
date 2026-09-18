use crate::{TransactionError, U256, decode, encode, integer, integer_bytes, proto};
use quai_crypto::{RecoverableSignature, SecretKey, keccak256};
use quai_primitives::{Address, Hash32, Ledger, QuaiAddress};

/// An account and its ordered storage-key access list. Order is preserved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccessTuple {
    /// Accessed account.
    pub address: Address,
    /// Ordered storage keys; no implicit deduplication alters signing bytes.
    pub storage_keys: Vec<Hash32>,
}

/// Unsigned type-0 Quai transaction. Mutation requires a new signature.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuaiTransaction {
    /// Replay-protection chain identity.
    pub chain_id: U256,
    /// Account sequence number, preserving the complete node u64 range.
    pub nonce: u64,
    /// Destination, or `None` for contract creation.
    pub to: Option<Address>,
    /// Value in base units.
    pub value: U256,
    /// Gas limit.
    pub gas_limit: u64,
    /// Price per gas unit.
    pub gas_price: U256,
    /// Call/deployment data; never silently dropped.
    pub data: Vec<u8>,
    /// Ordered access list.
    pub access_list: Vec<AccessTuple>,
}

impl QuaiTransaction {
    fn protobuf(&self) -> proto::Transaction {
        proto::Transaction {
            r#type: Some(0),
            to: self.to.map(|a| a.bytes().to_vec()),
            nonce: Some(self.nonce),
            value: Some(integer_bytes(self.value)),
            gas: Some(self.gas_limit),
            data: Some(self.data.clone()),
            chain_id: Some(integer_bytes(self.chain_id)),
            gas_price: Some(integer_bytes(self.gas_price)),
            access_list: Some(proto::AccessList {
                access_tuples: self
                    .access_list
                    .iter()
                    .map(|a| proto::AccessTuple {
                        address: a.address.bytes().to_vec(),
                        storage_key: a
                            .storage_keys
                            .iter()
                            .map(|h| proto::Hash {
                                value: h.bytes().to_vec(),
                            })
                            .collect(),
                    })
                    .collect(),
            }),
            ..Default::default()
        }
    }

    /// Canonical unsigned protobuf; explicit empty byte fields are retained.
    pub fn unsigned_bytes(&self) -> Result<Vec<u8>, TransactionError> {
        let messages = self
            .access_list
            .iter()
            .try_fold(2usize, |n, a| n.checked_add(1 + a.storage_keys.len()))
            .ok_or(TransactionError::TooLarge)?;
        let payload = self
            .access_list
            .iter()
            .try_fold(self.data.len(), |n, a| {
                a.storage_keys
                    .len()
                    .checked_mul(32)
                    .and_then(|k| n.checked_add(k))
                    .and_then(|n| n.checked_add(20))
            })
            .ok_or(TransactionError::TooLarge)?;
        if messages > crate::MAX_TRANSACTION_MESSAGES || payload > crate::MAX_TRANSACTION_BYTES {
            return Err(TransactionError::TooLarge);
        }
        encode(&self.protobuf())
    }
    /// Keccak signing preimage digest, distinct from the signed transaction ID.
    pub fn signing_digest(&self) -> Result<Hash32, TransactionError> {
        Ok(Hash32::from_bytes(keccak256(&self.unsigned_bytes()?)))
    }

    /// Decode strictly canonical unsigned type-0 bytes. Unknown extensions fail closed.
    pub fn decode_unsigned(bytes: &[u8]) -> Result<Self, TransactionError> {
        let p = decode(bytes)?;
        let tx = Self::from_proto(&p)?;
        if tx.unsigned_bytes()? != bytes {
            return Err(TransactionError::InvalidEncoding);
        }
        Ok(tx)
    }

    fn from_proto(p: &proto::Transaction) -> Result<Self, TransactionError> {
        if p.r#type != Some(0) {
            return Err(TransactionError::WrongType);
        }
        let to = p
            .to
            .as_ref()
            .map(|b| {
                Address::try_from(b.as_slice()).map_err(|_| TransactionError::InvalidField("to"))
            })
            .transpose()?;
        let mut access_list = Vec::new();
        for tuple in &p
            .access_list
            .as_ref()
            .ok_or(TransactionError::InvalidField("access_list"))?
            .access_tuples
        {
            let address = Address::try_from(tuple.address.as_slice())
                .map_err(|_| TransactionError::InvalidField("access address"))?;
            let storage_keys = tuple
                .storage_key
                .iter()
                .map(|h| {
                    <[u8; 32]>::try_from(h.value.as_slice())
                        .map(Hash32::from_bytes)
                        .map_err(|_| TransactionError::InvalidField("storage key"))
                })
                .collect::<Result<_, _>>()?;
            access_list.push(AccessTuple {
                address,
                storage_keys,
            });
        }
        Ok(Self {
            chain_id: integer(p.chain_id.as_deref(), "chain_id")?,
            nonce: p.nonce.ok_or(TransactionError::InvalidField("nonce"))?,
            to,
            value: integer(p.value.as_deref(), "value")?,
            gas_limit: p.gas.ok_or(TransactionError::InvalidField("gas"))?,
            gas_price: integer(p.gas_price.as_deref(), "gas_price")?,
            data: p
                .data
                .clone()
                .ok_or(TransactionError::InvalidField("data"))?,
            access_list,
        })
    }

    /// Sign only for a nonzero chain and a valid Quai sender, preserving the final payload.
    pub fn sign(&self, key: &SecretKey) -> Result<SignedQuaiTransaction, TransactionError> {
        let signature = key
            .sign_prehash(self.signing_digest()?.bytes())
            .map_err(|_| TransactionError::InvalidSignature)?;
        self.attach_signature(signature)
    }

    /// Attach and cryptographically recover a separately produced signature.
    pub fn attach_signature(
        &self,
        signature: RecoverableSignature,
    ) -> Result<SignedQuaiTransaction, TransactionError> {
        if self.chain_id == U256::ZERO {
            return Err(TransactionError::InvalidField("chain_id"));
        }
        if signature.recovery_id() > 1 {
            return Err(TransactionError::InvalidSignature);
        }
        let public = signature
            .recover_prehash(self.signing_digest()?.bytes())
            .map_err(|_| TransactionError::InvalidSignature)?;
        let from =
            QuaiAddress::try_from(public.address()).map_err(|_| TransactionError::InvalidScope)?;
        if let Some(to) = self.to {
            let zone = to.zone().map_err(|_| TransactionError::InvalidScope)?;
            if to.ledger() == Ledger::Qi {
                // A Qi destination is a Quai-to-Qi conversion request, not an
                // ordinary transfer. Cross-zone is rejected outright; a same-zone
                // one must satisfy the conversion envelope here, at signing,
                // rather than only in the typed builder.
                //
                // Enforcing it only in `QuaiToQiTransaction::new` left it opt-in:
                // this crate is published standalone, so a direct integrator
                // could sign a same-zone Qi destination with arbitrary data,
                // value and slippage. Out-of-range slippage is the sharp edge --
                // the node silently clamps it, so the user would authorize one
                // slippage and get another, which is exactly the substitution
                // this codec refuses to make elsewhere.
                //
                // Every SDK path already routes through the typed builder and so
                // already satisfies this; the check closes the raw-API gap.
                if zone != from.zone() {
                    return Err(TransactionError::InvalidScope);
                }
                if self.value < U256::from(crate::MIN_QUAI_CONVERSION_VALUE) || self.data.len() != 2
                {
                    return Err(TransactionError::InvalidField(
                        "Quai conversion value or data",
                    ));
                }
                crate::ConversionSlippage::new(u16::from_be_bytes([self.data[0], self.data[1]]))?;
            }
        }
        Ok(SignedQuaiTransaction {
            transaction: self.clone(),
            signature,
            from,
        })
    }
}

/// Immutable signed transaction whose sender is recovered from the exact payload.
#[derive(Clone, Debug)]
pub struct SignedQuaiTransaction {
    transaction: QuaiTransaction,
    signature: RecoverableSignature,
    from: QuaiAddress,
}
impl SignedQuaiTransaction {
    /// Read the immutable authorized payload.
    pub fn transaction(&self) -> &QuaiTransaction {
        &self.transaction
    }
    /// Recovered sender.
    pub fn from(&self) -> QuaiAddress {
        self.from
    }
    /// Signature over this transaction's signing digest.
    pub fn signature(&self) -> &RecoverableSignature {
        &self.signature
    }
    /// Canonical signed protobuf.
    pub fn signed_bytes(&self) -> Result<Vec<u8>, TransactionError> {
        let mut p = self.transaction.protobuf();
        p.v = Some(integer_bytes(U256::from(self.signature.recovery_id())));
        p.r = Some(integer_bytes(U256::from_be_bytes(self.signature.r())));
        p.s = Some(integer_bytes(U256::from_be_bytes(self.signature.s())));
        encode(&p)
    }
    /// Quai transaction ID, with origin and ledger bytes applied after hashing.
    pub fn hash(&self) -> Result<Hash32, TransactionError> {
        let mut h = keccak256(&self.signed_bytes()?);
        h[0] = self.from.zone().byte();
        h[1] &= 0x7f;
        h[2] = self.from.zone().byte();
        h[3] &= 0x7f;
        Ok(Hash32::from_bytes(h))
    }
    /// Decode and verify a canonical signature. Unknown/unsupported extensions fail closed.
    pub fn decode(bytes: &[u8]) -> Result<Self, TransactionError> {
        let p = decode(bytes)?;
        let tx = QuaiTransaction::from_proto(&p)?;
        let v = integer(p.v.as_deref(), "v")?;
        if v > U256::from(1) {
            return Err(TransactionError::InvalidSignature);
        }
        let mut compact = [0u8; 64];
        compact[..32].copy_from_slice(&integer(p.r.as_deref(), "r")?.to_be_bytes::<32>());
        compact[32..].copy_from_slice(&integer(p.s.as_deref(), "s")?.to_be_bytes::<32>());
        let sig = RecoverableSignature::from_compact(&compact, v.to::<u8>())
            .map_err(|_| TransactionError::InvalidSignature)?;
        let signed = tx.attach_signature(sig)?;
        if signed.signed_bytes()? != bytes {
            return Err(TransactionError::InvalidEncoding);
        }
        Ok(signed)
    }
}
