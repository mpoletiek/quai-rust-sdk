//! Quai-native transaction encoding and signing; never Ethereum transaction envelopes.
mod conversion;
mod qi_operation;
mod wrapping;
pub use qi_operation::SignedQiOperation;
pub use wrapping::{QiWrappingIntent, QiWrappingTransaction, SignedQiWrappingTransaction};
pub mod document;
pub mod proto;
mod qi;
mod quai;
pub use conversion::{
    ConversionSlippage, MIN_QUAI_CONVERSION_VALUE, QiConversionIntent, QiConversionTransaction,
    QuaiToQiTransaction, SignedQiConversionTransaction, conversion_batch_discount_bps,
};
use prost::Message;
pub use qi::{Denomination, OutPoint, QiInput, QiOutput, QiTransaction, SignedQiTransaction};
pub use quai::{AccessTuple, QuaiTransaction, SignedQuaiTransaction};
pub use ruint::aliases::U256;
use thiserror::Error;

/// Encoded input/output byte ceiling, not a promise that a node accepts this size.
pub const MAX_TRANSACTION_BYTES: usize = 1024 * 1024;
/// Total nested-message ceiling, checked before protobuf decoder allocation.
pub const MAX_TRANSACTION_MESSAGES: usize = 16384;

/// Transaction validation errors do not echo transaction data or secret material.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TransactionError {
    /// Input exceeds this SDK's allocation policy.
    #[error("transaction exceeds size limit")]
    TooLarge,
    /// Wire encoding is malformed, ambiguous, noncanonical, or includes unknown fields.
    #[error("invalid or noncanonical transaction encoding")]
    InvalidEncoding,
    /// A required field is absent or malformed.
    #[error("invalid transaction field: {0}")]
    InvalidField(&'static str),
    /// Unsupported transaction kind for this typed decoder.
    #[error("unexpected transaction type")]
    WrongType,
    /// The signer/recipient has an unsupported ledger or location combination.
    #[error("invalid transaction address scope")]
    InvalidScope,
    /// A signature failed validation or recovery.
    #[error("transaction signature validation failed")]
    InvalidSignature,
    /// A single-input signing method was given multiple inputs. Use the explicit
    /// ordered local-key aggregation signing path for such transactions.
    #[error("multi-input transaction requires explicit ordered-key signing")]
    MultiInputSigningUnavailable,
}

fn integer_bytes(n: U256) -> Vec<u8> {
    let bytes = n.to_be_bytes::<32>();
    let first = bytes.iter().position(|b| *b != 0).unwrap_or(bytes.len());
    bytes[first..].to_vec()
}
fn integer(bytes: Option<&[u8]>, name: &'static str) -> Result<U256, TransactionError> {
    let b = bytes.ok_or(TransactionError::InvalidField(name))?;
    if b.len() > 32 || b.first() == Some(&0) {
        return Err(TransactionError::InvalidField(name));
    }
    Ok(U256::from_be_slice(b))
}
fn encode(p: &proto::Transaction) -> Result<Vec<u8>, TransactionError> {
    if p.encoded_len() > MAX_TRANSACTION_BYTES {
        return Err(TransactionError::TooLarge);
    }
    Ok(p.encode_to_vec())
}
fn decode(bytes: &[u8]) -> Result<proto::Transaction, TransactionError> {
    if bytes.len() > MAX_TRANSACTION_BYTES {
        return Err(TransactionError::TooLarge);
    }
    let mut budget = MAX_TRANSACTION_MESSAGES;
    preflight(bytes, WireKind::Transaction, &mut budget)?;
    let p = proto::Transaction::decode(bytes).map_err(|_| TransactionError::InvalidEncoding)?;
    // Prost otherwise drops unknown fields and accepts duplicates/overlong varints.
    if p.encode_to_vec() != bytes {
        return Err(TransactionError::InvalidEncoding);
    }
    Ok(p)
}

#[derive(Clone, Copy)]
enum WireKind {
    Transaction,
    AccessList,
    AccessTuple,
    Hash,
    Inputs,
    Input,
    OutPoint,
    Outputs,
    Output,
}
fn preflight(mut bytes: &[u8], kind: WireKind, budget: &mut usize) -> Result<(), TransactionError> {
    *budget = budget.checked_sub(1).ok_or(TransactionError::TooLarge)?;
    fn varint(bytes: &mut &[u8]) -> Result<u64, TransactionError> {
        let mut n = 0u64;
        for shift in (0..70).step_by(7) {
            let (&b, rest) = bytes
                .split_first()
                .ok_or(TransactionError::InvalidEncoding)?;
            *bytes = rest;
            if shift == 63 && b > 1 {
                return Err(TransactionError::InvalidEncoding);
            }
            n |= u64::from(b & 127) << shift;
            if b < 128 {
                return Ok(n);
            }
        }
        Err(TransactionError::InvalidEncoding)
    }
    while !bytes.is_empty() {
        let key = varint(&mut bytes)?;
        let tag = key >> 3;
        if tag == 0 {
            return Err(TransactionError::InvalidEncoding);
        }
        match key & 7 {
            0 => {
                varint(&mut bytes)?;
            }
            1 => {
                bytes = bytes.get(8..).ok_or(TransactionError::InvalidEncoding)?;
            }
            5 => {
                bytes = bytes.get(4..).ok_or(TransactionError::InvalidEncoding)?;
            }
            2 => {
                let len =
                    usize::try_from(varint(&mut bytes)?).map_err(|_| TransactionError::TooLarge)?;
                let part = bytes.get(..len).ok_or(TransactionError::InvalidEncoding)?;
                bytes = &bytes[len..];
                use WireKind::*;
                let child = match (kind, tag) {
                    (Transaction, 9) => Some(AccessList),
                    (Transaction, 13 | 19 | 20) => Some(Hash),
                    (Transaction, 15) => Some(Inputs),
                    (Transaction, 16) => Some(Outputs),
                    (AccessList, 1) => Some(AccessTuple),
                    (AccessTuple, 2) => Some(Hash),
                    (Inputs, 1) => Some(Input),
                    (Input, 1) => Some(OutPoint),
                    (OutPoint, 1) => Some(Hash),
                    (Outputs, 1) => Some(Output),
                    _ => None,
                };
                if let Some(child) = child {
                    preflight(part, child, budget)?;
                }
            }
            _ => return Err(TransactionError::InvalidEncoding),
        }
    }
    Ok(())
}

/// Decode a bounded canonical protobuf object, preserving explicit field presence.
/// This checks the wire format, not transaction semantics, signatures or spendability.
/// Use document::TransactionDocument::from_proto or a concrete signed type to validate.
pub fn decode_proto_transaction(bytes: &[u8]) -> Result<proto::Transaction, TransactionError> {
    decode(bytes)
}
/// Encode a raw protobuf object within the byte/message policy. This does not
/// establish a valid transaction or authorize submission.
pub fn encode_proto_transaction(value: &proto::Transaction) -> Result<Vec<u8>, TransactionError> {
    let bytes = encode(value)?;
    let mut budget = MAX_TRANSACTION_MESSAGES;
    preflight(&bytes, WireKind::Transaction, &mut budget)?;
    Ok(bytes)
}
