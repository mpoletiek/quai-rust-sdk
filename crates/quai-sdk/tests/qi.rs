//! Deterministic orchestration tests using real WAL storage and no network transport.
#![cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
use quai_sdk::consensus::{Denomination, OutPoint};
use quai_sdk::provider::RpcData;
use quai_sdk::qi::{QiChangePool, QiError, QiIntent, QiPolicy, QiSession};
use quai_sdk::rpc::{RpcError, Transport};
use quai_sdk::wallet::discovery::Checkpoint;
use quai_sdk::wallet::storage::{
    NetworkScope, PublicAddress, ReservationId, ReservationState, SqliteStore,
};
use quai_sdk::wallet::{CandidateCoin, CoinType, HdWallet, Search, SelectionError};
use quai_sdk::{Endpoint, Provider, Routing, U256, Zone};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU8, AtomicU64, Ordering},
    },
};
const GENESIS: &str = "0x663a73416275109a01aad3a4c29ea9e310aded63c5eea491243b7312ad8cd16b";
const CHECKPOINT: &str = "0x0000000000000000000000000000000000000000000000000000000000000010";
static NEXT: AtomicU64 = AtomicU64::new(0);
#[derive(Clone, Default)]
struct Mock {
    calls: Arc<Mutex<Vec<(String, Value)>>>,
    fees: Arc<Mutex<VecDeque<u64>>>,
    mode: Arc<AtomicU8>,
    send_started: Arc<tokio::sync::Notify>,
    invalidate: Arc<Mutex<Option<(std::path::PathBuf, NetworkScope)>>>,
    outpoints: Arc<Mutex<std::collections::BTreeMap<String, Value>>>,
}
impl Transport for Mock {
    async fn request(&self, _: &Endpoint, method: &str, params: Value) -> Result<Value, RpcError> {
        self.calls
            .lock()
            .unwrap()
            .push((method.to_owned(), params.clone()));
        let mode = self.mode.load(Ordering::SeqCst);
        Ok(match method {
            "quai_chainId" => json!("0x3a98"),
            "quai_getTransactionReceipt" | "quai_getTransactionByHash" => Value::Null,
            "quai_getOutpointsByAddress" => self
                .outpoints
                .lock()
                .unwrap()
                .get(params[0].as_str().unwrap())
                .cloned()
                .unwrap_or(json!([])),
            "quai_getHeaderByNumber" if params[0] == "0x0" => {
                json!({"woHeader":{"hash": if mode == 4 { CHECKPOINT } else { GENESIS }, "number":"0x0","location":"0x","parentHash":format!("0x{}","00".repeat(32))}})
            }
            "quai_getHeaderByNumber" => {
                json!({"woHeader":{"hash":if mode==5 {GENESIS} else {CHECKPOINT}, "number": if params[0]=="latest" && mode==6 {"0x11"} else {"0x10"},"location":"0x0000","parentHash":GENESIS,"primeTerminusNumber": if mode==8 {"0x1ac778"} else {"0x10"}},"baseFeePerGas":"0x1","gasLimit":"0x100000","stateLimit":"0x100000"})
            }
            "quai_getLatestUTXOSetSize" => json!("0x1"),
            "quai_quaiToQi" => json!("0x5"),
            "quai_qiToQuai" => json!("0xffffffffff"),
            "quai_estimateFeeForQi" => {
                if mode == 7 {
                    self.mode.store(6, Ordering::SeqCst);
                }
                if let Some((path, scope)) = self.invalidate.lock().unwrap().take() {
                    let mut other = SqliteStore::open(path, scope).unwrap();
                    let generation = other.snapshot().unwrap().generation;
                    other.invalidate_snapshot(generation).unwrap();
                }
                let mut fees = self.fees.lock().unwrap();
                let fee = if fees.len() > 1 {
                    fees.pop_front().unwrap()
                } else {
                    *fees.front().unwrap_or(&1)
                };
                json!(format!("0x{fee:x}"))
            }
            "quai_sendRawTransaction" => {
                self.send_started.notify_one();
                match mode {
                    1 => return Err(RpcError::Timeout),
                    2 => std::future::pending::<()>().await,
                    3 => return Ok(json!(GENESIS)),
                    _ => (),
                }
                let bytes: RpcData = params[0].as_str().unwrap().parse().unwrap();
                json!(
                    quai_sdk::consensus::SignedQiOperation::decode(bytes.bytes())
                        .unwrap()
                        .hash()
                        .unwrap()
                        .to_string()
                )
            }
            _ => panic!("unexpected method {method}"),
        })
    }
}
struct Environment {
    path: std::path::PathBuf,
    mock: Mock,
    provider: Provider<Mock>,
    wallet: HdWallet,
    store: SqliteStore,
}
impl Drop for Environment {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.path.parent().unwrap());
    }
}
fn setup() -> Environment {
    let wallet = HdWallet::from_seed(&[7; 32], CoinType::Qi).unwrap();
    let account = wallet.account_public(0).unwrap();
    static INDICES: OnceLock<[u32; 2]> = OnceLock::new();
    let indices = INDICES.get_or_init(|| {
        let a = account
            .search(
                false,
                Search {
                    zone: Zone::Cyprus1,
                    start_index: 0,
                    max_attempts: 10000,
                },
                || false,
            )
            .unwrap()
            .address
            .index;
        let b = account
            .search(
                false,
                Search {
                    zone: Zone::Cyprus1,
                    start_index: a + 1,
                    max_attempts: 10000,
                },
                || false,
            )
            .unwrap()
            .address
            .index;
        [a, b]
    });
    let scope = NetworkScope {
        chain_id: U256::from(15000),
        genesis: GENESIS.parse().unwrap(),
        zone: Zone::Cyprus1,
    };
    let directory = std::env::temp_dir().join(format!(
        "quai-qi-session-public-test-{}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("wallet.sqlite");
    let mut store = SqliteStore::open(&path, scope).unwrap();
    let public: Vec<_> = indices
        .iter()
        .map(|index| PublicAddress::derive(&account, false, *index).unwrap())
        .collect();
    store.import_metadata(0, &public).unwrap();
    let mock = Mock::default();
    let provider = Provider::new(
        mock.clone(),
        Routing::direct("http://127.0.0.1:9200/exact", Zone::Cyprus1.into()).unwrap(),
        scope.chain_id,
    );
    let mut env = Environment {
        path,
        mock,
        provider,
        wallet,
        store,
    };
    refresh(&mut env);
    env
}
// This is explicitly a synthetic qualified source fixture, not a production RPC scanner.
fn refresh(env: &mut Environment) {
    let mut snapshot = env.store.snapshot().unwrap();
    snapshot.checkpoint = Some(Checkpoint {
        hash: CHECKPOINT.parse().unwrap(),
        height: U256::from(16),
    });
    snapshot.coins = env
        .store
        .addresses()
        .unwrap()
        .into_iter()
        .filter(|metadata| {
            matches!(
                metadata.origin(),
                quai_sdk::wallet::storage::KeyOrigin::Bip44 { change: false, .. }
            )
        })
        .enumerate()
        .map(|(i, public)| {
            let mut hash = [0u8; 32];
            hash[3] = 0x80;
            hash[31] = i as u8 + 1;
            CandidateCoin {
                outpoint: OutPoint {
                    transaction_hash: quai_sdk::primitives::Hash32::from_bytes(hash),
                    index: 0,
                },
                address: public.address().try_into().unwrap(),
                denomination: Denomination::new(1).unwrap(),
                unlock_height: U256::ZERO,
                expires_at: None,
                reserved: false,
            }
        })
        .collect();
    env.store.replace_snapshot(&snapshot).unwrap();
}
fn pool(env: &mut Environment, count: usize) -> QiChangePool {
    let account = env.wallet.account_public(0).unwrap();
    QiChangePool::allocate(&mut env.store, &account, count, 4000, || false).unwrap()
}
fn intent() -> QiIntent {
    QiIntent {
        amount: U256::from(5),
        destinations: vec![
            "0x0080000000000000000000000000000000000001"
                .parse()
                .unwrap(),
        ],
    }
}
fn policy() -> QiPolicy {
    QiPolicy {
        initial_fee: U256::from(5),
        max_fee: U256::from(5),
        max_inputs: 4,
        max_outputs: 16,
        max_fee_rounds: 4,
        max_snapshot_age: 2,
    }
}
fn id(n: u8) -> ReservationId {
    ReservationId([n; 16])
}
fn count_calls(mock: &Mock, method: &str) -> usize {
    mock.calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(name, _)| name == method)
        .count()
}

#[tokio::test]
async fn converges_exact_payload_and_persists_multi_input_signature_before_submission() {
    let mut env = setup();
    let change = pool(&mut env, 4);
    let allocated: Vec<_> = change
        .addresses()
        .iter()
        .map(|entry| entry.address())
        .collect();
    assert!(env.store.snapshot().unwrap().checkpoint.is_none());
    refresh(&mut env);
    *env.mock.fees.lock().unwrap() = [1, 2, 2].into();
    let mut constraints = policy();
    constraints.initial_fee = U256::ZERO;
    let prepared = QiSession::new(&env.provider, &env.wallet, &mut env.store)
        .unwrap()
        .prepare(id(1), intent(), constraints, change)
        .await
        .unwrap();
    assert_eq!(prepared.fee(), U256::from(2));
    assert_eq!(prepared.recipient_outputs(), 1);
    assert_eq!(prepared.transaction().inputs.len(), 2);
    assert_eq!(prepared.transaction().outputs.len(), 4);
    assert_eq!(prepared.transaction().outputs[1].address, allocated[0]);
    assert_eq!(count_calls(&env.mock, "quai_estimateFeeForQi"), 3);
    assert_eq!(count_calls(&env.mock, "quai_sendRawTransaction"), 0);
    let estimates: Vec<_> = env
        .mock
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(method, _)| method == "quai_estimateFeeForQi")
        .map(|(_, params)| params.clone())
        .collect();
    let final_request = &estimates.last().unwrap()[0];
    assert_eq!(final_request["txIn"].as_array().unwrap().len(), 2);
    for (rpc, input) in final_request["txIn"]
        .as_array()
        .unwrap()
        .iter()
        .zip(&prepared.transaction().inputs)
    {
        assert_eq!(
            rpc["previousOutPoint"]["txHash"],
            input.previous_output.transaction_hash.to_string()
        );
        assert_eq!(
            rpc["pubKey"],
            RpcData::new(input.public_key.to_compressed().to_vec())
                .unwrap()
                .to_hex()
        );
    }
    for (rpc, output) in final_request["txOut"]
        .as_array()
        .unwrap()
        .iter()
        .zip(&prepared.transaction().outputs)
    {
        assert_eq!(rpc["address"], output.address.to_string());
        assert_eq!(
            rpc["denomination"],
            format!("0x{:x}", output.denomination.index())
        );
        assert_eq!(rpc["lock"], "0x0");
    }
    let signed = QiSession::new(&env.provider, &env.wallet, &mut env.store)
        .unwrap()
        .sign(&prepared)
        .unwrap();
    assert_eq!(signed.transaction(), prepared.transaction());
    let persisted = env.store.signed_payload(id(1)).unwrap().unwrap();
    assert_eq!(persisted, signed.signed_bytes().unwrap());
    assert_eq!(
        env.store.reservation(id(1)).unwrap().unwrap().state,
        ReservationState::Signed
    );
    let result = QiSession::new(&env.provider, &env.wallet, &mut env.store)
        .unwrap()
        .broadcast(id(1))
        .await
        .unwrap();
    assert_eq!(result.transaction_hash, signed.hash().unwrap());
    assert_eq!(count_calls(&env.mock, "quai_sendRawTransaction"), 1);
    assert!(env.store.release_unsigned(id(1)).is_err());
}

#[tokio::test]
async fn preallocation_requires_refresh_and_failed_capacity_never_claims_coins() {
    let mut env = setup();
    let change = pool(&mut env, 1);
    let result = QiSession::new(&env.provider, &env.wallet, &mut env.store)
        .unwrap()
        .prepare(id(2), intent(), policy(), change)
        .await;
    assert!(matches!(result, Err(QiError::MissingSnapshot)));
    assert_eq!(count_calls(&env.mock, "quai_chainId"), 0);
    assert!(env.store.reservation(id(2)).unwrap().is_none());
    let cursor = env
        .store
        .next_derivation_index(&env.wallet.account_public(0).unwrap(), true)
        .unwrap()
        .unwrap();
    assert_eq!(cursor, 4000);
    refresh(&mut env);
    let change = pool(&mut env, 0);
    let mut constraints = policy();
    constraints.initial_fee = U256::from(1);
    let result = QiSession::new(&env.provider, &env.wallet, &mut env.store)
        .unwrap()
        .prepare(id(3), intent(), constraints, change)
        .await;
    assert!(matches!(result, Err(QiError::InsufficientChange)));
    assert!(env.store.reservation(id(3)).unwrap().is_none());
    assert_eq!(
        env.store
            .next_derivation_index(&env.wallet.account_public(0).unwrap(), true)
            .unwrap(),
        Some(cursor)
    );
}

#[tokio::test]
async fn fee_budget_and_rounds_fail_before_input_reservation() {
    let mut env = setup();
    *env.mock.fees.lock().unwrap() = [6].into();
    let change = pool(&mut env, 0);
    let result = QiSession::new(&env.provider, &env.wallet, &mut env.store)
        .unwrap()
        .prepare(id(4), intent(), policy(), change)
        .await;
    assert!(matches!(
        result,
        Err(QiError::Selection(SelectionError::FeeBudgetExceeded))
    ));
    assert!(env.store.reservation(id(4)).unwrap().is_none());
    // A first zero-fee exact-spend payload needs no change, but the estimate requires a second round.
    *env.mock.fees.lock().unwrap() = [1].into();
    let change = pool(&mut env, 0);
    let mut constraints = policy();
    constraints.initial_fee = U256::ZERO;
    constraints.max_fee_rounds = 1;
    let result = QiSession::new(&env.provider, &env.wallet, &mut env.store)
        .unwrap()
        .prepare(id(5), intent(), constraints, change)
        .await;
    assert!(matches!(
        result,
        Err(QiError::Selection(SelectionError::FeeDidNotConverge))
    ));
    assert!(env.store.reservation(id(5)).unwrap().is_none());
}

#[tokio::test]
async fn wrong_genesis_wallet_and_noncanonical_checkpoint_are_rejected() {
    let mut env = setup();
    env.mock.mode.store(4, Ordering::SeqCst);
    let change = pool(&mut env, 0);
    assert!(matches!(
        QiSession::new(&env.provider, &env.wallet, &mut env.store)
            .unwrap()
            .prepare(id(6), intent(), policy(), change)
            .await,
        Err(QiError::IdentityMismatch)
    ));
    assert_eq!(count_calls(&env.mock, "quai_estimateFeeForQi"), 0);
    env.mock.mode.store(0, Ordering::SeqCst);
    let change = pool(&mut env, 0);
    let wrong = HdWallet::from_seed(&[8; 32], CoinType::Qi).unwrap();
    assert!(matches!(
        QiSession::new(&env.provider, &wrong, &mut env.store)
            .unwrap()
            .prepare(id(7), intent(), policy(), change)
            .await,
        Err(QiError::IdentityMismatch)
    ));
    assert!(env.store.reservation(id(7)).unwrap().is_none());
    env.mock.mode.store(5, Ordering::SeqCst);
    let change = pool(&mut env, 0);
    assert!(matches!(
        QiSession::new(&env.provider, &env.wallet, &mut env.store)
            .unwrap()
            .prepare(id(8), intent(), policy(), change)
            .await,
        Err(QiError::StaleSnapshot)
    ));
    assert!(env.store.snapshot().unwrap().checkpoint.is_none());
    assert!(env.store.reservation(id(8)).unwrap().is_none());
}

#[tokio::test]
async fn concurrent_snapshot_invalidation_cannot_reserve_stale_selection() {
    let mut env = setup();
    let change = pool(&mut env, 0);
    *env.mock.invalidate.lock().unwrap() = Some((env.path.clone(), env.store.scope()));
    assert!(matches!(
        QiSession::new(&env.provider, &env.wallet, &mut env.store)
            .unwrap()
            .prepare(id(9), intent(), policy(), change)
            .await,
        Err(QiError::Storage(_))
    ));
    assert!(env.store.reservation(id(9)).unwrap().is_none());
}

#[tokio::test]
async fn expired_candidates_and_duplicate_or_input_destinations_cannot_be_signed() {
    let mut env = setup();
    let mut snapshot = env.store.snapshot().unwrap();
    snapshot.coins[0].expires_at = Some(U256::from(17));
    env.store.replace_snapshot(&snapshot).unwrap();
    let change = pool(&mut env, 0);
    assert!(matches!(
        QiSession::new(&env.provider, &env.wallet, &mut env.store)
            .unwrap()
            .prepare(id(10), intent(), policy(), change)
            .await,
        Err(QiError::Selection(SelectionError::InsufficientFunds))
    ));
    refresh(&mut env);
    let change = pool(&mut env, 0);
    let mut duplicate = intent();
    duplicate.destinations.push(duplicate.destinations[0]);
    assert!(matches!(
        QiSession::new(&env.provider, &env.wallet, &mut env.store)
            .unwrap()
            .prepare(id(11), duplicate, policy(), change)
            .await,
        Err(QiError::IdentityMismatch)
    ));
    let address = env.store.snapshot().unwrap().coins[0].address;
    let change = pool(&mut env, 0);
    let mut reused = intent();
    reused.destinations[0] = address;
    assert!(matches!(
        QiSession::new(&env.provider, &env.wallet, &mut env.store)
            .unwrap()
            .prepare(id(12), reused, policy(), change)
            .await,
        Err(QiError::Transaction(_))
    ));
}

#[tokio::test]
async fn ambiguous_send_and_restart_rebroadcast_exact_durable_bytes() {
    let mut env = setup();
    let change = pool(&mut env, 0);
    let prepared = QiSession::new(&env.provider, &env.wallet, &mut env.store)
        .unwrap()
        .prepare(id(13), intent(), policy(), change)
        .await
        .unwrap();
    QiSession::new(&env.provider, &env.wallet, &mut env.store)
        .unwrap()
        .sign(&prepared)
        .unwrap();
    let before = env.store.signed_payload(id(13)).unwrap().unwrap();
    for mode in [1, 3] {
        env.mock.mode.store(mode, Ordering::SeqCst);
        assert!(matches!(
            QiSession::new(&env.provider, &env.wallet, &mut env.store)
                .unwrap()
                .broadcast(id(13))
                .await,
            Err(QiError::Broadcast(_))
        ));
        assert_eq!(
            env.store.reservation(id(13)).unwrap().unwrap().state,
            ReservationState::Submitted
        );
        assert!(env.store.release_unsigned(id(13)).is_err());
    }
    let mut reopened = SqliteStore::open(&env.path, env.store.scope()).unwrap();
    assert_eq!(reopened.signed_payload(id(13)).unwrap().unwrap(), before);
    assert_eq!(reopened.reserved_outpoints(id(13)).unwrap().len(), 2);
    env.mock.mode.store(0, Ordering::SeqCst);
    QiSession::new(&env.provider, &env.wallet, &mut reopened)
        .unwrap()
        .broadcast(id(13))
        .await
        .unwrap();
    let sends: Vec<_> = env
        .mock
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(method, _)| method == "quai_sendRawTransaction")
        .map(|(_, params)| params.clone())
        .collect();
    assert_eq!(sends.len(), 3);
    assert!(sends.iter().all(|params| params == &sends[0]));
}

#[tokio::test]
async fn cancellation_at_send_retains_claims_and_preflight_mismatch_never_submits() {
    let mut env = setup();
    let change = pool(&mut env, 0);
    let prepared = QiSession::new(&env.provider, &env.wallet, &mut env.store)
        .unwrap()
        .prepare(id(14), intent(), policy(), change)
        .await
        .unwrap();
    QiSession::new(&env.provider, &env.wallet, &mut env.store)
        .unwrap()
        .sign(&prepared)
        .unwrap();
    env.mock.mode.store(4, Ordering::SeqCst);
    assert!(matches!(
        QiSession::new(&env.provider, &env.wallet, &mut env.store)
            .unwrap()
            .broadcast(id(14))
            .await,
        Err(QiError::IdentityMismatch)
    ));
    assert_eq!(count_calls(&env.mock, "quai_sendRawTransaction"), 0);
    assert_eq!(
        env.store.reservation(id(14)).unwrap().unwrap().state,
        ReservationState::Signed
    );
    env.mock.mode.store(2, Ordering::SeqCst);
    {
        let mut session = QiSession::new(&env.provider, &env.wallet, &mut env.store).unwrap();
        tokio::select! {
            result=session.broadcast(id(14))=>panic!("send should stay pending: {result:?}"),
            ()=env.mock.send_started.notified()=>(),
        }
    }
    assert_eq!(count_calls(&env.mock, "quai_sendRawTransaction"), 1);
    assert_eq!(
        env.store.reservation(id(14)).unwrap().unwrap().state,
        ReservationState::Submitted
    );
    assert!(env.store.signed_payload(id(14)).unwrap().is_some());
    assert!(env.store.release_unsigned(id(14)).is_err());
    assert!(
        env.store
            .snapshot()
            .unwrap()
            .coins
            .iter()
            .all(|coin| coin.reserved)
    );
}

#[tokio::test]
async fn change_pool_requires_its_persisted_cursor_and_metadata_in_the_same_wallet_store() {
    let mut original = setup();
    let change = pool(&mut original, 1);
    let mut other = setup();
    assert!(matches!(
        QiSession::new(&other.provider, &other.wallet, &mut other.store)
            .unwrap()
            .prepare(id(15), intent(), policy(), change)
            .await,
        Err(QiError::IdentityMismatch)
    ));
    assert_eq!(count_calls(&other.mock, "quai_chainId"), 0);
    assert!(other.store.reservation(id(15)).unwrap().is_none());
}

#[tokio::test]
async fn tip_age_and_expiry_during_estimation_are_rechecked_before_claiming() {
    let mut env = setup();
    env.mock.mode.store(6, Ordering::SeqCst);
    let mut constraints = policy();
    constraints.max_snapshot_age = 0;
    let change = pool(&mut env, 0);
    assert!(matches!(
        QiSession::new(&env.provider, &env.wallet, &mut env.store)
            .unwrap()
            .prepare(id(16), intent(), constraints, change)
            .await,
        Err(QiError::StaleSnapshot)
    ));
    env.mock.mode.store(7, Ordering::SeqCst);
    let mut snapshot = env.store.snapshot().unwrap();
    snapshot.coins[0].expires_at = Some(U256::from(18));
    env.store.replace_snapshot(&snapshot).unwrap();
    let change = pool(&mut env, 0);
    assert!(matches!(
        QiSession::new(&env.provider, &env.wallet, &mut env.store)
            .unwrap()
            .prepare(id(17), intent(), policy(), change)
            .await,
        Err(QiError::StaleSnapshot)
    ));
    assert!(env.store.reservation(id(17)).unwrap().is_none());
}

#[test]
fn allocation_cancellation_burns_reserved_range_and_resource_limits_fail_early() {
    let mut env = setup();
    let account = env.wallet.account_public(0).unwrap();
    let mut checks = 0;
    assert!(
        QiChangePool::allocate(&mut env.store, &account, 1, 4000, || {
            checks += 1;
            checks >= 5
        })
        .is_err()
    );
    assert_eq!(
        env.store.next_derivation_index(&account, true).unwrap(),
        Some(4000)
    );
    assert!(matches!(
        QiChangePool::allocate(&mut env.store, &account, 1025, 1, || false),
        Err(QiError::InvalidPolicy)
    ));
    assert!(matches!(
        QiChangePool::allocate(&mut env.store, &account, 100, 1001, || false),
        Err(QiError::InvalidPolicy)
    ));
    assert_eq!(
        env.store.next_derivation_index(&account, true).unwrap(),
        Some(4000)
    );
}

#[tokio::test]
async fn pool_cannot_cross_stores_even_with_identical_cursor_and_metadata() {
    let mut original = setup();
    let mut other = setup();
    let change = pool(&mut original, 1);
    let own = pool(&mut other, 1);
    refresh(&mut other);
    assert_eq!(change.addresses(), own.addresses());
    assert_eq!(original.store.scope(), other.store.scope());
    assert!(matches!(
        QiSession::new(&other.provider, &other.wallet, &mut other.store)
            .unwrap()
            .prepare(id(40), intent(), policy(), change)
            .await,
        Err(QiError::IdentityMismatch)
    ));
    assert!(other.store.reservation(id(40)).unwrap().is_none());
    QiSession::new(&other.provider, &other.wallet, &mut other.store)
        .unwrap()
        .prepare(id(40), intent(), policy(), own)
        .await
        .unwrap();
}

#[tokio::test]
async fn prepared_qi_cannot_cross_identical_stores_or_reopened_handles() {
    let mut original = setup();
    let mut other = setup();
    let change = pool(&mut original, 0);
    let own = pool(&mut other, 0);
    let prepared = QiSession::new(&original.provider, &original.wallet, &mut original.store)
        .unwrap()
        .prepare(id(41), intent(), policy(), change)
        .await
        .unwrap();
    let own = QiSession::new(&other.provider, &other.wallet, &mut other.store)
        .unwrap()
        .prepare(id(41), intent(), policy(), own)
        .await
        .unwrap();
    assert_eq!(
        original.store.reserved_outpoints(id(41)).unwrap(),
        other.store.reserved_outpoints(id(41)).unwrap()
    );
    assert!(matches!(
        QiSession::new(&other.provider, &other.wallet, &mut other.store)
            .unwrap()
            .sign(&prepared),
        Err(QiError::IdentityMismatch)
    ));
    assert!(other.store.signed_payload(id(41)).unwrap().is_none());
    QiSession::new(&other.provider, &other.wallet, &mut other.store)
        .unwrap()
        .sign(&own)
        .unwrap();
    let mut reopened = SqliteStore::open(&original.path, original.store.scope()).unwrap();
    assert!(matches!(
        QiSession::new(&original.provider, &original.wallet, &mut reopened)
            .unwrap()
            .sign(&prepared),
        Err(QiError::IdentityMismatch)
    ));
    assert_eq!(
        reopened.reservation(id(41)).unwrap().unwrap().state,
        ReservationState::Reserved
    );
}

#[cfg(feature = "payments")]
#[tokio::test]
async fn imported_and_payment_receive_inputs_are_spent_with_hd_inputs() {
    use quai_sdk::payments::{PaymentChannel, PaymentDirection, PrivatePaymentCode};
    use quai_sdk::wallet::qi_keys::QiKeyring;
    let mut env = setup();
    let change = pool(&mut env, 0);
    let mut scalar = [0u8; 32];
    scalar[31] = 130;
    let imported = quai_sdk::crypto::SecretKey::from_bytes(&scalar).unwrap();
    let public = PublicAddress::imported(&imported.public_key()).unwrap();
    let generation = env.store.snapshot().unwrap().generation;
    env.store
        .import_metadata(generation, std::slice::from_ref(&public))
        .unwrap();
    let owner = PrivatePaymentCode::from_seed(&[1; 32], 0).unwrap();
    let peer = PrivatePaymentCode::from_seed(&[2; 32], 0)
        .unwrap()
        .public_code()
        .clone();
    let channel = PaymentChannel::new(&owner, peer.clone());
    env.store
        .import_payment_channel(&owner, &channel, None)
        .unwrap();
    let payment = env
        .store
        .allocate_payment_address(&owner, &peer, PaymentDirection::Receive, 10_000, || false)
        .unwrap();
    refresh(&mut env);
    let mut snapshot = env.store.snapshot().unwrap();
    for (index, address) in [public.address(), payment.found.address.address()]
        .into_iter()
        .enumerate()
    {
        let mut hash = [0; 32];
        hash[3] = 0x80;
        hash[31] = 100 + index as u8;
        snapshot.coins.push(CandidateCoin {
            outpoint: OutPoint {
                transaction_hash: hash.into(),
                index: 0,
            },
            address: address.try_into().unwrap(),
            denomination: Denomination::new(1).unwrap(),
            unlock_height: U256::ZERO,
            expires_at: None,
            reserved: false,
        });
    }
    env.store.replace_snapshot(&snapshot).unwrap();
    let mut keys = QiKeyring::new(Some(&env.wallet)).unwrap();
    keys.import(imported).unwrap();
    assert_eq!(
        keys.load_payment_channel(&env.store, &owner, &peer)
            .unwrap(),
        1
    );
    assert_eq!(
        keys.load_payment_channel(&env.store, &owner, &peer)
            .unwrap(),
        0
    );
    let mut send = intent();
    send.amount = U256::from(15);
    send.destinations.push(
        "0x0080000000000000000000000000000000000002"
            .parse()
            .unwrap(),
    );
    send.destinations.push(
        "0x0080000000000000000000000000000000000003"
            .parse()
            .unwrap(),
    );
    let mut session = QiSession::with_keys(&env.provider, &keys, &mut env.store);
    let prepared = session
        .prepare(id(90), send, policy(), change)
        .await
        .unwrap();
    assert_eq!(prepared.transaction().inputs.len(), 4);
    let signed = session.sign(&prepared).unwrap();
    let expected = signed.hash().unwrap();
    assert_eq!(
        session.broadcast(id(90)).await.unwrap().transaction_hash,
        expected
    );
    drop(keys);
    let mut reopened = SqliteStore::open(&env.path, env.store.scope()).unwrap();
    assert_eq!(
        reopened.signed_payload(id(90)).unwrap().unwrap(),
        signed.signed_bytes().unwrap()
    );
}

#[tokio::test]
async fn specialized_operations_keep_exact_bytes_and_claims_across_restart() {
    use quai_sdk::consensus::{
        ConversionSlippage, QiConversionIntent, QiWrappingIntent, SignedQiOperation,
    };
    use quai_sdk::qi::QiSpecialIntent;
    for wrapping in [false, true] {
        let mut env = setup();
        let change = pool(&mut env, 0);
        refresh(&mut env);
        let destination = "0x0000000000000000000000000000000000000001"
            .parse()
            .unwrap();
        let intent = if wrapping {
            QiSpecialIntent::Wrapping(QiWrappingIntent {
                destination,
                owner_contract: "0x002b2596EcF05C93a31ff916E8b456DF6C77c750"
                    .parse()
                    .unwrap(),
            })
        } else {
            QiSpecialIntent::Conversion(QiConversionIntent {
                destination,
                refund: "0x0080000000000000000000000000000000000001"
                    .parse()
                    .unwrap(),
                slippage: ConversionSlippage::new(100).unwrap(),
            })
        };
        let mut session = QiSession::new(&env.provider, &env.wallet, &mut env.store).unwrap();
        let prepared = session
            .prepare_special(
                id(91),
                U256::from(5),
                intent,
                U256::from(5),
                policy(),
                change,
            )
            .await
            .unwrap();
        let signed = session.sign_special(&prepared).unwrap();
        assert_eq!(
            signed.transaction().data.len(),
            if wrapping { 20 } else { 22 }
        );
        let bytes = signed.signed_bytes().unwrap();
        let mut reopened = SqliteStore::open(&env.path, env.store.scope()).unwrap();
        assert_eq!(reopened.signed_payload(id(91)).unwrap().unwrap(), bytes);
        assert!(reopened.release_unsigned(id(91)).is_err());
        assert_eq!(
            SignedQiOperation::decode(&bytes).unwrap().hash().unwrap(),
            signed.hash().unwrap()
        );
        let mut recovered = QiSession::new(&env.provider, &env.wallet, &mut reopened).unwrap();
        assert_eq!(
            recovered.broadcast(id(91)).await.unwrap().transaction_hash,
            signed.hash().unwrap()
        );
    }
}

#[tokio::test]
async fn current_gap_scan_and_refresh_queries_stored_addresses_and_preserves_claims() {
    use quai_sdk::qi_discovery::{DEFAULT_QI_GAP, QiScanOptions, refresh_qi, scan_and_refresh_qi};
    use quai_sdk::wallet::discovery::{IndexRange, ScanStop};
    assert_eq!(QiScanOptions::default().gap_limit, Some(DEFAULT_QI_GAP));
    assert_eq!(DEFAULT_QI_GAP, 50);
    let mut env = setup();
    let _change = pool(&mut env, 1);
    let stored = env.store.addresses().unwrap();
    let funded = stored.last().unwrap().address().to_string();
    env.mock.outpoints.lock().unwrap().insert(funded,json!([{"txHash":"0x0080008033333333333333333333333333333333333333333333333333333333","index":"0x0","denomination":"0x2","lock":"0x20"}]));
    let account = env.wallet.account_public(0).unwrap();
    let options = QiScanOptions {
        receive: IndexRange {
            start: 0,
            end: 100_000,
        },
        change: IndexRange {
            start: 0,
            end: 100_000,
        },
        gap_limit: Some(2),
        max_addresses: 20,
    };
    let report = scan_and_refresh_qi(&env.provider, &mut env.store, &account, &options, || false)
        .await
        .unwrap();
    assert_eq!(report.stopped, [ScanStop::GapLimit; 2]);
    let snapshot = env.store.snapshot().unwrap();
    assert_eq!(snapshot.coins.len(), 1);
    assert_eq!(snapshot.coins[0].denomination.value(), 10);
    assert_eq!(snapshot.coins[0].unlock_height, U256::from(32));
    let balance = quai_sdk::qi_discovery::qi_balance(&mut env.store, U256::from(31)).unwrap();
    assert_eq!(balance.total, U256::from(10));
    assert_eq!(balance.locked, U256::from(10));
    assert_eq!(balance.spendable, U256::ZERO);
    assert_eq!(
        quai_sdk::qi_discovery::qi_balance(&mut env.store, U256::from(32))
            .unwrap()
            .spendable,
        U256::from(10)
    );
    assert!(snapshot.checkpoint.is_some());
    assert!(
        refresh_qi(&env.provider, &mut env.store, 100, || true)
            .await
            .is_err()
    );
    assert_eq!(
        env.store.snapshot().unwrap().generation,
        snapshot.generation
    );
}

#[tokio::test]
async fn reorg_invalidates_inclusion_but_never_releases_signed_claims() {
    use quai_sdk::recovery::{OperationObservation, reconcile_operation};
    let mut env = setup();
    let change = pool(&mut env, 0);
    refresh(&mut env);
    let mut session = QiSession::new(&env.provider, &env.wallet, &mut env.store).unwrap();
    let prepared = session
        .prepare(id(92), intent(), policy(), change)
        .await
        .unwrap();
    let signed = session.sign(&prepared).unwrap();
    let included = Checkpoint {
        hash: CHECKPOINT.parse().unwrap(),
        height: U256::from(16),
    };
    env.store
        .observe_inclusion(id(92), signed.hash().unwrap(), included)
        .unwrap();
    assert!(
        env.store
            .invalidate_inclusion(
                id(92),
                Checkpoint {
                    height: U256::from(15),
                    ..included
                }
            )
            .is_err()
    );
    env.mock.mode.store(5, Ordering::SeqCst);
    assert!(matches!(
        reconcile_operation(&env.provider, &mut env.store, id(92))
            .await
            .unwrap(),
        OperationObservation::Reorganized
    ));
    assert_eq!(
        env.store.reservation(id(92)).unwrap().unwrap().state,
        ReservationState::Submitted
    );
    assert_eq!(env.store.reserved_outpoints(id(92)).unwrap().len(), 2);
    assert_eq!(
        env.store.signed_payload(id(92)).unwrap().unwrap(),
        signed.signed_bytes().unwrap()
    );
    assert!(matches!(
        reconcile_operation(&env.provider, &mut env.store, id(92))
            .await
            .unwrap(),
        OperationObservation::NotObserved
    ));
    assert!(env.store.release_unsigned(id(92)).is_err());
}

#[cfg(feature = "payments")]
#[tokio::test]
async fn payment_scan_imports_matching_receive_children_and_is_idempotent() {
    use quai_sdk::payment_channels::{PaymentScanOptions, payment_intent, scan_payment_channel};
    use quai_sdk::payments::{PaymentChannel, PaymentDirection, PaymentSearch, PrivatePaymentCode};
    use quai_sdk::wallet::qi_keys::{QiKeyResolver, QiKeyring};
    let mut env = setup();
    let owner = PrivatePaymentCode::from_seed(&[1; 32], 0).unwrap();
    let sender = PrivatePaymentCode::from_seed(&[2; 32], 0).unwrap();
    let peer = sender.public_code();
    env.store
        .import_payment_channel(&owner, &PaymentChannel::new(&owner, peer.clone()), None)
        .unwrap();
    let found = owner
        .search(
            peer,
            PaymentDirection::Receive,
            PaymentSearch {
                zone: Zone::Cyprus1,
                start_index: 0,
                max_attempts: 10_000,
            },
            || false,
        )
        .unwrap();
    assert_eq!(
        sender
            .send_public_key(owner.public_code(), found.index)
            .unwrap(),
        found.public_key
    );
    env.mock.outpoints.lock().unwrap().insert(found.address.to_string(),json!([{"txHash":"0x0080008033333333333333333333333333333333333333333333333333333333","index":"0x0","denomination":"0x6","lock":"0x0"}]));
    let options = PaymentScanOptions {
        gap_limit: Some(2),
        ..Default::default()
    };
    let first = scan_payment_channel(
        &env.provider,
        &mut env.store,
        &owner,
        peer,
        &options,
        || false,
    )
    .await
    .unwrap();
    assert_eq!(first.indexes.len(), 3);
    let second = scan_payment_channel(
        &env.provider,
        &mut env.store,
        &owner,
        peer,
        &options,
        || false,
    )
    .await
    .unwrap();
    assert_eq!(first.indexes, second.indexes);
    let mut keys = QiKeyring::new(None).unwrap();
    assert_eq!(
        keys.load_payment_channel(&env.store, &owner, peer).unwrap(),
        3
    );
    let metadata = env
        .store
        .addresses()
        .unwrap()
        .into_iter()
        .find(|address| address.address() == found.address.address())
        .unwrap();
    assert_eq!(
        keys.resolve(&metadata).unwrap().public_key(),
        found.public_key
    );
    let wrong = PrivatePaymentCode::from_seed(&[3; 32], 0).unwrap();
    assert!(
        env.store
            .import_payment_receive_indexes(&wrong, peer, &first.indexes)
            .is_err()
    );
    let intent = payment_intent(
        &mut env.store,
        &owner,
        peer,
        U256::from(5),
        2,
        10_000,
        || false,
    )
    .unwrap();
    assert_ne!(intent.destinations[0], intent.destinations[1]);
    assert_eq!(
        env.store
            .payment_addresses(&owner, peer, PaymentDirection::Send)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(env.store.snapshot().unwrap().coins.len(), 1);
}

#[tokio::test]
async fn sweeps_and_cross_zone_transfers_use_durable_exact_payloads() {
    use quai_sdk::wallet::SweepMode;
    for aggregate in [false, true] {
        let mut env = setup();
        let outputs = pool(&mut env, 8);
        refresh(&mut env);
        env.mock.fees.lock().unwrap().clear();
        env.mock.fees.lock().unwrap().push_back(0);
        let mode = if aggregate {
            SweepMode::Aggregate {
                maximum: Denomination::new(14).unwrap(),
            }
        } else {
            SweepMode::PreserveDenominations
        };
        let mut session = QiSession::new(&env.provider, &env.wallet, &mut env.store).unwrap();
        let prepared = session
            .prepare_sweep(
                id(93),
                mode,
                QiPolicy {
                    initial_fee: U256::ZERO,
                    ..policy()
                },
                outputs,
            )
            .await
            .unwrap();
        assert_eq!(prepared.transaction().inputs.len(), 2);
        assert_eq!(
            prepared.transaction().outputs.len(),
            if aggregate { 1 } else { 2 }
        );
        assert_eq!(prepared.fee(), U256::ZERO);
        let signed = session.sign(&prepared).unwrap();
        assert_eq!(
            session.broadcast(id(93)).await.unwrap().transaction_hash,
            signed.hash().unwrap()
        );
    }
    let mut env = setup();
    let change = pool(&mut env, 0);
    refresh(&mut env);
    let mut send = intent();
    send.destinations = vec![
        "0x0180000000000000000000000000000000000001"
            .parse()
            .unwrap(),
    ];
    let mut session = QiSession::new(&env.provider, &env.wallet, &mut env.store).unwrap();
    let prepared = session
        .prepare_cross_zone(id(94), send, policy(), change)
        .await
        .unwrap();
    assert_eq!(
        prepared.transaction().outputs[0].address.zone().unwrap(),
        Zone::Cyprus2
    );
    session.sign(&prepared).unwrap();
    session.broadcast(id(94)).await.unwrap();
}

#[tokio::test]
async fn specialized_estimates_converge_before_claims_and_enforce_budget_and_rounds() {
    use quai_sdk::consensus::QiWrappingIntent;
    use quai_sdk::provider::QiFeeProfile;
    use quai_sdk::qi::QiSpecialIntent;
    for failure in 0..=3 {
        let mut env = setup();
        env.mock.mode.store(8, Ordering::SeqCst);
        let change = pool(&mut env, 0);
        refresh(&mut env);
        let mut limits = policy();
        limits.initial_fee = U256::ZERO;
        if failure == 1 {
            limits.max_fee = U256::from(4);
        }
        if failure == 2 {
            limits.max_fee_rounds = 1;
        }
        if failure == 3 {
            env.mock.mode.store(0, Ordering::SeqCst);
        }
        let intent = QiSpecialIntent::Wrapping(QiWrappingIntent {
            destination: "0x0000000000000000000000000000000000000001"
                .parse()
                .unwrap(),
            owner_contract: "0x002b2596EcF05C93a31ff916E8b456DF6C77c750"
                .parse()
                .unwrap(),
        });
        let mut session = QiSession::new(&env.provider, &env.wallet, &mut env.store).unwrap();
        let result = session
            .prepare_special_estimated(
                id(92),
                U256::from(5),
                intent,
                QiFeeProfile::V056ShaAnchored,
                limits,
                change,
            )
            .await;
        match failure {
            0 => {
                let prepared = result.unwrap();
                assert_eq!(prepared.fee(), U256::from(5));
                assert_eq!(prepared.fee_quote().unwrap().qits, U256::from(5));
                assert_eq!(prepared.transaction().transaction().inputs.len(), 2);
                assert_eq!(count_calls(&env.mock, "quai_quaiToQi"), 2);
                session.sign_special(&prepared).unwrap();
            }
            1 => assert!(matches!(
                result,
                Err(QiError::Selection(SelectionError::FeeBudgetExceeded))
            )),
            2 => assert!(matches!(
                result,
                Err(QiError::Selection(SelectionError::FeeDidNotConverge))
            )),
            3 => assert!(matches!(
                result,
                Err(QiError::Provider(
                    quai_sdk::provider::ProviderError::ConversionFeeEstimationUnavailable
                ))
            )),
            _ => unreachable!(),
        }
        if failure != 0 {
            assert!(env.store.reservation(id(92)).unwrap().is_none());
        }
        assert_eq!(count_calls(&env.mock, "quai_estimateFeeForQi"), 0);
    }
}

#[tokio::test]
async fn conflicting_qi_candidates_preserve_recipients_and_survive_restart_and_timeout() {
    use quai_sdk::qi::{QiCandidateStatus, QiReplacementIntent};
    let mut env = setup();
    let change = pool(&mut env, 4);
    refresh(&mut env);
    let limits = QiPolicy {
        initial_fee: U256::from(1),
        ..policy()
    };
    let mut session = QiSession::new(&env.provider, &env.wallet, &mut env.store).unwrap();
    let prepared = session
        .prepare(id(95), intent(), limits, change)
        .await
        .unwrap();
    let root = session.sign(&prepared).unwrap();
    assert_eq!(root.transaction().outputs.len(), 5);
    let replacement = QiReplacementIntent {
        parent: root.hash().unwrap(),
        change_indexes: vec![1],
        change_outputs: vec![],
    };
    let mut bad = replacement.clone();
    bad.change_indexes = vec![0];
    assert!(matches!(
        session.prepare_replacement(id(95), bad, limits, None).await,
        Err(QiError::IdentityMismatch)
    ));
    let mut bad = replacement.clone();
    bad.change_indexes = vec![1, 1];
    assert!(
        session
            .prepare_replacement(id(95), bad, limits, None)
            .await
            .is_err()
    );
    assert!(
        session
            .prepare_replacement(
                id(95),
                replacement.clone(),
                QiPolicy {
                    max_fee: U256::from(1),
                    ..limits
                },
                None
            )
            .await
            .is_err()
    );
    let raised = session
        .prepare_replacement(id(95), replacement, limits, None)
        .await
        .unwrap();
    assert_eq!(raised.fee(), U256::from(2));
    assert_eq!(raised.transaction().inputs, root.transaction().inputs);
    assert_eq!(
        raised.transaction().outputs[0],
        root.transaction().outputs[0]
    );
    let signed = session.sign_replacement(&raised).unwrap();
    let hash = signed.hash().unwrap();
    assert_eq!(
        session.sign_replacement(&raised).unwrap().hash().unwrap(),
        hash
    );
    let mut reopened = SqliteStore::open(&env.path, env.store.scope()).unwrap();
    let mut recovered = QiSession::new(&env.provider, &env.wallet, &mut reopened).unwrap();
    assert_eq!(recovered.signed_candidates(id(95)).unwrap().len(), 2);
    env.mock.mode.store(1, Ordering::SeqCst);
    assert!(recovered.broadcast_candidate(id(95), hash).await.is_err());
    let observed = recovered.observe_candidates(id(95)).await.unwrap();
    assert!(observed.canonical.is_none());
    assert_eq!(observed.candidates.len(), 2);
    assert!(
        observed
            .candidates
            .iter()
            .all(|(_, s)| matches!(s, QiCandidateStatus::NotObserved))
    );
    env.mock.mode.store(0, Ordering::SeqCst);
    assert_eq!(
        recovered
            .broadcast_candidate(id(95), hash)
            .await
            .unwrap()
            .transaction_hash,
        hash
    );
    assert_eq!(reopened.reserved_outpoints(id(95)).unwrap().len(), 2);
    assert_eq!(
        reopened.signed_payload(id(95)).unwrap().unwrap(),
        root.signed_bytes().unwrap()
    );
    assert!(reopened.release_unsigned(id(95)).is_err());
}

#[tokio::test]
async fn qi_replacement_rechecks_snapshot_after_fee_estimation() {
    use quai_sdk::qi::QiReplacementIntent;
    let mut env = setup();
    let change = pool(&mut env, 4);
    refresh(&mut env);
    let limits = QiPolicy {
        initial_fee: U256::from(1),
        ..policy()
    };
    let scope = env.store.scope();
    let mut session = QiSession::new(&env.provider, &env.wallet, &mut env.store).unwrap();
    let prepared = session
        .prepare(id(96), intent(), limits, change)
        .await
        .unwrap();
    let root = session.sign(&prepared).unwrap();
    *env.mock.invalidate.lock().unwrap() = Some((env.path.clone(), scope));
    let intent = QiReplacementIntent {
        parent: root.hash().unwrap(),
        change_indexes: vec![1],
        change_outputs: vec![],
    };
    let result = session
        .prepare_replacement(id(96), intent, limits, None)
        .await;
    assert!(matches!(result, Err(QiError::StaleSnapshot)), "{result:?}");
    assert_eq!(session.signed_candidates(id(96)).unwrap().len(), 1);
}

#[tokio::test]
async fn wrapping_observation_is_saved_before_return_and_survives_reopen() {
    use quai_sdk::consensus::QiWrappingIntent;
    use quai_sdk::qi::QiSpecialIntent;
    use quai_sdk::settlement::{SettlementKind, cached_settlement, track_settlement};
    let mut env = setup();
    let change = pool(&mut env, 0);
    refresh(&mut env);
    let special = QiSpecialIntent::Wrapping(QiWrappingIntent {
        destination: "0x0000000000000000000000000000000000000001"
            .parse()
            .unwrap(),
        owner_contract: "0x002b2596EcF05C93a31ff916E8b456DF6C77c750"
            .parse()
            .unwrap(),
    });
    let mut session = QiSession::new(&env.provider, &env.wallet, &mut env.store).unwrap();
    let prepared = session
        .prepare_special(
            id(99),
            U256::from(5),
            special,
            U256::from(5),
            policy(),
            change,
        )
        .await
        .unwrap();
    let signed = session.sign_special(&prepared).unwrap();
    let hash = signed.hash().unwrap();
    let request = quai_sdk::provider::EtxScanRequest {
        zone: Zone::Cyprus1,
        from: 16,
        to: 16,
        max_transactions_per_block: 16,
        max_total_transactions: 32,
        preceding_block: None,
    };
    let observed = track_settlement(
        &env.provider,
        &mut env.store,
        id(99),
        hash,
        SettlementKind::QiWrapping,
        request,
        16,
    )
    .await
    .unwrap();
    assert_eq!(observed.revision, 1);
    assert!(matches!(
        observed.external.unwrap().origin,
        quai_sdk::provider::ConversionOriginObservation::Unavailable
    ));
    let mut reopened = SqliteStore::open(&env.path, env.store.scope()).unwrap();
    let cached = cached_settlement(
        &reopened
            .observation_cache(id(99), hash, 0)
            .unwrap()
            .unwrap(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(cached["kind"], "qi_wrapping");
    assert_eq!(cached["candidate"], hash.to_string());
    assert!(cached["execution"].is_null());
    assert_eq!(reopened.reserved_outpoints(id(99)).unwrap().len(), 2);
}
