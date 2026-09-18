//! Native-independent browser boundaries plus wasm-only Fetch and injected Quai adapters.
//!
//! Browser APIs require an application-selected endpoint/provider. Construction performs no
//! network requests and never prompts for accounts. Heavy derivation and persistent wallet
//! storage remain separate worker/storage responsibilities.
#[cfg(any(target_arch = "wasm32", test))]
use quai_rpc::RemoteError;
use quai_rpc::RpcError;
#[cfg(any(target_arch = "wasm32", test))]
use serde::Deserialize;
#[cfg(any(target_arch = "wasm32", test))]
use serde_json::Value;

#[cfg(target_arch = "wasm32")]
mod browser;
#[cfg(target_arch = "wasm32")]
mod resource;
#[cfg(target_arch = "wasm32")]
pub use browser::{
    BrowserFetchTransport, InjectedProvider, InjectedSubmissionTransport,
    WalletSendAcknowledgement, WalletSendError, WalletSendIdentity, WalletSendObservation,
    fill_random,
};
#[cfg(target_arch = "wasm32")]
pub use quai_signer::{DomainPolicy, TypedData};
#[cfg(target_arch = "wasm32")]
pub use resource::BrowserResourceFetch;

/// Browser resource limits. Concurrent calls fail fast when capacity is exhausted.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct BrowserConfig {
    /// Total request wait deadline in milliseconds; browser suspension can delay timers.
    pub request_timeout_ms: u32,
    /// Maximum serialized outgoing JSON bytes.
    pub max_request_bytes: usize,
    /// Maximum streamed response or serialized injected result bytes.
    pub max_response_bytes: usize,
    /// Maximum active operations across all clones; no unbounded waiting queue.
    pub max_in_flight: usize,
}
impl BrowserConfig {
    /// Replace `request_timeout_ms`.
    pub const fn with_request_timeout_ms(mut self, request_timeout_ms: u32) -> Self {
        self.request_timeout_ms = request_timeout_ms;
        self
    }
    /// Replace `max_request_bytes`.
    pub const fn with_max_request_bytes(mut self, max_request_bytes: usize) -> Self {
        self.max_request_bytes = max_request_bytes;
        self
    }
    /// Replace `max_response_bytes`.
    pub const fn with_max_response_bytes(mut self, max_response_bytes: usize) -> Self {
        self.max_response_bytes = max_response_bytes;
        self
    }
    /// Replace `max_in_flight`.
    pub const fn with_max_in_flight(mut self, max_in_flight: usize) -> Self {
        self.max_in_flight = max_in_flight;
        self
    }
}
impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            request_timeout_ms: 10_000,
            max_request_bytes: 1024 * 1024,
            max_response_bytes: 2 * 1024 * 1024,
            max_in_flight: 8,
        }
    }
}
impl BrowserConfig {
    /// Validate limits without touching browser globals or the network.
    pub fn validate(&self) -> Result<(), BrowserError> {
        if self.request_timeout_ms == 0
            || self.request_timeout_ms > i32::MAX as u32
            || self.max_request_bytes == 0
            || self.max_request_bytes > 16 * 1024 * 1024
            || self.max_response_bytes == 0
            || self.max_response_bytes > 16 * 1024 * 1024
            || self.max_in_flight == 0
            || self.max_in_flight > 128
            || self
                .max_response_bytes
                .checked_mul(self.max_in_flight)
                .is_none_or(|n| n > 128 * 1024 * 1024)
        {
            return Err(BrowserError::InvalidConfig);
        }
        Ok(())
    }
}

/// Sanitized browser failures. No provider strings, payloads, keys or URLs are retained.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BrowserError {
    /// IndexedDB open/transaction/schema/quota failure; no remote text is retained.
    #[error("browser snapshot storage failed")]
    Storage,
    /// A concurrent writer or stale tab changed the snapshot revision.
    #[error("browser snapshot revision conflict")]
    StorageConflict,
    /// Configuration or endpoint does not match this adapter.
    #[error("invalid browser adapter configuration")]
    InvalidConfig,
    /// A configured concurrent-operation limit was reached.
    #[error("browser adapter concurrency limit reached")]
    Busy,
    /// Permission-sensitive RPC must use an explicit wallet API.
    #[error("method requires an explicit supported wallet operation")]
    AuthorizationRequired,
    /// The injected wallet reports another network.
    #[error("injected provider chain ID mismatch")]
    ChainMismatch,
    /// Accounts, chain or connection changed while an operation was awaiting the wallet.
    #[error("injected provider context changed during request")]
    ContextChanged,
    /// Wallet result or message/address request has an invalid shape.
    #[error("invalid injected provider result or request")]
    InvalidResult,
    /// The specified account is not currently exposed by the wallet.
    #[error("account is not authorized by the injected wallet")]
    AccountUnavailable,
    /// Numeric EIP-1193 failure, including 4001 user rejection and 4200 unsupported method.
    #[error("injected provider rejected request (code {0})")]
    Provider(i64),
    /// Web Crypto entropy is unavailable or the request exceeds its 65,536-byte limit.
    #[error("secure browser entropy unavailable")]
    EntropyUnavailable,
    /// Bounded RPC/transport error.
    #[error(transparent)]
    Rpc(#[from] RpcError),
}

#[cfg(any(target_arch = "wasm32", test))]
fn read_method(method: &str) -> bool {
    matches!(
        method,
        "quai_chainId"
            | "quai_blockNumber"
            | "quai_getBalance"
            | "quai_getTransactionCount"
            | "quai_gasPrice"
            | "quai_estimateGas"
            | "quai_getCode"
            | "quai_getStorageAt"
            | "quai_call"
            | "quai_getTransactionByHash"
            | "quai_getTransactionReceipt"
            | "quai_getOutpointsByAddress"
            | "quai_estimateFeeForQi"
            | "quai_getHeaderByNumber"
            | "quai_getHeaderByHash"
            | "quai_getBlockByNumber"
            | "quai_getBlockByHash"
            | "quai_getLogs"
            | "quai_listRunningChains"
            | "quai_getOutpointDeltasForAddressesInRange"
            | "quai_getLatestUTXOSetSize"
            | "quai_qiToQuai"
            | "quai_quaiToQi"
            | "quai_calculateConversionAmount"
            | "quai_getWrappedQiDeposit"
            | "quai_getLockedBalance"
            | "quai_createAccessList"
            | "quai_getPendingHeader"
            | "quai_getProtocolExpansionNumber"
            | "txpool_status"
            | "txpool_content"
            | "txpool_inspect"
            | "quai_accounts"
    )
}

#[cfg(any(target_arch = "wasm32", test))]
fn encode(value: &Value, max: usize) -> Result<String, RpcError> {
    use std::io::Write;
    struct Writer {
        bytes: Vec<u8>,
        max: usize,
    }
    impl Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.max - self.bytes.len() {
                return Err(std::io::Error::other("request limit"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Writer {
        bytes: Vec::new(),
        max,
    };
    serde_json::to_writer(&mut writer, value).map_err(|_| RpcError::RequestTooLarge)?;
    String::from_utf8(writer.bytes).map_err(|_| RpcError::InvalidConfig)
}

#[cfg(any(target_arch = "wasm32", test))]
fn decode_response(bytes: &[u8], expected_id: u64) -> Result<Value, RpcError> {
    #[derive(Default)]
    enum Slot {
        #[default]
        Absent,
        Present(Value),
    }
    fn slot<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Slot, D::Error> {
        Value::deserialize(d).map(Slot::Present)
    }
    #[derive(Deserialize)]
    struct Envelope {
        jsonrpc: String,
        id: u64,
        #[serde(default, deserialize_with = "slot")]
        result: Slot,
        #[serde(default, deserialize_with = "slot")]
        error: Slot,
    }
    let envelope: Envelope = serde_json::from_slice(bytes)
        .map_err(|_| RpcError::InvalidResponse("malformed envelope"))?;
    if envelope.jsonrpc != "2.0" || envelope.id != expected_id {
        return Err(RpcError::InvalidResponse(
            "protocol version or request ID mismatch",
        ));
    }
    match (envelope.result, envelope.error) {
        (Slot::Present(result), Slot::Absent) => Ok(result),
        (Slot::Absent, Slot::Present(error)) => Err(RpcError::Remote(
            serde_json::from_value::<RemoteError>(error)
                .map_err(|_| RpcError::InvalidResponse("malformed error"))?,
        )),
        _ => Err(RpcError::InvalidResponse(
            "expected exactly one result or error",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_and_authorization_boundaries() {
        BrowserConfig::default().validate().unwrap();
        assert!(
            BrowserConfig {
                max_in_flight: 0,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            BrowserConfig {
                max_response_bytes: 16 * 1024 * 1024,
                max_in_flight: 128,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        for method in [
            "quai_sendRawTransaction",
            "quai_sendTransaction",
            "quai_requestAccounts",
            "personal_sign",
            "quai_signTypedData_v4",
            "wallet_switchEthereumChain",
        ] {
            assert!(!read_method(method));
        }
        assert!(read_method("quai_getHeaderByNumber"));
    }
    #[test]
    fn bounded_encoding_and_strict_response_envelopes() {
        assert!(matches!(
            encode(&serde_json::json!(["abcdef"]), 3),
            Err(RpcError::RequestTooLarge)
        ));
        assert_eq!(
            decode_response(br#"{"jsonrpc":"2.0","id":1,"result":null}"#, 1).unwrap(),
            Value::Null
        );
        for value in [
            br#"{"jsonrpc":"2.0","id":2,"result":null}"#.as_slice(),
            br#"{"jsonrpc":"2.0","id":1,"id":1,"result":null}"#,
            br#"{"jsonrpc":"2.0","id":1,"result":null,"error":null}"#,
        ] {
            assert!(decode_response(value, 1).is_err());
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod storage;
#[cfg(target_arch = "wasm32")]
pub use storage::{
    BrowserSnapshot, BrowserSnapshotStore, BrowserSnapshotUpdate, BrowserStorageScope,
    compare_exchange_snapshots,
};

/// Browser WebSocket request and notification resource limits.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct BrowserSocketConfig {
    /// Request size, response size, deadline and shared concurrency limits.
    pub rpc: BrowserConfig,
    /// Active plus pending subscriptions, 1..=128.
    pub max_subscriptions: usize,
    /// Queued notifications per subscription, 1..=1024.
    pub max_notifications: usize,
    /// Total queued UTF-8 notification bytes across the session, at most 128 MiB.
    pub max_queued_bytes: usize,
}
impl BrowserSocketConfig {
    /// Replace `rpc`.
    pub const fn with_rpc(mut self, rpc: BrowserConfig) -> Self {
        self.rpc = rpc;
        self
    }
    /// Replace `max_subscriptions`.
    pub const fn with_max_subscriptions(mut self, max_subscriptions: usize) -> Self {
        self.max_subscriptions = max_subscriptions;
        self
    }
    /// Replace `max_notifications`.
    pub const fn with_max_notifications(mut self, max_notifications: usize) -> Self {
        self.max_notifications = max_notifications;
        self
    }
    /// Replace `max_queued_bytes`.
    pub const fn with_max_queued_bytes(mut self, max_queued_bytes: usize) -> Self {
        self.max_queued_bytes = max_queued_bytes;
        self
    }
}
impl Default for BrowserSocketConfig {
    fn default() -> Self {
        Self {
            rpc: BrowserConfig::default(),
            max_subscriptions: 16,
            max_notifications: 64,
            max_queued_bytes: 8 * 1024 * 1024,
        }
    }
}
impl BrowserSocketConfig {
    /// Validate without opening a socket or accessing browser globals.
    pub fn validate(&self) -> Result<(), BrowserError> {
        self.rpc.validate()?;
        if self
            .rpc
            .max_request_bytes
            .checked_mul(self.rpc.max_in_flight)
            .is_none_or(|n| n > 128 * 1024 * 1024)
            || self
                .rpc
                .max_response_bytes
                .checked_mul(self.max_subscriptions)
                .is_none_or(|n| n > 128 * 1024 * 1024)
            || !(1..=128).contains(&self.max_subscriptions)
            || !(1..=1024).contains(&self.max_notifications)
            || !(1..=128 * 1024 * 1024).contains(&self.max_queued_bytes)
        {
            return Err(BrowserError::InvalidConfig);
        }
        Ok(())
    }
}
#[cfg(target_arch = "wasm32")]
mod code_wait;
#[cfg(target_arch = "wasm32")]
pub use code_wait::wait_for_contract_code;
#[cfg(target_arch = "wasm32")]
mod transaction_wait;
#[cfg(target_arch = "wasm32")]
pub use transaction_wait::{BrowserTransactionWaitError, wait_for_transaction};
#[cfg(target_arch = "wasm32")]
mod account_wait;
#[cfg(target_arch = "wasm32")]
mod receipt_wait;
#[cfg(target_arch = "wasm32")]
pub use account_wait::{BrowserAccountWaitError, wait_for_account_transaction};
#[cfg(target_arch = "wasm32")]
mod socket;
#[cfg(target_arch = "wasm32")]
pub use receipt_wait::{BrowserReceiptWaitError, wait_for_receipt};
#[cfg(target_arch = "wasm32")]
pub use socket::{BrowserSubscription, BrowserWebSocketTransport};

/// Explicit browser transaction-wait limits. Milliseconds must be positive and fit a
/// browser timer; polling attempts are independently bounded. No implicit defaults.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct BrowserWaitConfig {
    /// Required positive observed depth, including the containing block.
    pub confirmations: u64,
    /// Overall observable monotonic timeout, including RPCs and polling delays.
    pub timeout_ms: u32,
    /// Delay between incomplete observations, no longer than the timeout.
    pub poll_interval_ms: u32,
    /// At most 100,000 completed observations; zero is invalid.
    pub max_polls: u32,
}
impl BrowserWaitConfig {
    /// Every limit is required: there is deliberately no default.
    pub const fn new(
        confirmations: u64,
        timeout_ms: u32,
        poll_interval_ms: u32,
        max_polls: u32,
    ) -> Self {
        Self {
            confirmations,
            timeout_ms,
            poll_interval_ms,
            max_polls,
        }
    }
    /// Replace `confirmations`.
    pub const fn with_confirmations(mut self, confirmations: u64) -> Self {
        self.confirmations = confirmations;
        self
    }
    /// Replace `timeout_ms`.
    pub const fn with_timeout_ms(mut self, timeout_ms: u32) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }
    /// Replace `poll_interval_ms`.
    pub const fn with_poll_interval_ms(mut self, poll_interval_ms: u32) -> Self {
        self.poll_interval_ms = poll_interval_ms;
        self
    }
    /// Replace `max_polls`.
    pub const fn with_max_polls(mut self, max_polls: u32) -> Self {
        self.max_polls = max_polls;
        self
    }
}
impl BrowserWaitConfig {
    /// Validate before allocating timers or performing provider I/O.
    pub fn validate(&self) -> Result<(), BrowserError> {
        if self.confirmations == 0
            || self.timeout_ms == 0
            || self.timeout_ms > i32::MAX as u32
            || self.poll_interval_ms == 0
            || self.poll_interval_ms > self.timeout_ms
            || !(1..=100_000).contains(&self.max_polls)
        {
            return Err(BrowserError::InvalidConfig);
        }
        Ok(())
    }
}

#[cfg(test)]
mod socket_config_tests {
    use super::*;
    #[test]
    fn socket_limits_bound_outgoing_buffers_and_notification_state() {
        let mut config = BrowserSocketConfig::default();
        config.validate().unwrap();
        config.rpc.max_request_bytes = 16 * 1024 * 1024;
        config.rpc.max_response_bytes = 1;
        config.rpc.max_in_flight = 128;
        assert!(config.validate().is_err());
        config.rpc.max_in_flight = 8;
        config.validate().unwrap();
        for (subscriptions, notifications, bytes) in [
            (0, 1, 1),
            (129, 1, 1),
            (1, 0, 1),
            (1, 1025, 1),
            (1, 1, 0),
            (1, 1, usize::MAX),
        ] {
            config.max_subscriptions = subscriptions;
            config.max_notifications = notifications;
            config.max_queued_bytes = bytes;
            assert!(config.validate().is_err());
        }
    }
}
