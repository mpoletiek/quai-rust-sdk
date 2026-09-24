//! ABI-bound account contract calls with explicit simulation and authorization boundaries.
mod io;
pub use io::{ContractLog, FallbackCall, MAX_QUERY_VALUE_BYTES, MAX_QUERY_VALUE_NODES};
use quai_abi::{
    AbiError, AbiEventValue, AbiFilterTopic, AbiFilterValue, AbiFunction, AbiInterface,
    StateMutability,
};
use quai_primitives::{Hash32, QuaiAddress};
use quai_provider::{
    BlockTag, CallRequest, Log, LogFilter, LogRange, Provider, ProviderError, RpcData, TopicMatch,
};
use quai_rpc::{Transport, U256};
use serde_json::Value;
use thiserror::Error;

/// Contract operations fail before submission; sending belongs to a wallet workflow.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ContractError {
    /// The selected raw-data path has no declared receive/fallback entry point.
    #[error("contract ABI has no applicable receive or fallback entry point")]
    MissingFallback,
    /// Invalid/ambiguous ABI symbol or noncanonical data.
    #[error(transparent)]
    Abi(#[from] AbiError),
    /// Node observation/simulation failure, including a remote revert.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// Ordinary calls require a sender in the contract's zone.
    #[error("contract sender is outside the destination zone")]
    ZoneMismatch,
    /// A supplied value conflicts with declared ABI mutability.
    #[error("nonpayable contract operation cannot transfer native value")]
    Nonpayable,
    /// `call` accepts declared read functions; use explicit simulation for writes.
    #[error("state-changing ABI function requires explicit simulation or wallet preparation")]
    WriteFunction,
    /// A prepared call belongs to another contract or differs from the compiled ABI.
    #[error("prepared contract call does not match this contract")]
    CallMismatch,
    /// Token metadata/result cannot fit its promised exact type.
    #[error("invalid typed contract result")]
    InvalidResult,
    /// Deployment nonce/scope/bytecode/search limits are invalid.
    #[error("invalid deployment parameters")]
    InvalidDeployment,
    /// A bounded contract-address search was cancelled or found no matching address.
    #[error("deployment address search cancelled or exhausted")]
    SearchIncomplete,
    /// Anonymous events have no signature topic and require explicit log selection.
    #[error("anonymous event requires explicitly selected logs")]
    AnonymousEvent,
    /// Event topic filters exceed the declared indexed field count.
    #[error("event filter exceeds its indexed field count")]
    InvalidEventFilter,
    /// A checked binding requires nonempty runtime code at its observed block.
    #[error("contract address has no deployed runtime code at the observed block")]
    MissingCode,
    /// Source genesis differs from the caller's explicitly trusted network identity.
    #[error("contract deployment genesis mismatch")]
    GenesisMismatch,
    /// Nonempty runtime differs from the explicitly supplied expected Keccak hash.
    #[error("contract runtime code hash mismatch")]
    RuntimeMismatch,
}

impl ContractError {
    /// How to react to this failure; see [`quai_primitives::ErrorClass`].
    /// Matched exhaustively so a new variant must choose a class.
    ///
    /// A deployment check retries only on `Stale`: a new block or reorg during
    /// the observation. `NetworkMismatch` (a wrong chain or genesis) and
    /// `Invalid` (missing code, a runtime mismatch) repeat on every attempt.
    pub fn class(&self) -> quai_primitives::ErrorClass {
        use quai_primitives::ErrorClass;
        match self {
            Self::Provider(error) => error.class(),
            Self::GenesisMismatch => ErrorClass::NetworkMismatch,
            // The same bounded search finds the same nothing; a caller that
            // cancelled it already knows.
            Self::SearchIncomplete
            | Self::MissingFallback
            | Self::Abi(_)
            | Self::ZoneMismatch
            | Self::Nonpayable
            | Self::WriteFunction
            | Self::CallMismatch
            | Self::InvalidResult
            | Self::InvalidDeployment
            | Self::AnonymousEvent
            | Self::InvalidEventFilter
            | Self::MissingCode
            | Self::RuntimeMismatch => ErrorClass::Invalid,
        }
    }
}

/// Decoded event values together with their complete source association and removal flag.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ContractEvent {
    /// Caller-selected ABI declaration, not proof of the emitter's semantics.
    pub signature: String,
    /// Positional values; indexed dynamic/compound values remain hashes.
    pub values: Vec<AbiEventValue>,
    /// Original log, including block hash, position and reorganization removal flag.
    pub log: Log,
}

/// ABI interpretation of one receipt log without losing unknown or malformed logs.
/// Matching a topic identifies an ABI declaration, not authentic emitter semantics.
#[derive(Debug)]
pub enum ReceiptLog<'a> {
    /// No matching nonanonymous declaration (or outside a bound contract's address).
    Unrecognized(&'a Log),
    /// Matching declaration and canonical decoded arguments.
    Decoded {
        /// Complete source log, including removal and inclusion metadata.
        log: &'a Log,
        /// Caller-supplied ABI declaration with name, signature and argument types.
        event: &'a quai_abi::AbiEvent,
        /// Positional values; indexed dynamic/compound fields remain hashes.
        values: Vec<AbiEventValue>,
    },
    /// A matching or ambiguous declaration could not decode this source log.
    Undecoded {
        /// Complete original log, retained despite the decoding failure.
        log: &'a Log,
        /// Structured ABI error, without arbitrary node diagnostic strings.
        error: AbiError,
    },
}
impl ReceiptLog<'_> {
    /// Original log for every interpretation, preserving order and source identity.
    pub fn log(&self) -> &Log {
        match self {
            Self::Unrecognized(log) | Self::Decoded { log, .. } | Self::Undecoded { log, .. } => {
                log
            }
        }
    }
}
/// Decode all receipt logs against an ABI with a caller budget of 1..65536 logs.
/// Like quais.js ContractTransactionReceipt, this free function considers every
/// emitter. Use `Contract::receipt_logs` to restrict decoding to one address.
/// No RPC, mutation or implicit anonymous-event selection occurs.
pub fn decode_receipt_logs<'a>(
    interface: &'a AbiInterface,
    receipt: &'a quai_provider::Receipt,
    max_logs: usize,
) -> Result<Vec<ReceiptLog<'a>>, ContractError> {
    receipt_logs(interface, receipt, max_logs, None)
}
fn receipt_logs<'a>(
    interface: &'a AbiInterface,
    receipt: &'a quai_provider::Receipt,
    max_logs: usize,
    emitter: Option<quai_primitives::Address>,
) -> Result<Vec<ReceiptLog<'a>>, ContractError> {
    if !(1..=65536).contains(&max_logs) || receipt.logs.len() > max_logs {
        return Err(AbiError::Limit.into());
    }
    Ok(receipt
        .logs
        .iter()
        .map(|log| {
            if emitter.is_some_and(|address| address != log.address) {
                return ReceiptLog::Unrecognized(log);
            }
            match interface.parse_log(&log.topics, log.data.bytes()) {
                Ok(parsed) => ReceiptLog::Decoded {
                    log,
                    event: parsed.event,
                    values: parsed.values,
                },
                Err(AbiError::NotFound) => ReceiptLog::Unrecognized(log),
                Err(error) => ReceiptLog::Undecoded { log, error },
            }
        })
        .collect())
}

/// Explicit four-byte suffix search, matching the JS deployment-grinding mechanism.
/// Appending bytes can affect hand-written init code that inspects trailing code;
/// review/simulate the final bytes, never assume arbitrary init code ignores them.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct DeploymentSearch {
    /// First suffix, big endian. Overflow stops rather than reusing a suffix.
    pub start_salt: u32,
    /// Maximum candidates, from one through 10,000.
    pub max_attempts: u32,
}
impl DeploymentSearch {
    /// Up to `max_attempts` suffixes from `start_salt`.
    pub const fn new(start_salt: u32, max_attempts: u32) -> Self {
        Self {
            start_salt,
            max_attempts,
        }
    }
}
/// Frozen deployment bytes, nonce and predicted address, before fee parameters.
#[derive(Debug)]
pub struct PreparedDeployment {
    transaction: quai_consensus::QuaiTransaction,
    address: QuaiAddress,
    salt: u32,
    attempts: u32,
}
impl PreparedDeployment {
    /// Fixed sender nonce used for grinding. Changing it changes the predicted address.
    pub fn nonce(&self) -> u64 {
        self.transaction.nonce
    }

    /// Expected contract account for the exact reserved nonce and init data.
    pub fn address(&self) -> QuaiAddress {
        self.address
    }
    /// Suffix actually appended to the complete constructor init data.
    pub fn salt(&self) -> u32 {
        self.salt
    }
    /// Candidates hashed before obtaining the matching ledger/zone.
    pub fn attempts(&self) -> u32 {
        self.attempts
    }
    /// Exact init data, including constructor arguments and the suffix.
    pub fn init_data(&self) -> &[u8] {
        &self.transaction.data
    }
    /// Fill explicitly chosen gas/price without changing nonce, init code or the
    /// mandatory predicted-address access list. The caller must enforce its fee
    /// budget and reserve this nonce before signing or broadcasting.
    pub fn into_transaction(
        mut self,
        gas_limit: u64,
        gas_price: U256,
    ) -> Result<quai_consensus::QuaiTransaction, ContractError> {
        if gas_limit == 0 {
            return Err(ContractError::InvalidDeployment);
        }
        self.transaction.gas_limit = gas_limit;
        self.transaction.gas_price = gas_price;
        Ok(self.transaction)
    }
}

/// Encode constructor arguments and grind a same-zone Quai CREATE address for an
/// explicitly reserved nonce. There are no network requests or implicit signing.
///
/// The node requires the predicted contract address in the transaction's access
/// list; this builder inserts it before estimation/signing. It preserves leading
/// zero init-code bytes, correcting the legacy JS address-prediction defect.
#[allow(clippy::too_many_arguments)]
pub fn prepare_deployment(
    interface: &AbiInterface,
    init_code: &[u8],
    arguments: &[Value],
    sender: QuaiAddress,
    chain_id: U256,
    nonce: u64,
    value: U256,
    search: DeploymentSearch,
    mut cancelled: impl FnMut() -> bool,
) -> Result<PreparedDeployment, ContractError> {
    if chain_id == U256::ZERO
        || init_code.is_empty()
        || search.max_attempts == 0
        || search.max_attempts > 10_000
        // Bounded against what the node's pool takes, not the codec ceiling: a
        // deployment above `txMaxSize` is refused with `ErrOversizedData` after
        // the nonce is reserved and the transaction signed.
        || init_code.len() > quai_consensus::MAX_POOL_TRANSACTION_BYTES - 512
    {
        return Err(ContractError::InvalidDeployment);
    }
    let arguments = if let Some(constructor) = interface.constructor() {
        if value != U256::ZERO && constructor.state_mutability() != StateMutability::Payable {
            return Err(ContractError::Nonpayable);
        }
        constructor.encode_arguments(arguments)?
    } else {
        if !arguments.is_empty() || value != U256::ZERO {
            return Err(ContractError::InvalidDeployment);
        }
        vec![]
    };
    let total = init_code
        .len()
        .checked_add(arguments.len())
        .and_then(|n| n.checked_add(4))
        .ok_or(ContractError::InvalidDeployment)?;
    // The whole payload goes on the wire, not just the init code, so this is the
    // binding check: constructor arguments can carry a deployment past the pool
    // limit on their own. Bounding only `init_code` above would let such a
    // transaction reserve a nonce and be signed, after which submission refuses
    // it locally and the nonce is consumed without ever reaching the chain.
    if total > quai_consensus::MAX_POOL_TRANSACTION_BYTES - 512 {
        return Err(ContractError::InvalidDeployment);
    }
    let mut data = Vec::with_capacity(total);
    data.extend_from_slice(init_code);
    data.extend_from_slice(&arguments);
    data.extend_from_slice(&[0; 4]);
    for attempt in 0..search.max_attempts {
        if cancelled() {
            return Err(ContractError::SearchIncomplete);
        }
        let salt = search
            .start_salt
            .checked_add(attempt)
            .ok_or(ContractError::SearchIncomplete)?;
        data[total - 4..].copy_from_slice(&salt.to_be_bytes());
        let raw = quai_primitives::contract_address(sender.address(), nonce, &data);
        if let Ok(address) = QuaiAddress::try_from(raw)
            && address.zone() == sender.zone()
        {
            return Ok(PreparedDeployment {
                transaction: quai_consensus::QuaiTransaction {
                    chain_id,
                    nonce,
                    to: None,
                    value,
                    gas_limit: 0,
                    gas_price: U256::ZERO,
                    data,
                    access_list: vec![quai_consensus::AccessTuple {
                        address: raw,
                        storage_keys: vec![],
                    }],
                },
                address,
                salt,
                attempts: attempt + 1,
            });
        }
    }
    Err(ContractError::SearchIncomplete)
}

/// Immutable selector, arguments, value and destination to review and authorize.
#[derive(Clone, Debug)]
pub struct ContractCall {
    to: QuaiAddress,
    function: AbiFunction,
    value: U256,
    data: RpcData,
    access_list: Vec<quai_consensus::AccessTuple>,
}
impl ContractCall {
    /// Exact called account.
    pub fn destination(&self) -> QuaiAddress {
        self.to
    }
    /// Canonical signature identifies overloads without ambiguity.
    pub fn signature(&self) -> &str {
        self.function.signature()
    }
    /// Native base units included in the call.
    pub fn value(&self) -> U256 {
        self.value
    }
    /// Borrow exact selector-prefixed call bytes for external signing.
    pub fn data(&self) -> &RpcData {
        &self.data
    }
    /// Ordered access declaration carried into simulation and durable signing.
    pub fn access_list(&self) -> &[quai_consensus::AccessTuple] {
        &self.access_list
    }
    /// Attach an explicit bounded declaration before review/signing. go-quai
    /// requires called accounts in the signed list even when RPC simulation
    /// discovers them automatically. No node discovery or reordering occurs here.
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
    /// Decode exact arguments for an application review screen; values may be sensitive.
    pub fn arguments(&self) -> Result<Vec<Value>, ContractError> {
        Ok(self.function.decode_call(self.data.bytes())?)
    }
    /// Convert to portable/native account preparation without changing call bytes or access declarations.
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

/// Contract ABI and address bound to an application-owned typed provider.
pub struct Contract<'a, T> {
    address: QuaiAddress,
    interface: AbiInterface,
    provider: &'a Provider<T>,
}
impl<'a, T: Transport> Contract<'a, T> {
    /// Bind a known Quai address and parsed ABI without requesting node state.
    pub fn new(address: QuaiAddress, interface: AbiInterface, provider: &'a Provider<T>) -> Self {
        Self {
            address,
            interface,
            provider,
        }
    }
    /// Bound contract address; this does not prove deployed code exists.
    pub fn address(&self) -> QuaiAddress {
        self.address
    }
    /// Compiled exact ABI; caller supplies and trusts its semantics.
    pub fn interface(&self) -> &AbiInterface {
        &self.interface
    }
    /// Verify nonempty runtime at a rechecked mined block in the explicitly trusted
    /// genesis. An optional runtime Keccak pins exact bytes, not proxy semantics or
    /// future code. This observation does not authorize any later transaction.
    ///
    /// The genesis is confirmed before the contract address is sent. To verify
    /// several contracts at one block, use
    /// [`Provider::observe_contract_codes`](quai_provider::Provider::observe_contract_codes).
    pub async fn verify_deployment(
        &self,
        expected_genesis: Hash32,
        expected_runtime: Option<Hash32>,
        block: BlockTag,
    ) -> Result<quai_provider::ContractCodeObservation, ContractError> {
        if expected_genesis == Hash32::ZERO {
            return Err(ContractError::InvalidDeployment);
        }
        let observation = match self
            .provider
            .observe_contract_codes(expected_genesis, &[(self.address, expected_runtime)], block)
            .await
        {
            Ok(mut observations) => observations.remove(0),
            Err(ProviderError::GenesisMismatch) => return Err(ContractError::GenesisMismatch),
            Err(error) => return Err(error.into()),
        };
        if observation.code.bytes.bytes().is_empty() {
            return Err(ContractError::MissingCode);
        }
        if observation.code.matches_expected == Some(false) {
            return Err(ContractError::RuntimeMismatch);
        }
        Ok(observation)
    }
    /// [`Self::verify_deployment`] with the code hash proven against the
    /// block's state root instead of hashed from downloaded bytecode.
    ///
    /// The node sends an account proof, about 2 KB, rather than the runtime.
    /// The same errors apply: `GenesisMismatch` before the address is sent,
    /// `MissingCode` for an absent account or one without code, and
    /// `RuntimeMismatch` when `expected_runtime` differs. A proof that does not
    /// verify is `ContractError::Provider(ProviderError::Proof(_))`.
    ///
    /// The block is still the node's report of the canonical chain; see
    /// [`ProvenAccount`](quai_provider::ProvenAccount). To confirm it with a
    /// second node first, read a [`StateAnchor`](quai_provider::StateAnchor)
    /// and call [`Self::prove_deployment_at`].
    pub async fn prove_deployment(
        &self,
        expected_genesis: Hash32,
        expected_runtime: Option<Hash32>,
        block: BlockTag,
    ) -> Result<quai_provider::ProvenAccount, ContractError> {
        if expected_genesis == Hash32::ZERO {
            return Err(ContractError::InvalidDeployment);
        }
        let anchor = match self
            .provider
            .state_anchor(expected_genesis, self.address.zone(), block)
            .await
        {
            Ok(anchor) => anchor,
            Err(ProviderError::GenesisMismatch) => return Err(ContractError::GenesisMismatch),
            Err(error) => return Err(error.into()),
        };
        self.prove_deployment_at(&anchor, expected_runtime).await
    }
    /// [`Self::prove_deployment`] at an anchor already read, and perhaps
    /// confirmed with a second node: one round trip. Several contracts of the
    /// anchor's zone can share one anchor.
    pub async fn prove_deployment_at(
        &self,
        anchor: &quai_provider::StateAnchor,
        expected_runtime: Option<Hash32>,
    ) -> Result<quai_provider::ProvenAccount, ContractError> {
        let proven = self
            .provider
            .prove_accounts_at(anchor, &[(self.address, &[])])
            .await?
            .remove(0);
        if !proven.has_code() {
            return Err(ContractError::MissingCode);
        }
        if expected_runtime.is_some_and(|expected| expected != proven.code_hash()) {
            return Err(ContractError::RuntimeMismatch);
        }
        Ok(proven)
    }
    /// Decode an explicitly selected log, checking its emitter and exact ABI shape.
    /// Anonymous events are accepted here because the caller supplies the declaration;
    /// their topic layout cannot identify that declaration unambiguously on its own.
    pub fn decode_event(&self, event: &str, log: Log) -> Result<ContractEvent, ContractError> {
        if log.address != self.address.address() {
            return Err(ContractError::CallMismatch);
        }
        let event = self.interface.event(event)?;
        let values = event.decode_log(&log.topics, log.data.bytes())?;
        Ok(ContractEvent {
            signature: event.signature().to_owned(),
            values,
            log,
        })
    }
    /// Interpret matching logs from this contract and retain every other log as
    /// unrecognized. Malformed matching logs retain their original data and error.
    pub fn receipt_logs<'b>(
        &'b self,
        receipt: &'b quai_provider::Receipt,
        max_logs: usize,
    ) -> Result<Vec<ReceiptLog<'b>>, ContractError> {
        receipt_logs(
            &self.interface,
            receipt,
            max_logs,
            Some(self.address.address()),
        )
    }
    /// Query a bounded block range for a nonanonymous event from this exact emitter.
    /// Filters refer only to indexed argument positions; the signature topic is inserted
    /// automatically. Results preserve removed logs and do not imply finality.
    pub async fn events(
        &self,
        event: &str,
        range: LogRange,
        indexed_topics: &[TopicMatch],
    ) -> Result<Vec<ContractEvent>, ContractError> {
        let declaration = self.interface.event(event)?;
        if declaration.anonymous() {
            return Err(ContractError::AnonymousEvent);
        }
        if indexed_topics.len() > declaration.indexed().iter().filter(|b| **b).count() {
            return Err(ContractError::InvalidEventFilter);
        }
        let mut topics = Vec::with_capacity(1 + indexed_topics.len());
        topics.push(TopicMatch::Exact(declaration.topic_hash()));
        topics.extend_from_slice(indexed_topics);
        self.query_events(event, range, topics).await
    }
    /// Query this emitter using typed filters in full event declaration order.
    /// Non-indexed fields must be `Any`; omitted suffix fields are unrestricted.
    /// Strings/bytes/compound values are hashed, and `AnyOf` supplies bounded OR
    /// alternatives. Anonymous events require explicitly selected logs instead.
    pub async fn events_by_values(
        &self,
        event: &str,
        range: LogRange,
        filters: &[AbiFilterValue<'_>],
    ) -> Result<Vec<ContractEvent>, ContractError> {
        let declaration = self.interface.event(event)?;
        if declaration.anonymous() {
            return Err(ContractError::AnonymousEvent);
        }
        let topics = declaration
            .encode_filter_topics(filters)?
            .into_iter()
            .map(|topic| match topic {
                AbiFilterTopic::Any => TopicMatch::Any,
                AbiFilterTopic::Exact(hash) => TopicMatch::Exact(hash),
                AbiFilterTopic::AnyOf(hashes) => TopicMatch::AnyOf(hashes),
            })
            .collect();
        self.query_events(event, range, topics).await
    }
    async fn query_events(
        &self,
        event: &str,
        range: LogRange,
        topics: Vec<TopicMatch>,
    ) -> Result<Vec<ContractEvent>, ContractError> {
        self.provider
            .logs(
                &LogFilter::new(self.address.zone(), range)
                    .with_addresses(vec![self.address.address()])
                    .with_topics(topics),
            )
            .await?
            .into_iter()
            .map(|log| self.decode_event(event, log))
            .collect()
    }
    /// Encode exact intent without any network, wallet prompt or signature.
    pub fn prepare(
        &self,
        function: &str,
        arguments: &[Value],
        value: U256,
    ) -> Result<ContractCall, ContractError> {
        let function = self.interface.function(function)?;
        if value != U256::ZERO && function.state_mutability() != StateMutability::Payable {
            return Err(ContractError::Nonpayable);
        }
        let data = RpcData::new(function.encode_call(arguments)?)?;
        Ok(ContractCall {
            to: self.address,
            function: function.clone(),
            value,
            data,
            access_list: vec![],
        })
    }
    /// Read a declared pure/view function at an explicit block. ABI declarations
    /// are caller-supplied descriptions, not proof of the actual contract behavior.
    pub async fn call(
        &self,
        from: QuaiAddress,
        function: &str,
        arguments: &[Value],
        block: BlockTag,
    ) -> Result<Vec<Value>, ContractError> {
        let function = self.interface.function(function)?;
        if !matches!(
            function.state_mutability(),
            StateMutability::Pure | StateMutability::View
        ) {
            return Err(ContractError::WriteFunction);
        }
        let prepared = self.prepare(function.signature(), arguments, U256::ZERO)?;
        self.simulate(from, &prepared, block, None).await
    }
    /// Execute a read-only node simulation of the exact prepared call. A successful
    /// simulation does not reserve funds, authorize a signature or guarantee mining.
    pub async fn simulate(
        &self,
        from: QuaiAddress,
        call: &ContractCall,
        block: BlockTag,
        gas: Option<u64>,
    ) -> Result<Vec<Value>, ContractError> {
        if from.zone() != self.address.zone() {
            return Err(ContractError::ZoneMismatch);
        }
        let function = self.interface.function(call.signature())?;
        if call.to != self.address
            || function.outputs() != call.function.outputs()
            || function.state_mutability() != call.function.state_mutability()
        {
            return Err(ContractError::CallMismatch);
        }
        function.decode_call(call.data.bytes())?;
        let mut request = CallRequest::new(from, self.address);
        request.value = Some(call.value);
        request.input = call.data.clone();
        request.gas = gas;
        request.access_list = call
            .access_list
            .iter()
            .map(|entry| quai_provider::AccessListItem {
                address: entry.address,
                storage_keys: entry.storage_keys.clone(),
            })
            .collect();
        let result = self.provider.call(&request, block).await?;
        Ok(function.decode_returns(result.bytes())?)
    }
}

const ERC20_ABI:&[u8]=br#"[
{"type":"function","name":"balanceOf","stateMutability":"view","inputs":[{"name":"owner","type":"address"}],"outputs":[{"name":"balance","type":"uint256"}]},
{"type":"function","name":"allowance","stateMutability":"view","inputs":[{"name":"owner","type":"address"},{"name":"spender","type":"address"}],"outputs":[{"name":"amount","type":"uint256"}]},
{"type":"function","name":"decimals","stateMutability":"view","inputs":[],"outputs":[{"name":"value","type":"uint8"}]},
{"type":"function","name":"name","stateMutability":"view","inputs":[],"outputs":[{"name":"value","type":"string"}]},
{"type":"function","name":"symbol","stateMutability":"view","inputs":[],"outputs":[{"name":"value","type":"string"}]},
{"type":"function","name":"totalSupply","stateMutability":"view","inputs":[],"outputs":[{"name":"amount","type":"uint256"}]},
{"type":"function","name":"transfer","stateMutability":"nonpayable","inputs":[{"name":"to","type":"address"},{"name":"amount","type":"uint256"}],"outputs":[{"name":"success","type":"bool"}]},
{"type":"function","name":"approve","stateMutability":"nonpayable","inputs":[{"name":"spender","type":"address"},{"name":"amount","type":"uint256"}],"outputs":[{"name":"success","type":"bool"}]},
{"type":"function","name":"transferFrom","stateMutability":"nonpayable","inputs":[{"name":"owner","type":"address"},{"name":"to","type":"address"},{"name":"amount","type":"uint256"}],"outputs":[{"name":"success","type":"bool"}]}
]"#;

/// Conventional ERC-20 ABI helpers on the Quai account ledger.
///
/// Nonstandard token return formats fail explicitly; token metadata is untrusted.
/// Approval requires an exact amount; this type never selects an unlimited approval.
pub struct Erc20<'a, T>(Contract<'a, T>);
impl<'a, T: Transport> Erc20<'a, T> {
    /// Bind the conventional token ABI; no metadata is fetched automatically.
    pub fn new(address: QuaiAddress, provider: &'a Provider<T>) -> Result<Self, ContractError> {
        Ok(Self(Contract::new(
            address,
            AbiInterface::from_json(ERC20_ABI)?,
            provider,
        )))
    }
    /// Generic contract access for exact balance, allowance and token metadata calls.
    pub fn contract(&self) -> &Contract<'a, T> {
        &self.0
    }
    /// Read an exact U256 token balance; decimals are a separate untrusted metadata query.
    pub async fn balance_of(
        &self,
        from: QuaiAddress,
        owner: QuaiAddress,
        block: BlockTag,
    ) -> Result<U256, ContractError> {
        let result = self
            .0
            .call(
                from,
                "balanceOf",
                &[Value::String(owner.to_string())],
                block,
            )
            .await?;
        parse_uint_result(&result)
    }
    /// Read the exact allowance for an owner and spender.
    pub async fn allowance(
        &self,
        from: QuaiAddress,
        owner: QuaiAddress,
        spender: QuaiAddress,
        block: BlockTag,
    ) -> Result<U256, ContractError> {
        let result = self
            .0
            .call(
                from,
                "allowance",
                &[
                    Value::String(owner.to_string()),
                    Value::String(spender.to_string()),
                ],
                block,
            )
            .await?;
        parse_uint_result(&result)
    }
    /// Prepare an exact token transfer without using native coin value.
    pub fn transfer(&self, to: QuaiAddress, amount: U256) -> Result<ContractCall, ContractError> {
        if to.zone() != self.0.address.zone() {
            return Err(ContractError::ZoneMismatch);
        }
        self.0.prepare(
            "transfer",
            &[
                Value::String(to.to_string()),
                Value::String(amount.to_string()),
            ],
            U256::ZERO,
        )
    }
    /// Prepare an exact approval, including explicit zero revocation. Applications
    /// must handle allowance races and token-specific zero-first policies deliberately.
    pub fn approve(
        &self,
        spender: QuaiAddress,
        amount: U256,
    ) -> Result<ContractCall, ContractError> {
        if spender.zone() != self.0.address.zone() {
            return Err(ContractError::ZoneMismatch);
        }
        self.0.prepare(
            "approve",
            &[
                Value::String(spender.to_string()),
                Value::String(amount.to_string()),
            ],
            U256::ZERO,
        )
    }
}
fn parse_uint_result(values: &[Value]) -> Result<U256, ContractError> {
    if values.len() != 1 {
        return Err(ContractError::InvalidResult);
    }
    U256::from_str_radix(values[0].as_str().ok_or(ContractError::InvalidResult)?, 10)
        .map_err(|_| ContractError::InvalidResult)
}
