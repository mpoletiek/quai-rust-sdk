use crate::{
    MAX_TRANSACTION_BYTES, TransactionError, U256, decode, encode, integer, integer_bytes, proto,
};
use quai_crypto::{
    OrderedKeyAggregate, PublicKey, SchnorrPublicKey, SchnorrSignature, SecretKey, keccak256,
};
use quai_primitives::{Address, Hash32, Ledger, QiAddress, Zone};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum QiMode {
    Ordinary,
    Conversion,
    Wrapping,
}

/// Validated Qi denomination index. Amounts are in Qits (1/1000 Qi).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Denomination(u8);
impl Denomination {
    /// Supported Qit values, indexed by their consensus denomination byte.
    pub const VALUES: [u64; 15] = [
        1, 5, 10, 50, 100, 500, 1000, 5000, 10000, 20000, 100000, 1000000, 10000000, 100000000,
        1000000000,
    ];
    /// Reject indices outside the fifteen supported denominations.
    pub fn new(index: u8) -> Result<Self, TransactionError> {
        if usize::from(index) >= Self::VALUES.len() {
            return Err(TransactionError::InvalidField("denomination"));
        }
        Ok(Self(index))
    }
    /// Consensus denomination index.
    pub const fn index(self) -> u8 {
        self.0
    }
    /// Corresponding Qit value.
    pub const fn value(self) -> u64 {
        Self::VALUES[self.0 as usize]
    }
}

/// Unique reference to an existing transaction output.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct OutPoint {
    /// Creating transaction ID, whose destination zone byte identifies the UTXO zone.
    /// Its ledger bit may be Quai: a Qi-to-Quai refund creates Qi outputs under
    /// the refund ETX hash. Ownership and UTXO existence require separate checks.
    pub transaction_hash: Hash32,
    /// Node-supported output index.
    pub index: u16,
}
/// An input with a validated public key; construction normalizes SEC1 encoding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QiInput {
    /// Referenced output.
    pub previous_output: OutPoint,
    /// Full curve point; wire serialization uses compressed SEC1.
    pub public_key: PublicKey,
}
/// A user-created output. Arbitrary nonzero output locks are deliberately unsupported.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QiOutput {
    /// Recipient. Quai destinations are reserved for conversion flows.
    pub address: Address,
    /// Validated value denomination.
    pub denomination: Denomination,
}
/// Unsigned Qi transaction; serialization preserves input/output order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QiTransaction {
    /// Replay-protection chain identity.
    pub chain_id: U256,
    /// Ordered inputs, including the first input used for hash location bytes.
    pub inputs: Vec<QiInput>,
    /// Ordered outputs, including change.
    pub outputs: Vec<QiOutput>,
    /// Conversion/wrapping data. Semantic node-profile validation is separate.
    pub data: Vec<u8>,
}
impl QiTransaction {
    /// Validate the ordinary transaction's address/outpoint shape and return its origin.
    /// This does not verify that any input exists or is spendable on the node.
    pub fn origin_zone(&self) -> Result<Zone, TransactionError> {
        self.validate()
    }
    fn validate(&self) -> Result<Zone, TransactionError> {
        self.validate_for(QiMode::Ordinary)
    }
    pub(crate) fn validate_for(&self, mode: QiMode) -> Result<Zone, TransactionError> {
        if self.inputs.is_empty() || self.outputs.is_empty() {
            return Err(TransactionError::InvalidField("empty inputs or outputs"));
        }
        let messages = self
            .inputs
            .len()
            .checked_mul(3)
            .and_then(|n| n.checked_add(self.outputs.len()))
            .and_then(|n| n.checked_add(3))
            .ok_or(TransactionError::TooLarge)?;
        if messages > crate::MAX_TRANSACTION_MESSAGES
            || self.inputs.len() > u16::MAX as usize
            || self.outputs.len() > u16::MAX as usize
            || self.data.len() > MAX_TRANSACTION_BYTES
        {
            return Err(TransactionError::TooLarge);
        }
        let first = self.inputs[0].previous_output.transaction_hash.bytes();
        let origin = Zone::from_byte(first[2]).map_err(|_| TransactionError::InvalidScope)?;
        if mode == QiMode::Wrapping {
            let owner = quai_primitives::QuaiAddress::try_from(
                Address::try_from(self.data.as_slice())
                    .map_err(|_| TransactionError::InvalidScope)?,
            )
            .map_err(|_| TransactionError::InvalidScope)?;
            if self.chain_id == U256::ZERO || owner.zone() != origin {
                return Err(TransactionError::InvalidScope);
            }
        }
        if mode == QiMode::Conversion {
            if self.chain_id == U256::ZERO || self.data.len() != 22 {
                return Err(TransactionError::InvalidField("conversion chain or data"));
            }
            crate::ConversionSlippage::new(u16::from_be_bytes([self.data[0], self.data[1]]))?;
            let refund = QiAddress::try_from(
                Address::try_from(&self.data[2..]).map_err(|_| TransactionError::InvalidScope)?,
            )
            .map_err(|_| TransactionError::InvalidScope)?;
            if refund.zone() != origin {
                return Err(TransactionError::InvalidScope);
            }
        }
        let mut seen = BTreeSet::new();
        let mut addresses = BTreeSet::new();
        for input in &self.inputs {
            if !seen.insert(input.previous_output) {
                return Err(TransactionError::InvalidField("duplicate outpoint"));
            }
            let h = input.previous_output.transaction_hash.bytes();
            let address = QiAddress::try_from(input.public_key.address())
                .map_err(|_| TransactionError::InvalidScope)?;
            if h[2] != origin.byte() || *h == [0; 32] || address.zone() != origin {
                return Err(TransactionError::InvalidScope);
            }
            addresses.insert(address.address());
        }
        let mut conversion_destination = None;
        for output in &self.outputs {
            let zone = output
                .address
                .zone()
                .map_err(|_| TransactionError::InvalidScope)?;
            if output.address.ledger() == Ledger::Quai {
                if mode == QiMode::Ordinary {
                    return Err(TransactionError::InvalidField(
                        "conversion requires explicit builder",
                    ));
                }
                if zone != origin
                    || conversion_destination
                        .is_some_and(|destination| destination != output.address)
                {
                    return Err(TransactionError::InvalidScope);
                }
                // Go block processing aggregates equal conversion addresses and removes
                // them from the address-reuse set; distinct destinations are invalid.
                conversion_destination = Some(output.address);
                continue;
            }
            if mode != QiMode::Ordinary && zone != origin {
                return Err(TransactionError::InvalidScope);
            }
            if !addresses.insert(output.address) {
                return Err(TransactionError::InvalidField("reused output address"));
            }
        }
        if mode != QiMode::Ordinary && conversion_destination.is_none() {
            return Err(TransactionError::InvalidField("missing conversion output"));
        }
        Ok(origin)
    }
    fn protobuf(&self) -> Result<proto::Transaction, TransactionError> {
        self.protobuf_for(QiMode::Ordinary)
    }
    pub(crate) fn protobuf_for(
        &self,
        mode: QiMode,
    ) -> Result<proto::Transaction, TransactionError> {
        self.validate_for(mode)?;
        Ok(proto::Transaction {
            r#type: Some(2),
            chain_id: Some(integer_bytes(self.chain_id)),
            data: Some(self.data.clone()),
            tx_ins: Some(proto::Inputs {
                tx_ins: self
                    .inputs
                    .iter()
                    .map(|i| proto::Input {
                        previous_out_point: Some(proto::OutPoint {
                            hash: Some(proto::Hash {
                                value: i.previous_output.transaction_hash.bytes().to_vec(),
                            }),
                            index: Some(u32::from(i.previous_output.index)),
                        }),
                        pub_key: Some(i.public_key.to_compressed().to_vec()),
                    })
                    .collect(),
            }),
            tx_outs: Some(proto::Outputs {
                tx_outs: self
                    .outputs
                    .iter()
                    .map(|o| proto::Output {
                        denomination: Some(u32::from(o.denomination.index())),
                        address: Some(o.address.bytes().to_vec()),
                        lock: Some(Vec::new()),
                    })
                    .collect(),
            }),
            ..Default::default()
        })
    }
    /// Canonical unsigned protobuf. Node UTXO existence/value checks remain required.
    pub fn unsigned_bytes(&self) -> Result<Vec<u8>, TransactionError> {
        encode(&self.protobuf()?)
    }
    /// Digest signed by Schnorr or the eventual qualified local MuSig implementation.
    pub fn signing_digest(&self) -> Result<Hash32, TransactionError> {
        Ok(Hash32::from_bytes(keccak256(&self.unsigned_bytes()?)))
    }
    fn from_proto(p: &proto::Transaction) -> Result<Self, TransactionError> {
        Self::from_proto_for(p, QiMode::Ordinary)
    }
    pub(crate) fn from_proto_for(
        p: &proto::Transaction,
        mode: QiMode,
    ) -> Result<Self, TransactionError> {
        if p.r#type != Some(2) {
            return Err(TransactionError::WrongType);
        }
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        for i in &p
            .tx_ins
            .as_ref()
            .ok_or(TransactionError::InvalidField("inputs"))?
            .tx_ins
        {
            let point = i
                .previous_out_point
                .as_ref()
                .ok_or(TransactionError::InvalidField("outpoint"))?;
            let h = point
                .hash
                .as_ref()
                .ok_or(TransactionError::InvalidField("outpoint hash"))?;
            let transaction_hash = Hash32::from_bytes(
                h.value
                    .as_slice()
                    .try_into()
                    .map_err(|_| TransactionError::InvalidField("outpoint hash"))?,
            );
            let index = u16::try_from(
                point
                    .index
                    .ok_or(TransactionError::InvalidField("outpoint index"))?,
            )
            .map_err(|_| TransactionError::InvalidField("outpoint index"))?;
            let key = i
                .pub_key
                .as_deref()
                .ok_or(TransactionError::InvalidField("public key"))?;
            let public_key = PublicKey::from_sec1_bytes(key)
                .map_err(|_| TransactionError::InvalidField("public key"))?;
            inputs.push(QiInput {
                previous_output: OutPoint {
                    transaction_hash,
                    index,
                },
                public_key,
            });
        }
        for o in &p
            .tx_outs
            .as_ref()
            .ok_or(TransactionError::InvalidField("outputs"))?
            .tx_outs
        {
            if integer(o.lock.as_deref(), "lock")? != U256::ZERO {
                return Err(TransactionError::InvalidField("nonzero user output lock"));
            }
            let denomination = Denomination::new(
                u8::try_from(
                    o.denomination
                        .ok_or(TransactionError::InvalidField("denomination"))?,
                )
                .map_err(|_| TransactionError::InvalidField("denomination"))?,
            )?;
            let address = Address::try_from(
                o.address
                    .as_deref()
                    .ok_or(TransactionError::InvalidField("output address"))?,
            )
            .map_err(|_| TransactionError::InvalidField("output address"))?;
            outputs.push(QiOutput {
                address,
                denomination,
            });
        }
        let tx = Self {
            chain_id: integer(p.chain_id.as_deref(), "chain_id")?,
            inputs,
            outputs,
            data: p
                .data
                .clone()
                .ok_or(TransactionError::InvalidField("data"))?,
        };
        tx.validate_for(mode)?;
        Ok(tx)
    }
    /// Decode only canonical unsigned ordinary Qi transfers; unknown fields fail closed.
    pub fn decode_unsigned(bytes: &[u8]) -> Result<Self, TransactionError> {
        let tx = Self::from_proto(&decode(bytes)?)?;
        if tx.unsigned_bytes()? != bytes {
            return Err(TransactionError::InvalidEncoding);
        }
        Ok(tx)
    }
    /// Sign a single-input transaction using OS-randomized BIP340 auxiliary input.
    pub fn sign_single(&self, key: &SecretKey) -> Result<SignedQiTransaction, TransactionError> {
        self.validate()?;
        self.validate_ordinary_signing()?;
        if self.inputs.len() != 1 {
            return Err(TransactionError::MultiInputSigningUnavailable);
        }
        if key.public_key() != self.inputs[0].public_key {
            return Err(TransactionError::InvalidSignature);
        }
        let sig = key
            .sign_schnorr(self.signing_digest()?.bytes())
            .map_err(|_| TransactionError::InvalidSignature)?;
        self.attach_single_signature(sig)
    }
    /// Verify a separately generated single-input signature against its input key.
    pub fn attach_single_signature(
        &self,
        signature: SchnorrSignature,
    ) -> Result<SignedQiTransaction, TransactionError> {
        if self.inputs.len() != 1 {
            return Err(TransactionError::InvalidField("expected single input"));
        }
        self.attach_signature(signature)
    }

    /// Sign an ordinary transfer with every input key held locally, in input order.
    /// Multi-input signing uses ordered MuSig key aggregation; no distributed session is exposed.
    pub fn sign_local(&self, keys: &[&SecretKey]) -> Result<SignedQiTransaction, TransactionError> {
        self.attach_signature(self.sign_local_for(keys, QiMode::Ordinary)?)
    }
    pub(crate) fn sign_local_for(
        &self,
        keys: &[&SecretKey],
        mode: QiMode,
    ) -> Result<SchnorrSignature, TransactionError> {
        self.validate_for(mode)?;
        if mode == QiMode::Ordinary {
            self.validate_ordinary_signing()?;
        }
        if keys.len() != self.inputs.len()
            || keys
                .iter()
                .zip(&self.inputs)
                .any(|(key, input)| key.public_key() != input.public_key)
        {
            return Err(TransactionError::InvalidSignature);
        }
        let digest = keccak256(&encode(&self.protobuf_for(mode)?)?);
        if keys.len() == 1 {
            return keys[0]
                .sign_schnorr(&digest)
                .map_err(|_| TransactionError::InvalidSignature);
        }
        let public: Vec<_> = self.inputs.iter().map(|input| input.public_key).collect();
        OrderedKeyAggregate::new(&public)
            .and_then(|context| context.sign_local(keys, &digest))
            .map_err(|_| TransactionError::InvalidSignature)
    }

    /// Verify a signature against the exact ordered input keys and unsigned payload.
    pub fn attach_signature(
        &self,
        signature: SchnorrSignature,
    ) -> Result<SignedQiTransaction, TransactionError> {
        self.verify_signature_for(signature, QiMode::Ordinary)?;
        Ok(SignedQiTransaction {
            transaction: self.clone(),
            signature,
        })
    }
    pub(crate) fn verify_signature_for(
        &self,
        signature: SchnorrSignature,
        mode: QiMode,
    ) -> Result<(), TransactionError> {
        self.validate_for(mode)?;
        if mode == QiMode::Ordinary {
            self.validate_ordinary_signing()?;
        }
        let digest = keccak256(&encode(&self.protobuf_for(mode)?)?);
        if self.inputs.len() > 1 {
            let public: Vec<_> = self.inputs.iter().map(|input| input.public_key).collect();
            OrderedKeyAggregate::new(&public)
                .and_then(|context| context.verify_prehash(&digest, &signature))
                .map_err(|_| TransactionError::InvalidSignature)?;
        } else {
            let compressed = self.inputs[0].public_key.to_compressed();
            let x: [u8; 32] = compressed[1..]
                .try_into()
                .map_err(|_| TransactionError::InvalidSignature)?;
            SchnorrPublicKey::from_bytes(&x)
                .and_then(|key| key.verify(&digest, &signature))
                .map_err(|_| TransactionError::InvalidSignature)?;
        }
        Ok(())
    }
}
impl QiTransaction {
    fn validate_ordinary_signing(&self) -> Result<(), TransactionError> {
        if !self.data.is_empty() {
            return Err(TransactionError::InvalidField(
                "ordinary Qi transfers require empty data; conversion/wrapping needs a qualified builder",
            ));
        }
        if self.chain_id == U256::ZERO {
            return Err(TransactionError::InvalidField("chain_id"));
        }
        Ok(())
    }
}

/// Immutable signature-verified ordinary Qi transfer. UTXO acceptance still requires a node.
#[derive(Clone, Debug)]
pub struct SignedQiTransaction {
    transaction: QiTransaction,
    signature: SchnorrSignature,
}
impl SignedQiTransaction {
    /// Immutable authorized payload.
    pub fn transaction(&self) -> &QiTransaction {
        &self.transaction
    }
    /// Signature over the exact unsigned digest.
    pub fn signature(&self) -> SchnorrSignature {
        self.signature
    }
    /// Canonical signed bytes.
    pub fn signed_bytes(&self) -> Result<Vec<u8>, TransactionError> {
        let mut p = self.transaction.protobuf()?;
        p.signature = Some(self.signature.to_bytes().to_vec());
        encode(&p)
    }
    /// Transaction ID with the first input outpoint's destination zone and Qi bits.
    pub fn hash(&self) -> Result<Hash32, TransactionError> {
        let origin = self.transaction.validate()?.byte();
        let mut h = keccak256(&self.signed_bytes()?);
        h[0] = origin;
        h[1] |= 0x80;
        h[2] = origin;
        h[3] |= 0x80;
        Ok(h.into())
    }
    /// Decode canonical ordinary transfer bytes and verify the ordered-input signature.
    pub fn decode(bytes: &[u8]) -> Result<Self, TransactionError> {
        let p = decode(bytes)?;
        let tx = QiTransaction::from_proto(&p)?;
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
