//! Chain-guard bracketing on batched reads.
//!
//! A read sends `[quai_chainId, method, quai_chainId]` as one batch where the
//! transport supports batching, instead of a sequential guard then call. These
//! tests pin the two properties that makes load bearing: the round-trip count,
//! and that either guard failing rejects the read rather than returning a
//! payload the guard did not cover.

use quai_primitives::Zone;
use quai_provider::{Provider, ProviderError};
use quai_rpc::{BatchResult, Endpoint, Routing, RpcError, Transport, U256};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering::SeqCst},
};

const URL: &str = "http://127.0.0.1:9200/rpc?fixture=public";
const CHAIN: u64 = 9;

/// Batching transport that returns a caller-chosen triple.
#[derive(Clone)]
struct Bracketed {
    leading: Value,
    payload: Result<Value, RpcError>,
    trailing: Value,
    /// Extra responses to append, to exercise the count check.
    extra: usize,
    batches: Arc<AtomicUsize>,
    singles: Arc<AtomicUsize>,
}

impl Bracketed {
    fn new(leading: u64, trailing: u64) -> Self {
        Self {
            leading: json!(format!("{leading:#x}")),
            payload: Ok(json!("0x2a")),
            trailing: json!(format!("{trailing:#x}")),
            extra: 0,
            batches: Arc::default(),
            singles: Arc::default(),
        }
    }
}

impl Transport for Bracketed {
    async fn request(&self, _: &Endpoint, _: &str, _: Value) -> Result<Value, RpcError> {
        self.singles.fetch_add(1, SeqCst);
        panic!("a batching transport must not fall back to sequential requests")
    }
    async fn request_batch(
        &self,
        _: &Endpoint,
        requests: Vec<(&str, Value)>,
    ) -> Option<BatchResult> {
        self.batches.fetch_add(1, SeqCst);
        // The guard must bracket the call, not merely precede it.
        assert_eq!(requests.len(), 3, "read must send exactly three calls");
        assert_eq!(requests[0].0, "quai_chainId");
        assert_eq!(requests[1].0, "quai_blockNumber");
        assert_eq!(requests[2].0, "quai_chainId");
        let mut responses = vec![
            Ok(self.leading.clone()),
            self.payload.clone(),
            Ok(self.trailing.clone()),
        ];
        for _ in 0..self.extra {
            responses.push(Ok(json!("0x0")));
        }
        Some(Ok(responses))
    }
}

fn provider(transport: Bracketed) -> Provider<Bracketed> {
    Provider::new(
        transport,
        Routing::direct(URL, Zone::Cyprus1.into()).unwrap(),
        U256::from(CHAIN),
    )
}

#[tokio::test]
async fn a_batched_read_costs_one_round_trip_and_carries_both_guards() {
    let transport = Bracketed::new(CHAIN, CHAIN);
    let batches = transport.batches.clone();
    let singles = transport.singles.clone();
    let value = provider(transport).block_number(Zone::Cyprus1.into()).await;
    assert_eq!(value.unwrap(), U256::from(0x2au64));
    // The point of the change: one round trip, not a guard followed by a call.
    assert_eq!(batches.load(SeqCst), 1);
    assert_eq!(singles.load(SeqCst), 0);
}

#[tokio::test]
async fn a_leading_guard_mismatch_rejects_the_read() {
    let result = provider(Bracketed::new(0xa, CHAIN))
        .block_number(Zone::Cyprus1.into())
        .await;
    assert!(
        matches!(result, Err(ProviderError::ChainMismatch { .. })),
        "expected ChainMismatch, got {result:?}"
    );
}

#[tokio::test]
async fn a_trailing_guard_mismatch_rejects_the_read() {
    // The backend-swap case: the endpoint answered the guard correctly, served
    // the payload, then reported a different chain. The payload must not be
    // returned, since it is no longer covered by a passing guard.
    let result = provider(Bracketed::new(CHAIN, 0xa))
        .block_number(Zone::Cyprus1.into())
        .await;
    assert!(
        matches!(result, Err(ProviderError::ChainMismatch { .. })),
        "expected ChainMismatch, got {result:?}"
    );
}

#[tokio::test]
async fn a_payload_error_surfaces_rather_than_being_masked_by_the_guards() {
    let mut transport = Bracketed::new(CHAIN, CHAIN);
    transport.payload = Err(RpcError::Timeout);
    let result = provider(transport).block_number(Zone::Cyprus1.into()).await;
    assert!(
        matches!(result, Err(ProviderError::Rpc(RpcError::Timeout))),
        "expected the payload error, got {result:?}"
    );
}

#[tokio::test]
async fn a_short_or_long_batch_response_is_rejected() {
    let mut transport = Bracketed::new(CHAIN, CHAIN);
    transport.extra = 1;
    let result = provider(transport).block_number(Zone::Cyprus1.into()).await;
    assert!(
        matches!(result, Err(ProviderError::InvalidResult(_))),
        "expected InvalidResult for a four-response batch, got {result:?}"
    );
}

/// A transport without batching must still be guarded, sequentially.
#[derive(Clone, Default)]
struct Sequential {
    calls: Arc<AtomicUsize>,
    chain: Arc<AtomicUsize>,
}

impl Transport for Sequential {
    async fn request(&self, _: &Endpoint, method: &str, _: Value) -> Result<Value, RpcError> {
        self.calls.fetch_add(1, SeqCst);
        if method == "quai_chainId" {
            self.chain.fetch_add(1, SeqCst);
            return Ok(json!("0x9"));
        }
        Ok(json!("0x2a"))
    }
    // request_batch is not implemented: the default returns None, having sent
    // nothing, which is what makes the fallback safe.
}

#[tokio::test]
async fn a_transport_without_batching_falls_back_to_a_sequential_guard() {
    let transport = Sequential::default();
    let calls = transport.calls.clone();
    let chain = transport.chain.clone();
    let provider = Provider::new(
        transport,
        Routing::direct(URL, Zone::Cyprus1.into()).unwrap(),
        U256::from(CHAIN),
    );
    assert_eq!(
        provider.block_number(Zone::Cyprus1.into()).await.unwrap(),
        U256::from(0x2au64)
    );
    // Guard then call: two round trips, and the guard still ran.
    assert_eq!(calls.load(SeqCst), 2);
    assert_eq!(chain.load(SeqCst), 1);
}
