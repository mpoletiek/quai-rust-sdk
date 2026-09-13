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
    *mock.missing.lock().unwrap() = Some(2);
    assert!(matches!(
        tracker.poll(&provider).await,
        Err(ProviderError::ReplayHistoryUnavailable)
    ));
    assert_eq!(tracker.checkpoint().number, 0);
    *mock.missing.lock().unwrap() = None;
    for _ in 0..3 {
        tracker.poll(&provider).await.unwrap();
    }
    let before = tracker.checkpoint();
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
        Err(ProviderError::InvalidResult(_))
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
        HeadFollowPolicy {
            max_connect_attempts: 2,
            retry_delay: Duration::from_millis(5),
            idle_poll_interval: Duration::from_millis(10),
        }
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
