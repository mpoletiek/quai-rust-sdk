use crate::{Log, Provider, ProviderError, types};
use quai_primitives::{Hash32, QuaiAddress, Zone};
use quai_rpc::Transport;
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// Explicit bounded log location; no implicit genesis-to-latest query.
#[derive(Clone, Copy, Debug)]
pub enum LogRange {
    /// One block hash, mutually exclusive with numeric range selectors.
    BlockHash(Hash32),
    /// Inclusive canonical number range, at most 10,000 blocks per request.
    Inclusive {
        /// First block number.
        from: u64,
        /// Last block number.
        to: u64,
    },
}
/// Match one event topic position.
#[derive(Clone, Debug)]
pub enum TopicMatch {
    /// Any topic at this position; the log must still contain the position.
    Any,
    /// One exact topic.
    Exact(Hash32),
    /// Nonempty OR list, at most 128 alternatives.
    AnyOf(Vec<Hash32>),
}
/// Zone-explicit log query with bounded addresses, topics and numeric span.
#[derive(Clone, Debug)]
pub struct LogFilter {
    /// Endpoint zone; no address/hash inference chooses a route implicitly.
    pub zone: Zone,
    /// Explicit block selection.
    pub range: LogRange,
    /// Emitting contracts, up to 128, all in this zone. Empty means any emitter.
    pub addresses: Vec<QuaiAddress>,
    /// Up to four indexed topic filters, in positional order.
    pub topics: Vec<TopicMatch>,
}
impl LogFilter {
    fn rpc_value(&self) -> Result<Value, ProviderError> {
        let invalid = ProviderError::InvalidRequest("invalid bounded log filter");
        if self.addresses.len() > 128
            || self.topics.len() > 4
            || self.addresses.iter().any(|a| a.zone() != self.zone)
        {
            return Err(invalid);
        }
        let mut value = json!({});
        match self.range {
            LogRange::BlockHash(hash) => value["blockHash"] = json!(hash.to_string()),
            LogRange::Inclusive { from, to } => {
                if from > to || to > i64::MAX as u64 || to - from >= 10_000 {
                    return Err(invalid);
                }
                value["fromBlock"] = json!(format!("{from:#x}"));
                value["toBlock"] = json!(format!("{to:#x}"));
            }
        }
        if !self.addresses.is_empty() {
            value["address"] = json!(
                self.addresses
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            );
        }
        let mut topics = Vec::with_capacity(self.topics.len());
        for topic in &self.topics {
            topics.push(match topic {
                TopicMatch::Any => Value::Null,
                TopicMatch::Exact(hash) => json!(hash.to_string()),
                TopicMatch::AnyOf(hashes) => {
                    if hashes.is_empty() || hashes.len() > 128 {
                        return Err(invalid);
                    }
                    json!(hashes.iter().map(ToString::to_string).collect::<Vec<_>>())
                }
            });
        }
        value["topics"] = json!(topics);
        Ok(value)
    }
    fn matches(&self, log: &Log) -> bool {
        if log.address.zone() != self.zone
            || (!self.addresses.is_empty() && !self.addresses.contains(&log.address))
            || self.topics.len() > log.topics.len()
        {
            return false;
        }
        if !match self.range {
            LogRange::BlockHash(hash) => log.inclusion.block_hash == hash,
            LogRange::Inclusive { from, to } => (from..=to).contains(&log.inclusion.block_number),
        } {
            return false;
        }
        self.topics
            .iter()
            .zip(&log.topics)
            .all(|(filter, topic)| match filter {
                TopicMatch::Any => true,
                TopicMatch::Exact(hash) => hash == topic,
                TopicMatch::AnyOf(hashes) => hashes.contains(topic),
            })
    }
}
impl<T: Transport> Provider<T> {
    /// Query logs and validate every returned emitter/topic/range association.
    /// Removed flags are preserved. Results and block hashes are node claims;
    /// applications must reconcile canonical history/reorgs before finality claims.
    pub async fn logs(&self, filter: &LogFilter) -> Result<Vec<Log>, ProviderError> {
        let request = filter.rpc_value()?;
        let Value::Array(values) = self
            .read(filter.zone.into(), "quai_getLogs", json!([request]))
            .await?
        else {
            return Err(ProviderError::InvalidResult("expected log array"));
        };
        if values.len() > 65_536 {
            return Err(ProviderError::InvalidResult("too many logs"));
        }
        let mut seen = BTreeSet::new();
        let mut result = Vec::with_capacity(values.len());
        for value in values {
            let log = types::parse_log(value)?;
            if !filter.matches(&log) || !seen.insert((log.inclusion.block_hash, log.log_index)) {
                return Err(ProviderError::InvalidResult(
                    "log does not match query or repeats block position",
                ));
            }
            result.push(log);
        }
        Ok(result)
    }
}
