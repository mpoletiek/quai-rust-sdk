//! Portable interpretation and destination observations for exact signed payloads.
use quai_consensus::{SignedQiOperation, SignedQuaiTransaction, TransactionError};
use quai_primitives::{Hash32, QuaiAddress};
use quai_provider::{
    ConversionObservation, ConversionReference, EtxScanRequest, ExternalObservation,
    ExternalReference, Provider, ProviderError, QiCreditObservation,
};
use quai_rpc::{Transport, U256};
use quai_wallet::discovery::NetworkScope;
/// Static identity/operation mismatch or provider observation failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SettlementObservationError {
    /// Invalid scope, signed origin, operation kind or exact intent.
    #[error("invalid signed settlement observation")]
    Invalid,
    /// Signed transaction failed consensus decoding.
    #[error(transparent)]
    Transaction(#[from] TransactionError),
    /// Destination scan or canonical observation failed.
    #[error(transparent)]
    Provider(#[from] ProviderError),
}
/// Explicit interpretation of a locally signed operation. No operation is inferred
/// from its transaction hash, and contract deployment identity remains caller-selected.
#[derive(Clone, Copy, Debug)]
pub enum SettlementKind {
    /// Direct Quai-to-Qi or Qi-to-Quai conversion, including its refund path.
    Conversion,
    /// Native Qi wrapping into protocol backing, before claimDeposit token minting.
    QiWrapping,
    /// Exact WQI ABI redemption into a native Qi beneficiary.
    WqiRedemption {
        /// Explicit expected contract deployment.
        contract: QuaiAddress,
        /// Emitted external index, normally zero for a direct unwrap call.
        etx_index: u16,
    },
    /// One explicit cross-zone Qi output; each output has its own ETX correlation.
    CrossZoneQi {
        /// Index in the original signed Qi output list.
        output_index: u16,
    },
    /// Direct cross-zone Quai transfer/call.
    CrossZoneQuai,
}
/// Advisory destination observations; origin inclusion does not imply maturity.
#[derive(Clone, Debug)]
pub struct SignedSettlementObservation {
    /// Conversion and possible refund path, when explicitly selected.
    pub conversion: Option<ConversionObservation>,
    /// Wrapping, redemption or cross-zone external execution.
    pub external: Option<ExternalObservation>,
    /// Attributed currently indexed Qi value and reported locks, when available.
    pub qi_credit: Option<QiCreditObservation>,
}
/// Reconstruct the exact conversion/wrapping/redemption/cross-zone reference from
/// canonical signed bytes in a trusted network scope. Observe only the requested
/// bounded destination range and current Qi credit; no custody/cache write, send
/// or finality claim occurs. Historical/atomic UTXO coverage is not inferred.
pub async fn observe_signed_settlement<T: Transport>(
    provider: &Provider<T>,
    scope: NetworkScope,
    bytes: &[u8],
    kind: SettlementKind,
    request: EtxScanRequest,
    max_outputs: usize,
) -> Result<SignedSettlementObservation, SettlementObservationError> {
    if scope.chain_id == U256::ZERO || scope.genesis == Hash32::ZERO {
        return Err(SettlementObservationError::Invalid);
    }
    if let Ok(qi) = SignedQiOperation::decode(bytes) {
        if qi.transaction().chain_id != scope.chain_id || qi.origin_zone()? != scope.zone {
            return Err(SettlementObservationError::Invalid);
        }
    } else {
        let account = SignedQuaiTransaction::decode(bytes)?;
        if account.transaction().chain_id != scope.chain_id || account.from().zone() != scope.zone {
            return Err(SettlementObservationError::Invalid);
        }
    }
    let genesis = scope.genesis;
    let conversion = match kind {
        SettlementKind::Conversion => Some(
            if let Ok(SignedQiOperation::Conversion(qi)) = SignedQiOperation::decode(bytes) {
                ConversionReference::from_qi(genesis, &qi)?
            } else {
                ConversionReference::from_quai(genesis, &SignedQuaiTransaction::decode(bytes)?)?
            },
        ),
        _ => None,
    };
    if let Some(reference) = conversion {
        let (observation, credit) = provider
            .observe_conversion_qi_credit(&reference, request, max_outputs)
            .await?;
        return Ok(SignedSettlementObservation {
            conversion: Some(observation),
            external: None,
            qi_credit: credit,
        });
    }
    let reference = match kind {
        SettlementKind::QiWrapping => {
            let SignedQiOperation::Wrapping(qi) = SignedQiOperation::decode(bytes)? else {
                return Err(SettlementObservationError::Invalid);
            };
            ExternalReference::from_qi_wrapping(genesis, &qi)?
        }
        SettlementKind::WqiRedemption {
            contract,
            etx_index,
        } => ExternalReference::from_wqi_unwrap(
            genesis,
            &SignedQuaiTransaction::decode(bytes)?,
            contract,
            etx_index,
        )?,
        SettlementKind::CrossZoneQi { output_index } => {
            let SignedQiOperation::Transfer(qi) = SignedQiOperation::decode(bytes)? else {
                return Err(SettlementObservationError::Invalid);
            };
            ExternalReference::from_cross_zone_qi(genesis, &qi, output_index)?
        }
        SettlementKind::CrossZoneQuai => ExternalReference::from_cross_zone_quai(
            genesis,
            &SignedQuaiTransaction::decode(bytes)?,
        )?,
        _ => return Err(SettlementObservationError::Invalid),
    };
    let (observation, credit) = provider
        .observe_external_qi_credit(&reference, request, max_outputs)
        .await?;
    Ok(SignedSettlementObservation {
        conversion: None,
        external: Some(observation),
        qi_credit: credit,
    })
}
