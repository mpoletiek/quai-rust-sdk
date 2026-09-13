//! Wallet application of canonical head replay with real SQLite rollback/custody.
#![cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
use quai_sdk::primitives::{Hash32, QuaiAddress};
use quai_sdk::provider::{BlockReference, HeadTracker};
use quai_sdk::recovery::reconcile_head_replay;
use quai_sdk::rpc::{RpcError, Transport};
use quai_sdk::wallet::storage::{
    Checkpoint, NetworkScope, PublicAddress, ReservationId, ReservationState, SqliteStore,
};
use quai_sdk::{Endpoint, Provider, Routing, U256, Zone};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
fn hash(n: u64) -> Hash32 {
    let mut b = [0; 32];
    b[24..].copy_from_slice(&(n + 1).to_be_bytes());
    Hash32::from_bytes(b)
}
fn scope() -> NetworkScope {
    NetworkScope {
        chain_id: U256::from(9),
        genesis: hash(0),
        zone: Zone::Cyprus1,
    }
}
#[derive(Clone)]
struct Mock {
    chain: Arc<Mutex<Vec<Hash32>>>,
    missing: Arc<Mutex<Option<usize>>>,
    mutate: Arc<Mutex<Option<PathBuf>>>,
}
impl Transport for Mock {
    async fn request(&self, _: &Endpoint, method: &str, params: Value) -> Result<Value, RpcError> {
        if method == "quai_chainId" {
            return Ok(json!("0x9"));
        }
        assert_eq!(method, "quai_getHeaderByNumber");
        if params[0] == "latest"
            && let Some(path) = self.mutate.lock().unwrap().take()
        {
            let mut other = SqliteStore::open(path, scope()).unwrap();
            other
                .invalidate_snapshot(other.observation_generation().unwrap())
                .unwrap();
        }
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
            json!({"woHeader":{"hash":chain[n].to_string(),"number":format!("0x{n:x}"),"parentHash":if n==0{Hash32::ZERO}else{chain[n-1]}.to_string(),"location":if n==0{"0x"}else{"0x0000"},"primeTerminusNumber":"0x1"},"gasLimit":"0x100000","stateLimit":"0x100000"}),
        )
    }
}
struct Database(PathBuf);
impl Database {
    fn new() -> Self {
        Self::at_time(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        )
    }
    fn at_time(timestamp: u128) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "quai-replay-{}-{}-{}.sqlite",
            std::process::id(),
            timestamp,
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        ));
        // Wall-clock precision differs across platforms; parallel tests must
        // never share or delete a database even when their timestamps match.
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        Self(path)
    }
    fn open(&self) -> SqliteStore {
        SqliteStore::open(&self.0, scope()).unwrap()
    }
}
#[test]
fn replay_test_databases_are_isolated_even_with_identical_clock_values() {
    let first = Database::at_time(0);
    let second = Database::at_time(0);
    assert_ne!(first.0, second.0);
    let mut store = first.open();
    store.invalidate_snapshot(0).unwrap();
    assert_eq!(second.open().observation_generation().unwrap(), 0);
    drop(store);
    drop(first);
    assert_eq!(second.open().observation_generation().unwrap(), 0);
}
impl Drop for Database {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
        }
    }
}
fn setup() -> (Mock, Provider<Mock>, HeadTracker) {
    let mock = Mock {
        chain: Arc::new(Mutex::new((0..=6).map(hash).collect())),
        missing: Default::default(),
        mutate: Default::default(),
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
        8,
        2,
    )
    .unwrap();
    (mock, provider, tracker)
}
#[tokio::test]
async fn wallet_replay_invalidates_reorg_before_advancing_and_preserves_restart_custody() {
    let db = Database::new();
    let mut store = db.open();
    let (mock, provider, mut tracker) = setup();
    let mut scalar = [0; 32];
    scalar[30..].copy_from_slice(&805u16.to_be_bytes());
    let key = quai_sdk::crypto::SecretKey::from_bytes(&scalar).unwrap();
    let public = PublicAddress::imported(&key.public_key()).unwrap();
    let owner = QuaiAddress::try_from(public.address()).unwrap();
    store.import_metadata(0, &[public]).unwrap();
    let id = ReservationId([1; 16]);
    let nonce = store.reserve_nonce(id, owner, 0).unwrap();
    let signed = quai_sdk::consensus::QuaiTransaction {
        chain_id: scope().chain_id,
        nonce,
        to: Some(owner.address()),
        value: U256::from(1),
        gas_limit: 21000,
        gas_price: U256::from(1),
        data: vec![],
        access_list: vec![],
    }
    .sign(&key)
    .unwrap();
    let tx_hash = signed.hash().unwrap();
    store.commit_signed_quai(id, &signed).unwrap();
    store
        .observe_inclusion(
            id,
            tx_hash,
            Checkpoint {
                height: U256::from(5),
                hash: hash(5),
            },
        )
        .unwrap();
    store
        .compare_exchange_observation(id, tx_hash, 0, None, Some(b"old settlement"))
        .unwrap();
    for end in [2, 4, 6] {
        let page = reconcile_head_replay(&provider, &mut store, &mut tracker)
            .await
            .unwrap();
        assert_eq!(page.heads.checkpoint.number, end);
        assert!(page.invalidation.is_none());
        assert!(page.refresh_required);
    }
    assert!(
        !reconcile_head_replay(&provider, &mut store, &mut tracker)
            .await
            .unwrap()
            .refresh_required
    );
    for n in 3..=6 {
        mock.chain.lock().unwrap()[n] = hash(100 + n as u64);
    }
    let result = reconcile_head_replay(&provider, &mut store, &mut tracker)
        .await
        .unwrap();
    assert_eq!(
        result
            .heads
            .removed
            .iter()
            .map(|b| b.number)
            .collect::<Vec<_>>(),
        [6, 5, 4, 3]
    );
    assert_eq!(result.invalidation.unwrap().inclusions, 1);
    assert_eq!(tracker.checkpoint().hash, hash(104));
    assert_eq!(
        store.reservation(id).unwrap().unwrap().state,
        ReservationState::Submitted
    );
    assert!(
        store
            .observation_cache(id, tx_hash, 0)
            .unwrap()
            .unwrap()
            .payload
            .is_none()
    );
    drop(store);
    let mut store = db.open();
    assert_eq!(
        store.signed_payload(id).unwrap().unwrap(),
        signed.signed_bytes().unwrap()
    );
    assert_eq!(store.reserved_nonce(id).unwrap(), Some((owner, nonce)));
    let result = reconcile_head_replay(&provider, &mut store, &mut tracker)
        .await
        .unwrap();
    assert!(result.heads.caught_up);
}
#[tokio::test]
async fn wallet_replay_cursor_survives_missing_history_generation_race_and_wrong_scope() {
    let db = Database::new();
    let mut store = db.open();
    let (mock, provider, mut tracker) = setup();
    for _ in 0..3 {
        reconcile_head_replay(&provider, &mut store, &mut tracker)
            .await
            .unwrap();
    }
    let before = tracker.checkpoint();
    let generation = store.observation_generation().unwrap();
    for n in 3..=6 {
        mock.chain.lock().unwrap()[n] = hash(100 + n as u64);
    }
    *mock.missing.lock().unwrap() = Some(4);
    assert!(
        reconcile_head_replay(&provider, &mut store, &mut tracker)
            .await
            .is_err()
    );
    assert_eq!(tracker.checkpoint(), before);
    assert_eq!(store.observation_generation().unwrap(), generation);
    *mock.missing.lock().unwrap() = None;
    *mock.mutate.lock().unwrap() = Some(db.0.clone());
    assert!(
        reconcile_head_replay(&provider, &mut store, &mut tracker)
            .await
            .is_err()
    );
    assert_eq!(tracker.checkpoint(), before);
    assert_eq!(store.observation_generation().unwrap(), generation + 1);
    assert!(
        reconcile_head_replay(&provider, &mut store, &mut tracker)
            .await
            .unwrap()
            .invalidation
            .is_some()
    );
    let mut wrong = HeadTracker::new(
        Zone::Cyprus2,
        hash(0),
        BlockReference {
            number: 0,
            hash: hash(0),
        },
        8,
        2,
    )
    .unwrap();
    let before = wrong.checkpoint();
    assert!(
        reconcile_head_replay(&provider, &mut store, &mut wrong)
            .await
            .is_err()
    );
    assert_eq!(wrong.checkpoint(), before);
}
