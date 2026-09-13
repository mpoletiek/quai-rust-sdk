//! Bounded canonical head replay. Notifications are hints; numbered reads establish updates.
use crate::{BlockReference, Provider, ProviderError, ZoneHeader};
use quai_primitives::{Hash32, Zone};
use quai_rpc::Transport;
use std::collections::VecDeque;

/// A bounded update in application order: undo removed blocks, then apply added blocks.
#[derive(Clone, Debug)]
pub struct HeadUpdate {
    /// Previously observed blocks removed, newest first.
    pub removed: Vec<BlockReference>,
    /// Canonical linked headers to apply, oldest first.
    pub added: Vec<ZoneHeader>,
    /// Last applied checkpoint, including when there were no new blocks.
    pub checkpoint: BlockReference,
    /// Whether this page reached the sampled latest height.
    pub caught_up: bool,
}
/// Memory-bounded replay cursor; clone before polling when application persistence
/// must commit before advancing its in-memory cursor. No transaction is resubmitted.
#[derive(Clone, Debug)]
pub struct HeadTracker {
    zone: Zone,
    genesis: Hash32,
    anchors: VecDeque<BlockReference>,
    retain: usize,
    page_size: usize,
}
impl HeadTracker {
    /// Start from a caller-trusted checkpoint. A reorg older than retained anchors
    /// returns `ReplayHistoryUnavailable`; explicitly restore an older checkpoint.
    /// Retain 2..=4096 anchors and replay 1..=256 blocks per page.
    pub fn new(
        zone: Zone,
        genesis: Hash32,
        checkpoint: BlockReference,
        retain: usize,
        page_size: usize,
    ) -> Result<Self, ProviderError> {
        if genesis == Hash32::ZERO
            || checkpoint.hash == Hash32::ZERO
            || !(2..=4096).contains(&retain)
            || !(1..=256).contains(&page_size)
        {
            return Err(ProviderError::InvalidRequest(
                "head replay bounds or identity",
            ));
        }
        Ok(Self {
            zone,
            genesis,
            anchors: VecDeque::from([checkpoint]),
            retain,
            page_size,
        })
    }
    /// Latest applied source checkpoint. It is not a consensus proof or finality claim.
    pub fn checkpoint(&self) -> BlockReference {
        *self.anchors.back().expect("nonempty anchors")
    }
    /// Reconcile a bounded page, detecting missed notifications, duplicates and
    /// reorgs within retained history. Any error leaves the tracker unchanged.
    pub async fn poll<T: Transport>(
        &mut self,
        provider: &Provider<T>,
    ) -> Result<HeadUpdate, ProviderError> {
        if provider.genesis_hash(self.zone).await? != self.genesis {
            return Err(ProviderError::InvalidResult("head replay genesis mismatch"));
        }
        let tip = provider
            .latest_header(self.zone)
            .await?
            .ok_or(ProviderError::ReplayHistoryUnavailable)?;
        let mut common = None;
        for (index, anchor) in self.anchors.iter().enumerate().rev() {
            if anchor.number > tip.number {
                continue;
            }
            let actual_hash = if anchor.number == 0 {
                self.genesis
            } else {
                provider
                    .header_at(self.zone, anchor.number)
                    .await?
                    .ok_or(ProviderError::ReplayHistoryUnavailable)?
                    .hash
            };
            if actual_hash == anchor.hash {
                common = Some(index);
                break;
            }
        }
        let common = common.ok_or(ProviderError::ReplayHistoryUnavailable)?;
        let base = self.anchors[common];
        let end = base
            .number
            .saturating_add(self.page_size as u64)
            .min(tip.number);
        let mut added = Vec::with_capacity((end - base.number) as usize);
        let mut previous = base;
        for number in base.number.saturating_add(1)..=end {
            let header = provider
                .header_at(self.zone, number)
                .await?
                .ok_or(ProviderError::ReplayHistoryUnavailable)?;
            if header.parent_hash != previous.hash {
                return Err(ProviderError::ObservationChanged);
            }
            previous = BlockReference {
                number,
                hash: header.hash,
            };
            added.push(header);
        }
        // Check both ends after the page. A forward extension is harmless; a
        // replaced anchor/page is not committed. These remain trusted-node reads.
        for anchor in [base, previous] {
            if anchor.number == 0 {
                if provider.genesis_hash(self.zone).await? != anchor.hash {
                    return Err(ProviderError::ObservationChanged);
                }
                continue;
            }
            if provider
                .header_at(self.zone, anchor.number)
                .await?
                .is_none_or(|h| h.hash != anchor.hash)
            {
                return Err(ProviderError::ObservationChanged);
            }
        }
        let removed = self
            .anchors
            .iter()
            .skip(common + 1)
            .rev()
            .copied()
            .collect();
        self.anchors.truncate(common + 1);
        self.anchors.extend(added.iter().map(|h| BlockReference {
            number: h.number,
            hash: h.hash,
        }));
        while self.anchors.len() > self.retain {
            self.anchors.pop_front();
        }
        Ok(HeadUpdate {
            removed,
            added,
            checkpoint: previous,
            caught_up: end == tip.number,
        })
    }
}
