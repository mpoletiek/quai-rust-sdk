//! Explicit Qi wrapping envelopes. Contract credit, activation and maturity require node observations.
use crate::{
    Denomination, QiInput, QiOutput, QiTransaction, TransactionError, U256, decode, encode,
    qi::QiMode,
};
use quai_crypto::{SchnorrSignature, SecretKey, keccak256};
use quai_primitives::{Hash32, Ledger, QuaiAddress, Zone};

/// Explicit Qi wrapping beneficiary and owner contract intent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QiWrappingIntent {
    /// Single same-zone account destination; repeated denominations aggregate here.
    pub destination: QuaiAddress,
    /// Same-zone contract that receives protocol backing for the wrapped tokens.
    pub owner_contract: QuaiAddress,
}
/// Immutable unsigned Qi wrapping with validated static semantics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QiWrappingTransaction {
    transaction: QiTransaction,
    intent: QiWrappingIntent,
}
impl QiWrappingTransaction {
    /// Build wrapping denominations first, followed by unique same-zone Qi change.
    /// Input/output denominations are not proof of value conservation or fee sufficiency.
    pub fn new(
        chain_id: U256,
        inputs: Vec<QiInput>,
        wrapped: Vec<Denomination>,
        change: Vec<QiOutput>,
        intent: QiWrappingIntent,
    ) -> Result<Self, TransactionError> {
        if wrapped.is_empty() {
            return Err(TransactionError::InvalidField("missing wrapping output"));
        }
        let output_len = wrapped
            .len()
            .checked_add(change.len())
            .ok_or(TransactionError::TooLarge)?;
        let messages = inputs
            .len()
            .checked_mul(3)
            .and_then(|n| n.checked_add(output_len))
            .and_then(|n| n.checked_add(3))
            .ok_or(TransactionError::TooLarge)?;
        if messages > crate::MAX_TRANSACTION_MESSAGES
            || inputs.len() > u16::MAX as usize
            || output_len > u16::MAX as usize
        {
            return Err(TransactionError::TooLarge);
        }
        if change
            .iter()
            .any(|output| output.address.ledger() != Ledger::Qi)
        {
            return Err(TransactionError::InvalidScope);
        }
        let data = intent.owner_contract.address().bytes().to_vec();
        let mut outputs = Vec::with_capacity(output_len);
        outputs.extend(wrapped.into_iter().map(|denomination| QiOutput {
            address: intent.destination.address(),
            denomination,
        }));
        outputs.extend(change);
        Self::from_transaction(QiTransaction {
            chain_id,
            inputs,
            outputs,
            data,
        })
    }
    /// Explicitly classify an existing unsigned payload as wrapping. No bytes
    /// are changed; mixed beneficiaries, malformed owner data and cross-zone change fail.
    pub fn from_transaction(transaction: QiTransaction) -> Result<Self, TransactionError> {
        transaction.validate_for(QiMode::Wrapping)?;
        let destination = transaction
            .outputs
            .iter()
            .find_map(|output| QuaiAddress::try_from(output.address).ok())
            .ok_or(TransactionError::InvalidScope)?;
        let owner_contract = QuaiAddress::try_from(
            quai_primitives::Address::try_from(transaction.data.as_slice())
                .map_err(|_| TransactionError::InvalidScope)?,
        )
        .map_err(|_| TransactionError::InvalidScope)?;
        Ok(Self {
            transaction,
            intent: QiWrappingIntent {
                destination,
                owner_contract,
            },
        })
    }
    /// Exact immutable wire fields. Ordinary Qi signing still rejects this payload.
    pub fn transaction(&self) -> &QiTransaction {
        &self.transaction
    }
    /// Explicit decoded beneficiary and owner contract.
    pub fn intent(&self) -> QiWrappingIntent {
        self.intent
    }
    /// Replay-protection chain identity.
    pub fn chain_id(&self) -> U256 {
        self.transaction.chain_id
    }
    /// Validated input-origin zone.
    pub fn origin_zone(&self) -> Zone {
        self.intent.destination.zone()
    }
    /// Canonical unsigned protobuf, retaining all 20 wrapping bytes.
    pub fn unsigned_bytes(&self) -> Result<Vec<u8>, TransactionError> {
        encode(&self.transaction.protobuf_for(QiMode::Wrapping)?)
    }
    /// Keccak digest over the exact unsigned envelope.
    pub fn signing_digest(&self) -> Result<Hash32, TransactionError> {
        Ok(keccak256(&self.unsigned_bytes()?).into())
    }
    /// Decode canonical unsigned wrapping bytes without losing ordering or extensions.
    pub fn decode_unsigned(bytes: &[u8]) -> Result<Self, TransactionError> {
        let tx = Self::from_transaction(QiTransaction::from_proto_for(
            &decode(bytes)?,
            QiMode::Wrapping,
        )?)?;
        if tx.unsigned_bytes()? != bytes {
            return Err(TransactionError::InvalidEncoding);
        }
        Ok(tx)
    }
    /// Sign only a single-input wrapping with its matching local key.
    pub fn sign_single(
        &self,
        key: &SecretKey,
    ) -> Result<SignedQiWrappingTransaction, TransactionError> {
        if self.transaction.inputs.len() != 1 {
            return Err(TransactionError::InvalidField("expected single input"));
        }
        self.sign_local(&[key])
    }
    /// Sign with all keys held locally in input order; duplicates preserve MuSig order.
    pub fn sign_local(
        &self,
        keys: &[&SecretKey],
    ) -> Result<SignedQiWrappingTransaction, TransactionError> {
        let signature = self.transaction.sign_local_for(keys, QiMode::Wrapping)?;
        self.attach_signature(signature)
    }
    /// Verify a supplied Schnorr/ordered aggregate signature against the exact payload.
    pub fn attach_signature(
        &self,
        signature: SchnorrSignature,
    ) -> Result<SignedQiWrappingTransaction, TransactionError> {
        self.transaction
            .verify_signature_for(signature, QiMode::Wrapping)?;
        Ok(SignedQiWrappingTransaction {
            transaction: self.clone(),
            signature,
        })
    }
}
/// Immutable verified Qi wrapping; not a node-acceptance guarantee.
#[derive(Clone, Debug)]
pub struct SignedQiWrappingTransaction {
    transaction: QiWrappingTransaction,
    signature: SchnorrSignature,
}
impl SignedQiWrappingTransaction {
    /// Authorized wrapping payload.
    pub fn transaction(&self) -> &QiWrappingTransaction {
        &self.transaction
    }
    /// Signature covering the exact 20-byte owner-contract data and ordered inputs/outputs.
    pub fn signature(&self) -> SchnorrSignature {
        self.signature
    }
    /// Canonical signed protobuf.
    pub fn signed_bytes(&self) -> Result<Vec<u8>, TransactionError> {
        let mut p = self
            .transaction
            .transaction
            .protobuf_for(QiMode::Wrapping)?;
        p.signature = Some(self.signature.to_bytes().to_vec());
        encode(&p)
    }
    /// Qi transaction ID, retaining the input origin and Qi ledger bits.
    pub fn hash(&self) -> Result<Hash32, TransactionError> {
        let zone = self.transaction.origin_zone().byte();
        let mut h = keccak256(&self.signed_bytes()?);
        h[0] = zone;
        h[1] |= 0x80;
        h[2] = zone;
        h[3] |= 0x80;
        Ok(h.into())
    }
    /// Decode canonical wrapping bytes and verify every ordered input's signature.
    pub fn decode(bytes: &[u8]) -> Result<Self, TransactionError> {
        let p = decode(bytes)?;
        let tx = QiWrappingTransaction::from_transaction(QiTransaction::from_proto_for(
            &p,
            QiMode::Wrapping,
        )?)?;
        let raw: [u8; 64] = p
            .signature
            .as_deref()
            .ok_or(TransactionError::InvalidSignature)?
            .try_into()
            .map_err(|_| TransactionError::InvalidSignature)?;
        let signature =
            SchnorrSignature::from_bytes(&raw).map_err(|_| TransactionError::InvalidSignature)?;
        let signed = tx.attach_signature(signature)?;
        if signed.signed_bytes()? != bytes {
            return Err(TransactionError::InvalidEncoding);
        }
        Ok(signed)
    }
}
