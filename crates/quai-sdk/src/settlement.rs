//! Restartable bounded settlement observation for exact persisted signed candidates.
use crate::qi::QiError;
use quai_consensus::{SignedQiOperation, SignedQuaiTransaction};
use quai_primitives::{Hash32, QuaiAddress};
use quai_provider::{
    ConversionObservation, ConversionReference, EtxScanRequest, ExternalObservation,
    ExternalReference, Provider, QiCreditObservation,
};
use quai_rpc::Transport;
use quai_wallet::storage::{ObservationCache, ReservationId, SqliteStore};
use serde_json::{Value, json};

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
/// Current observations saved before this result is returned. A later reorg or
/// source change can invalidate them; claims and signed bytes remain untouched.
#[derive(Clone, Debug)]
pub struct SettlementUpdate {
    /// Compare-and-exchange cache revision after persistence.
    pub revision: u64,
    /// Conversion/refund observation, when that operation kind was selected.
    pub conversion: Option<ConversionObservation>,
    /// Wrapping/redemption/cross-zone observation for other kinds.
    pub external: Option<ExternalObservation>,
    /// Attributed currently indexed Qi amounts and reported output locks.
    pub qi_credit: Option<QiCreditObservation>,
}
fn block(block: quai_provider::BlockReference) -> Value {
    json!({"number": block.number, "hash": block.hash.to_string()})
}
/// Observe one bounded range for an exact durable candidate and atomically save a
/// compact public checkpoint/credit summary. Concurrent observers cannot overwrite
/// one another. RPC errors invalidate the prior cache using the same revision;
/// no error releases a nonce/input claim or rebroadcasts a transaction.
///
/// `SqliteStore::observation_cache` retrieves the version-1 JSON summary after
/// restart. Revalidate its block hashes before choosing a continuation range.
/// Cached scans and maturity are excluded from backups; signed references survive.
pub async fn track_settlement<T: Transport>(
    provider: &Provider<T>,
    store: &mut SqliteStore,
    id: ReservationId,
    candidate: Hash32,
    kind: SettlementKind,
    request: EtxScanRequest,
    max_outputs: usize,
) -> Result<SettlementUpdate, QiError> {
    let slot = match kind {
        SettlementKind::WqiRedemption { etx_index, .. } => etx_index,
        SettlementKind::CrossZoneQi { output_index } => output_index,
        _ => 0,
    };
    let previous = store.observation_cache(id, candidate, slot)?;
    let expected = previous.as_ref().map(|v| v.revision);
    let result = observe(provider, store, id, candidate, kind, request, max_outputs).await;
    let (conversion, external, qi_credit) = match result {
        Ok(value) => value,
        Err(error) => {
            // Never leave an old successful-looking observation in the cache
            // when this revalidation failed. A competing observer wins its CAS.
            let _ = store.compare_exchange_observation(id, candidate, slot, expected, None);
            return Err(error);
        }
    };
    let (origin, scan) = if let Some(c) = &conversion {
        (&c.origin, &c.scan)
    } else {
        let e = external.as_ref().ok_or(QiError::InvalidPolicy)?;
        (&e.origin, &e.scan)
    };
    let origin_block = match origin {
        quai_provider::ConversionOriginObservation::Emitted { block: b, .. } => Some(block(*b)),
        _ => None,
    };
    let scan_end = scan.as_ref().and_then(|s| s.last_block).map(block);
    let execution = scan.as_ref().and_then(|s| s.execution.as_ref()).map(|e| {
        json!({"hash": e.transaction.hash.to_string(), "block": e.transaction.inclusion.map(|i| block(quai_provider::BlockReference { number: i.block_number, hash: i.block_hash })), "outcome": e.receipt.as_ref().map(|r| match r.outcome { quai_provider::ReceiptOutcome::Succeeded => "succeeded", quai_provider::ReceiptOutcome::Failed => "failed", quai_provider::ReceiptOutcome::PostState(_) => "legacy" })})
    });
    let credit = qi_credit.as_ref().map(|c| json!({"beneficiary": c.beneficiary.to_string(), "head": block(c.head), "locked_qits": c.locked_qits.to_string(), "unlocked_qits": c.unlocked_qits.to_string(), "unobserved_qits": c.unobserved_qits.to_string()}));
    let (label, etx_index) = match kind {
        SettlementKind::Conversion => ("conversion", 0),
        SettlementKind::QiWrapping => ("qi_wrapping", 0),
        SettlementKind::WqiRedemption { etx_index, .. } => ("wqi_redemption", etx_index),
        SettlementKind::CrossZoneQuai => ("cross_zone_quai", 0),
        SettlementKind::CrossZoneQi { output_index } => ("cross_zone_qi", output_index),
    };
    let payload = serde_json::to_vec(&json!({"version": 1, "candidate": candidate.to_string(), "kind": label, "etx_index": etx_index, "zone": request.zone.byte(), "from": request.from, "to": request.to, "origin": origin_block, "scanned_through": scan_end, "execution": execution, "qi_credit": credit})).map_err(|_| QiError::InvalidPolicy)?;
    let revision =
        store.compare_exchange_observation(id, candidate, slot, expected, Some(&payload))?;
    Ok(SettlementUpdate {
        revision,
        conversion,
        external,
        qi_credit,
    })
}
// Reconstruct the reference from original immutable bytes or one validated edge.
async fn observe<T: Transport>(
    provider: &Provider<T>,
    store: &mut SqliteStore,
    id: ReservationId,
    candidate: Hash32,
    kind: SettlementKind,
    request: EtxScanRequest,
    max_outputs: usize,
) -> Result<
    (
        Option<ConversionObservation>,
        Option<ExternalObservation>,
        Option<QiCreditObservation>,
    ),
    QiError,
> {
    let root = store
        .signed_payload(id)?
        .ok_or(QiError::MissingSignedPayload)?;
    let variants = store.replacement_candidates(id)?;
    let bytes = std::iter::once(root)
        .chain(variants.into_iter().map(|v| v.payload))
        .find(|b| {
            if let Ok(qi) = SignedQiOperation::decode(b) {
                qi.hash().ok() == Some(candidate)
            } else {
                SignedQuaiTransaction::decode(b).and_then(|q| q.hash()).ok() == Some(candidate)
            }
        })
        .ok_or(QiError::MissingSignedPayload)?;
    let genesis = store.scope().genesis;
    let conversion = match kind {
        SettlementKind::Conversion => Some(
            if let Ok(SignedQiOperation::Conversion(qi)) = SignedQiOperation::decode(&bytes) {
                ConversionReference::from_qi(genesis, &qi)?
            } else {
                ConversionReference::from_quai(genesis, &SignedQuaiTransaction::decode(&bytes)?)?
            },
        ),
        _ => None,
    };
    if let Some(reference) = conversion {
        let (observation, credit) = provider
            .observe_conversion_qi_credit(&reference, request, max_outputs)
            .await?;
        return Ok((Some(observation), None, credit));
    }
    let reference = match kind {
        SettlementKind::QiWrapping => {
            let SignedQiOperation::Wrapping(qi) = SignedQiOperation::decode(&bytes)? else {
                return Err(QiError::InvalidPolicy);
            };
            ExternalReference::from_qi_wrapping(genesis, &qi)?
        }
        SettlementKind::WqiRedemption {
            contract,
            etx_index,
        } => ExternalReference::from_wqi_unwrap(
            genesis,
            &SignedQuaiTransaction::decode(&bytes)?,
            contract,
            etx_index,
        )?,
        SettlementKind::CrossZoneQi { output_index } => {
            let SignedQiOperation::Transfer(qi) = SignedQiOperation::decode(&bytes)? else {
                return Err(QiError::InvalidPolicy);
            };
            ExternalReference::from_cross_zone_qi(genesis, &qi, output_index)?
        }
        SettlementKind::CrossZoneQuai => ExternalReference::from_cross_zone_quai(
            genesis,
            &SignedQuaiTransaction::decode(&bytes)?,
        )?,
        _ => return Err(QiError::InvalidPolicy),
    };
    let (observation, credit) = provider
        .observe_external_qi_credit(&reference, request, max_outputs)
        .await?;
    Ok((None, Some(observation), credit))
}
/// Decode the bounded public cache envelope for a continuation UI. Its fields are
/// observations only; use a live tracker call before reporting current settlement.
pub fn cached_settlement(cache: &ObservationCache) -> Result<Option<Value>, QiError> {
    let Some(bytes) = &cache.payload else {
        return Ok(None);
    };
    if bytes.len() > 4096 {
        return Err(QiError::InvalidPolicy);
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|_| QiError::InvalidPolicy)?;
    if value["version"] != 1 || !value.is_object() {
        return Err(QiError::InvalidPolicy);
    }
    Ok(Some(value))
}
