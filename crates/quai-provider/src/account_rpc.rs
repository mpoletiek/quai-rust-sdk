//! Bounded account simulation, topology and transaction-pool reads.
use crate::{
    AccessListItem, BlockTag, CallRequest, Provider, ProviderError, RpcData, Transaction,
    TransactionDetails, types,
};
use quai_primitives::{QuaiAddress, Region, Shard, Zone};
use quai_rpc::Transport;
use serde_json::{Value, json};
use std::collections::BTreeSet;
/// Node-generated access declaration and simulated gas for one exact call.
#[derive(Clone, Debug)]
pub struct AccessListEstimate {
    /// Ordered access declaration, not silently merged into a signed transaction.
    pub access_list: Vec<AccessListItem>,
    /// Source-reported simulation gas. Reprepare before signing changed access lists.
    pub gas_used: u64,
}
/// Current pool counts, including Qi even though account pool content excludes Qi.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PoolStatus {
    /// Executable account transactions.
    pub pending: u64,
    /// Account transactions queued behind nonce gaps.
    pub queued: u64,
    /// Qi transactions in the node pool.
    pub qi: u64,
}
/// One validated account pool transaction. Absence cannot authorize nonce release.
#[derive(Clone, Debug)]
pub struct PoolTransaction {
    /// True if queued behind a gap; false if in the node's pending account set.
    pub queued: bool,
    /// Account/nonce-associated transaction without a mined inclusion.
    pub transaction: Transaction,
}
/// One source-formatted pool entry; its text is not parsed as authoritative values.
#[derive(Clone)]
pub struct PoolInspection {
    /// Account map key, checked against the requested zone.
    pub sender: QuaiAddress,
    /// Canonical decimal nonce map key.
    pub nonce: u64,
    /// True for queued entries.
    pub queued: bool,
    /// Source summary, at most 4096 bytes. Use typed pool content for amounts/hashes.
    pub summary: String,
}
impl std::fmt::Debug for PoolInspection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolInspection")
            .field("sender", &self.sender)
            .field("nonce", &self.nonce)
            .field("queued", &self.queued)
            .field("summary_bytes", &self.summary.len())
            .finish()
    }
}
fn invalid(message: &'static str) -> ProviderError {
    ProviderError::InvalidResult(message)
}
fn nonce(text: &str) -> Result<u64, ProviderError> {
    let value = text
        .parse::<u64>()
        .map_err(|_| invalid("invalid pool nonce"))?;
    if text != value.to_string() {
        return Err(invalid("noncanonical pool nonce"));
    }
    Ok(value)
}
fn pool_entries(
    value: Value,
    zone: Zone,
    max_entries: usize,
) -> Result<Vec<(bool, QuaiAddress, u64, Value)>, ProviderError> {
    let Value::Object(mut root) = value else {
        return Err(invalid("expected pool object"));
    };
    if root.len() != 2 {
        return Err(invalid("unknown pool section"));
    }
    let mut result = Vec::new();
    let mut keys = BTreeSet::new();
    for (section, queued) in [("pending", false), ("queued", true)] {
        let Value::Object(accounts) = root
            .remove(section)
            .ok_or(invalid("missing pool section"))?
        else {
            return Err(invalid("expected pool accounts"));
        };
        if accounts.len() > max_entries {
            return Err(invalid("pool account budget exceeded"));
        }
        for (account, transactions) in accounts {
            let sender: QuaiAddress = account
                .parse()
                .map_err(|_| invalid("invalid pool account"))?;
            if sender.zone() != zone {
                return Err(invalid("pool account zone mismatch"));
            }
            let Value::Object(transactions) = transactions else {
                return Err(invalid("expected nonce map"));
            };
            if result.len().saturating_add(transactions.len()) > max_entries {
                return Err(invalid("pool transaction budget exceeded"));
            }
            for (index, transaction) in transactions {
                let nonce = nonce(&index)?;
                if !keys.insert((sender.address(), nonce)) {
                    return Err(invalid("conflicting pool nonce groups"));
                }
                result.push((queued, sender, nonce, transaction));
            }
        }
    }
    Ok(result)
}
fn bound(max_entries: usize) -> Result<(), ProviderError> {
    if !(1..=4096).contains(&max_entries) {
        return Err(ProviderError::InvalidRequest("invalid pool entry budget"));
    }
    Ok(())
}
impl<T: Transport> Provider<T> {
    /// Generate an ordered access list at an explicit block selector. VM errors
    /// fail instead of returning a seemingly usable partial access declaration.
    pub async fn create_access_list(
        &self,
        request: &CallRequest,
        block: BlockTag,
    ) -> Result<AccessListEstimate, ProviderError> {
        let args = request.rpc_value()?;
        let block = block.rpc_value()?;
        let Value::Object(mut object) = self
            .read(
                request.from.zone().into(),
                "quai_createAccessList",
                json!([args, block]),
            )
            .await?
        else {
            return Err(invalid("expected access-list estimate"));
        };
        if let Some(error) = object.remove("error") {
            let error = error
                .as_str()
                .ok_or(invalid("invalid access-list execution error"))?;
            if !error.is_empty() {
                return Err(invalid("access-list execution failed"));
            }
        }
        let access_list = types::access_list(
            object
                .remove("accessList")
                .ok_or(invalid("missing access list"))?,
        )?;
        let gas_used = types::uint64(
            object
                .remove("gasUsed")
                .ok_or(invalid("missing access-list gas"))?,
        )?;
        if !object.is_empty() {
            return Err(invalid("unsupported access-list estimate extension"));
        }
        Ok(AccessListEstimate {
            access_list,
            gas_used,
        })
    }
    /// Reported protocol expansion at an explicitly routed shard. Network activity
    /// should be read from running_zones; a number alone does not add active routes.
    pub async fn protocol_expansion(&self, shard: Shard) -> Result<u64, ProviderError> {
        types::uint64(
            self.read(shard, "quai_getProtocolExpansionNumber", json!([]))
                .await?,
        )
    }
    /// Regions actually advertised by the Prime running-chain response. This
    /// avoids the pinned JS expansion-threshold inconsistency for active regions.
    pub async fn running_regions(&self) -> Result<Vec<Region>, ProviderError> {
        let zones = self.running_zones().await?;
        Ok(Region::ALL
            .into_iter()
            .filter(|region| zones.iter().any(|zone| zone.region() == *region))
            .collect())
    }
    /// Bounded pending-header protobuf returned by the pinned node, preserved
    /// as wire bytes. It is not an ordinary execution-header JSON response.
    pub async fn pending_header_bytes(&self, zone: Zone) -> Result<RpcData, ProviderError> {
        types::data(
            self.read(zone.into(), "quai_getPendingHeader", json!([]))
                .await?,
        )
    }
    /// Current node-local pool counters. They are not network-wide propagation evidence.
    pub async fn pool_status(&self, zone: Zone) -> Result<PoolStatus, ProviderError> {
        let Value::Object(mut object) = self.read(zone.into(), "txpool_status", json!([])).await?
        else {
            return Err(invalid("expected pool status"));
        };
        let mut count =
            |key| types::uint64(object.remove(key).ok_or(invalid("missing pool count"))?);
        let result = PoolStatus {
            pending: count("pending")?,
            queued: count("queued")?,
            qi: count("qi")?,
        };
        if !object.is_empty() {
            return Err(invalid("unknown pool status field"));
        }
        Ok(result)
    }
    /// Bounded account pool contents, checked for chain, sender, nonce, zone,
    /// duplicate hash and pending inclusion. Pinned txpool_content excludes Qi.
    pub async fn pool_content(
        &self,
        zone: Zone,
        max_entries: usize,
    ) -> Result<Vec<PoolTransaction>, ProviderError> {
        bound(max_entries)?;
        let rows = pool_entries(
            self.read(zone.into(), "txpool_content", json!([])).await?,
            zone,
            max_entries,
        )?;
        let mut hashes = BTreeSet::new();
        let mut result = Vec::with_capacity(rows.len());
        for (queued, sender, nonce, value) in rows {
            let transaction = Transaction::try_from(value)?;
            let TransactionDetails::Quai(account) = &transaction.details else {
                return Err(invalid("expected account pool transaction"));
            };
            if account.from != sender
                || account.nonce != nonce
                || account.chain_id != self.expected_chain_id
                || transaction.hash == quai_primitives::Hash32::ZERO
                || transaction.inclusion.is_some()
                || !hashes.insert(transaction.hash)
            {
                return Err(invalid("pool transaction association mismatch"));
            }
            result.push(PoolTransaction {
                queued,
                transaction,
            });
        }
        Ok(result)
    }
    /// Bounded account pool inspection strings. They are diagnostic text, not
    /// decoded transactions, and do not prove a signed operation was dropped.
    pub async fn pool_inspect(
        &self,
        zone: Zone,
        max_entries: usize,
    ) -> Result<Vec<PoolInspection>, ProviderError> {
        bound(max_entries)?;
        let rows = pool_entries(
            self.read(zone.into(), "txpool_inspect", json!([])).await?,
            zone,
            max_entries,
        )?;
        rows.into_iter()
            .map(|(queued, sender, nonce, value)| {
                let summary = value
                    .as_str()
                    .filter(|s| s.len() <= 4096)
                    .ok_or(invalid("invalid pool summary"))?
                    .to_owned();
                Ok(PoolInspection {
                    queued,
                    sender,
                    nonce,
                    summary,
                })
            })
            .collect()
    }
}
