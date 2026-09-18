//! Bounded canonical head replay. Notifications are hints; numbered reads establish updates.
use crate::{BlockReference, BlockTag, Provider, ProviderError, ZoneHeader, types};
use quai_primitives::{Hash32, Zone};
use quai_rpc::{Transport, U256};
use serde_json::Value;
use std::collections::{BTreeSet, VecDeque};
mod state;
pub use state::MAX_HEAD_STATE_BYTES;

/// A bounded update in application order: undo removed blocks, then apply added blocks.
#[derive(Clone, Debug)]
#[non_exhaustive]
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
    /// Explicit zone whose linked history this cursor tracks.
    pub fn zone(&self) -> Zone {
        self.zone
    }
    /// Trusted genesis identity supplied when constructing this cursor.
    pub fn genesis(&self) -> Hash32 {
        self.genesis
    }
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
            || (checkpoint.number == 0 && checkpoint.hash != genesis)
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
        // Genesis, the tip and the newest anchor are independent reads, so
        // they travel as one guarded batch. The newest anchor matching is the
        // common case; older anchors are only read on a reorg. Reads are
        // address-free, so batching discloses nothing new.
        let newest = self.checkpoint();
        let first = provider
            .header_values(
                self.zone,
                &[
                    BlockTag::Number(U256::ZERO),
                    BlockTag::Latest,
                    BlockTag::Number(U256::from(newest.number)),
                ],
            )
            .await?;
        let [genesis, tip, newest_header]: [Value; 3] = first
            .try_into()
            .map_err(|_| ProviderError::InvalidResult("header batch count"))?;
        if types::genesis_hash(genesis)? != self.genesis {
            return Err(ProviderError::InvalidResult("head replay genesis mismatch"));
        }
        let tip = Provider::<T>::parse_zone_header(tip, self.zone, BlockTag::Latest)?
            .ok_or(ProviderError::ReplayHistoryUnavailable)?;
        let mut common = None;
        for (index, anchor) in self.anchors.iter().enumerate().rev() {
            if anchor.number > tip.number {
                continue;
            }
            let actual_hash = if anchor.number == 0 {
                self.genesis
            } else if *anchor == newest {
                Provider::<T>::parse_zone_header(
                    newest_header.clone(),
                    self.zone,
                    BlockTag::Number(U256::from(anchor.number)),
                )?
                .ok_or(ProviderError::ReplayHistoryUnavailable)?
                .hash
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
        let mut seen: BTreeSet<_> = self
            .anchors
            .iter()
            .take(common + 1)
            .map(|a| a.hash)
            .collect();
        let numbers: Vec<_> = (base.number + 1..=end)
            .map(|number| BlockTag::Number(U256::from(number)))
            .collect();
        let values = if numbers.is_empty() {
            Vec::new()
        } else {
            provider.header_values(self.zone, &numbers).await?
        };
        if values.len() != numbers.len() {
            return Err(ProviderError::InvalidResult("header batch count"));
        }
        // Validate in order, so the first broken link is the error reported.
        for (value, block) in values.into_iter().zip(numbers) {
            let header = Provider::<T>::parse_zone_header(value, self.zone, block)?
                .ok_or(ProviderError::ReplayHistoryUnavailable)?;
            if header.parent_hash != previous.hash
                || header.hash == Hash32::ZERO
                || !seen.insert(header.hash)
            {
                return Err(ProviderError::ObservationChanged);
            }
            previous = BlockReference {
                number: header.number,
                hash: header.hash,
            };
            added.push(header);
        }
        // Check both ends after the page, in a later read than the page itself.
        // A forward extension is harmless; a replaced anchor/page is not
        // committed. With nothing added, the base was read in the same batch as
        // the tip and nothing new would be committed, so the check is skipped.
        if !added.is_empty() {
            let ends = [base, previous];
            let values = provider
                .header_values(
                    self.zone,
                    &ends.map(|anchor| BlockTag::Number(U256::from(anchor.number))),
                )
                .await?;
            if values.len() != ends.len() {
                return Err(ProviderError::InvalidResult("header batch count"));
            }
            for (anchor, value) in ends.into_iter().zip(values) {
                let hash = if anchor.number == 0 {
                    Some(types::genesis_hash(value)?)
                } else {
                    Provider::<T>::parse_zone_header(
                        value,
                        self.zone,
                        BlockTag::Number(U256::from(anchor.number)),
                    )?
                    .map(|h| h.hash)
                };
                if hash != Some(anchor.hash) {
                    return Err(ProviderError::ObservationChanged);
                }
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
