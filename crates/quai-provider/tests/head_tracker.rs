//! Missed-head replay and reorg tests against explicit canonical numbered history.
use quai_primitives::{Hash32, Zone};
use quai_provider::{BlockReference, HeadTracker, Provider, ProviderError};
use quai_rpc::{Endpoint, Routing, RpcError, Transport, U256};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
fn hash(n: u64) -> Hash32 {
    let mut b = [0; 32];
    b[24..].copy_from_slice(&(n + 1).to_be_bytes());
    Hash32::from_bytes(b)
}
#[derive(Clone)]
struct Mock {
    chain: Arc<Mutex<Vec<Hash32>>>,
    missing: Arc<Mutex<Option<usize>>>,
}
impl Transport for Mock {
    async fn request(&self, _: &Endpoint, method: &str, params: Value) -> Result<Value, RpcError> {
        if method == "quai_chainId" {
            return Ok(json!("0x9"));
        }
        assert_eq!(method, "quai_getHeaderByNumber");
        let chain = self.chain.lock().unwrap();
        let n = if params[0] == "latest" {
            chain.len() - 1
        } else {
            usize::from_str_radix(params[0].as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
        };
        if *self.missing.lock().unwrap() == Some(n) || n >= chain.len() {
            return Ok(Value::Null);
        }
        Ok(
            json!({"woHeader":{"hash":chain[n].to_string(),"number":format!("0x{n:x}"),"parentHash":if n==0 {Hash32::ZERO}else{chain[n-1]}.to_string(),"location":if n==0{"0x"}else{"0x0000"},"primeTerminusNumber":"0x1"},"gasLimit":"0x100000","stateLimit":"0x100000"}),
        )
    }
}
fn setup(retain: usize) -> (Mock, Provider<Mock>, HeadTracker) {
    let mock = Mock {
        chain: Arc::new(Mutex::new((0..=6).map(hash).collect())),
        missing: Default::default(),
    };
    let provider = Provider::new(
        mock.clone(),
        Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
        U256::from(9),
    );
    let tracker = HeadTracker::new(
        Zone::Cyprus1,
        hash(0),
        BlockReference {
            number: 0,
            hash: hash(0),
        },
        retain,
        2,
    )
    .unwrap();
    (mock, provider, tracker)
}
#[tokio::test]
async fn paginates_missed_heads_and_replays_reorganization_in_application_order() {
    let (mock, provider, mut tracker) = setup(8);
    for end in [2, 4, 6] {
        let page = tracker.poll(&provider).await.unwrap();
        assert!(page.removed.is_empty());
        assert_eq!(page.added.len(), 2);
        assert_eq!(page.checkpoint.number, end);
        assert_eq!(page.caught_up, end == 6);
    }
    assert!(tracker.poll(&provider).await.unwrap().added.is_empty());
    for n in 3..=6 {
        mock.chain.lock().unwrap()[n] = hash(n as u64 + 100);
    }
    let update = tracker.poll(&provider).await.unwrap();
    assert_eq!(
        update.removed.iter().map(|b| b.number).collect::<Vec<_>>(),
        [6, 5, 4, 3]
    );
    assert_eq!(
        update.added.iter().map(|b| b.number).collect::<Vec<_>>(),
        [3, 4]
    );
    assert_eq!(update.checkpoint.hash, hash(104));
    assert!(!update.caught_up);
    assert!(tracker.poll(&provider).await.unwrap().caught_up);
    mock.chain.lock().unwrap().truncate(3);
    let update = tracker.poll(&provider).await.unwrap();
    assert_eq!(update.removed.len(), 4);
    assert!(update.added.is_empty());
    assert!(update.caught_up);
}
#[tokio::test]
async fn pruned_history_deep_reorg_and_foreign_genesis_leave_cursor_unchanged() {
    let (mock, provider, mut tracker) = setup(2);
    // Above a genesis base, which is the trusted hash and never read, a
    // missing block may be history the node lacks: re-anchor explicitly.
    *mock.missing.lock().unwrap() = Some(2);
    assert!(matches!(
        tracker.poll(&provider).await,
        Err(ProviderError::ReplayHistoryUnavailable)
    ));
    assert_eq!(tracker.checkpoint().number, 0);
    *mock.missing.lock().unwrap() = None;
    tracker.poll(&provider).await.unwrap();
    // Above a base this poll read from the node, a missing page block means
    // the chain changed since the tip was read (a shorter branch, or a
    // lagging backend), so the poll is retryable.
    let base = tracker.checkpoint();
    assert_ne!(base.number, 0);
    *mock.missing.lock().unwrap() = Some(base.number as usize + 2);
    assert!(matches!(
        tracker.poll(&provider).await,
        Err(ProviderError::ObservationChanged)
    ));
    assert_eq!(tracker.checkpoint(), base);
    *mock.missing.lock().unwrap() = None;
    for _ in 0..2 {
        tracker.poll(&provider).await.unwrap();
    }
    let before = tracker.checkpoint();
    // An older anchor the node no longer serves, once the newest one is
    // replaced, is lost history: re-anchor explicitly.
    let newest = before.number as usize;
    let saved = mock.chain.lock().unwrap()[newest];
    mock.chain.lock().unwrap()[newest] = hash(newest as u64 + 500);
    *mock.missing.lock().unwrap() = Some(newest - 1);
    assert!(matches!(
        tracker.poll(&provider).await,
        Err(ProviderError::ReplayHistoryUnavailable)
    ));
    assert_eq!(tracker.checkpoint(), before);
    mock.chain.lock().unwrap()[newest] = saved;
    *mock.missing.lock().unwrap() = None;
    for n in 1..=6 {
        mock.chain.lock().unwrap()[n] = hash(n as u64 + 100);
    }
    assert!(matches!(
        tracker.poll(&provider).await,
        Err(ProviderError::ReplayHistoryUnavailable)
    ));
    assert_eq!(tracker.checkpoint(), before);
    mock.chain.lock().unwrap()[0] = hash(900);
    assert!(matches!(
        tracker.poll(&provider).await,
        Err(ProviderError::GenesisMismatch)
    ));
    assert_eq!(tracker.checkpoint(), before);
}

#[cfg(all(feature = "ws", not(target_arch = "wasm32")))]
mod websocket {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use quai_provider::{HeadFollowPolicy, WsHeadFollower};
    use quai_rpc::WsConfig;
    use std::time::Duration;
    use tokio_tungstenite::{accept_async, tungstenite::Message};
    fn policy() -> HeadFollowPolicy {
        HeadFollowPolicy::new(2, Duration::from_millis(5), Duration::from_millis(10))
    }
    #[tokio::test]
    async fn reconnect_replays_missed_blocks_and_quiet_polling_catches_lost_notifications() {
        let (mock, provider, tracker) = setup(8);
        mock.chain.lock().unwrap().truncate(3);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint =
            Endpoint::parse(&format!("ws://{}", listener.local_addr().unwrap())).unwrap();
        let (close_tx, close_rx) = tokio::sync::oneshot::channel();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let mut close_rx = Some(close_rx);
            let mut stop_rx = Some(stop_rx);
            for n in 0..2 {
                let (tcp, _) = listener.accept().await.unwrap();
                let mut socket = accept_async(tcp).await.unwrap();
                let msg = socket.next().await.unwrap().unwrap();
                let request: Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
                assert_eq!(request["method"], "quai_subscribe");
                assert_eq!(request["params"], json!(["newHeads"]));
                socket
                    .send(Message::Text(
                        json!({"jsonrpc":"2.0","id":request["id"],"result":format!("sub{n}")})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
                if n == 0 {
                    close_rx.take().unwrap().await.unwrap();
                    socket.close(None).await.unwrap();
                } else {
                    stop_rx.take().unwrap().await.unwrap();
                }
            }
        });
        let mut follower =
            WsHeadFollower::new(provider, tracker, endpoint, WsConfig::default(), policy())
                .unwrap();
        assert_eq!(follower.next().await.unwrap().checkpoint.number, 2);
        mock.chain.lock().unwrap().extend((3..=5).map(hash));
        close_tx.send(()).unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(3), follower.next())
                .await
                .unwrap()
                .unwrap()
                .checkpoint
                .number,
            4
        );
        assert_eq!(follower.next().await.unwrap().checkpoint.number, 5);
        mock.chain.lock().unwrap().push(hash(6));
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(3), follower.next())
                .await
                .unwrap()
                .unwrap()
                .checkpoint
                .number,
            6
        );
        stop_tx.send(()).unwrap();
        server.await.unwrap();
    }
    #[tokio::test]
    async fn reconnect_attempts_are_bounded_and_do_not_replay_writes() {
        let (_, provider, tracker) = setup(8);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint =
            Endpoint::parse(&format!("ws://{}", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (tcp, _) = listener.accept().await.unwrap();
                let mut socket = accept_async(tcp).await.unwrap();
                let msg = socket.next().await.unwrap().unwrap();
                let request: Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
                assert_eq!(request["method"], "quai_subscribe");
                socket.send(Message::Text(json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32000,"message":"fixture"}}).to_string().into())).await.unwrap();
            }
        });
        let mut follower =
            WsHeadFollower::new(provider, tracker, endpoint, WsConfig::default(), policy())
                .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(3), follower.next())
                .await
                .unwrap()
                .is_err()
        );
        assert_eq!(follower.tracker().checkpoint().number, 0);
        server.await.unwrap();
    }
}

#[tokio::test]
async fn restored_ancestry_replays_reorg_after_restart_and_retains_exact_bounds() {
    let (mock, provider, mut tracker) = setup(8);
    for _ in 0..3 {
        tracker.poll(&provider).await.unwrap();
    }
    let bytes = tracker.export_state();
    let mut restored = HeadTracker::from_state(&bytes, Zone::Cyprus1, hash(0)).unwrap();
    assert_eq!(restored.export_state(), bytes);
    for n in 3..=6 {
        mock.chain.lock().unwrap()[n] = hash(100 + n as u64);
    }
    let update = restored.poll(&provider).await.unwrap();
    assert_eq!(
        update.removed.iter().map(|h| h.number).collect::<Vec<_>>(),
        [6, 5, 4, 3]
    );
    assert_eq!(
        update.added.iter().map(|h| h.number).collect::<Vec<_>>(),
        [3, 4]
    );
    assert!(!update.caught_up);
    assert_eq!(
        restored.export_state(),
        HeadTracker::from_state(&restored.export_state(), Zone::Cyprus1, hash(0))
            .unwrap()
            .export_state()
    );
}
#[tokio::test]
async fn malformed_cursor_bytes_and_repeated_node_hashes_fail_without_advancing() {
    let (mock, provider, mut tracker) = setup(8);
    tracker.poll(&provider).await.unwrap();
    let bytes = tracker.export_state();
    for len in 0..bytes.len() {
        assert!(HeadTracker::from_state(&bytes[..len], Zone::Cyprus1, hash(0)).is_err());
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(HeadTracker::from_state(&trailing, Zone::Cyprus1, hash(0)).is_err());
    for index in [0, 8, 9, 41, 43, 45, 47, 87] {
        let mut corrupt = bytes.clone();
        corrupt[index] = 255;
        assert!(
            HeadTracker::from_state(&corrupt, Zone::Cyprus1, hash(0)).is_err(),
            "{index}"
        );
    }
    for index in [42, 44, 46] {
        let mut corrupt = bytes.clone();
        corrupt[index] = 0;
        assert!(HeadTracker::from_state(&corrupt, Zone::Cyprus1, hash(0)).is_err());
    }
    let mut duplicate = bytes.clone();
    duplicate[95..127].copy_from_slice(&bytes[55..87]);
    assert!(HeadTracker::from_state(&duplicate, Zone::Cyprus1, hash(0)).is_err());
    assert!(HeadTracker::from_state(&bytes, Zone::Cyprus2, hash(0)).is_err());
    assert!(HeadTracker::from_state(&bytes, Zone::Cyprus1, hash(900)).is_err());
    assert!(
        HeadTracker::new(
            Zone::Cyprus1,
            hash(0),
            BlockReference {
                number: 0,
                hash: hash(1)
            },
            8,
            2
        )
        .is_err()
    );
    for bad in [hash(2), Hash32::ZERO] {
        mock.chain.lock().unwrap()[3] = bad;
        assert!(tracker.poll(&provider).await.is_err());
        assert_eq!(tracker.export_state(), bytes);
    }
}
#[test]
fn cursor_maximum_ancestry_is_bounded_before_allocation_and_checks_height_overflow() {
    let (_, _, tracker) = setup(4096);
    let mut bytes = tracker.export_state()[..47].to_vec();
    bytes[45..47].copy_from_slice(&4096u16.to_be_bytes());
    for n in 0..4096u64 {
        bytes.extend_from_slice(&n.to_be_bytes());
        bytes.extend_from_slice(hash(n).bytes());
    }
    assert_eq!(bytes.len(), quai_provider::MAX_HEAD_STATE_BYTES);
    let restored = HeadTracker::from_state(&bytes, Zone::Cyprus1, hash(0)).unwrap();
    assert_eq!(restored.export_state(), bytes);
    bytes.extend_from_slice(&[0; 40]);
    assert!(HeadTracker::from_state(&bytes, Zone::Cyprus1, hash(0)).is_err());
    let mut overflow = tracker.export_state();
    overflow[47..55].copy_from_slice(&u64::MAX.to_be_bytes());
    overflow[45..47].copy_from_slice(&2u16.to_be_bytes());
    overflow.extend_from_slice(&0u64.to_be_bytes());
    overflow.extend_from_slice(hash(1).bytes());
    assert!(HeadTracker::from_state(&overflow, Zone::Cyprus1, hash(0)).is_err());
}

/// The same chain behind a batching transport, counting round trips.
#[derive(Clone)]
struct Batching {
    inner: Mock,
    batches: Arc<Mutex<Vec<usize>>>,
}
impl Transport for Batching {
    async fn request(&self, _: &Endpoint, method: &str, _: Value) -> Result<Value, RpcError> {
        panic!("every read should batch, including guarded single reads: {method}")
    }
    async fn request_batch(
        &self,
        e: &Endpoint,
        requests: Vec<(&str, Value)>,
    ) -> Option<quai_rpc::BatchResult> {
        self.batches.lock().unwrap().push(requests.len());
        let mut out = vec![];
        for (method, params) in requests {
            out.push(self.inner.request(e, method, params).await);
        }
        Some(Ok(out))
    }
}

#[tokio::test]
async fn polls_cost_one_round_trip_idle_and_three_per_page() {
    // Before batching: 5 sequential reads per idle poll and 5 + k per k new
    // blocks, so an hour of missed ~5 s blocks took ~735 round trips.
    let mock = Mock {
        chain: Arc::new(Mutex::new((0..=300).map(hash).collect())),
        missing: Default::default(),
    };
    let batches = Arc::new(Mutex::new(vec![]));
    let provider = Provider::new(
        Batching {
            inner: mock.clone(),
            batches: batches.clone(),
        },
        Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
        U256::from(9),
    );
    let start = BlockReference {
        number: 100,
        hash: hash(100),
    };
    let mut tracker = HeadTracker::new(Zone::Cyprus1, hash(0), start, 512, 256).unwrap();
    // 200 missed blocks in one page: identity batch, two header pages (126 +
    // 74), then the end checks.
    let page = tracker.poll(&provider).await.unwrap();
    assert_eq!(page.added.len(), 200);
    assert!(page.caught_up);
    assert_eq!(batches.lock().unwrap().len(), 4);
    // Idle: one batch.
    batches.lock().unwrap().clear();
    assert!(tracker.poll(&provider).await.unwrap().added.is_empty());
    assert_eq!(batches.lock().unwrap().len(), 1);
    // One new block: identity, the header, the end checks.
    mock.chain.lock().unwrap().push(hash(301));
    batches.lock().unwrap().clear();
    assert_eq!(tracker.poll(&provider).await.unwrap().added.len(), 1);
    assert_eq!(batches.lock().unwrap().len(), 3);
    // A reorg still replays correctly through the batched path.
    for n in 250..=301 {
        mock.chain.lock().unwrap()[n] = hash(n as u64 + 1_000);
    }
    let update = tracker.poll(&provider).await.unwrap();
    assert_eq!(update.removed.len(), 52);
    assert_eq!(update.added.len(), 52);
    assert_eq!(update.checkpoint.hash, hash(1_301));
}

#[test]
fn a_zero_header_hash_is_rejected() {
    use quai_provider::ZoneHeader;
    let parent = hash(1);
    let header = |own: Hash32| json!({"woHeader":{"hash":own.to_string(),"parentHash":parent.to_string(),"number":"0x2","primeTerminusNumber":"0x1","location":"0x0000"},"gasLimit":"0x1","stateLimit":"0x1"});
    assert!(ZoneHeader::try_from(header(hash(2))).is_ok());
    assert!(ZoneHeader::try_from(header(Hash32::ZERO)).is_err());
}

/// The mock chain, failing every header read above height zero.
#[derive(Clone)]
struct FailsAboveGenesis {
    inner: Mock,
    batch: bool,
}
impl Transport for FailsAboveGenesis {
    async fn request(&self, e: &Endpoint, method: &str, params: Value) -> Result<Value, RpcError> {
        if method == "quai_getHeaderByNumber" && params[0] != "0x0" {
            return Err(RpcError::Timeout);
        }
        self.inner.request(e, method, params).await
    }
    async fn request_batch(
        &self,
        e: &Endpoint,
        requests: Vec<(&str, Value)>,
    ) -> Option<quai_rpc::BatchResult> {
        if !self.batch {
            return None;
        }
        let mut out = vec![];
        for (method, params) in requests {
            out.push(self.request(e, method, params).await);
        }
        Some(Ok(out))
    }
}

#[tokio::test]
async fn a_wrong_genesis_decides_before_a_later_header_error() {
    use quai_provider::BlockTag;
    for batch in [true, false] {
        let (mock, _, _) = setup(8);
        let provider = Provider::new(
            FailsAboveGenesis { inner: mock, batch },
            Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
            U256::from(9),
        );
        let blocks = [BlockTag::Latest, BlockTag::Number(U256::from(3))];
        assert!(matches!(
            provider
                .headers_on_network(Zone::Cyprus1, hash(0), &blocks)
                .await,
            Err(ProviderError::Rpc(RpcError::Timeout))
        ));
        assert!(matches!(
            provider
                .headers_on_network(Zone::Cyprus1, hash(99), &blocks)
                .await,
            Ok(None)
        ));
        let start = BlockReference {
            number: 3,
            hash: hash(3),
        };
        let mut tracker = HeadTracker::new(Zone::Cyprus1, hash(99), start, 8, 8).unwrap();
        assert!(matches!(
            tracker.poll(&provider).await,
            Err(ProviderError::GenesisMismatch)
        ));
    }
}
