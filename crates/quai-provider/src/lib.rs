//! Typed provider with explicit routing, validated reads and signed transaction submission.
use quai_primitives::{Hash32, QiAddress, QuaiAddress, Shard, Zone};
use quai_rpc::{
    Endpoint, QuantityError, RouteError, Routing, RpcError, Transport, U256, parse_quantity,
};
use serde_json::{Value, json};
use thiserror::Error;

mod network;
pub use network::{FeeData, Network, NetworkError, NetworkMatch, NetworkRegistry};

mod confirmation;
mod transaction_confirmation;
pub use confirmation::{ConfirmedReceipt, ReceiptConfirmation};
pub use transaction_confirmation::{ConfirmedTransaction, TransactionConfirmation};
mod account_replacement;
pub use account_replacement::{
    AccountNonceCandidate, AccountReplacementPoll, AccountReplacementScan,
    AccountReplacementScanRequest, AccountReplacementTracker, ReplacementReason,
};
mod deployment;
#[cfg(all(feature = "polling", not(target_arch = "wasm32")))]
pub use deployment::DeploymentWaitError;
pub use deployment::{DeploymentCode, DeploymentObservation, DeploymentReference};
mod code_wait;
mod contract_code;
pub use code_wait::{CodeWaitConfig, CodeWaitError, ContractCodeTarget};
pub use contract_code::ContractCodeObservation;
mod blocks;
pub use blocks::{
    BlockHashes, BlockMetadata, BlockTransactionId, MinedBlock, OutboundBlockTransaction,
};
mod qi_credit;
pub use qi_credit::QiCreditObservation;
mod external_tracking;
pub use external_tracking::{ExternalObservation, ExternalReference};
mod account_rpc;
/// Runtime-independent bounded local event registration and delivery.
pub mod event_hub;
pub use account_rpc::{AccessListEstimate, PoolInspection, PoolStatus, PoolTransaction};
mod conversion_tracking;
pub use conversion_tracking::{
    BlockReference, ConversionEffect, ConversionObservation, ConversionOriginObservation,
    ConversionReference, ConversionSpendability, EtxCorrelation, EtxExecutionObservation,
    EtxScanRequest, EtxScanResult, LockedBalanceObservation, ScanCoverage, TransactionBlock,
};
mod head_tracker;
mod logs;
mod qi;
pub use head_tracker::{HeadTracker, HeadUpdate, MAX_HEAD_STATE_BYTES};
mod qi_special_fee;
pub use qi_special_fee::{QiFeeProfile, QiFeeQuote, qi_special_gas};
mod wallet_rpc;
pub use logs::{LogFilter, LogRange, TopicMatch};
pub use wallet_rpc::{
    AccountState, ConversionEstimate, MAX_ACCOUNT_STATES, MAX_OUTPOINT_ADDRESSES, OutpointDeltas,
};
mod response_json;
mod submission;
mod types;
#[cfg(all(feature = "polling", not(target_arch = "wasm32")))]
mod wait;
pub use submission::{BroadcastError, BroadcastResult};
pub use types::{
    AccessListItem, AddressOutpoint, CallRequest, EcdsaValues, Extensions, ExternalTransaction,
    Inclusion, Log, MAX_RPC_DATA_BYTES, OutPoint, QiInput, QiOutput, QiTransaction,
    QuaiTransaction, Receipt, ReceiptOutcome, RpcData, Transaction, TransactionDetails,
    TransactionKind, ZoneHeader,
};
#[cfg(all(feature = "polling", not(target_arch = "wasm32")))]
pub use wait::{WaitConfig, WaitError};

/// A block selector supported by the initial account read methods.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockTag {
    /// The node's latest block.
    Latest,
    /// The node's pending state.
    Pending,
    /// An explicit block number. Values above `i64::MAX` are rejected before I/O.
    Number(U256),
}
impl BlockTag {
    fn rpc_value(self) -> Result<String, ProviderError> {
        match self {
            Self::Latest => Ok("latest".into()),
            Self::Pending => Ok("pending".into()),
            Self::Number(n) if n <= U256::from(i64::MAX as u64) => Ok(format!("{n:#x}")),
            Self::Number(_) => Err(ProviderError::BlockNumberOutOfRange),
        }
    }
}

/// Provider failures without raw endpoint URLs or remote response bodies.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ProviderError {
    /// Requested canonical replay predates retained anchors or source history.
    #[error("canonical replay history unavailable; restore an older checkpoint explicitly")]
    ReplayHistoryUnavailable,
    /// A typed request violates the supported account simulation contract.
    #[error("invalid RPC request: {0}")]
    InvalidRequest(&'static str),
    /// The pinned RPC estimator omits conversion data and cannot quote this exact envelope.
    #[error("exact Qi conversion fee estimation is not qualified for this node API")]
    ConversionFeeEstimationUnavailable,
    /// go-quai block-number selectors are signed 64-bit values; longer hex may mean a hash.
    #[error("block number exceeds the node's signed 64-bit range")]
    BlockNumberOutOfRange,
    /// No valid route exists for the requested shard.
    #[error(transparent)]
    Route(#[from] RouteError),
    /// Transport or JSON-RPC protocol failure.
    #[error(transparent)]
    Rpc(#[from] RpcError),
    /// An invalid or out-of-range unsigned quantity.
    #[error(transparent)]
    Quantity(#[from] QuantityError),
    /// The endpoint identifies a different chain.
    #[error("chain ID mismatch: expected {expected}, received {actual}")]
    ChainMismatch {
        /// Caller-configured chain identity.
        expected: U256,
        /// Identity reported by the endpoint.
        actual: U256,
    },
    /// The endpoint's genesis is not the one the caller trusts: a different
    /// network with the same chain ID.
    #[error("network genesis mismatch")]
    GenesisMismatch,
    /// A canonical block anchor or parent link changed during a multi-request observation.
    #[error("canonical observation changed during the request")]
    ObservationChanged,
    /// A method result does not match its expected shape.
    #[error("invalid RPC result: {0}")]
    InvalidResult(&'static str),
}

impl ProviderError {
    /// How to react to this failure; see [`quai_primitives::ErrorClass`].
    pub fn class(&self) -> quai_primitives::ErrorClass {
        use quai_primitives::ErrorClass;
        match self {
            Self::Rpc(error) => error.class(),
            Self::ChainMismatch { .. } | Self::GenesisMismatch => ErrorClass::NetworkMismatch,
            Self::ObservationChanged => ErrorClass::Stale,
            Self::ReplayHistoryUnavailable
            | Self::InvalidRequest(_)
            | Self::ConversionFeeEstimationUnavailable
            | Self::BlockNumberOutOfRange
            | Self::Route(_)
            | Self::Quantity(_)
            | Self::InvalidResult(_) => ErrorClass::Invalid,
        }
    }
}

/// A provider with a fixed routing table and an explicitly expected chain ID.
///
/// Every high-level read checks the target endpoint's chain ID first. This is a
/// configuration check, not authentication or an atomic snapshot: a malicious
/// endpoint can lie, and a load balancer can change backends between requests.
/// Call `genesis_hash` and compare it with a trusted network scope when needed;
/// fork validation is not implemented. Submission requires an immutable
/// signed transaction and never retries automatically.
#[derive(Clone, Debug)]
pub struct Provider<T> {
    transport: T,
    routing: Routing,
    expected_chain_id: U256,
}

impl<T: Transport> Provider<T> {
    /// Construct without network access. Reads fail if the endpoint's chain ID differs.
    pub fn new(transport: T, routing: Routing, expected_chain_id: U256) -> Self {
        Self {
            transport,
            routing,
            expected_chain_id,
        }
    }

    /// Read and verify the chain ID at an explicitly configured shard.
    pub async fn chain_id(&self, shard: Shard) -> Result<U256, ProviderError> {
        let result = self
            .transport
            .request(self.routing.endpoint(shard)?, "quai_chainId", json!([]))
            .await?;
        let actual = quantity(result)?;
        if actual != self.expected_chain_id {
            return Err(ProviderError::ChainMismatch {
                expected: self.expected_chain_id,
                actual,
            });
        }
        Ok(actual)
    }

    /// The expected chain ID this provider was constructed with.
    pub fn expected_chain_id(&self) -> U256 {
        self.expected_chain_id
    }

    /// Check an observed chain ID against the expected one.
    fn check_chain_id(&self, observed: Value) -> Result<(), ProviderError> {
        let actual = quantity(observed)?;
        if actual != self.expected_chain_id {
            return Err(ProviderError::ChainMismatch {
                expected: self.expected_chain_id,
                actual,
            });
        }
        Ok(())
    }

    /// Read with the chain check preceding the call.
    ///
    /// Where the transport supports batching, the guard and the payload travel
    /// as `[quai_chainId, method]` in one request instead of two sequential
    /// round trips, so one backend answers both. It remains a configuration
    /// check, not authentication: a malicious endpoint can answer the guard
    /// truthfully and lie in the payload.
    ///
    /// Transports without batching return `None`, having sent nothing, and fall
    /// back to the sequential form.
    async fn read(
        &self,
        shard: Shard,
        method: &str,
        params: Value,
    ) -> Result<Value, ProviderError> {
        let endpoint = self.routing.endpoint(shard)?;
        if let Some(batch) = self
            .guarded_batch(endpoint, vec![(method, params.clone())])
            .await
        {
            return Ok(batch?.pop().expect("checked count")?);
        }
        self.chain_id(shard).await?;
        Ok(self.transport.request(endpoint, method, params).await?)
    }

    /// Send `calls` in one batch led by a chain-ID guard.
    ///
    /// `None` means the transport does not batch and sent nothing. The guard
    /// must pass before any payload is returned, so a chain mismatch is never
    /// reported as a successful read. Results keep the order of `calls`.
    ///
    /// There is no trailing guard. A batch is one request that one backend
    /// answers, so nothing can change chains between its elements; and a
    /// gateway that splits a batch across backends could route the payload
    /// away from a guard at either end, so a second guard would not cover
    /// that case either. It cost a third of every single read's calls, which
    /// counts against a metered or rate-limited gateway.
    async fn guarded_batch(
        &self,
        endpoint: &Endpoint,
        calls: Vec<(&str, Value)>,
    ) -> Option<Result<Vec<Result<Value, RpcError>>, ProviderError>> {
        let count = calls.len();
        let mut requests = Vec::with_capacity(count + 1);
        requests.push(("quai_chainId", json!([])));
        requests.extend(calls);
        let batch = self.transport.request_batch(endpoint, requests).await?;
        Some((|| {
            let mut responses = batch?;
            if responses.len() != count + 1 {
                return Err(ProviderError::InvalidResult("batch response count"));
            }
            let leading = responses.remove(0);
            self.check_chain_id(leading?)?;
            Ok(responses)
        })())
    }

    /// Read the latest block number for this shard, without narrowing to a machine integer.
    pub async fn block_number(&self, shard: Shard) -> Result<U256, ProviderError> {
        quantity(self.read(shard, "quai_blockNumber", json!([])).await?)
    }

    /// Read the network's root genesis identity through an explicitly routed zone.
    ///
    /// The pinned node returns a root-location header even on zone endpoints at
    /// height zero. Validate its zero height, root location and zero parent, then
    /// return its exact work-object hash. This validates the response shape; the
    /// caller must compare the returned identity with its trusted network scope.
    pub async fn genesis_hash(&self, zone: Zone) -> Result<Hash32, ProviderError> {
        let value = self
            .read(zone.into(), "quai_getHeaderByNumber", json!(["0x0"]))
            .await?;
        types::genesis_hash(value)
    }

    /// Read a latest zone header with explicit location validation. None means unavailable.
    pub async fn latest_header(&self, zone: Zone) -> Result<Option<ZoneHeader>, ProviderError> {
        self.zone_header(zone, BlockTag::Latest).await
    }

    /// Read a canonical-by-number zone header and validate the returned location and height.
    pub async fn header_at(
        &self,
        zone: Zone,
        number: u64,
    ) -> Result<Option<ZoneHeader>, ProviderError> {
        self.zone_header(zone, BlockTag::Number(U256::from(number)))
            .await
    }

    async fn zone_header(
        &self,
        zone: Zone,
        block: BlockTag,
    ) -> Result<Option<ZoneHeader>, ProviderError> {
        let selector = block.rpc_value()?;
        let value = self
            .read(zone.into(), "quai_getHeaderByNumber", json!([selector]))
            .await?;
        Self::parse_zone_header(value, zone, block)
    }

    /// Several zone headers in one round trip where the transport batches, in
    /// order, each validated as [`Self::header_at`] and [`Self::latest_header`]
    /// validate theirs. `None` means the node reported no header.
    pub async fn headers(
        &self,
        zone: Zone,
        blocks: &[BlockTag],
    ) -> Result<Vec<Option<ZoneHeader>>, ProviderError> {
        let values = self.header_values(zone, blocks).await.into_iter();
        Self::parse_zone_headers(values, zone, blocks)
    }

    /// [`Self::headers`], read together with the network's genesis. `None` when
    /// the genesis is not `genesis`: that is checked before any header is
    /// parsed and before any header's RPC error is returned, so a wrong network
    /// is reported as such rather than as a bad or missing header. Chain ID is
    /// guarded on every read, as always.
    pub async fn headers_on_network(
        &self,
        zone: Zone,
        genesis: Hash32,
        blocks: &[BlockTag],
    ) -> Result<Option<Vec<Option<ZoneHeader>>>, ProviderError> {
        let mut selectors = Vec::with_capacity(blocks.len() + 1);
        selectors.push(BlockTag::Number(U256::ZERO));
        selectors.extend_from_slice(blocks);
        let mut values = self.header_values(zone, &selectors).await.into_iter();
        let observed = values
            .next()
            .ok_or(ProviderError::InvalidResult("header batch count"))??;
        if types::genesis_hash(observed)? != genesis {
            return Ok(None);
        }
        Self::parse_zone_headers(values, zone, blocks).map(Some)
    }

    /// Parse raw header responses in order, each for the matching block.
    fn parse_zone_headers(
        values: impl Iterator<Item = Result<Value, ProviderError>>,
        zone: Zone,
        blocks: &[BlockTag],
    ) -> Result<Vec<Option<ZoneHeader>>, ProviderError> {
        let headers = values
            .zip(blocks)
            .map(|(value, block)| Self::parse_zone_header(value?, zone, *block))
            .collect::<Result<Vec<_>, _>>()?;
        if headers.len() != blocks.len() {
            return Err(ProviderError::InvalidResult("header batch count"));
        }
        Ok(headers)
    }

    /// Several headers from one zone, in order, as raw responses: guarded
    /// batches of up to `MAX_BATCH_CALLS - 1` where the transport batches,
    /// otherwise one guarded read each. Parse each with `parse_zone_header`, or
    /// `types::genesis_hash` for height zero.
    ///
    /// Reading stops at the first error, which is the last entry, so a caller
    /// can check an earlier response, such as the genesis, before a later
    /// failure decides the outcome.
    pub(crate) async fn header_values(
        &self,
        zone: Zone,
        blocks: &[BlockTag],
    ) -> Vec<Result<Value, ProviderError>> {
        let mut values = Vec::with_capacity(blocks.len());
        if let Err(error) = self.read_header_values(zone, blocks, &mut values).await {
            values.push(Err(error));
        }
        values
    }

    async fn read_header_values(
        &self,
        zone: Zone,
        blocks: &[BlockTag],
        values: &mut Vec<Result<Value, ProviderError>>,
    ) -> Result<(), ProviderError> {
        let endpoint = self.routing.endpoint(zone.into())?;
        for page in blocks.chunks(quai_rpc::MAX_BATCH_CALLS - 1) {
            let calls = page
                .iter()
                .map(|block| Ok(("quai_getHeaderByNumber", json!([block.rpc_value()?]))))
                .collect::<Result<Vec<_>, ProviderError>>()?;
            match self.guarded_batch(endpoint, calls).await {
                Some(batch) => {
                    for value in batch? {
                        values.push(Ok(value?));
                    }
                }
                // Nothing was sent, so read each block alone.
                None => {
                    for block in page {
                        let params = json!([block.rpc_value()?]);
                        values.push(Ok(self
                            .read(zone.into(), "quai_getHeaderByNumber", params)
                            .await?));
                    }
                }
            }
        }
        Ok(())
    }

    /// Validate a header response's location and, for a numbered read, height.
    pub(crate) fn parse_zone_header(
        value: Value,
        zone: Zone,
        block: BlockTag,
    ) -> Result<Option<ZoneHeader>, ProviderError> {
        if value.is_null() {
            return Ok(None);
        }
        let header = ZoneHeader::try_from(value)?;
        if header.zone != zone {
            return Err(ProviderError::InvalidResult("header zone mismatch"));
        }
        if let BlockTag::Number(number) = block
            && number != U256::from(header.number)
        {
            return Err(ProviderError::InvalidResult("header number mismatch"));
        }
        Ok(Some(header))
    }

    /// Read a Quai account balance in base units, routed by the address's zone.
    /// Qi UTXO balances require a separate API and cannot be passed here.
    pub async fn balance(
        &self,
        address: QuaiAddress,
        block: BlockTag,
    ) -> Result<U256, ProviderError> {
        let block = block.rpc_value()?;
        quantity(
            self.read(
                Shard::Zone(address.zone()),
                "quai_getBalance",
                json!([address.to_string(), block]),
            )
            .await?,
        )
    }

    /// Account nonce at a block or pending pool state; routed by the sender.
    pub async fn transaction_count(
        &self,
        address: QuaiAddress,
        block: BlockTag,
    ) -> Result<u64, ProviderError> {
        let block = block.rpc_value()?;
        types::uint64(
            self.read(
                address.zone().into(),
                "quai_getTransactionCount",
                json!([address.to_string(), block]),
            )
            .await?,
        )
    }

    /// Current node-recommended gas price for a zone; this is not a historical quote.
    pub async fn gas_price(&self, zone: Zone) -> Result<U256, ProviderError> {
        quantity(self.read(zone.into(), "quai_gasPrice", json!([])).await?)
    }

    /// [`Self::gas_price`], read together with the network's genesis in one
    /// round trip where the transport batches. `None` when the genesis is not
    /// `genesis`, decided before the price is parsed, so a wrong network is
    /// reported as such. Both reads are address-free, so joining them discloses
    /// nothing before the network is confirmed.
    pub async fn gas_price_on_network(
        &self,
        zone: Zone,
        genesis: Hash32,
    ) -> Result<Option<U256>, ProviderError> {
        let endpoint = self.routing.endpoint(zone.into())?;
        let calls = vec![
            ("quai_getHeaderByNumber", json!(["0x0"])),
            ("quai_gasPrice", json!([])),
        ];
        let (observed, price) = match self.guarded_batch(endpoint, calls).await {
            Some(batch) => {
                let mut values = batch?.into_iter();
                match (values.next(), values.next()) {
                    (Some(observed), Some(price)) => (observed?, price),
                    _ => return Err(ProviderError::InvalidResult("batch response count")),
                }
            }
            // Nothing was sent: the genesis first, so a wrong network is never
            // asked for anything else.
            None => {
                let observed = self
                    .read(zone.into(), "quai_getHeaderByNumber", json!(["0x0"]))
                    .await?;
                if types::genesis_hash(observed)? != genesis {
                    return Ok(None);
                }
                return self.gas_price(zone).await.map(Some);
            }
        };
        if types::genesis_hash(observed)? != genesis {
            return Ok(None);
        }
        Ok(Some(quantity(price?)?))
    }

    /// Read account/contract bytecode; empty code is a valid result.
    pub async fn code(
        &self,
        address: QuaiAddress,
        block: BlockTag,
    ) -> Result<RpcData, ProviderError> {
        let block = block.rpc_value()?;
        types::data(
            self.read(
                address.zone().into(),
                "quai_getCode",
                json!([address.to_string(), block]),
            )
            .await?,
        )
    }

    /// Read exactly one 32-byte storage word using a full-width unsigned slot index.
    pub async fn storage_at(
        &self,
        address: QuaiAddress,
        slot: U256,
        block: BlockTag,
    ) -> Result<Hash32, ProviderError> {
        let block = block.rpc_value()?;
        types::hash(
            self.read(
                address.zone().into(),
                "quai_getStorageAt",
                json!([address.to_string(), format!("{slot:#x}"), block]),
            )
            .await?,
        )
    }

    /// Simulate an account transaction without submitting it. Remote reverts remain Rpc errors.
    pub async fn call(
        &self,
        request: &CallRequest,
        block: BlockTag,
    ) -> Result<RpcData, ProviderError> {
        let block = block.rpc_value()?;
        let args = request.rpc_value()?;
        types::data(
            self.read(
                request.from.zone().into(),
                "quai_call",
                json!([args, block]),
            )
            .await?,
        )
    }

    /// Estimate account gas at an explicit selector; unlike JS, the node supports a second selector parameter.
    /// Node simulation may replace a supplied nonce with state nonce. No fee or inclusion guarantee is implied.
    pub async fn estimate_gas(
        &self,
        request: &CallRequest,
        block: BlockTag,
    ) -> Result<u64, ProviderError> {
        let block = block.rpc_value()?;
        let args = request.rpc_value()?;
        types::uint64(
            self.read(
                request.from.zone().into(),
                "quai_estimateGas",
                json!([args, block]),
            )
            .await?,
        )
    }

    /// Look up an included or pending transaction in an explicitly selected zone.
    /// This does not infer a route from modified transaction-hash bytes.
    pub async fn transaction(
        &self,
        zone: Zone,
        hash: Hash32,
    ) -> Result<Option<Transaction>, ProviderError> {
        let value = self
            .read(
                zone.into(),
                "quai_getTransactionByHash",
                json!([hash.to_string()]),
            )
            .await?;
        if value.is_null() {
            return Ok(None);
        }
        let tx = Transaction::try_from(value)?;
        if tx.hash != hash {
            return Err(ProviderError::InvalidResult("transaction hash mismatch"));
        }
        let chain = match &tx.details {
            TransactionDetails::Quai(tx) => Some(tx.chain_id),
            TransactionDetails::Qi(tx) => Some(tx.chain_id),
            TransactionDetails::External(_) => None,
        };
        if let Some(actual) = chain
            && actual != self.expected_chain_id
        {
            return Err(ProviderError::ChainMismatch {
                expected: self.expected_chain_id,
                actual,
            });
        }
        Ok(Some(tx))
    }

    /// Look up a receipt in an explicit zone; None is not evidence of rejection or finality.
    pub async fn receipt(
        &self,
        zone: Zone,
        hash: Hash32,
    ) -> Result<Option<Receipt>, ProviderError> {
        let value = self
            .read(
                zone.into(),
                "quai_getTransactionReceipt",
                json!([hash.to_string()]),
            )
            .await?;
        if value.is_null() {
            return Ok(None);
        }
        let receipt = Receipt::try_from(value)?;
        if receipt.transaction_hash != hash {
            return Err(ProviderError::InvalidResult(
                "receipt transaction hash mismatch",
            ));
        }
        Ok(Some(receipt))
    }

    /// Query current-head indexed Qi outputs. Empty results do not prove a complete index.
    /// Returned outputs require independent maturity, expiry and ownership checks before spending.
    pub async fn outpoints(
        &self,
        address: QiAddress,
    ) -> Result<Vec<AddressOutpoint>, ProviderError> {
        types::parse_outpoints(
            self.read(
                address.zone().into(),
                "quai_getOutpointsByAddress",
                json!([address.to_string()]),
            )
            .await?,
        )
    }

    /// Read the Prime chain's advertised active zones without adding routes implicitly.
    pub async fn running_zones(&self) -> Result<Vec<Zone>, ProviderError> {
        let value = self
            .read(Shard::Prime, "quai_listRunningChains", json!([]))
            .await?;
        let locations = value
            .as_array()
            .ok_or(ProviderError::InvalidResult("expected location array"))?;
        if locations.len() > Zone::ALL.len() {
            return Err(ProviderError::InvalidResult("too many locations"));
        }
        let mut zones = Vec::with_capacity(locations.len());
        for location in locations {
            let pair = location
                .as_array()
                .filter(|p| p.len() == 2)
                .ok_or(ProviderError::InvalidResult("expected region/zone pair"))?;
            let region = pair[0]
                .as_u64()
                .filter(|r| *r <= 2)
                .ok_or(ProviderError::InvalidResult("unknown region"))?;
            let index = pair[1]
                .as_u64()
                .filter(|z| *z <= 2)
                .ok_or(ProviderError::InvalidResult("unknown zone"))?;
            let zone = Zone::from_byte(((region << 4) | index) as u8)
                .map_err(|_| ProviderError::InvalidResult("unknown zone"))?;
            if zones.contains(&zone) {
                return Err(ProviderError::InvalidResult("duplicate zone"));
            }
            zones.push(zone);
        }
        Ok(zones)
    }
}

fn quantity(value: Value) -> Result<U256, ProviderError> {
    let value = value
        .as_str()
        .ok_or(ProviderError::InvalidResult("expected hex quantity string"))?;
    Ok(parse_quantity(value)?)
}

#[cfg(all(feature = "ws", not(target_arch = "wasm32")))]
mod head_follower;
#[cfg(all(feature = "ws", not(target_arch = "wasm32")))]
pub use head_follower::{HeadFollowPolicy, WsHeadFollower};

/// Internal response parsers exposed for fuzzing only.
///
/// Not public API: the `fuzzing` feature is off by default, these items are
/// hidden from documentation, and their signatures may change without notice.
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzz_internals {
    use crate::{AddressOutpoint, BlockReference, MinedBlock, ProviderError};
    use quai_primitives::{Hash32, QuaiAddress, Zone};
    use serde_json::{Map, Value};

    /// Parse a `quai_getOutpointsByAddress` result.
    pub fn parse_outpoints(value: Value) -> Result<Vec<AddressOutpoint>, ProviderError> {
        crate::types::parse_outpoints(value)
    }

    /// Validate a block response for a zone and selector.
    pub fn block_fields(
        value: Value,
        zone: Zone,
        selector: MinedBlock,
    ) -> Result<(BlockReference, Hash32, Map<String, Value>), ProviderError> {
        crate::blocks::block_fields(value, zone, selector)
    }

    /// Parse a `txpool_content` result for one zone.
    pub fn pool_entries(
        value: Value,
        zone: Zone,
        max_entries: usize,
    ) -> Result<Vec<(bool, QuaiAddress, u64, Value)>, ProviderError> {
        crate::account_rpc::pool_entries(value, zone, max_entries)
    }
}
