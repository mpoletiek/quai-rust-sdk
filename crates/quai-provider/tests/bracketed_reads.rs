//! Chain-guard batching on reads.
//!
//! A read sends `[quai_chainId, method]` as one batch where the transport
//! supports batching, instead of a sequential guard then call. These tests pin
//! the properties that make that load bearing: the round-trip count, one guard
//! and no more, and that a failing guard rejects the read rather than returning
//! a payload it did not cover.
//!
//! There is deliberately no trailing guard. A batch is one request that one
//! backend answers, so nothing changes chains between its elements, and a
//! gateway that splits a batch across backends can route the payload away from
//! a guard at either end, so a second guard would not cover that case either.

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

/// Batching transport that returns a caller-chosen guard and payload.
#[derive(Clone)]
struct Guarded {
    leading: Value,
    payload: Result<Value, RpcError>,
    /// Extra responses to append, to exercise the count check.
    extra: usize,
    batches: Arc<AtomicUsize>,
    singles: Arc<AtomicUsize>,
}

impl Guarded {
    fn new(leading: u64) -> Self {
        Self {
            leading: json!(format!("{leading:#x}")),
            payload: Ok(json!("0x2a")),
            extra: 0,
            batches: Arc::default(),
            singles: Arc::default(),
        }
    }
}

impl Transport for Guarded {
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
        let methods: Vec<_> = requests.iter().map(|(method, _)| *method).collect();
        assert_eq!(
            methods,
            ["quai_chainId", "quai_blockNumber"],
            "a read sends exactly one guard, ahead of its call"
        );
        let mut responses = vec![Ok(self.leading.clone()), self.payload.clone()];
        for _ in 0..self.extra {
            responses.push(Ok(json!("0x0")));
        }
        Some(Ok(responses))
    }
}

fn provider(transport: Guarded) -> Provider<Guarded> {
    Provider::new(
        transport,
        Routing::direct(URL, Zone::Cyprus1.into()).unwrap(),
        U256::from(CHAIN),
    )
}

#[tokio::test]
async fn a_batched_read_costs_one_round_trip_and_two_calls() {
    let transport = Guarded::new(CHAIN);
    let batches = transport.batches.clone();
    let singles = transport.singles.clone();
    let value = provider(transport).block_number(Zone::Cyprus1.into()).await;
    assert_eq!(value.unwrap(), U256::from(0x2au64));
    assert_eq!(batches.load(SeqCst), 1);
    assert_eq!(singles.load(SeqCst), 0);
}

#[tokio::test]
async fn a_guard_mismatch_rejects_the_read() {
    let result = provider(Guarded::new(0xa))
        .block_number(Zone::Cyprus1.into())
        .await;
    assert!(
        matches!(result, Err(ProviderError::ChainMismatch { .. })),
        "expected ChainMismatch, got {result:?}"
    );
}

#[tokio::test]
async fn a_payload_error_surfaces_rather_than_being_masked_by_the_guard() {
    let mut transport = Guarded::new(CHAIN);
    transport.payload = Err(RpcError::Timeout);
    let result = provider(transport).block_number(Zone::Cyprus1.into()).await;
    assert!(
        matches!(result, Err(ProviderError::Rpc(RpcError::Timeout))),
        "expected the payload error, got {result:?}"
    );
}

#[tokio::test]
async fn a_long_batch_response_is_rejected() {
    let mut transport = Guarded::new(CHAIN);
    transport.extra = 1;
    let result = provider(transport).block_number(Zone::Cyprus1.into()).await;
    assert!(
        matches!(result, Err(ProviderError::InvalidResult(_))),
        "expected InvalidResult for a three-response batch, got {result:?}"
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

// ---------------------------------------------------------------------------
// Adaptive outpoint paging.
// ---------------------------------------------------------------------------

use quai_primitives::QiAddress;
use std::sync::Mutex;

/// Batching transport that rejects any page larger than `limit` with
/// ResponseTooLarge, recording the page sizes it was asked for.
#[derive(Clone)]
struct Paged {
    limit: usize,
    /// Payload calls per batch, in order, excluding the chain guard.
    pages: Arc<Mutex<Vec<usize>>>,
    /// Fail every page with this instead, to prove other errors are not retried.
    always: Option<RpcError>,
}

impl Transport for Paged {
    async fn request(&self, _: &Endpoint, _: &str, _: Value) -> Result<Value, RpcError> {
        panic!("a batching transport must not fall back to sequential requests")
    }
    async fn request_batch(
        &self,
        _: &Endpoint,
        requests: Vec<(&str, Value)>,
    ) -> Option<BatchResult> {
        let payload = requests.len() - 1;
        self.pages.lock().unwrap().push(payload);
        if let Some(error) = self.always.clone() {
            return Some(Err(error));
        }
        if payload > self.limit {
            // The batch was sent; the response exceeded the cap.
            return Some(Err(RpcError::ResponseTooLarge));
        }
        let mut responses = vec![Ok(json!(format!("{CHAIN:#x}")))];
        responses.extend((0..payload).map(|_| Ok(json!([]))));
        Some(Ok(responses))
    }
}

fn qi_addresses(count: usize) -> Vec<QiAddress> {
    (0..count)
        .map(|i| {
            let mut bytes = [0u8; 20];
            bytes[0] = Zone::Cyprus1.byte();
            bytes[1] = 0x80;
            bytes[18..20].copy_from_slice(&(i as u16).to_be_bytes());
            QiAddress::try_from(bytes).expect("valid Cyprus-1 Qi address")
        })
        .collect()
}

#[tokio::test]
async fn an_oversize_page_halves_and_the_working_size_is_remembered() {
    // The response cap cannot be predicted: AddressOutpoint carries uninterpreted
    // node extensions, so the node decides the per-row size. Adapt downward
    // rather than fixing a conservative constant.
    let pages = Arc::new(Mutex::new(Vec::new()));
    let transport = Paged {
        limit: 30,
        pages: pages.clone(),
        always: None,
    };
    let provider = Provider::new(
        transport,
        Routing::direct(URL, Zone::Cyprus1.into()).unwrap(),
        U256::from(CHAIN),
    );
    let addresses = qi_addresses(140);
    let result = provider.outpoints_many(&addresses).await.unwrap();
    assert_eq!(result.len(), 140);

    let observed = pages.lock().unwrap().clone();
    // The exact sequence, not a bound. Asserting only "every later page is <= 15"
    // plus a sum would also pass an implementation that kept decaying to 1 and
    // issued 121 pages instead of 13 -- the precise opposite of "remembered".
    // 140 addresses at a working size of 15 is nine pages, the last of size 5.
    assert_eq!(
        observed,
        vec![127, 63, 31, 15, 15, 15, 15, 15, 15, 15, 15, 15, 5],
        "expected three probes then a stable working size"
    );
    // Every address is queried exactly once despite the retries.
    assert_eq!(observed[3..].iter().sum::<usize>(), 140);
}

#[tokio::test]
async fn a_single_address_over_the_cap_is_a_real_error_and_does_not_loop() {
    let pages = Arc::new(Mutex::new(Vec::new()));
    let provider = Provider::new(
        Paged {
            limit: 0,
            pages: pages.clone(),
            always: None,
        },
        Routing::direct(URL, Zone::Cyprus1.into()).unwrap(),
        U256::from(CHAIN),
    );
    let result = provider.outpoints_many(&qi_addresses(4)).await;
    assert!(
        matches!(result, Err(ProviderError::Rpc(RpcError::ResponseTooLarge))),
        "expected the error to surface, got {result:?}"
    );
    // Halving terminates at one address rather than spinning.
    let observed = pages.lock().unwrap().clone();
    assert_eq!(*observed.last().unwrap(), 1);
    assert!(observed.len() <= 10, "bounded probing: {observed:?}");
}

#[tokio::test]
async fn any_error_other_than_an_oversize_response_is_not_retried() {
    // Replaying a read is only safe because the failure is deterministic and
    // specific. A timeout says nothing about page size and must propagate.
    let pages = Arc::new(Mutex::new(Vec::new()));
    let provider = Provider::new(
        Paged {
            limit: usize::MAX,
            pages: pages.clone(),
            always: Some(RpcError::Timeout),
        },
        Routing::direct(URL, Zone::Cyprus1.into()).unwrap(),
        U256::from(CHAIN),
    );
    let result = provider.outpoints_many(&qi_addresses(140)).await;
    assert!(matches!(result, Err(ProviderError::Rpc(RpcError::Timeout))));
    assert_eq!(
        pages.lock().unwrap().len(),
        1,
        "a non-size error must not trigger a second attempt"
    );
}

/// Non-batching transport whose per-address responses are always oversize.
///
/// `request_batch` is not implemented, so the default returns None having sent
/// nothing and the provider falls back to single reads.
#[derive(Clone, Default)]
struct OversizeSingles {
    reads: Arc<AtomicUsize>,
}

impl Transport for OversizeSingles {
    async fn request(&self, _: &Endpoint, method: &str, _: Value) -> Result<Value, RpcError> {
        if method == "quai_chainId" {
            return Ok(json!(format!("{CHAIN:#x}")));
        }
        self.reads.fetch_add(1, SeqCst);
        Err(RpcError::ResponseTooLarge)
    }
}

#[tokio::test]
async fn the_page_is_not_halved_when_the_transport_does_not_batch() {
    // Page size cannot influence any individual response on the fallback path,
    // because each address is its own request. An oversize response there means
    // one address's own set exceeds the cap, so halving is provably futile: it
    // would re-read every prefix address at every level and surface the same
    // error. Without this rule the change would be strictly worse than the fixed
    // page it replaced -- roughly 750 reads instead of at most 127.
    let transport = OversizeSingles::default();
    let reads = transport.reads.clone();
    let provider = Provider::new(
        transport,
        Routing::direct(URL, Zone::Cyprus1.into()).unwrap(),
        U256::from(CHAIN),
    );
    let result = provider.outpoints_many(&qi_addresses(140)).await;
    assert!(
        matches!(result, Err(ProviderError::Rpc(RpcError::ResponseTooLarge))),
        "expected the error to surface, got {result:?}"
    );
    // `buffered(4)` has up to four reads in flight when the first error returns,
    // so the bound is the concurrency window, not the page and not a halving
    // chain over it.
    assert!(
        reads.load(SeqCst) <= 4,
        "the fallback must not retry: {} reads",
        reads.load(SeqCst)
    );
}
