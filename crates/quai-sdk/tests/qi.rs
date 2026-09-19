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
    call_result: Arc<Mutex<Option<Value>>>,
    logs: Arc<Mutex<Vec<Value>>>,
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
            "quai_call" => self
                .call_result
                .lock()
                .unwrap()
                .clone()
                .expect("unexpected quai_call"),
            "quai_getLogs" => Value::Array(self.logs.lock().unwrap().clone()),
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
    let expected = env
        .wallet
        .account_public(0)
        .unwrap()
        .search(
            true,
            Search {
                zone: Zone::Cyprus1,
                start_index: 0,
                max_attempts: 4000,
            },
            || false,
        )
        .unwrap()
        .address
        .index
        + 1;
    assert_eq!(cursor, expected);
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
        Err(QiError::NetworkMismatch)
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
    // Background sync on its own connection invalidates the snapshot while
    // this session awaits a fee quote: the session must not reserve, and the
    // error must tell a sync loop to observe again rather than give up.
    *env.mock.invalidate.lock().unwrap() = Some((env.path.clone(), env.store.scope()));
    let error = QiSession::new(&env.provider, &env.wallet, &mut env.store)
        .unwrap()
        .prepare(id(9), intent(), policy(), change)
        .await
        .unwrap_err();
    assert!(matches!(error, QiError::Storage(_)));
    assert_eq!(error.class(), quai_sdk::ErrorClass::Stale);
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
        Err(QiError::NetworkMismatch)
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
fn unexposed_compact_allocation_rolls_back_and_resource_limits_fail_early() {
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
        None
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
        None
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
        assert_eq!(signed.origin_zone().unwrap(), Zone::Cyprus1);
        assert!(signed.transaction().origin_zone().is_err());
        // Reopening must preserve specialized classification through BOTH recovery APIs.
        let observed = Provider::new(
            recovery_support::RecoveryMock {
                base: env.mock.clone(),
                receipt: recovery_support::receipt(
                    signed.hash().unwrap().to_string(),
                    2,
                    None,
                    None,
                ),
                change_head: false,
                head_rechecked: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                reorg_inclusion: false,
            },
            Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
            env.store.scope().chain_id,
        );
        assert!(matches!(
            quai_sdk::recovery::reconcile_operation(&observed, &mut reopened, id(91))
                .await
                .unwrap(),
            quai_sdk::recovery::OperationObservation::Included {
                confirmations: 2,
                ..
            }
        ));
        let family = quai_sdk::recovery::track_family(&observed, &mut reopened, id(91))
            .await
            .unwrap();
        assert!(family.canonical.is_some());
        assert_eq!(family.candidates.len(), 1);
        assert_eq!(reopened.signed_payload(id(91)).unwrap().unwrap(), bytes);
        assert_eq!(reopened.reserved_outpoints(id(91)).unwrap().len(), 2);
        assert!(reopened.release_unsigned(id(91)).is_err());
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
    let options = QiScanOptions::default()
        .with_receive(IndexRange {
            start: 0,
            end: 100_000,
        })
        .with_change(IndexRange {
            start: 0,
            end: 100_000,
        })
        .with_gap_limit(Some(2))
        .with_max_addresses(20);
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
    let options = PaymentScanOptions::default().with_gap_limit(Some(2));
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
    // Loading every registered channel finds the same keys without the peer code.
    let mut all = QiKeyring::new(None).unwrap();
    assert_eq!(all.load_payment_channels(&env.store, &owner).unwrap(), 3);
    assert_eq!(keys.load_payment_channels(&env.store, &owner).unwrap(), 0);
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
    let message = b"Public payment-channel message fixture";
    let signature = quai_sdk::qi::sign_message(&keys, &metadata, message).unwrap();
    quai_sdk::signer::verify_qi_message(found.address, &found.public_key, message, &signature)
        .unwrap();
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
    let family = quai_sdk::recovery::track_family(&env.provider, &mut reopened, id(95))
        .await
        .unwrap();
    assert_eq!(family.candidates.len(), 2);
    assert_eq!(family.candidates[1].0, hash);
    assert!(family.canonical.is_none());
    let cache = reopened
        .observation_cache(id(95), root.hash().unwrap(), u16::MAX)
        .unwrap()
        .unwrap();
    assert_eq!(cache.revision, family.revision);
    assert_eq!(reopened.reserved_outpoints(id(95)).unwrap().len(), 2);
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
    let request = quai_sdk::provider::EtxScanRequest::new(Zone::Cyprus1, 16, 16, 16, 32)
        .with_preceding_block(None);
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

async fn persisted_settlement_cursor_fixture() -> (Environment, quai_sdk::primitives::Hash32, Value)
{
    use quai_sdk::consensus::QiWrappingIntent;
    use quai_sdk::qi::QiSpecialIntent;
    let mut env = setup();
    let change = pool(&mut env, 0);
    refresh(&mut env);
    let mut session = QiSession::new(&env.provider, &env.wallet, &mut env.store).unwrap();
    let prepared = session
        .prepare_special(
            id(97),
            U256::from(5),
            QiSpecialIntent::Wrapping(QiWrappingIntent {
                destination: "0x0000000000000000000000000000000000000001"
                    .parse()
                    .unwrap(),
                owner_contract: "0x002b2596EcF05C93a31ff916E8b456DF6C77c750"
                    .parse()
                    .unwrap(),
            }),
            U256::from(5),
            policy(),
            change,
        )
        .await
        .unwrap();
    let hash = session.sign_special(&prepared).unwrap().hash().unwrap();
    // Public synthetic saved-page metadata, with source headers supplied by Mock.
    // This tests resumption mechanics, not a funded wrapping execution.
    let payload = json!({"version":1,"candidate":hash.to_string(),"kind":"qi_wrapping","etx_index":0,"zone":0,"from":16,"to":16,"origin":{"number":16,"hash":CHECKPOINT},"scanned_through":{"number":16,"hash":CHECKPOINT},"execution":null,"qi_credit":null});
    env.store
        .compare_exchange_observation(
            id(97),
            hash,
            0,
            None,
            Some(&serde_json::to_vec(&payload).unwrap()),
        )
        .unwrap();
    (env, hash, payload)
}
#[tokio::test]
async fn settlement_cursor_reopens_revalidates_and_preserves_revision_through_tracking() {
    use quai_sdk::settlement::{SettlementKind, revalidate_settlement_cursor};
    let (mut env, hash, _) = persisted_settlement_cursor_fixture().await;
    let mut reopened = SqliteStore::open(&env.path, env.store.scope()).unwrap();
    let cursor = revalidate_settlement_cursor(
        &env.provider,
        &mut reopened,
        id(97),
        hash,
        SettlementKind::QiWrapping,
        Zone::Cyprus1,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(cursor.scanned_through().number, 16);
    assert!(cursor.execution().is_none());
    let request = cursor.request(500, 16, 32).unwrap();
    assert_eq!((request.from, request.to), (17, 272));
    assert_eq!(
        request.preceding_block.unwrap().hash.to_string(),
        CHECKPOINT
    );
    for (through, per, total) in [
        (16, 16, 32),
        (500, 0, 32),
        (500, 4097, 32),
        (500, 16, 65537),
    ] {
        assert!(cursor.request(through, per, total).is_err());
    }
    let update = cursor
        .track(&env.provider, &mut reopened, 18, 16, 32, 16)
        .await
        .unwrap();
    assert_eq!(update.revision, 2); // Mock reports origin unavailable, never settled.
    assert_eq!(reopened.reserved_outpoints(id(97)).unwrap().len(), 2);
    let calls = env.mock.calls.lock().unwrap().len();
    assert!(matches!(
        cursor
            .track(&env.provider, &mut env.store, 18, 16, 32, 16)
            .await,
        Err(QiError::Storage(
            quai_sdk::wallet::storage::StorageError::Conflict
        ))
    ));
    assert_eq!(env.mock.calls.lock().unwrap().len(), calls);
    // A valid incomplete observation has no continuation and retains its UI cache.
    assert!(
        revalidate_settlement_cursor(
            &env.provider,
            &mut reopened,
            id(97),
            hash,
            SettlementKind::QiWrapping,
            Zone::Cyprus1
        )
        .await
        .unwrap()
        .is_none()
    );
    let cache = reopened
        .observation_cache(id(97), hash, 0)
        .unwrap()
        .unwrap();
    assert_eq!(cache.revision, 2);
    assert!(cache.payload.is_some());
}
#[tokio::test]
async fn settlement_cursor_rereads_execution_and_invalidates_reorg_or_wrong_identity() {
    use quai_sdk::settlement::{SettlementKind, revalidate_settlement_cursor};
    let (mut env, hash, mut payload) = persisted_settlement_cursor_fixture().await;
    payload["execution"] =
        json!({"hash":CHECKPOINT,"block":{"number":16,"hash":CHECKPOINT},"outcome":"locked"});
    env.store
        .compare_exchange_observation(
            id(97),
            hash,
            0,
            Some(1),
            Some(&serde_json::to_vec(&payload).unwrap()),
        )
        .unwrap();
    let cursor = revalidate_settlement_cursor(
        &env.provider,
        &mut env.store,
        id(97),
        hash,
        SettlementKind::QiWrapping,
        Zone::Cyprus1,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(cursor.execution().unwrap().number, 16);
    let r = cursor.request(200, 16, 32).unwrap();
    assert_eq!((r.from, r.to), (16, 16));
    assert!(r.preceding_block.is_none());
    env.mock.mode.store(5, Ordering::SeqCst);
    assert!(
        revalidate_settlement_cursor(
            &env.provider,
            &mut env.store,
            id(97),
            hash,
            SettlementKind::QiWrapping,
            Zone::Cyprus1
        )
        .await
        .unwrap()
        .is_none()
    );
    let cache = env
        .store
        .observation_cache(id(97), hash, 0)
        .unwrap()
        .unwrap();
    assert_eq!(cache.revision, 3);
    assert!(cache.payload.is_none());
    assert!(env.store.signed_payload(id(97)).unwrap().is_some());
    assert_eq!(env.store.reserved_outpoints(id(97)).unwrap().len(), 2);
    env.mock.mode.store(0, Ordering::SeqCst);
    payload["candidate"] = json!(GENESIS);
    env.store
        .compare_exchange_observation(
            id(97),
            hash,
            0,
            Some(3),
            Some(&serde_json::to_vec(&payload).unwrap()),
        )
        .unwrap();
    let calls = env.mock.calls.lock().unwrap().len();
    assert!(
        revalidate_settlement_cursor(
            &env.provider,
            &mut env.store,
            id(97),
            hash,
            SettlementKind::QiWrapping,
            Zone::Cyprus1
        )
        .await
        .is_err()
    );
    assert_eq!(env.mock.calls.lock().unwrap().len(), calls);
    assert!(
        env.store
            .observation_cache(id(97), hash, 0)
            .unwrap()
            .unwrap()
            .payload
            .is_none()
    );
}

#[tokio::test]
async fn settlement_cursor_cannot_clear_or_overwrite_a_concurrent_observer() {
    use quai_sdk::settlement::{SettlementKind, revalidate_settlement_cursor};
    #[derive(Clone)]
    struct Racing {
        inner: Mock,
        path: std::path::PathBuf,
        scope: NetworkScope,
        candidate: quai_sdk::primitives::Hash32,
        payload: Vec<u8>,
        trigger: Arc<AtomicU8>,
    }
    impl Transport for Racing {
        async fn request(
            &self,
            endpoint: &Endpoint,
            method: &str,
            params: Value,
        ) -> Result<Value, RpcError> {
            let trigger = self.trigger.load(Ordering::SeqCst);
            if ((trigger == 1 && method == "quai_getHeaderByNumber" && params[0] != "0x0")
                || (trigger == 2 && method == "quai_getTransactionReceipt"))
                && self.trigger.swap(0, Ordering::SeqCst) != 0
            {
                let mut concurrent = SqliteStore::open(&self.path, self.scope).unwrap();
                let cache = concurrent
                    .observation_cache(id(97), self.candidate, 0)
                    .unwrap()
                    .unwrap();
                concurrent
                    .compare_exchange_observation(
                        id(97),
                        self.candidate,
                        0,
                        Some(cache.revision),
                        Some(&self.payload),
                    )
                    .unwrap();
            }
            self.inner.request(endpoint, method, params).await
        }
    }
    let (mut env, hash, payload) = persisted_settlement_cursor_fixture().await;
    let race = Racing {
        inner: env.mock.clone(),
        path: env.path.clone(),
        scope: env.store.scope(),
        candidate: hash,
        payload: serde_json::to_vec(&payload).unwrap(),
        trigger: Arc::new(AtomicU8::new(1)),
    };
    let provider = Provider::new(
        race.clone(),
        Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
        U256::from(15000),
    );
    env.mock.mode.store(5, Ordering::SeqCst);
    assert!(
        revalidate_settlement_cursor(
            &provider,
            &mut env.store,
            id(97),
            hash,
            SettlementKind::QiWrapping,
            Zone::Cyprus1
        )
        .await
        .unwrap()
        .is_none()
    );
    let cache = env
        .store
        .observation_cache(id(97), hash, 0)
        .unwrap()
        .unwrap();
    assert_eq!(cache.revision, 2);
    assert!(cache.payload.is_some());
    env.mock.mode.store(0, Ordering::SeqCst);
    let cursor = revalidate_settlement_cursor(
        &provider,
        &mut env.store,
        id(97),
        hash,
        SettlementKind::QiWrapping,
        Zone::Cyprus1,
    )
    .await
    .unwrap()
    .unwrap();
    race.trigger.store(2, Ordering::SeqCst);
    assert!(matches!(
        cursor
            .track(&provider, &mut env.store, 18, 16, 32, 16)
            .await,
        Err(QiError::Storage(
            quai_sdk::wallet::storage::StorageError::Conflict
        ))
    ));
    let cache = env
        .store
        .observation_cache(id(97), hash, 0)
        .unwrap()
        .unwrap();
    assert_eq!(cache.revision, 3);
    assert_eq!(cache.payload.unwrap(), race.payload);
    assert_eq!(env.store.reserved_outpoints(id(97)).unwrap().len(), 2);
}

#[path = "support/recovery.rs"]
mod recovery_support;
#[tokio::test]
async fn recovery_rejects_a_quai_receipt_for_signed_qi_before_recording_inclusion() {
    use recovery_support::{RecoveryMock, receipt};
    use std::sync::atomic::AtomicBool;
    let mut env = setup();
    let change = pool(&mut env, 0);
    refresh(&mut env);
    let mut session = QiSession::new(&env.provider, &env.wallet, &mut env.store).unwrap();
    let prepared = session
        .prepare(id(97), intent(), policy(), change)
        .await
        .unwrap();
    let signed = session.sign(&prepared).unwrap();
    for kind in [0, 2] {
        let observed = Provider::new(
            RecoveryMock {
                base: env.mock.clone(),
                receipt: receipt(
                    signed.hash().unwrap().to_string(),
                    kind,
                    if kind == 0 {
                        Some("0x0000000000000000000000000000000000000001".into())
                    } else {
                        None
                    },
                    None,
                ),
                change_head: false,
                head_rechecked: Arc::new(AtomicBool::new(false)),
                reorg_inclusion: false,
            },
            Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
            env.store.scope().chain_id,
        );
        let result =
            quai_sdk::recovery::reconcile_operation(&observed, &mut env.store, id(97)).await;
        if kind == 0 {
            assert!(matches!(result, Err(QiError::IdentityMismatch)));
            assert_eq!(
                env.store.reservation(id(97)).unwrap().unwrap().state,
                ReservationState::Signed
            );
        } else {
            assert!(matches!(
                result.unwrap(),
                quai_sdk::recovery::OperationObservation::Included {
                    confirmations: 2,
                    ..
                }
            ));
        }
    }
    assert_eq!(env.store.reserved_outpoints(id(97)).unwrap().len(), 2);
    assert!(env.store.release_unsigned(id(97)).is_err());
}

#[tokio::test]
async fn native_use_hints_extend_gap_and_failed_checker_preserves_storage() {
    use quai_sdk::qi_discovery::{QiScanOptions, scan_and_refresh_qi_with_use_checker};
    use quai_sdk::wallet::discovery::{IndexRange, ScanStop};
    let mut env = setup();
    let account = env.wallet.account_public(0).unwrap();
    let options = QiScanOptions::default()
        .with_receive(IndexRange {
            start: 0,
            end: 100_000,
        })
        .with_change(IndexRange { start: 0, end: 0 })
        .with_gap_limit(Some(1))
        .with_max_addresses(4);
    let before = env.store.snapshot().unwrap();
    let before_addresses = env.store.addresses().unwrap();
    let error = scan_and_refresh_qi_with_use_checker(
        &env.provider,
        &mut env.store,
        &account,
        &options,
        || false,
        |_, _| std::future::ready(Err(QiError::UseCheckFailed)),
    )
    .await
    .unwrap_err();
    assert!(matches!(error, QiError::UseCheckFailed));
    assert_eq!(env.store.snapshot().unwrap().generation, before.generation);
    assert_eq!(env.store.addresses().unwrap(), before_addresses);
    let mut calls = 0;
    let report = scan_and_refresh_qi_with_use_checker(
        &env.provider,
        &mut env.store,
        &account,
        &options,
        || false,
        |actual_scope, _| {
            assert_eq!(actual_scope, before.scope);
            calls += 1;
            std::future::ready(Ok(calls == 1))
        },
    )
    .await
    .unwrap();
    assert_eq!(calls, 2);
    assert_eq!(report.addresses.len(), 2);
    assert_eq!(report.stopped[0], ScanStop::GapLimit);
    assert!(env.store.snapshot().unwrap().coins.is_empty());
    assert!(env.store.snapshot().unwrap().checkpoint.is_some());
    for address in report.addresses {
        assert!(env.store.addresses().unwrap().contains(&address));
    }
}

#[test]
fn qi_message_resolver_checks_exact_hd_and_imported_ownership() {
    use quai_sdk::crypto::SecretKey;
    use quai_sdk::wallet::qi_keys::{QiKeyResolver, QiKeyring};
    use quai_sdk::wallet::storage::StorageError;
    let env = setup();
    let account = env.wallet.account_public(0).unwrap();
    let found = account
        .search(
            false,
            Search {
                zone: Zone::Cyprus1,
                start_index: 0,
                max_attempts: 100_000,
            },
            || false,
        )
        .unwrap();
    let hd = PublicAddress::derive(&account, false, found.address.index).unwrap();
    let message = "Qi authorization café 🐬".as_bytes();
    let signature = quai_sdk::qi::sign_message(&env.wallet, &hd, message).unwrap();
    let public_key = quai_sdk::crypto::PublicKey::from_sec1_bytes(hd.public_key()).unwrap();
    quai_sdk::signer::verify_qi_message(
        hd.address().try_into().unwrap(),
        &public_key,
        message,
        &signature,
    )
    .unwrap();
    let mut bytes = [0; 32];
    bytes[31] = 130; // Public toy key.
    let mut ring = QiKeyring::new(None).unwrap();
    let imported = ring.import(SecretKey::from_bytes(&bytes).unwrap()).unwrap();
    let signature = quai_sdk::qi::sign_message(&ring, &imported, message).unwrap();
    let public_key = quai_sdk::crypto::PublicKey::from_sec1_bytes(imported.public_key()).unwrap();
    quai_sdk::signer::verify_qi_message(
        imported.address().try_into().unwrap(),
        &public_key,
        message,
        &signature,
    )
    .unwrap();
    assert!(quai_sdk::qi::sign_message(&ring, &hd, message).is_err());
    struct WrongKey;
    impl QiKeyResolver for WrongKey {
        fn resolve(&self, _: &PublicAddress) -> Result<SecretKey, StorageError> {
            let mut bytes = [0; 32];
            bytes[31] = 130;
            Ok(SecretKey::from_bytes(&bytes).unwrap())
        }
    }
    assert!(matches!(
        quai_sdk::qi::sign_message(&WrongKey, &hd, message),
        Err(QiError::IdentityMismatch)
    ));
    // Bytes carrying a top-level inputs field are a spend, whatever the
    // encoding; the refusal has its own variant so a wallet can tell the
    // requester, rather than report a generic signing failure.
    let spend_shaped = [0x7a, 0x00];
    assert!(quai_sdk::consensus::has_transaction_inputs(&spend_shaped));
    let refused = quai_sdk::qi::sign_message(&env.wallet, &hd, &spend_shaped).unwrap_err();
    assert!(matches!(refused, QiError::TransactionMessage));
    assert_eq!(refused.class(), quai_sdk::primitives::ErrorClass::Invalid);
}

#[cfg(feature = "payments")]
#[test]
fn balance_buckets_cover_all_origins_and_preserve_claim_priority() {
    use quai_sdk::payments::{PaymentChannel, PaymentDirection, PrivatePaymentCode};
    use quai_sdk::qi_discovery::qi_balance;
    let mut env = setup();
    let receive = env.store.addresses().unwrap()[0].clone();
    let change = pool(&mut env, 1).addresses()[0].clone();
    let mut scalar = [0; 32];
    scalar[31] = 130;
    let imported = PublicAddress::imported(
        &quai_sdk::crypto::SecretKey::from_bytes(&scalar)
            .unwrap()
            .public_key(),
    )
    .unwrap();
    let generation = env.store.snapshot().unwrap().generation;
    env.store
        .import_metadata(generation, std::slice::from_ref(&imported))
        .unwrap();
    let owner = PrivatePaymentCode::from_seed(&[1; 32], 0).unwrap();
    let peer = PrivatePaymentCode::from_seed(&[2; 32], 0).unwrap();
    env.store
        .import_payment_channel(
            &owner,
            &PaymentChannel::new(&owner, peer.public_code().clone()),
            None,
        )
        .unwrap();
    let payment = env
        .store
        .allocate_payment_address(
            &owner,
            peer.public_code(),
            PaymentDirection::Receive,
            10_000,
            || false,
        )
        .unwrap();
    let mut snapshot = env.store.snapshot().unwrap();
    snapshot.checkpoint = Some(Checkpoint {
        hash: CHECKPOINT.parse().unwrap(),
        height: U256::from(16),
    });
    snapshot.coins = [
        receive.address(),
        change.address(),
        imported.address(),
        payment.found.address.address(),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, address)| {
        let mut hash = [0; 32];
        hash[3] = 0x80;
        hash[31] = 140 + i as u8;
        CandidateCoin {
            outpoint: OutPoint {
                transaction_hash: hash.into(),
                index: 0,
            },
            address: address.try_into().unwrap(),
            denomination: Denomination::new(i as u8).unwrap(),
            unlock_height: if i == 1 { U256::from(32) } else { U256::ZERO },
            expires_at: if i == 0 || i == 3 {
                Some(U256::from(17))
            } else {
                None
            },
            reserved: false,
        }
    })
    .collect();
    let generation = env.store.replace_snapshot(&snapshot).unwrap();
    env.store
        .reserve_qi(
            id(120),
            generation,
            U256::from(16),
            &[snapshot.coins[3].outpoint],
        )
        .unwrap();
    let balance = qi_balance(&mut env.store, U256::from(17)).unwrap();
    assert_eq!(balance.total, U256::from(66));
    assert_eq!(balance.spendable, U256::from(10));
    assert_eq!(balance.locked, U256::from(5));
    assert_eq!(balance.expired, U256::from(1));
    assert_eq!(balance.reserved, U256::from(50));
    assert_eq!(
        balance.total,
        balance.spendable + balance.locked + balance.expired + balance.reserved
    );
    assert!(env.mock.calls.lock().unwrap().is_empty()); // Explicit cached read.
    let mut reopened = SqliteStore::open(&env.path, env.store.scope()).unwrap();
    assert_eq!(qi_balance(&mut reopened, U256::from(17)).unwrap(), balance);
    assert!(matches!(
        qi_balance(&mut reopened, U256::from(15)),
        Err(QiError::StaleSnapshot)
    ));
    let generation = reopened.snapshot().unwrap().generation;
    reopened.invalidate_snapshot(generation).unwrap();
    assert!(matches!(
        qi_balance(&mut reopened, U256::from(17)),
        Err(QiError::MissingSnapshot)
    ));
    assert_eq!(reopened.reserved_outpoints(id(120)).unwrap().len(), 1);
}

#[cfg(feature = "payments")]
#[tokio::test]
async fn payment_discovery_continues_past_an_empty_gap_after_reopen() {
    use quai_sdk::payment_channels::{
        PaymentScanOptions, continue_payment_channel, scan_payment_channel,
    };
    use quai_sdk::payments::{PaymentChannel, PaymentDirection, PrivatePaymentCode};
    let mut env = setup();
    let receiver = PrivatePaymentCode::from_seed(&[1; 32], 0).unwrap();
    let sender = PrivatePaymentCode::from_seed(&[2; 32], 0).unwrap();
    let peer = sender.public_code();
    env.store
        .import_payment_channel(
            &receiver,
            &PaymentChannel::new(&receiver, peer.clone()),
            None,
        )
        .unwrap();
    let options = PaymentScanOptions::default().with_gap_limit(Some(1));
    let first = scan_payment_channel(
        &env.provider,
        &mut env.store,
        &receiver,
        peer,
        &options,
        || false,
    )
    .await
    .unwrap();
    assert_eq!(first.indexes.len(), 1);
    assert!(env.store.snapshot().unwrap().coins.is_empty());
    let found = receiver
        .search(
            peer,
            PaymentDirection::Receive,
            quai_sdk::payments::PaymentSearch {
                zone: Zone::Cyprus1,
                start_index: first.next_index,
                max_attempts: 10000,
            },
            || false,
        )
        .unwrap();
    env.mock.outpoints.lock().unwrap().insert(found.address.to_string(),json!([{"txHash":"0x0080008033333333333333333333333333333333333333333333333333333333","index":"0x0","denomination":"0x6","lock":"0x0"}]));
    // Reopen only the persisted metadata; no in-memory report supplies the cursor.
    env.store = SqliteStore::open(&env.path, env.store.scope()).unwrap();
    let next = continue_payment_channel(
        &env.provider,
        &mut env.store,
        &receiver,
        peer,
        &options,
        || false,
    )
    .await
    .unwrap();
    assert_eq!(next.indexes[0], found.index);
    assert_eq!(next.indexes.len(), 2);
    assert_eq!(env.store.snapshot().unwrap().coins.len(), 1);
    assert_eq!(
        env.store
            .payment_channel(&receiver, peer)
            .unwrap()
            .unwrap()
            .channel
            .next_index(PaymentDirection::Send, Zone::Cyprus1),
        Some(0)
    );
    // A full rescan remains possible, without replacing the explicit range by a cursor.
    let deep = PaymentScanOptions::default()
        .with_range(quai_sdk::wallet::discovery::IndexRange {
            start: 0,
            end: next.next_index,
        })
        .with_gap_limit(None);
    let all = scan_payment_channel(
        &env.provider,
        &mut env.store,
        &receiver,
        peer,
        &deep,
        || false,
    )
    .await
    .unwrap();
    assert_eq!(all.indexes.len(), 3);
    assert_eq!(env.store.snapshot().unwrap().coins.len(), 1);
}

#[tokio::test]
async fn refund_outpoints_with_quai_hash_bits_survive_selection_signing_and_restart() {
    let mut env = setup();
    let mut snapshot = env.store.snapshot().unwrap();
    for coin in &mut snapshot.coins {
        let mut bytes = *coin.outpoint.transaction_hash.bytes();
        bytes[3] &= 0x7f;
        coin.outpoint.transaction_hash = quai_sdk::primitives::Hash32::from_bytes(bytes);
    }
    env.store.replace_snapshot(&snapshot).unwrap();
    let outputs = pool(&mut env, 0);
    let mut session = QiSession::new(&env.provider, &env.wallet, &mut env.store).unwrap();
    let prepared = session
        .prepare(id(111), intent(), policy(), outputs)
        .await
        .unwrap();
    let signed = session.sign(&prepared).unwrap();
    assert!(
        signed
            .transaction()
            .inputs
            .iter()
            .all(|i| i.previous_output.transaction_hash.bytes()[3] & 0x80 == 0)
    );
    let saved = signed.signed_bytes().unwrap();
    env.store = SqliteStore::open(&env.path, env.store.scope()).unwrap();
    assert_eq!(env.store.signed_payload(id(111)).unwrap().unwrap(), saved);
    assert!(!env.store.reserved_outpoints(id(111)).unwrap().is_empty());
    let mut invalid = env.store.snapshot().unwrap();
    invalid.coins[0].outpoint.transaction_hash = quai_sdk::primitives::Hash32::ZERO;
    assert!(env.store.replace_snapshot(&invalid).is_err());
}

#[cfg(all(feature = "payments", feature = "abi"))]
#[tokio::test]
async fn mailbox_discovery_registers_bounded_announced_channels_and_finds_funds() {
    use quai_sdk::payment_channels::{PaymentScanOptions, discover_mailbox_channels};
    use quai_sdk::payment_mailbox::{PELAGUS_MAILBOX_ADDRESS, PaymentMailbox};
    use quai_sdk::payments::{PaymentDirection, PaymentSearch, PrivatePaymentCode};
    let mut env = setup();
    let receiver = PrivatePaymentCode::from_seed(&[1; 32], 0).unwrap();
    let sender = PrivatePaymentCode::from_seed(&[2; 32], 0).unwrap();
    let other = PrivatePaymentCode::from_seed(&[3; 32], 0).unwrap();
    // Funds at the first receive address derived for the announced sender.
    let found = receiver
        .search(
            sender.public_code(),
            PaymentDirection::Receive,
            PaymentSearch {
                zone: Zone::Cyprus1,
                start_index: 0,
                max_attempts: 10000,
            },
            || false,
        )
        .unwrap();
    env.mock.outpoints.lock().unwrap().insert(found.address.to_string(),json!([{"txHash":"0x0080008044444444444444444444444444444444444444444444444444444444","index":"0x0","denomination":"0x7","lock":"0x0"}]));
    let announced = [
        sender.public_code().to_base58(),
        receiver.public_code().to_base58(), // a self-announcement is not a channel
        "not-a-payment-code".to_string(),
        other.public_code().to_base58(),
        sender.public_code().to_base58(),
    ];
    let encoded = quai_sdk::abi::AbiCoder::encode(
        &[quai_sdk::abi::AbiType::parse("string[]").unwrap()],
        &[json!(announced)],
    )
    .unwrap();
    *env.mock.call_result.lock().unwrap() = Some(json!(RpcData::new(encoded).unwrap().to_hex()));
    let mailbox =
        PaymentMailbox::new(PELAGUS_MAILBOX_ADDRESS.parse().unwrap(), &env.provider).unwrap();
    let caller = "0x0006506bDE7140b85DED58a40D7444F84cde4821"
        .parse()
        .unwrap();
    use quai_sdk::payment_channels::{ChannelRegistration, MailboxDiscovery, MailboxRegistration};
    let options = PaymentScanOptions::default().with_gap_limit(Some(2));
    let page = |start| MailboxDiscovery::new(start, 2).with_options(options.clone());
    assert!(
        discover_mailbox_channels(
            &env.provider,
            &mut env.store,
            &receiver,
            &mailbox,
            caller,
            &MailboxDiscovery::new(0, 0),
            || false
        )
        .await
        .is_err()
    );
    // Report-only by default: the funded sender is reported with its value,
    // and nothing is persisted, since announcements are unauthenticated.
    let stored = env.store.addresses().unwrap().len();
    let report = discover_mailbox_channels(
        &env.provider,
        &mut env.store,
        &receiver,
        &mailbox,
        caller,
        &page(0),
        || false,
    )
    .await
    .unwrap();
    assert_eq!(report.scanned.len(), 1);
    assert_eq!(
        report.scanned[0].registration,
        ChannelRegistration::Unregistered
    );
    // The fixture funds one output of denomination index 7.
    assert_eq!(
        report.scanned[0].found,
        U256::from(quai_sdk::consensus::Denomination::new(7).unwrap().value())
    );
    assert!(
        env.store
            .payment_channel(&receiver, sender.public_code())
            .unwrap()
            .is_none()
    );
    assert_eq!(env.store.addresses().unwrap().len(), stored);
    // Two slots: the sender and the self-announcement; the other peer is deferred.
    let register = |start| page(start).with_registration(MailboxRegistration::RegisterFunded);
    // Below the caller's minimum, a funded probe registers nothing: dust
    // cannot buy a permanent, rescanned channel.
    let funded = U256::from(quai_sdk::consensus::Denomination::new(7).unwrap().value());
    let report = discover_mailbox_channels(
        &env.provider,
        &mut env.store,
        &receiver,
        &mailbox,
        caller,
        &register(0).with_min_funded(funded + U256::from(1)),
        || false,
    )
    .await
    .unwrap();
    assert_eq!(
        report.scanned[0].registration,
        ChannelRegistration::Unregistered
    );
    assert_eq!(report.scanned[0].found, funded);
    assert!(
        env.store
            .payment_channel(&receiver, sender.public_code())
            .unwrap()
            .is_none()
    );
    assert_eq!(env.store.addresses().unwrap().len(), stored);
    let report = discover_mailbox_channels(
        &env.provider,
        &mut env.store,
        &receiver,
        &mailbox,
        caller,
        &register(0),
        || false,
    )
    .await
    .unwrap();
    assert_eq!(report.scanned.len(), 1);
    assert_eq!(report.scanned[0].sender, *sender.public_code());
    assert_eq!(
        report.scanned[0].registration,
        ChannelRegistration::Registered
    );
    assert_eq!(report.scanned[0].report.indexes[0], found.index);
    assert_eq!(report.deferred, vec![other.public_code().clone()]);
    assert_eq!(report.next_start, Some(2));
    assert_eq!(report.duplicates, 1);
    assert_eq!(report.invalid.len(), 2);
    assert!(
        env.store
            .payment_channel(&receiver, other.public_code())
            .unwrap()
            .is_none()
    );
    let coins = env.store.snapshot().unwrap().coins;
    assert!(
        coins
            .iter()
            .any(|c| c.address.to_string() == found.address.to_string())
    );
    // Rerunning keeps the registered channel and rescans it without re-registration.
    let again = discover_mailbox_channels(
        &env.provider,
        &mut env.store,
        &receiver,
        &mailbox,
        caller,
        &register(0),
        || false,
    )
    .await
    .unwrap();
    assert_eq!(again.scanned[0].registration, ChannelRegistration::Existing);
    assert_eq!(again.next_start, Some(2));
    // The next page reaches the deferred announcement. That sender has no
    // funds, so even under RegisterFunded its probe persists nothing.
    let stored = env.store.addresses().unwrap().len();
    let next = discover_mailbox_channels(
        &env.provider,
        &mut env.store,
        &receiver,
        &mailbox,
        caller,
        &register(again.next_start.unwrap()),
        || false,
    )
    .await
    .unwrap();
    assert_eq!(next.scanned.len(), 1);
    assert_eq!(next.scanned[0].sender, *other.public_code());
    assert_eq!(
        next.scanned[0].registration,
        ChannelRegistration::Unregistered
    );
    assert_eq!(next.scanned[0].found, U256::ZERO);
    assert!(
        env.store
            .payment_channel(&receiver, other.public_code())
            .unwrap()
            .is_none()
    );
    assert_eq!(env.store.addresses().unwrap().len(), stored);
    assert!(next.deferred.is_empty());
    assert_eq!(next.next_start, None);

    // The same discovery reading NotificationSent logs for a block range: the
    // funded sender's announcement is found and its channel rescanned, and an
    // announcement to another receiver is ignored.
    use quai_sdk::payment_channels::MailboxSource;
    let event = mailbox
        .contract()
        .interface()
        .event("NotificationSent")
        .unwrap()
        .clone();
    let log = |index: u64, to: &PrivatePaymentCode| {
        let (topics, data) = event
            .encode_log(&[
                json!(sender.public_code().to_base58()),
                json!(to.public_code().to_base58()),
            ])
            .unwrap();
        json!({"address":PELAGUS_MAILBOX_ADDRESS,"topics":topics.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "data":RpcData::new(data).unwrap().to_hex(),"blockHash":GENESIS,"blockNumber":"0x10",
            "transactionHash":CHECKPOINT,"transactionIndex":"0x0","logIndex":format!("{index:#x}"),"removed":false})
    };
    *env.mock.logs.lock().unwrap() = vec![log(0, &other), log(1, &receiver)];
    let logged = |from, to| register(0).with_source(MailboxSource::Logs { from, to });
    let report = discover_mailbox_channels(
        &env.provider,
        &mut env.store,
        &receiver,
        &mailbox,
        caller,
        &logged(0x10, 0x10),
        || false,
    )
    .await
    .unwrap();
    assert_eq!(report.scanned.len(), 1);
    assert_eq!(report.scanned[0].sender, *sender.public_code());
    assert_eq!(
        report.scanned[0].registration,
        ChannelRegistration::Existing
    );
    assert_eq!(report.next_start, None);
    assert!(matches!(
        discover_mailbox_channels(
            &env.provider,
            &mut env.store,
            &receiver,
            &mailbox,
            caller,
            &logged(0x11, 0x10),
            || false,
        )
        .await,
        Err(QiError::InvalidPolicy)
    ));
}

#[tokio::test]
async fn a_windowed_scan_queries_exactly_the_addresses_it_reports() {
    // The property that separates a gap-bounded safe window from a speculative
    // one. The window is bounded by the gap counter's guarantee, so every
    // address it reads is one the sequential scan would also have read, and
    // therefore every queried address must appear in the report.
    //
    // A speculative window fails this: it queries past the point the gap rule
    // stopped, disclosing unissued addresses to the node and reading
    // observations the sequential scan never made. Asserting set equality
    // rather than a subset makes this a privacy regression test as well as a
    // correctness one.
    use quai_sdk::qi_discovery::{QiScanOptions, scan_qi};
    use quai_sdk::wallet::discovery::{IndexRange, ScanStop};

    for gap_limit in [1u32, 2, 3, 7, 50] {
        let env = setup();
        let account = env.wallet.account_public(0).unwrap();
        let options = QiScanOptions::default()
            .with_receive(IndexRange {
                start: 0,
                end: 100_000,
            })
            .with_change(IndexRange {
                start: 0,
                end: 100_000,
            })
            .with_gap_limit(Some(gap_limit))
            .with_max_addresses(10_000);
        env.mock.calls.lock().unwrap().clear();
        let report = scan_qi(&env.provider, env.store.scope(), &account, &options, || {
            false
        })
        .await
        .unwrap();
        assert_eq!(report.stopped, [ScanStop::GapLimit; 2]);

        // Every address the node was asked about.
        let queried: std::collections::BTreeSet<String> = env
            .mock
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(method, _)| method == "quai_getOutpointsByAddress")
            .map(|(_, params)| params[0].as_str().unwrap().to_ascii_lowercase())
            .collect();
        // Every address the scan reported.
        let reported: std::collections::BTreeSet<String> = report
            .addresses
            .iter()
            .map(|a| a.address().to_string().to_ascii_lowercase())
            .collect();

        assert_eq!(
            queried, reported,
            "gap {gap_limit}: the window read addresses it did not report"
        );
        // An empty wallet stops after exactly gap_limit consecutive unused
        // addresses on each branch, windowing or not.
        assert_eq!(
            reported.len(),
            (gap_limit as usize) * 2,
            "gap {gap_limit}: wrong number of addresses examined"
        );
    }
}

#[tokio::test]
async fn a_funded_address_resets_the_gap_across_a_window_boundary() {
    // The gap counter must reset mid-window exactly as it does mid-loop, and
    // the scan must continue past the funded address rather than stopping at
    // the window edge.
    use quai_sdk::qi_discovery::{QiScanOptions, scan_qi};
    use quai_sdk::wallet::Search;
    use quai_sdk::wallet::discovery::{IndexRange, ScanStop};
    use quai_sdk::wallet::metadata::{KeyOrigin, PublicAddress};

    let env = setup();
    let account = env.wallet.account_public(0).unwrap();

    // Fund the third usable receive address. With a gap limit of 3 the window is
    // also 3 wide, so the reset lands on the last address of the first window --
    // the boundary case. Funding any position at or beyond the limit would be a
    // test error rather than a window test: the sequential scan stops before
    // reaching it too.
    let mut index = 0u32;
    let mut funded_position = 0usize;
    let mut funded = String::new();
    for position in 0..3 {
        let found = account
            .search(
                false,
                Search {
                    zone: env.store.scope().zone,
                    start_index: index,
                    max_attempts: 100_000,
                },
                || false,
            )
            .unwrap();
        index = found.next_index.unwrap();
        if position == 2 {
            let metadata = PublicAddress::derive(&account, false, found.address.index).unwrap();
            funded = metadata.address().to_string();
            funded_position = position;
        }
    }
    assert_eq!(funded_position, 2);
    env.mock.outpoints.lock().unwrap().insert(funded.clone(), json!([{"txHash":"0x0080008033333333333333333333333333333333333333333333333333333333","index":"0x0","denomination":"0x2","lock":"0x0"}]));

    let options = QiScanOptions::default()
        .with_receive(IndexRange {
            start: 0,
            end: 100_000,
        })
        .with_change(IndexRange {
            start: 0,
            end: 100_000,
        })
        .with_gap_limit(Some(3))
        .with_max_addresses(10_000);
    env.mock.calls.lock().unwrap().clear();
    let report = scan_qi(&env.provider, env.store.scope(), &account, &options, || {
        false
    })
    .await
    .unwrap();
    assert_eq!(report.stopped, [ScanStop::GapLimit; 2]);

    let receive: Vec<String> = report
        .addresses
        .iter()
        .filter(|a| matches!(a.origin(), KeyOrigin::Bip44 { change: false, .. }))
        .map(|a| a.address().to_string())
        .collect();
    // Two unused, the funded one at position 2 resetting the counter, then three
    // more unused reaching the limit: six examined rather than stopping at three.
    assert_eq!(
        receive.len(),
        6,
        "the funded address must reset the gap across the window edge: {receive:?}"
    );
    assert_eq!(receive[2], funded);

    // And still nothing speculative was read.
    let queried: std::collections::BTreeSet<String> = env
        .mock
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(method, _)| method == "quai_getOutpointsByAddress")
        .map(|(_, params)| params[0].as_str().unwrap().to_ascii_lowercase())
        .collect();
    let reported: std::collections::BTreeSet<String> = report
        .addresses
        .iter()
        .map(|a| a.address().to_string().to_ascii_lowercase())
        .collect();
    assert_eq!(queried, reported);
}

#[tokio::test]
async fn refresh_reads_every_stored_address_in_one_batch_per_page() {
    // `outpoints_many` pages and adapts its own batch size. `refresh_qi` used
    // to feed it eight addresses at a time, which split a small wallet into
    // many batches and restarted the adaptation on every one.
    use quai_sdk::qi_discovery::refresh_qi;
    use quai_sdk::rpc::BatchResult;
    #[derive(Clone)]
    struct Batching(Mock, Arc<Mutex<Vec<usize>>>);
    impl Transport for Batching {
        async fn request(&self, e: &Endpoint, m: &str, p: Value) -> Result<Value, RpcError> {
            self.0.request(e, m, p).await
        }
        async fn request_batch(
            &self,
            endpoint: &Endpoint,
            requests: Vec<(&str, Value)>,
        ) -> Option<BatchResult> {
            let outpoints = requests
                .iter()
                .filter(|(m, _)| *m == "quai_getOutpointsByAddress")
                .count();
            if outpoints > 0 {
                self.1.lock().unwrap().push(outpoints);
            }
            let mut responses = vec![];
            for (method, params) in requests {
                responses.push(self.0.request(endpoint, method, params).await);
            }
            Some(Ok(responses))
        }
    }
    let mut env = setup();
    let _change = pool(&mut env, 16);
    let stored = env.store.addresses().unwrap().len();
    assert!(stored > 8, "enough addresses to have needed several pages");
    let batches = Arc::new(Mutex::new(vec![]));
    let provider = Provider::new(
        Batching(env.mock.clone(), batches.clone()),
        Routing::direct("http://127.0.0.1:9200/exact", Zone::Cyprus1.into()).unwrap(),
        env.store.scope().chain_id,
    );
    refresh_qi(&provider, &mut env.store, 100, || false)
        .await
        .unwrap();
    assert_eq!(*batches.lock().unwrap(), [stored]);
}

#[tokio::test]
async fn a_pool_lent_to_prepare_loses_only_the_change_a_prepared_spend_uses() {
    // A pool given by value was dropped on every error, burning addresses that
    // never reached a signed payload; enough failures pushed later change past
    // a gap-limited restore.
    let mut env = setup();
    let mut change = pool(&mut env, 4);
    let allocated: Vec<_> = change.addresses().to_vec();
    refresh(&mut env);
    *env.mock.fees.lock().unwrap() = [6].into();
    let failed = QiSession::new(&env.provider, &env.wallet, &mut env.store)
        .unwrap()
        .prepare(id(7), intent(), policy(), &mut change)
        .await;
    assert!(matches!(
        failed,
        Err(QiError::Selection(SelectionError::FeeBudgetExceeded))
    ));
    assert_eq!(
        change.addresses(),
        &allocated[..],
        "a failure takes nothing"
    );

    *env.mock.fees.lock().unwrap() = [1, 2, 2].into();
    let mut constraints = policy();
    constraints.initial_fee = U256::ZERO;
    let prepared = QiSession::new(&env.provider, &env.wallet, &mut env.store)
        .unwrap()
        .prepare(id(8), intent(), constraints, &mut change)
        .await
        .unwrap();
    let used = prepared.transaction().outputs.len() - prepared.recipient_outputs();
    assert!(used > 0 && used < allocated.len());
    for (output, address) in prepared.transaction().outputs[prepared.recipient_outputs()..]
        .iter()
        .zip(&allocated)
    {
        assert_eq!(output.address, address.address());
    }
    assert_eq!(
        change.addresses(),
        &allocated[used..],
        "the unused tail stays"
    );
}

#[tokio::test]
async fn refresh_retries_a_moving_tip_and_never_labels_across_blocks() {
    // Every output read must come from the chain through the label, so the tip
    // may not move across the reads. A new block retries them; a tip that
    // keeps moving fails without writing.
    use quai_sdk::qi_discovery::refresh_qi;
    #[derive(Clone)]
    struct Advancing {
        inner: Mock,
        latest_reads: Arc<std::sync::atomic::AtomicUsize>,
        always: bool,
    }
    impl Transport for Advancing {
        async fn request(
            &self,
            e: &Endpoint,
            method: &str,
            params: Value,
        ) -> Result<Value, RpcError> {
            if method == "quai_getHeaderByNumber" && params[0] == "latest" {
                let n = self.latest_reads.fetch_add(1, Ordering::SeqCst) as u64;
                let height = if self.always { 16 + n } else { 16 + n.min(1) };
                if height > 16 {
                    return Ok(
                        json!({"woHeader":{"hash":format!("0x{:064x}", 0x7700 + height),"number":format!("{height:#x}"),"location":"0x0000","parentHash":CHECKPOINT,"primeTerminusNumber":"0x10"},"baseFeePerGas":"0x1","gasLimit":"0x100000","stateLimit":"0x100000"}),
                    );
                }
            }
            self.inner.request(e, method, params).await
        }
    }
    for always in [false, true] {
        let mut env = setup();
        let before = env.store.snapshot().unwrap();
        let provider = Provider::new(
            Advancing {
                inner: env.mock.clone(),
                latest_reads: Arc::default(),
                always,
            },
            Routing::direct("http://127.0.0.1:9200/exact", Zone::Cyprus1.into()).unwrap(),
            env.store.scope().chain_id,
        );
        let result = refresh_qi(&provider, &mut env.store, 100, || false).await;
        if always {
            assert!(matches!(result, Err(QiError::StaleSnapshot)));
            assert_eq!(env.store.snapshot().unwrap().checkpoint, before.checkpoint);
        } else {
            let checkpoint = result.unwrap();
            assert_eq!(
                checkpoint.height,
                U256::from(17),
                "labelled with the stable tip"
            );
            assert_eq!(env.store.snapshot().unwrap().checkpoint, Some(checkpoint));
        }
    }
}

#[tokio::test]
async fn payment_channel_windows_query_exactly_the_reported_addresses() {
    // The payment scanner now reads gap-bounded windows like the HD scanners;
    // set equality between queried and reported addresses is the privacy check.
    use quai_sdk::payment_channels::{PaymentScanOptions, scan_payment_channel};
    use quai_sdk::payments::{PaymentChannel, PaymentDirection, PaymentSearch, PrivatePaymentCode};
    let owner = PrivatePaymentCode::from_seed(&[1; 32], 0).unwrap();
    let peer = PrivatePaymentCode::from_seed(&[2; 32], 0)
        .unwrap()
        .public_code()
        .clone();
    for gap_limit in [1u32, 3] {
        let mut env = setup();
        env.store
            .import_payment_channel(&owner, &PaymentChannel::new(&owner, peer.clone()), None)
            .unwrap();
        env.mock.calls.lock().unwrap().clear();
        let report = scan_payment_channel(
            &env.provider,
            &mut env.store,
            &owner,
            &peer,
            &PaymentScanOptions::default().with_gap_limit(Some(gap_limit)),
            || false,
        )
        .await
        .unwrap();
        assert_eq!(report.indexes.len(), gap_limit as usize);
        let reported: std::collections::BTreeSet<String> = report
            .indexes
            .iter()
            .map(|&index| {
                owner
                    .search(
                        &peer,
                        PaymentDirection::Receive,
                        PaymentSearch {
                            zone: Zone::Cyprus1,
                            start_index: index,
                            max_attempts: 1,
                        },
                        || false,
                    )
                    .unwrap()
                    .address
                    .to_string()
                    .to_ascii_lowercase()
            })
            .collect();
        // The refresh after the scan re-reads every stored address, so the
        // property is: all scanned addresses were read, and nothing outside
        // the stored set (which now includes them) ever was.
        let queried: std::collections::BTreeSet<String> = env
            .mock
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(method, _)| method == "quai_getOutpointsByAddress")
            .map(|(_, params)| params[0].as_str().unwrap().to_ascii_lowercase())
            .collect();
        let stored: std::collections::BTreeSet<String> = env
            .store
            .addresses()
            .unwrap()
            .iter()
            .map(|a| a.address().to_string().to_ascii_lowercase())
            .collect();
        assert!(reported.is_subset(&queried));
        assert!(queried.is_subset(&stored), "read an address past the gap");
    }
}
