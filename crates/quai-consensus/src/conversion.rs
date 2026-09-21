//! Explicit conversion envelopes, separate from ordinary Qi signing.
//! Validation covers the pinned wire/static rules, not controller activation,
//! fork holds, exchange rates, fees, UTXO existence or eventual settlement.
use crate::{
    Denomination, QiInput, QiOutput, QiTransaction, QuaiTransaction, SignedQuaiTransaction,
    TransactionError, U256, decode, encode, qi::QiMode,
};
use quai_crypto::{RecoverableSignature, SchnorrSignature, SecretKey, keccak256};
use quai_primitives::{Hash32, Ledger, QiAddress, QuaiAddress, Zone};

/// Prime-block height at which go-quai v0.56.0 changed the k-Quai controller for
/// the KawPow transition, and one of the two heights it holds conversions after.
pub const KAWPOW_FORK_BLOCK: u64 = 1_171_500;
/// Prime-block height at which it changed the controller again for SHA-equivalent
/// difficulty. This is also where [`QiFeeProfile::V056ShaAnchored`] starts to apply.
///
/// [`QiFeeProfile::V056ShaAnchored`]: https://docs.rs/quai-provider
pub const SHA_EQUIVALENT_DIFFICULTY_FORK_BLOCK: u64 = 1_755_000;
/// Length of each of the two hold windows go-quai v0.56.0 implements, about six
/// to seven days of prime blocks. The pinned node's `KQuaiChangeHoldInterval`.
///
/// The node applies this constant to two specific fork heights. It is not a
/// duration the protocol promises for any future controller change.
pub const KQUAI_CHANGE_HOLD_INTERVAL: u64 = 20_000;

/// Whether the pinned node refuses conversions at this prime terminus height.
///
/// Mirrors go-quai v0.56.0, which hard-codes exactly two windows — one after
/// each k-Quai controller change it knows about — as literal height
/// comparisons. It is **not** a general rule that a controller change halts
/// conversions, and it forecasts nothing about future ones.
///
/// Both directions are held and they fail differently. Qi-to-Quai is refused at
/// pool admission, so nothing is spent and the same signed bytes work once the
/// window passes. Quai-to-Qi is refused inside the EVM, in `CreateETX` and
/// `opConvert`, so the transaction is mined and **its nonce and gas are burned**.
/// Wrapping is exempt in both directions.
///
/// The SDK does not enforce this — both windows are behind mainnet, and a chain
/// still below one runs parameters a matching chain ID does not attest. It is
/// exposed so a caller on such a chain can check before building. There is no
/// node-side signal to query instead.
///
/// See [docs/conversions.md] for the full account, including why no margin or
/// upper bound is applied.
///
/// [docs/conversions.md]: https://github.com/mpoletiek/quai-rust-sdk/blob/main/docs/conversions.md
pub const fn conversion_held(prime_terminus: u64) -> bool {
    const fn held_after(prime_terminus: u64, fork: u64) -> bool {
        prime_terminus >= fork && prime_terminus < fork + KQUAI_CHANGE_HOLD_INTERVAL
    }
    held_after(prime_terminus, KAWPOW_FORK_BLOCK)
        || held_after(prime_terminus, SHA_EQUIVALENT_DIFFICULTY_FORK_BLOCK)
}

/// Explicit slippage in ten-thousandths, restricted to the pinned node's clamp range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConversionSlippage(u16);
impl ConversionSlippage {
    /// Reject values outside 30..=9000 rather than silently clamping user intent.
    pub fn new(value: u16) -> Result<Self, TransactionError> {
        if !(30..=9000).contains(&value) {
            return Err(TransactionError::InvalidField("conversion slippage"));
        }
        Ok(Self(value))
    }
    /// Slippage in ten-thousandths.
    pub const fn value(self) -> u16 {
        self.0
    }
    /// Exact two-byte big-endian conversion data prefix.
    pub const fn to_be_bytes(self) -> [u8; 2] {
        self.0.to_be_bytes()
    }
}
/// Explicit Qi-to-Quai destination, refund and slippage intent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QiConversionIntent {
    /// Single same-zone account destination; repeated denominations aggregate here.
    pub destination: QuaiAddress,
    /// Same-zone Qi refund destination, retained in the signed data.
    /// The pinned node emits refund denominations of at least 1 Qi (1000 Qits);
    /// a smaller remainder may create no outputs even with a successful receipt.
    /// Inspect attributed outpoints rather than assuming the full value returned.
    pub refund: QiAddress,
    /// Caller-selected slippage; no implicit default.
    pub slippage: ConversionSlippage,
}
/// Immutable unsigned Qi-to-Quai conversion with validated static semantics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QiConversionTransaction {
    transaction: QiTransaction,
    intent: QiConversionIntent,
}
impl QiConversionTransaction {
    /// Build conversion denominations first, followed by unique same-zone Qi change.
    /// Input/output denominations are not proof of value conservation or fee sufficiency.
    pub fn new(
        chain_id: U256,
        inputs: Vec<QiInput>,
        converted: Vec<Denomination>,
        change: Vec<QiOutput>,
        intent: QiConversionIntent,
    ) -> Result<Self, TransactionError> {
        if converted.is_empty() {
            return Err(TransactionError::InvalidField("missing conversion output"));
        }
        let output_len = converted
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
        let mut data = Vec::with_capacity(22);
        data.extend_from_slice(&intent.slippage.to_be_bytes());
        data.extend_from_slice(intent.refund.address().bytes());
        let mut outputs = Vec::with_capacity(output_len);
        outputs.extend(converted.into_iter().map(|denomination| QiOutput {
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
    /// Explicitly classify an existing unsigned payload as a conversion. No bytes
    /// are changed; mixed destinations, wrapping data and cross-zone change fail.
    pub fn from_transaction(transaction: QiTransaction) -> Result<Self, TransactionError> {
        transaction.validate_for(QiMode::Conversion)?;
        let destination = transaction
            .outputs
            .iter()
            .find_map(|output| QuaiAddress::try_from(output.address).ok())
            .ok_or(TransactionError::InvalidScope)?;
        let refund = QiAddress::try_from(
            quai_primitives::Address::try_from(&transaction.data[2..])
                .map_err(|_| TransactionError::InvalidScope)?,
        )
        .map_err(|_| TransactionError::InvalidScope)?;
        let slippage = ConversionSlippage::new(u16::from_be_bytes([
            transaction.data[0],
            transaction.data[1],
        ]))?;
        Ok(Self {
            transaction,
            intent: QiConversionIntent {
                destination,
                refund,
                slippage,
            },
        })
    }
    /// Exact immutable wire fields. Ordinary Qi signing still rejects this payload.
    pub fn transaction(&self) -> &QiTransaction {
        &self.transaction
    }
    /// Explicit decoded destination, refund and slippage.
    pub fn intent(&self) -> QiConversionIntent {
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
    /// Canonical unsigned protobuf, retaining all 22 conversion bytes.
    pub fn unsigned_bytes(&self) -> Result<Vec<u8>, TransactionError> {
        encode(&self.transaction.protobuf_for(QiMode::Conversion)?)
    }
    /// Keccak digest over the exact unsigned envelope.
    pub fn signing_digest(&self) -> Result<Hash32, TransactionError> {
        Ok(keccak256(&self.unsigned_bytes()?).into())
    }
    /// Decode canonical unsigned conversion bytes without losing ordering or extensions.
    pub fn decode_unsigned(bytes: &[u8]) -> Result<Self, TransactionError> {
        let tx = Self::from_transaction(QiTransaction::from_proto_for(
            &decode(bytes)?,
            QiMode::Conversion,
        )?)?;
        if tx.unsigned_bytes()? != bytes {
            return Err(TransactionError::InvalidEncoding);
        }
        Ok(tx)
    }
    /// Sign only a single-input conversion with its matching local key.
    pub fn sign_single(
        &self,
        key: &SecretKey,
    ) -> Result<SignedQiConversionTransaction, TransactionError> {
        if self.transaction.inputs.len() != 1 {
            return Err(TransactionError::InvalidField("expected single input"));
        }
        self.sign_local(&[key])
    }
    /// Sign with all keys held locally in input order; duplicates preserve MuSig order.
    pub fn sign_local(
        &self,
        keys: &[&SecretKey],
    ) -> Result<SignedQiConversionTransaction, TransactionError> {
        let signature = self.transaction.sign_local_for(keys, QiMode::Conversion)?;
        self.attach_signature(signature)
    }
    /// Verify a supplied Schnorr/ordered aggregate signature against the exact payload.
    pub fn attach_signature(
        &self,
        signature: SchnorrSignature,
    ) -> Result<SignedQiConversionTransaction, TransactionError> {
        self.transaction
            .verify_signature_for(signature, QiMode::Conversion)?;
        Ok(SignedQiConversionTransaction {
            transaction: self.clone(),
            signature,
        })
    }
}
/// Immutable verified Qi-to-Quai conversion; not a node-acceptance guarantee.
#[derive(Clone, Debug)]
pub struct SignedQiConversionTransaction {
    transaction: QiConversionTransaction,
    signature: SchnorrSignature,
}
impl SignedQiConversionTransaction {
    /// Authorized conversion payload.
    pub fn transaction(&self) -> &QiConversionTransaction {
        &self.transaction
    }
    /// Signature covering the exact 22-byte conversion data and ordered inputs/outputs.
    pub fn signature(&self) -> SchnorrSignature {
        self.signature
    }
    /// Canonical signed protobuf.
    pub fn signed_bytes(&self) -> Result<Vec<u8>, TransactionError> {
        let mut p = self
            .transaction
            .transaction
            .protobuf_for(QiMode::Conversion)?;
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
    /// Decode canonical conversion bytes and verify every ordered input's signature.
    pub fn decode(bytes: &[u8]) -> Result<Self, TransactionError> {
        let p = decode(bytes)?;
        let tx = QiConversionTransaction::from_transaction(QiTransaction::from_proto_for(
            &p,
            QiMode::Conversion,
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

/// Conservative basis points removed by the pinned node's cubic flow discount from
/// every conversion in one prime-block batch. `batch_total` is the combined
/// pre-discount Quai value (Its) of all conversions processed up to and including
/// the one being evaluated; `flow_amount` is the block's conversion flow amount.
///
/// Conversions are processed in descending slippage order, and a conversion is
/// refunded when this discount exceeds its slippage. Other users' concurrent
/// conversions therefore affect the result, so single-transaction quote RPCs
/// cannot predict it. Returns 20 at or below the flow amount, `10 + ceil(10·(T/F)³)`
/// up to ten times it, and the node's 9000 floor beyond. Any k-Quai discount is
/// separate. Returns `None` for a zero flow amount or arithmetic overflow.
pub fn conversion_batch_discount_bps(batch_total: U256, flow_amount: U256) -> Option<u16> {
    if flow_amount == U256::ZERO {
        return None;
    }
    if batch_total <= flow_amount {
        return Some(20);
    }
    if batch_total > flow_amount.checked_mul(U256::from(10))? {
        return Some(9000);
    }
    let numerator = batch_total
        .checked_mul(batch_total)?
        .checked_mul(batch_total)?
        .checked_mul(U256::from(10))?;
    let denominator = flow_amount
        .checked_mul(flow_amount)?
        .checked_mul(flow_amount)?;
    let cubic = numerator.div_ceil(denominator);
    u16::try_from(cubic.checked_add(U256::from(10))?.min(U256::from(9000))).ok()
}

/// Static minimum value for Quai-to-Qi conversion in the pinned node: 10^19 base units.
pub const MIN_QUAI_CONVERSION_VALUE: u64 = 10_000_000_000_000_000_000;
/// Immutable explicit type-0 Quai-to-Qi request. Runtime activation and fee checks remain external.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuaiToQiTransaction {
    transaction: QuaiTransaction,
    destination: QiAddress,
    slippage: ConversionSlippage,
}
impl QuaiToQiTransaction {
    /// Classify an exact type-0 request: same-zone Qi destination is enforced on
    /// signature recovery, value >=10^19, data exactly two valid slippage bytes.
    /// No destination, amount, calldata or gas parameter is silently replaced.
    pub fn new(transaction: QuaiTransaction) -> Result<Self, TransactionError> {
        if transaction.chain_id == U256::ZERO
            || transaction.value < U256::from(MIN_QUAI_CONVERSION_VALUE)
            || transaction.data.len() != 2
        {
            return Err(TransactionError::InvalidField(
                "Quai conversion chain, value or data",
            ));
        }
        let destination =
            QiAddress::try_from(transaction.to.ok_or(TransactionError::InvalidScope)?)
                .map_err(|_| TransactionError::InvalidScope)?;
        let slippage = ConversionSlippage::new(u16::from_be_bytes([
            transaction.data[0],
            transaction.data[1],
        ]))?;
        transaction.unsigned_bytes()?;
        Ok(Self {
            transaction,
            destination,
            slippage,
        })
    }
    /// Exact immutable account transaction, suitable for external signer review.
    pub fn transaction(&self) -> &QuaiTransaction {
        &self.transaction
    }
    /// Typed Qi destination.
    pub fn destination(&self) -> QiAddress {
        self.destination
    }
    /// Explicit validated slippage.
    pub fn slippage(&self) -> ConversionSlippage {
        self.slippage
    }
    /// Canonical unsigned bytes retaining the exact two-byte conversion data.
    pub fn unsigned_bytes(&self) -> Result<Vec<u8>, TransactionError> {
        self.transaction.unsigned_bytes()
    }
    /// Exact account signing digest.
    pub fn signing_digest(&self) -> Result<Hash32, TransactionError> {
        self.transaction.signing_digest()
    }
    /// Decode a canonical explicit account conversion request.
    pub fn decode_unsigned(bytes: &[u8]) -> Result<Self, TransactionError> {
        Self::new(QuaiTransaction::decode_unsigned(bytes)?)
    }
    /// Sign and enforce that the recovered account sender shares the Qi destination zone.
    pub fn sign(&self, key: &SecretKey) -> Result<SignedQuaiTransaction, TransactionError> {
        self.transaction.sign(key)
    }
    /// Attach a signature; ordinary account recovery enforces same-zone Qi destinations.
    pub fn attach_signature(
        &self,
        signature: RecoverableSignature,
    ) -> Result<SignedQuaiTransaction, TransactionError> {
        self.transaction.attach_signature(signature)
    }
}
