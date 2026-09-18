//! Revalidate durable observation anchors before resuming a destination scan.
use super::{ObservationRequest, SettlementKind, SettlementUpdate, cached_settlement, track_inner};
use crate::qi::QiError;
use quai_primitives::{Hash32, Zone};
use quai_provider::{BlockReference, EtxScanRequest, Provider};
use quai_rpc::Transport;
use quai_wallet::storage::{NetworkScope, ReservationId, SqliteStore};
use serde_json::Value;
/// A revalidated source cursor, not a proof of finality or full wallet history.
/// Signed bytes and claims remain authoritative; a later reorg can invalidate this view.
#[derive(Clone, Copy, Debug)]
pub struct SettlementCursor {
    zone: Zone,
    id: ReservationId,
    candidate: Hash32,
    kind: SettlementKind,
    revision: u64,
    scope: NetworkScope,
    last: BlockReference,
    execution: Option<BlockReference>,
}
impl SettlementCursor {
    /// Refresh the next bounded page or previously found execution and atomically
    /// save it only if this cursor's durable revision is still current. Stale
    /// cursors fail before network I/O; concurrent writes also fail the final CAS.
    pub async fn track<T: Transport>(
        self,
        provider: &Provider<T>,
        store: &mut SqliteStore,
        through: u64,
        max_transactions_per_block: usize,
        max_total_transactions: usize,
        max_outputs: usize,
    ) -> Result<SettlementUpdate, QiError> {
        if store.scope() != self.scope {
            return Err(QiError::IdentityMismatch);
        }
        let request = self.request(through, max_transactions_per_block, max_total_transactions)?;
        track_inner(
            provider,
            store,
            ObservationRequest {
                id: self.id,
                candidate: self.candidate,
                kind: self.kind,
                request,
                max_outputs,
            },
            Some(self.revision),
        )
        .await
    }

    /// Last block covered by the saved page and checked against the current source.
    pub fn scanned_through(self) -> BlockReference {
        self.last
    }
    /// Previously observed execution, whose block was also revalidated.
    pub fn execution(self) -> Option<BlockReference> {
        self.execution
    }
    /// Construct a bounded next page. An existing execution is reread at its exact
    /// height so receipt and current output locks are refreshed rather than skipped.
    /// `through` is an explicit inclusive ceiling, never an implicit latest query.
    pub fn request(
        self,
        through: u64,
        max_transactions_per_block: usize,
        max_total_transactions: usize,
    ) -> Result<EtxScanRequest, QiError> {
        let from = self
            .execution
            .map(|b| b.number)
            .or_else(|| self.last.number.checked_add(1))
            .ok_or(QiError::InvalidPolicy)?;
        if through < from
            || through > i64::MAX as u64
            || !(1..=4096).contains(&max_transactions_per_block)
            || !(1..=65536).contains(&max_total_transactions)
        {
            return Err(QiError::InvalidPolicy);
        }
        let to = if self.execution.is_some() {
            from
        } else {
            through.min(from.saturating_add(255))
        };
        Ok(EtxScanRequest::new(
            self.zone,
            from,
            to,
            max_transactions_per_block,
            max_total_transactions,
        )
        .with_preceding_block(self.execution.is_none().then_some(self.last)))
    }
}
fn anchor(value: &Value) -> Result<Option<BlockReference>, QiError> {
    if value.is_null() {
        return Ok(None);
    }
    let object = value.as_object().ok_or(QiError::InvalidPolicy)?;
    if object.len() != 2 {
        return Err(QiError::InvalidPolicy);
    }
    let number = value["number"]
        .as_u64()
        .filter(|n| *n > 0 && *n <= i64::MAX as u64)
        .ok_or(QiError::InvalidPolicy)?;
    let hash = value["hash"]
        .as_str()
        .ok_or(QiError::InvalidPolicy)?
        .parse::<Hash32>()
        .map_err(|_| QiError::InvalidPolicy)?;
    if hash == Hash32::ZERO {
        return Err(QiError::InvalidPolicy);
    }
    Ok(Some(BlockReference { number, hash }))
}
/// Recheck network identity, origin, page end and any destination execution saved
/// for an exact durable candidate. Returns None when no complete cursor exists or
/// an anchor was reorganized/unavailable; the caller must restart its explicit range.
/// Errors and invalid anchors tombstone only the revision read by this call.
/// A competing observer's newer cache is never cleared. This never submits or
/// releases funds. Use `SettlementCursor::track` to reconstruct the signed intent
/// again and retain revision checks through the resumed observation.
pub async fn revalidate_settlement_cursor<T: Transport>(
    provider: &Provider<T>,
    store: &mut SqliteStore,
    id: ReservationId,
    candidate: Hash32,
    kind: SettlementKind,
    zone: Zone,
) -> Result<Option<SettlementCursor>, QiError> {
    let (label, slot) = match kind {
        SettlementKind::Conversion => ("conversion", 0),
        SettlementKind::QiWrapping => ("qi_wrapping", 0),
        SettlementKind::WqiRedemption { etx_index, .. } => ("wqi_redemption", etx_index),
        SettlementKind::CrossZoneQuai => ("cross_zone_quai", 0),
        SettlementKind::CrossZoneQi { output_index } => ("cross_zone_qi", output_index),
    };
    let generation = store.observation_generation()?;
    let Some(cache) = store.observation_cache(id, candidate, slot)? else {
        return Ok(None);
    };
    if cache.payload.is_none() {
        return Ok(None);
    }
    let scope = store.scope();
    let mut had_cursor = false;
    let result = async {
        let value = cached_settlement(&cache)?.ok_or(QiError::InvalidPolicy)?;
        if value["candidate"] != candidate.to_string()
            || value["kind"] != label
            || value["etx_index"] != slot
            || value["zone"] != zone.byte()
        {
            return Err(QiError::InvalidPolicy);
        }
        let from = value["from"].as_u64().ok_or(QiError::InvalidPolicy)?;
        let to = value["to"].as_u64().ok_or(QiError::InvalidPolicy)?;
        if from == 0 || to < from || to > i64::MAX as u64 || to - from >= 256 {
            return Err(QiError::InvalidPolicy);
        }
        let origin = anchor(&value["origin"])?;
        let last = anchor(&value["scanned_through"])?;
        let execution = if value["execution"].is_null() {
            None
        } else {
            let hash = value["execution"]["hash"]
                .as_str()
                .ok_or(QiError::InvalidPolicy)?
                .parse::<Hash32>()
                .map_err(|_| QiError::InvalidPolicy)?;
            if hash == Hash32::ZERO {
                return Err(QiError::InvalidPolicy);
            }
            Some(anchor(&value["execution"]["block"])?.ok_or(QiError::InvalidPolicy)?)
        };
        if (last.is_some() && origin.is_none()) || (execution.is_some() && last.is_none()) {
            return Err(QiError::InvalidPolicy);
        }
        let (Some(origin), Some(last)) = (origin, last) else {
            return Ok(None);
        };
        had_cursor = true;
        if last.number < from
            || last.number > to
            || execution.is_some_and(|b| b.number < from || b.number > last.number)
        {
            return Err(QiError::InvalidPolicy);
        }
        for target in [scope.zone, zone] {
            if !crate::network::on_network(provider, scope, target).await? {
                return Err(QiError::NetworkMismatch);
            }
        }
        for (target, block) in [
            (scope.zone, Some(origin)),
            (zone, Some(last)),
            (zone, execution),
        ] {
            if let Some(block) = block
                && provider
                    .header_at(target, block.number)
                    .await?
                    .is_none_or(|h| h.hash != block.hash)
            {
                return Ok(None);
            }
        }
        // Recheck the origin around the destination queries as in settlement observation.
        if provider
            .header_at(scope.zone, origin.number)
            .await?
            .is_none_or(|h| h.hash != origin.hash)
        {
            return Ok(None);
        }
        Ok(Some(SettlementCursor {
            zone,
            last,
            execution,
            id,
            candidate,
            kind,
            revision: cache.revision,
            scope,
        }))
    }
    .await;
    if result.is_err() || (had_cursor && matches!(&result, Ok(None))) {
        let _ = store.compare_exchange_observation_scoped(
            id,
            candidate,
            slot,
            (generation, Some(cache.revision)),
            None,
        );
    }
    result
}
