use crate::{Provider, ProviderError, RpcData, types};
use quai_consensus::{
    SignedQiConversionTransaction, SignedQiTransaction, SignedQuaiTransaction, TransactionError,
};
use quai_primitives::{Hash32, Zone};
use quai_rpc::{RpcError, Transport};
use serde_json::json;
use thiserror::Error;

/// The server acknowledged the exact expected ID. Inclusion/finality are separate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BroadcastResult {
    /// Locally computed transaction identity, matched against the node response.
    pub transaction_hash: Hash32,
    /// Origin zone selected from the recovered signing address.
    pub zone: Zone,
}

/// Submission errors distinguish preflight failures from ambiguous send outcomes.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BroadcastError {
    /// No submit request was made; configuration/chain validation failed.
    #[error("transaction preflight failed: {0}")]
    Preflight(#[source] ProviderError),
    /// No submit request was made; the immutable transaction could not be encoded.
    #[error("signed transaction encoding failed: {0}")]
    Encoding(#[source] TransactionError),
    /// A submit was attempted. Acceptance may have occurred; never blindly retry.
    #[error("transaction {transaction_hash} submission outcome is ambiguous: {source}")]
    Ambiguous {
        /// Identity to use for reconciliation after transport/remote failures.
        transaction_hash: Hash32,
        /// Origin zone for reconciliation.
        zone: Zone,
        /// Sanitized transport/remote failure; raw remote fields require explicit access.
        #[source]
        source: RpcError,
    },
    /// A submit was attempted but the node response was malformed or named another ID.
    #[error(
        "transaction {transaction_hash} has an invalid acknowledgement; acceptance is ambiguous"
    )]
    InvalidAcknowledgement {
        /// Locally computed identity; do not replace it with a conflicting remote value.
        transaction_hash: Hash32,
        /// Origin zone for reconciliation.
        zone: Zone,
        /// Well-formed but conflicting remote ID, or None for malformed response bytes.
        reported_hash: Option<Hash32>,
    },
}

impl BroadcastError {
    /// How to react; any failure after a submit was attempted is ambiguous.
    pub fn class(&self) -> quai_primitives::ErrorClass {
        match self {
            Self::Preflight(error) => error.class(),
            Self::Encoding(_) => quai_primitives::ErrorClass::Invalid,
            Self::Ambiguous { .. } | Self::InvalidAcknowledgement { .. } => {
                quai_primitives::ErrorClass::Ambiguous
            }
        }
    }
}
impl BroadcastError {
    /// True for errors after calling the submit transport. Conservative for remote errors too.
    pub fn acceptance_is_ambiguous(&self) -> bool {
        matches!(
            self,
            Self::Ambiguous { .. } | Self::InvalidAcknowledgement { .. }
        )
    }
    /// Expected identity retained for every send-stage failure.
    pub fn transaction_hash(&self) -> Option<Hash32> {
        match self {
            Self::Ambiguous {
                transaction_hash, ..
            }
            | Self::InvalidAcknowledgement {
                transaction_hash, ..
            } => Some(*transaction_hash),
            _ => None,
        }
    }
}

impl<T: Transport> Provider<T> {
    /// Submit an explicitly verified Qi wrapping transaction exactly once.
    /// Callers must persist its bytes before exposing them to the network.
    pub async fn broadcast_qi_wrapping(
        &self,
        signed: &quai_consensus::SignedQiWrappingTransaction,
    ) -> Result<BroadcastResult, BroadcastError> {
        let actual = signed.transaction().chain_id();
        if actual != self.expected_chain_id {
            return Err(BroadcastError::Preflight(ProviderError::ChainMismatch {
                expected: self.expected_chain_id,
                actual,
            }));
        }
        self.submit_signed_bytes(
            signed.transaction().origin_zone(),
            signed.hash().map_err(BroadcastError::Encoding)?,
            signed.signed_bytes().map_err(BroadcastError::Encoding)?,
        )
        .await
    }
    /// Submit exactly one canonical signed Quai transaction, with no automatic retry.
    ///
    /// Chain identity is checked locally before any RPC, then at the sender's routed
    /// endpoint. The expected ID and canonical bytes are computed before submission.
    /// Dropping this future cancels waiting but does not roll back a possibly accepted
    /// transaction. Retain `signed.hash()` before awaiting so cancellation can be reconciled.
    pub async fn broadcast(
        &self,
        signed: &SignedQuaiTransaction,
    ) -> Result<BroadcastResult, BroadcastError> {
        let actual = signed.transaction().chain_id;
        if actual != self.expected_chain_id {
            return Err(BroadcastError::Preflight(ProviderError::ChainMismatch {
                expected: self.expected_chain_id,
                actual,
            }));
        }
        let transaction_hash = signed.hash().map_err(BroadcastError::Encoding)?;
        let bytes = signed.signed_bytes().map_err(BroadcastError::Encoding)?;
        let zone = signed.from().zone();
        self.submit_signed_bytes(zone, transaction_hash, bytes)
            .await
    }

    /// Submit one verified ordinary Qi transaction to its validated input-origin zone.
    /// Fee sufficiency and UTXO acceptance are node decisions. Ambiguity retains the
    /// locally computed transaction ID; no automatic retry occurs.
    pub async fn broadcast_qi(
        &self,
        signed: &SignedQiTransaction,
    ) -> Result<BroadcastResult, BroadcastError> {
        let actual = signed.transaction().chain_id;
        if actual != self.expected_chain_id {
            return Err(BroadcastError::Preflight(ProviderError::ChainMismatch {
                expected: self.expected_chain_id,
                actual,
            }));
        }
        let zone = signed
            .transaction()
            .origin_zone()
            .map_err(BroadcastError::Encoding)?;
        let hash = signed.hash().map_err(BroadcastError::Encoding)?;
        let bytes = signed.signed_bytes().map_err(BroadcastError::Encoding)?;
        self.submit_signed_bytes(zone, hash, bytes).await
    }

    /// Explicitly submit a verified Qi-to-Quai conversion once. This does not
    /// qualify controller activation, hold intervals, fees, refunds or settlement.
    /// Retain canonical bytes durably before calling; ambiguous sends never retry.
    pub async fn broadcast_qi_conversion(
        &self,
        signed: &SignedQiConversionTransaction,
    ) -> Result<BroadcastResult, BroadcastError> {
        let actual = signed.transaction().chain_id();
        if actual != self.expected_chain_id {
            return Err(BroadcastError::Preflight(ProviderError::ChainMismatch {
                expected: self.expected_chain_id,
                actual,
            }));
        }
        let zone = signed.transaction().origin_zone();
        let hash = signed.hash().map_err(BroadcastError::Encoding)?;
        let bytes = signed.signed_bytes().map_err(BroadcastError::Encoding)?;
        self.submit_signed_bytes(zone, hash, bytes).await
    }

    async fn submit_signed_bytes(
        &self,
        zone: Zone,
        transaction_hash: Hash32,
        bytes: Vec<u8>,
    ) -> Result<BroadcastResult, BroadcastError> {
        let data = RpcData::new(bytes).map_err(BroadcastError::Preflight)?;
        let endpoint = self
            .routing
            .endpoint(zone.into())
            .map_err(|error| BroadcastError::Preflight(error.into()))?;
        self.chain_id(zone.into())
            .await
            .map_err(BroadcastError::Preflight)?;
        let response = self
            .transport
            .request(endpoint, "quai_sendRawTransaction", json!([data.to_hex()]))
            .await
            .map_err(|source| BroadcastError::Ambiguous {
                transaction_hash,
                zone,
                source,
            })?;
        let reported = types::hash(response).ok();
        if reported != Some(transaction_hash) {
            return Err(BroadcastError::InvalidAcknowledgement {
                transaction_hash,
                zone,
                reported_hash: reported,
            });
        }
        Ok(BroadcastResult {
            transaction_hash,
            zone,
        })
    }
}
