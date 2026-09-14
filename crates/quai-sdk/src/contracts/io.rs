//! Explicit fallback/receive intents and lossless contract event queries.
use super::*;
use std::{collections::BTreeMap, sync::Arc};

/// Aggregate decoded JSON nodes accepted by one lossless log query.
pub const MAX_QUERY_VALUE_NODES: usize = 65_536;
/// Aggregate decoded string/key bytes accepted by one lossless log query.
pub const MAX_QUERY_VALUE_BYTES: usize = 4 * 1024 * 1024;

/// Owned ABI interpretation of a source log, suitable for caller-owned event queues.
#[derive(Clone, Debug)]
pub enum ContractLog {
    /// No matching nonanonymous ABI event, or a foreign emitter.
    Unrecognized(Log),
    /// Canonical arguments under the caller-supplied ABI.
    Decoded {
        /// Original association/removal metadata and exact log bytes.
        log: Log,
        /// Owned declaration for name/signature/indexed parameter inspection.
        event: Arc<quai_abi::AbiEvent>,
        /// Positional values; indexed compounds remain hashes.
        values: Vec<AbiEventValue>,
    },
    /// Known/ambiguous topic whose bytes cannot be decoded canonically.
    Undecoded {
        /// Original source data, retained on ABI failure.
        log: Log,
        /// Structured ABI failure without arbitrary node error text.
        error: AbiError,
    },
}
impl ContractLog {
    /// Original log, regardless of decoding outcome.
    pub fn log(&self) -> &Log {
        match self {
            Self::Unrecognized(log) | Self::Decoded { log, .. } | Self::Undecoded { log, .. } => {
                log
            }
        }
    }
}
/// Immutable raw call prepared under declared fallback/receive mutability.
/// Calldata may still select an actual function in deployed code: supplied ABI
/// metadata does not prove which runtime path executes.
#[derive(Clone, Debug)]
pub struct FallbackCall {
    to: QuaiAddress,
    value: U256,
    data: RpcData,
    access_list: Vec<quai_consensus::AccessTuple>,
}
impl FallbackCall {
    /// Exact bound recipient.
    pub fn destination(&self) -> QuaiAddress {
        self.to
    }
    /// Exact native value.
    pub fn value(&self) -> U256 {
        self.value
    }
    /// Raw calldata, empty for a receive path.
    pub fn data(&self) -> &RpcData {
        &self.data
    }
    /// Ordered access declaration; never implicitly sorted/deduplicated.
    pub fn access_list(&self) -> &[quai_consensus::AccessTuple] {
        &self.access_list
    }
    /// Attach an explicit bounded declaration before review and signing. This
    /// does not discover or insert accounts implicitly; node rules still apply.
    pub fn with_access_list(
        mut self,
        access_list: Vec<quai_consensus::AccessTuple>,
    ) -> Result<Self, ContractError> {
        let shape = quai_consensus::QuaiTransaction {
            chain_id: U256::from(1),
            nonce: 0,
            to: Some(self.to.address()),
            value: self.value,
            gas_limit: 0,
            gas_price: U256::ZERO,
            data: self.data.bytes().to_vec(),
            access_list,
        };
        shape
            .unsigned_bytes()
            .map_err(|_| ContractError::InvalidResult)?;
        self.access_list = shape.access_list;
        Ok(self)
    }
    /// Carry the reviewed bytes/value/access declaration into native or browser
    /// durable account preparation. Signing and submission remain explicit.
    #[cfg(feature = "wallet")]
    pub fn into_account_intent(self) -> crate::account_preflight::AccountIntent {
        crate::account_preflight::AccountIntent {
            to: self.to,
            value: self.value,
            data: self.data,
            access_list: self.access_list,
        }
    }
}
impl<'a, T: Transport> Contract<'a, T> {
    /// Borrow the explicitly bound provider; no new connection or task is started.
    pub fn provider(&self) -> &'a Provider<T> {
        self.provider
    }
    /// Bind the same immutable ABI/provider to another validated address without I/O.
    pub fn attach(&self, address: QuaiAddress) -> Self {
        Self::new(address, self.interface.clone(), self.provider)
    }
    /// Bind the same address/ABI to an explicitly supplied provider without I/O.
    pub fn connect<'b, U: Transport>(&self, provider: &'b Provider<U>) -> Contract<'b, U> {
        Contract::new(self.address, self.interface.clone(), provider)
    }
    /// Interpret one log, preserving unknown/ambiguous/malformed results. Foreign
    /// emitters remain unrecognized. Anonymous events still require decode_event
    /// with an explicitly selected declaration.
    pub fn decode_log(&self, log: Log) -> ContractLog {
        self.decode_log_cached(log, &mut BTreeMap::new())
    }
    fn decode_log_cached(
        &self,
        log: Log,
        cache: &mut BTreeMap<String, Arc<quai_abi::AbiEvent>>,
    ) -> ContractLog {
        if log.address != self.address.address() {
            return ContractLog::Unrecognized(log);
        }
        match self.interface.parse_log(&log.topics, log.data.bytes()) {
            Ok(parsed) => ContractLog::Decoded {
                event: cache
                    .entry(parsed.event.signature().to_owned())
                    .or_insert_with(|| Arc::new(parsed.event.clone()))
                    .clone(),
                values: parsed.values,
                log,
            },
            Err(AbiError::NotFound) => ContractLog::Unrecognized(log),
            Err(error) => ContractLog::Undecoded { log, error },
        }
    }
    /// Query this exact emitter with raw positional topics (empty means all
    /// events), retaining unknown/malformed logs in source order. max_logs is
    /// 1..65536; the provider additionally limits ranges to 10000 blocks, four
    /// topics and 128 OR alternatives. Decoded values additionally share a 65536
    /// node / four-MiB string budget, and event declarations share Arc ownership.
    /// Transport response limits remain separate.
    pub async fn query_logs(
        &self,
        range: LogRange,
        topics: &[TopicMatch],
        max_logs: usize,
    ) -> Result<Vec<ContractLog>, ContractError> {
        if !(1..=65536).contains(&max_logs) {
            return Err(AbiError::Limit.into());
        }
        if topics.len() > 4
            || topics
                .iter()
                .any(|t| matches!(t,TopicMatch::AnyOf(v) if v.is_empty() || v.len()>128))
        {
            return Err(ContractError::InvalidEventFilter);
        }
        let logs = self
            .provider
            .logs(&LogFilter {
                zone: self.address.zone(),
                range,
                addresses: vec![self.address.address()],
                topics: topics.to_vec(),
            })
            .await?;
        if logs.len() > max_logs {
            return Err(AbiError::Limit.into());
        }
        let mut cache = BTreeMap::new();
        let mut nodes = MAX_QUERY_VALUE_NODES;
        let mut bytes = MAX_QUERY_VALUE_BYTES;
        let mut result = Vec::with_capacity(logs.len());
        for log in logs {
            let decoded = self.decode_log_cached(log, &mut cache);
            if let ContractLog::Decoded { values, .. } = &decoded {
                for value in values {
                    match value {
                        AbiEventValue::IndexedHash(_) => {
                            nodes = nodes.checked_sub(1).ok_or(AbiError::Limit)?
                        }
                        AbiEventValue::Value(value) => {
                            count_value(value, &mut nodes, &mut bytes, 0)?
                        }
                    }
                }
            }
            result.push(decoded);
        }
        Ok(result)
    }
    /// Prepare exact raw bytes/value under declared fallback/receive rules.
    /// Empty data selects receive when declared; otherwise fallback is required.
    /// Nonempty data requires fallback. Value requires the selected payable path.
    /// No ABI metadata authenticates the runtime, and no node/wallet I/O occurs.
    pub fn prepare_fallback(
        &self,
        data: RpcData,
        value: U256,
    ) -> Result<FallbackCall, ContractError> {
        let payable = if data.bytes().is_empty() && self.interface.has_receive() {
            true
        } else {
            self.interface
                .fallback_mutability()
                .ok_or(ContractError::MissingFallback)?
                == StateMutability::Payable
        };
        if value != U256::ZERO && !payable {
            return Err(ContractError::Nonpayable);
        }
        Ok(FallbackCall {
            to: self.address,
            value,
            data,
            access_list: vec![],
        })
    }
    fn fallback_request(
        &self,
        from: QuaiAddress,
        call: &FallbackCall,
        gas: Option<u64>,
    ) -> Result<CallRequest, ContractError> {
        if call.to != self.address {
            return Err(ContractError::CallMismatch);
        }
        if from.zone() != self.address.zone() {
            return Err(ContractError::ZoneMismatch);
        }
        self.prepare_fallback(call.data.clone(), call.value)?;
        let mut request = CallRequest::new(from, self.address);
        request.value = Some(call.value);
        request.input = call.data.clone();
        request.gas = gas;
        request.access_list = call
            .access_list
            .iter()
            .map(|a| quai_provider::AccessListItem {
                address: a.address,
                storage_keys: a.storage_keys.clone(),
            })
            .collect();
        Ok(request)
    }
    /// Read-only simulation returning exact raw return bytes without guessing a
    /// function result schema. Success does not authorize or guarantee execution.
    pub async fn simulate_fallback(
        &self,
        from: QuaiAddress,
        call: &FallbackCall,
        block: BlockTag,
        gas: Option<u64>,
    ) -> Result<RpcData, ContractError> {
        Ok(self
            .provider
            .call(&self.fallback_request(from, call, gas)?, block)
            .await?)
    }
    /// Explicit gas estimate for the same bound raw intent and block selector.
    /// Fee population, nonce reservation, signing and broadcast remain separate.
    pub async fn estimate_fallback(
        &self,
        from: QuaiAddress,
        call: &FallbackCall,
        block: BlockTag,
    ) -> Result<u64, ContractError> {
        Ok(self
            .provider
            .estimate_gas(&self.fallback_request(from, call, None)?, block)
            .await?)
    }
}

fn count_value(
    value: &Value,
    nodes: &mut usize,
    bytes: &mut usize,
    depth: usize,
) -> Result<(), AbiError> {
    if depth > 64 {
        return Err(AbiError::Limit);
    }
    *nodes = nodes.checked_sub(1).ok_or(AbiError::Limit)?;
    match value {
        Value::String(s) => *bytes = bytes.checked_sub(s.len()).ok_or(AbiError::Limit)?,
        Value::Array(a) => {
            for value in a {
                count_value(value, nodes, bytes, depth + 1)?;
            }
        }
        Value::Object(o) => {
            for (key, value) in o {
                *bytes = bytes.checked_sub(key.len()).ok_or(AbiError::Limit)?;
                count_value(value, nodes, bytes, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}
