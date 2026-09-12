//! Explicit supported Qi operation dispatch for durable custody and recovery.
use crate::{
    QiTransaction, SignedQiConversionTransaction, SignedQiTransaction, SignedQiWrappingTransaction,
    TransactionError,
};
use quai_primitives::Hash32;

/// A verified signed operation. Classification never weakens ordinary signing.
#[derive(Clone, Debug)]
pub enum SignedQiOperation {
    /// Ordinary Qi transfer.
    Transfer(SignedQiTransaction),
    /// Native Qi-to-Quai conversion.
    Conversion(SignedQiConversionTransaction),
    /// Native Qi deposit to an owner contract.
    Wrapping(SignedQiWrappingTransaction),
}
impl SignedQiOperation {
    /// Bounded canonical decode, selected by the exact data length. Each variant
    /// independently enforces its ledger, output and signature constraints.
    pub fn decode(bytes: &[u8]) -> Result<Self, TransactionError> {
        let p = crate::decode(bytes)?;
        match p.data.as_ref().map_or(0, Vec::len) {
            0 => SignedQiTransaction::decode(bytes).map(Self::Transfer),
            20 => SignedQiWrappingTransaction::decode(bytes).map(Self::Wrapping),
            22 => SignedQiConversionTransaction::decode(bytes).map(Self::Conversion),
            _ => Err(TransactionError::InvalidField(
                "unsupported Qi operation data",
            )),
        }
    }
    /// Exact ordered input/output/data fields.
    pub fn transaction(&self) -> &QiTransaction {
        match self {
            Self::Transfer(tx) => tx.transaction(),
            Self::Conversion(tx) => tx.transaction().transaction(),
            Self::Wrapping(tx) => tx.transaction().transaction(),
        }
    }
    /// Locally verified transaction identity.
    pub fn hash(&self) -> Result<Hash32, TransactionError> {
        match self {
            Self::Transfer(tx) => tx.hash(),
            Self::Conversion(tx) => tx.hash(),
            Self::Wrapping(tx) => tx.hash(),
        }
    }
    /// Canonical signed bytes retained for exact recovery/rebroadcast.
    pub fn signed_bytes(&self) -> Result<Vec<u8>, TransactionError> {
        match self {
            Self::Transfer(tx) => tx.signed_bytes(),
            Self::Conversion(tx) => tx.signed_bytes(),
            Self::Wrapping(tx) => tx.signed_bytes(),
        }
    }
}
