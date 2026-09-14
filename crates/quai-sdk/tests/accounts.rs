//! End-to-end account orchestration against a deterministic transport, never a node.
#![cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
use quai_sdk::accounts::{AccountError, AccountIntent, AccountSession, FeePolicy};
use quai_sdk::consensus::{
    QiTransaction, QuaiTransaction, SignedQiTransaction, SignedQuaiTransaction,
};
use quai_sdk::crypto::{RecoverableSignature, SecretKey};
use quai_sdk::provider::RpcData;
use quai_sdk::rpc::{RpcError, Transport};
use quai_sdk::signer::{LocalSigner, Signer, SignerError};
use quai_sdk::wallet::storage::{
    NetworkScope, PublicAddress, ReservationId, ReservationState, SqliteStore,
};
use quai_sdk::{Endpoint, Provider, Routing, U256, Zone};
use serde_json::{Value, json};
use std::sync::atomic::AtomicU64;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU8, Ordering},
};

static NEXT_DB: AtomicU64 = AtomicU64::new(0);
struct TestDirectory(std::path::PathBuf);
impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const GENESIS: &str = "0x663a73416275109a01aad3a4c29ea9e310aded63c5eea491243b7312ad8cd16b";
#[derive(Clone, Default)]
struct Mock {
    calls: Arc<Mutex<Vec<(String, Value)>>>,
    mode: Arc<AtomicU8>,
}
impl Transport for Mock {
    async fn request(&self, _: &Endpoint, method: &str, params: Value) -> Result<Value, RpcError> {
        self.calls
            .lock()
            .unwrap()
            .push((method.to_owned(), params.clone()));
        let mode = self.mode.load(Ordering::SeqCst);
        if mode >= 5
            && params
                .as_array()
                .is_some_and(|v| v.iter().any(|p| p == "pending"))
        {
            return Err(RpcError::Timeout);
        }
        Ok(match method {
            "quai_chainId" => json!("0x3a98"),
            "quai_getHeaderByNumber" if mode >= 5 && params[0] != "0x0" => {
                let changed = mode == 6
                    && self
                        .calls
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|(m, _)| m == "quai_getBalance");
                json!({"woHeader":{"hash": if changed {format!("0x{}","22".repeat(32))} else {format!("0x{}","11".repeat(32))},"number":"0x10","location":"0x0000","parentHash":GENESIS,"primeTerminusNumber":"0x10"},"gasLimit":"0x100000","stateLimit":"0x100000"})
            }
            "quai_getHeaderByNumber" => {
                json!({"woHeader":{"hash":GENESIS,"number":"0x0","location":"0x","parentHash":format!("0x{}","00".repeat(32))}})
            }
            "quai_getTransactionByHash" | "quai_getTransactionReceipt" => Value::Null,
            "quai_getTransactionCount" => json!("0x4"),
            "quai_quaiToQi" => {
                if mode == 7 {
                    Value::Null
                } else {
                    json!("0x5e7f4")
                }
            }
            "quai_gasPrice" => json!("0x2"),
            "quai_estimateGas" => {
                if self.mode.load(Ordering::SeqCst) == 3 && params[0]["nonce"] != "0x4" {
                    json!("0xffff")
                } else {
                    json!("0x5208")
                }
            }
            "quai_getBalance" => {
                if self.mode.load(Ordering::SeqCst) >= 4 {
                    json!("0xffffffffffffffffffffffff")
                } else {
                    json!("0xffffffffffff")
                }
            }
            "quai_sendRawTransaction" => {
                match self.mode.load(Ordering::SeqCst) {
                    1 => return Err(RpcError::Timeout),
                    2 => std::future::pending::<()>().await,
                    _ => (),
                }
                let bytes: RpcData = params[0].as_str().unwrap().parse().unwrap();
                json!(
                    SignedQuaiTransaction::decode(bytes.bytes())
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
fn setup() -> (
    TestDirectory,
    Mock,
    Provider<Mock>,
    LocalSigner,
    SqliteStore,
) {
    let mut key = [0u8; 32];
    key[30] = 3;
    key[31] = 0x25;
    let key = SecretKey::from_bytes(&key).unwrap();
    let public = PublicAddress::imported(&key.public_key()).unwrap();
    let signer = LocalSigner::new(key, U256::from(15000)).unwrap();
    let scope = NetworkScope {
        chain_id: U256::from(15000),
        genesis: GENESIS.parse().unwrap(),
        zone: Zone::Cyprus1,
    };
    // Real WAL storage is required: in-memory SQLite cannot provide durability.
    let path = std::env::temp_dir().join(format!(
        "quai-account-public-test-{}-{}-{}",
        std::process::id(),
        NEXT_DB.fetch_add(1, Ordering::Relaxed),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&path).unwrap();
    let directory = TestDirectory(path);
    let mut store = SqliteStore::open(directory.0.join("wallet.sqlite"), scope).unwrap();
    store.import_metadata(0, &[public]).unwrap();
    let mock = Mock::default();
    let provider = Provider::new(
        mock.clone(),
        Routing::direct("http://127.0.0.1:9200/exact", Zone::Cyprus1.into()).unwrap(),
        scope.chain_id,
    );
    (directory, mock, provider, signer, store)
}
fn intent() -> AccountIntent {
    AccountIntent {
        to: "0x0011223344556677889900112233445566778899"
            .parse()
            .unwrap(),
        value: U256::from(10),
        data: RpcData::new(vec![0x12, 0x34]).unwrap(),
        access_list: vec![],
    }
}
fn policy() -> FeePolicy {
    FeePolicy {
        max_gas: 30_000,
        max_gas_price: U256::from(3),
        max_total_fee: U256::from(100_000),
        gas_margin_bps: 1000,
    }
}

#[tokio::test]
async fn exact_reviewed_payload_is_durable_before_one_ambiguous_send() {
    let (_directory, mock, provider, signer, mut store) = setup();
    let id = ReservationId([1; 16]);
    let mut session = AccountSession::new(&provider, &signer, &mut store).unwrap();
    let prepared = session.prepare(id, intent(), policy()).await.unwrap();
    assert_eq!(prepared.transaction().nonce, 4);
    assert_eq!(prepared.transaction().gas_limit, 23_100);
    assert_eq!(prepared.maximum_fee(), U256::from(46_200));
    assert_eq!(prepared.transaction().data, [0x12, 0x34]);
    let signed = session.sign(&prepared).unwrap();
    assert_eq!(signed.transaction(), prepared.transaction());
    assert_eq!(
        prepared.signing_digest(),
        signed.transaction().signing_digest().unwrap()
    );
    mock.mode.store(1, Ordering::SeqCst);
    assert!(
        matches!(session.broadcast(id).await,Err(AccountError::Broadcast(e)) if e.acceptance_is_ambiguous())
    );
    assert_eq!(
        store.reservation(id).unwrap().unwrap().state,
        ReservationState::Submitted
    );
    assert_eq!(
        store.signed_payload(id).unwrap().unwrap(),
        signed.signed_bytes().unwrap()
    );
    assert!(store.release_unsigned(id).is_err());
    let calls = mock.calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|(m, _)| m == "quai_sendRawTransaction")
            .count(),
        1
    );
    let estimate = &calls
        .iter()
        .find(|(m, _)| m == "quai_estimateGas")
        .unwrap()
        .1;
    assert_eq!(estimate[0]["input"], "0x1234");
    assert_eq!(estimate[0]["value"], "0xa");
}

#[tokio::test]
async fn fee_failure_does_not_allocate_nonce_and_cancelled_send_retains_claim() {
    let (_directory, mock, provider, signer, mut store) = setup();
    let id = ReservationId([2; 16]);
    let mut low = policy();
    low.max_total_fee = U256::from(1);
    {
        let mut session = AccountSession::new(&provider, &signer, &mut store).unwrap();
        assert!(matches!(
            session.prepare(id, intent(), low).await,
            Err(AccountError::FeeLimit)
        ));
    }
    assert!(store.reservation(id).unwrap().is_none());
    {
        let mut session = AccountSession::new(&provider, &signer, &mut store).unwrap();
        let prepared = session.prepare(id, intent(), policy()).await.unwrap();
        session.sign(&prepared).unwrap();
        mock.mode.store(2, Ordering::SeqCst);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), session.broadcast(id))
                .await
                .is_err()
        );
    }
    assert_eq!(
        store.reservation(id).unwrap().unwrap().state,
        ReservationState::Submitted
    );
    assert!(store.release_unsigned(id).is_err());
    // Restart-style new workflow object can retry only the identical stored payload.
    mock.mode.store(0, Ordering::SeqCst);
    let saved = store.signed_payload(id).unwrap().unwrap();
    let mut session = AccountSession::new(&provider, &signer, &mut store).unwrap();
    let result = session.broadcast(id).await.unwrap();
    assert_eq!(
        result.transaction_hash,
        SignedQuaiTransaction::decode(&saved)
            .unwrap()
            .hash()
            .unwrap()
    );
}

struct AlteringSigner(LocalSigner);
impl Signer for AlteringSigner {
    fn address(&self) -> quai_sdk::Address {
        self.0.address()
    }
    fn chain_id(&self) -> U256 {
        self.0.chain_id()
    }
    fn sign_quai(&self, tx: &QuaiTransaction) -> Result<SignedQuaiTransaction, SignerError> {
        let mut changed = tx.clone();
        changed.value += U256::from(1);
        self.0.sign_quai(&changed)
    }
    fn sign_qi_single(&self, tx: &QiTransaction) -> Result<SignedQiTransaction, SignerError> {
        self.0.sign_qi_single(tx)
    }
    fn sign_message(&self, m: &[u8]) -> Result<RecoverableSignature, SignerError> {
        self.0.sign_message(m)
    }
}
#[tokio::test]
async fn changed_signer_payload_never_enters_signed_store() {
    let (_directory, _, provider, signer, mut store) = setup();
    let signer = AlteringSigner(signer);
    let id = ReservationId([3; 16]);
    let mut session = AccountSession::new(&provider, &signer, &mut store).unwrap();
    let prepared = session.prepare(id, intent(), policy()).await.unwrap();
    assert!(matches!(
        session.sign(&prepared),
        Err(AccountError::PayloadMismatch)
    ));
    assert_eq!(
        store.reservation(id).unwrap().unwrap().state,
        ReservationState::Reserved
    );
    assert!(store.signed_payload(id).unwrap().is_none());
}

#[cfg(feature = "abi")]
#[tokio::test]
async fn deployment_reserves_before_grinding_and_estimates_exact_nonce_code_and_access_list() {
    use quai_sdk::abi::AbiInterface;
    use quai_sdk::contracts::{DeploymentSearch, prepare_deployment};
    let (_directory, mock, provider, signer, mut store) = setup();
    mock.mode.store(5, Ordering::SeqCst);
    let mut session = AccountSession::new(&provider, &signer, &mut store)
        .unwrap()
        .with_observation_policy(quai_sdk::accounts::AccountObservationPolicy::PinnedLatest);
    assert_eq!(
        session
            .reserve_deployment_nonce(ReservationId([20; 16]))
            .await
            .unwrap(),
        4
    );
    let id = ReservationId([21; 16]);
    let nonce = session.reserve_deployment_nonce(id).await.unwrap();
    assert_eq!(nonce, 5);
    let deployment = prepare_deployment(
        &AbiInterface::from_json(b"[]").unwrap(),
        &[0x60, 0x00, 0x60, 0x00, 0xf3],
        &[],
        quai_sdk::QuaiAddress::try_from(signer.address()).unwrap(),
        signer.chain_id(),
        nonce,
        U256::ZERO,
        DeploymentSearch {
            start_salt: 0,
            max_attempts: 10_000,
        },
        || false,
    )
    .unwrap();
    let predicted = deployment.address();
    let data = deployment.init_data().to_vec();
    let prepared = session
        .prepare_deployment(id, deployment, policy())
        .await
        .unwrap();
    assert_eq!(prepared.created_address(), Some(predicted));
    assert!(prepared.transaction().to.is_none());
    assert_eq!(prepared.transaction().nonce, nonce);
    assert_eq!(prepared.transaction().data, data);
    let signed = session.sign(&prepared).unwrap();
    assert_eq!(signed.transaction(), prepared.transaction());
    assert!(session.broadcast(id).await.is_ok());
    {
        let calls = mock.calls.lock().unwrap();
        let estimate = &calls
            .iter()
            .find(|(method, _)| method == "quai_estimateGas")
            .unwrap()
            .1[0];
        assert_eq!(estimate["nonce"], "0x5");
        assert!(estimate.get("to").is_none());
        assert_eq!(estimate["input"], RpcData::new(data).unwrap().to_hex());
        assert_eq!(estimate["accessList"][0]["address"], predicted.to_string());
        assert_eq!(
            store.signed_payload(id).unwrap().unwrap(),
            signed.signed_bytes().unwrap()
        );
    }
    let update = quai_sdk::deployments::track_deployment(
        &provider,
        &mut store,
        id,
        signed.hash().unwrap(),
        None,
    )
    .await
    .unwrap();
    assert!(matches!(
        update.observation,
        quai_sdk::provider::DeploymentObservation::NoReceipt {
            transaction_known: false
        }
    ));
    let mut reopened =
        SqliteStore::open(_directory.0.join("wallet.sqlite"), store.scope()).unwrap();
    let cache = reopened
        .observation_cache(id, signed.hash().unwrap(), 0)
        .unwrap()
        .unwrap();
    assert_eq!(cache.revision, update.revision);
    let summary: Value = serde_json::from_slice(cache.payload.as_deref().unwrap()).unwrap();
    assert_eq!(summary["address"], predicted.to_string());
    assert_eq!(summary["observation"]["state"], "no_receipt");
    assert_eq!(
        reopened.reservation(id).unwrap().unwrap().state,
        ReservationState::Submitted
    );
}

#[cfg(feature = "abi")]
#[tokio::test]
async fn deployment_fee_failure_keeps_reserved_nonce_for_explicit_recovery() {
    use quai_sdk::abi::AbiInterface;
    use quai_sdk::contracts::{DeploymentSearch, prepare_deployment};
    let (_directory, mock, provider, signer, mut store) = setup();
    let id = ReservationId([22; 16]);
    let mut session = AccountSession::new(&provider, &signer, &mut store).unwrap();
    let nonce = session.reserve_deployment_nonce(id).await.unwrap();
    let deployment = prepare_deployment(
        &AbiInterface::from_json(b"[]").unwrap(),
        &[0x60, 0, 0xf3],
        &[],
        quai_sdk::QuaiAddress::try_from(signer.address()).unwrap(),
        signer.chain_id(),
        nonce,
        U256::ZERO,
        DeploymentSearch {
            start_salt: 0,
            max_attempts: 10_000,
        },
        || false,
    )
    .unwrap();
    let mut limited = policy();
    limited.max_total_fee = U256::from(1);
    assert!(matches!(
        session.prepare_deployment(id, deployment, limited).await,
        Err(AccountError::FeeLimit)
    ));
    assert_eq!(store.reserved_nonce(id).unwrap().unwrap().1, nonce);
    assert_eq!(
        store.reservation(id).unwrap().unwrap().state,
        ReservationState::Reserved
    );
    assert!(store.signed_payload(id).unwrap().is_none());
    assert!(
        !mock
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|(m, _)| m == "quai_sendRawTransaction")
    );
}

#[tokio::test]
async fn ordinary_repeated_prepare_estimates_the_actual_reserved_nonce() {
    let (_directory, mock, provider, signer, mut store) = setup();
    let mut session = AccountSession::new(&provider, &signer, &mut store).unwrap();
    let first = session
        .prepare(ReservationId([40; 16]), intent(), policy())
        .await
        .unwrap();
    let second = session
        .prepare(ReservationId([41; 16]), intent(), policy())
        .await
        .unwrap();
    assert_eq!(first.transaction().nonce, 4);
    assert_eq!(second.transaction().nonce, 5);
    let signed = session.sign(&second).unwrap();
    let calls = mock.calls.lock().unwrap();
    let last = calls
        .iter()
        .rev()
        .find(|(method, _)| method == "quai_estimateGas")
        .unwrap();
    assert_eq!(last.1[0]["nonce"], "0x5");
    assert_eq!(signed.transaction().nonce, second.transaction().nonce);
}

#[tokio::test]
async fn actual_nonce_reestimate_failure_retains_an_unsigned_reservation() {
    let (_directory, mock, provider, signer, mut store) = setup();
    let sender = quai_sdk::QuaiAddress::try_from(signer.address()).unwrap();
    store
        .reserve_nonce(ReservationId([42; 16]), sender, 4)
        .unwrap();
    mock.mode.store(3, Ordering::SeqCst);
    let id = ReservationId([43; 16]);
    let result = AccountSession::new(&provider, &signer, &mut store)
        .unwrap()
        .prepare(id, intent(), policy())
        .await;
    assert!(matches!(result, Err(AccountError::FeeLimit)));
    assert_eq!(store.reserved_nonce(id).unwrap(), Some((sender, 5)));
    assert_eq!(
        store.reservation(id).unwrap().unwrap().state,
        ReservationState::Reserved
    );
    assert!(store.signed_payload(id).unwrap().is_none());
    let calls = mock.calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .rev()
            .find(|(method, _)| method == "quai_estimateGas")
            .unwrap()
            .1[0]["nonce"],
        "0x5"
    );
    assert!(
        !calls
            .iter()
            .any(|(method, _)| method == "quai_sendRawTransaction")
    );
}

#[tokio::test]
async fn prepared_account_cannot_cross_identical_stores_or_reopened_handles() {
    let (directory, _, provider, signer, mut store) = setup();
    let (_other_directory, _, other_provider, other_signer, mut other) = setup();
    let id = ReservationId([44; 16]);
    let prepared = AccountSession::new(&provider, &signer, &mut store)
        .unwrap()
        .prepare(id, intent(), policy())
        .await
        .unwrap();
    let own = AccountSession::new(&other_provider, &other_signer, &mut other)
        .unwrap()
        .prepare(id, intent(), policy())
        .await
        .unwrap();
    assert_eq!(store.scope(), other.scope());
    assert_eq!(
        store.reserved_nonce(id).unwrap(),
        other.reserved_nonce(id).unwrap()
    );
    assert!(matches!(
        AccountSession::new(&other_provider, &other_signer, &mut other)
            .unwrap()
            .sign(&prepared),
        Err(AccountError::IdentityMismatch)
    ));
    assert!(other.signed_payload(id).unwrap().is_none());
    AccountSession::new(&other_provider, &other_signer, &mut other)
        .unwrap()
        .sign(&own)
        .unwrap();
    let scope = store.scope();
    drop(store);
    let mut reopened = SqliteStore::open(directory.0.join("wallet.sqlite"), scope).unwrap();
    assert!(matches!(
        AccountSession::new(&provider, &signer, &mut reopened)
            .unwrap()
            .sign(&prepared),
        Err(AccountError::IdentityMismatch)
    ));
    assert_eq!(
        reopened.reservation(id).unwrap().unwrap().state,
        ReservationState::Reserved
    );
}

#[tokio::test]
async fn conversion_freezes_slippage_and_recovers_signed_nonce() {
    use quai_sdk::consensus::{ConversionSlippage, QuaiToQiTransaction};
    let (directory, mock, provider, signer, mut store) = setup();
    mock.mode.store(4, Ordering::SeqCst);
    let id = ReservationId([70; 16]);
    let destination = "0x0080000000000000000000000000000000000001"
        .parse()
        .unwrap();
    let mut session = AccountSession::new(&provider, &signer, &mut store).unwrap();
    let prepared = session
        .prepare_conversion(
            id,
            destination,
            U256::from(10_000_000_000_000_000_000u64),
            ConversionSlippage::new(100).unwrap(),
            FeePolicy {
                max_gas: 500_000,
                max_total_fee: U256::from(1_000_000),
                ..policy()
            },
        )
        .await
        .unwrap();
    assert_eq!(prepared.transaction().nonce, 4);
    assert_eq!(prepared.transaction().data, [0, 100]);
    QuaiToQiTransaction::new(prepared.transaction().clone()).unwrap();
    let signed = session.sign(&prepared).unwrap();
    let mut reopened = SqliteStore::open(directory.0.join("wallet.sqlite"), store.scope()).unwrap();
    assert_eq!(
        reopened.signed_payload(id).unwrap().unwrap(),
        signed.signed_bytes().unwrap()
    );
    AccountSession::new(&provider, &signer, &mut reopened)
        .unwrap()
        .broadcast(id)
        .await
        .unwrap();
    let calls = mock.calls.lock().unwrap();
    let estimate = &calls
        .iter()
        .find(|(method, _)| method == "quai_estimateGas")
        .unwrap()
        .1;
    assert_eq!(estimate[0]["input"], "0x0064");
    assert_eq!(estimate[0]["to"], destination.to_string());
    assert_eq!(estimate[1], "pending");
}

#[tokio::test]
async fn released_unsigned_nonce_can_be_explicitly_repaired_without_rewinding_cursor() {
    let (_directory, _mock, provider, signer, mut store) = setup();
    let id = ReservationId([71; 16]);
    let sender = signer.address().try_into().unwrap();
    assert_eq!(store.reserve_nonce(id, sender, 4).unwrap(), 4);
    store.release_unsigned(id).unwrap();
    assert!(
        AccountSession::new(&provider, &signer, &mut store)
            .unwrap()
            .prepare_reserved(id, intent(), policy())
            .await
            .is_err()
    );
    store.reopen_unsigned_nonce(id).unwrap();
    let mut session = AccountSession::new(&provider, &signer, &mut store).unwrap();
    let prepared = session
        .prepare_reserved(
            id,
            AccountIntent {
                to: sender,
                value: U256::ZERO,
                data: RpcData::default(),
                access_list: vec![],
            },
            policy(),
        )
        .await
        .unwrap();
    assert_eq!(prepared.transaction().nonce, 4);
    session.sign(&prepared).unwrap();
    assert!(store.reopen_unsigned_nonce(id).is_err());
    assert_eq!(
        store
            .reserve_nonce(ReservationId([72; 16]), sender, 4)
            .unwrap(),
        5
    );
}

#[tokio::test]
async fn explicit_confirmed_observations_pin_every_state_read_and_reject_head_changes() {
    use quai_sdk::accounts::AccountObservationPolicy;
    for mode in [5, 6] {
        for conversion in [false, true] {
            let (_directory, mock, provider, signer, mut store) = setup();
            mock.mode.store(mode, Ordering::SeqCst);
            let id = ReservationId([80; 16]);
            // Unsupported pending observations propagate and do not silently change policy.
            let result = AccountSession::new(&provider, &signer, &mut store)
                .unwrap()
                .prepare(id, intent(), policy())
                .await;
            assert!(matches!(result, Err(AccountError::Provider(_))));
            assert!(store.reservation(id).unwrap().is_none());
            mock.calls.lock().unwrap().clear();
            let mut session = AccountSession::new(&provider, &signer, &mut store)
                .unwrap()
                .with_observation_policy(AccountObservationPolicy::PinnedLatest);
            let result = if conversion {
                session
                    .prepare_conversion(
                        id,
                        "0x0080000000000000000000000000000000000001"
                            .parse()
                            .unwrap(),
                        U256::from(10_000_000_000_000_000_000u64),
                        quai_sdk::consensus::ConversionSlippage::new(100).unwrap(),
                        FeePolicy {
                            max_gas: 500_000,
                            max_total_fee: U256::from(1_000_000),
                            ..policy()
                        },
                    )
                    .await
            } else {
                session.prepare(id, intent(), policy()).await
            };
            if mode == 6 {
                assert!(matches!(result, Err(AccountError::ObservationChanged)));
                assert!(store.reservation(id).unwrap().is_none());
            } else {
                let prepared = result.unwrap();
                session.sign(&prepared).unwrap();
            }
            for (method, params) in mock.calls.lock().unwrap().iter() {
                if matches!(
                    method.as_str(),
                    "quai_getTransactionCount" | "quai_estimateGas" | "quai_getBalance"
                ) {
                    assert_eq!(params[1], "0x10");
                }
            }
        }
    }
}

#[tokio::test]
async fn replacement_fee_review_preserves_nonce_and_all_signed_candidates_after_restart() {
    use quai_sdk::accounts::{AccountObservationPolicy, ReplacementPolicy};
    let (directory, mock, provider, signer, mut store) = setup();
    mock.mode.store(5, Ordering::SeqCst);
    let id = ReservationId([91; 16]);
    let mut session = AccountSession::new(&provider, &signer, &mut store)
        .unwrap()
        .with_observation_policy(AccountObservationPolicy::PinnedLatest);
    let initial = session.prepare(id, intent(), policy()).await.unwrap();
    let root = session.sign(&initial).unwrap();
    let bump = ReplacementPolicy {
        minimum_price_bump_percent: 5,
        fees: policy(),
    };
    let prepared = session
        .prepare_replacement(id, root.hash().unwrap(), bump)
        .await
        .unwrap();
    assert_eq!(prepared.transaction().gas_price, U256::from(3));
    assert_eq!(prepared.transaction().nonce, root.transaction().nonce);
    let mut expected = root.transaction().clone();
    expected.gas_price = U256::from(3);
    assert_eq!(prepared.transaction(), &expected);
    let first = session.sign_replacement(&prepared).unwrap();
    assert!(matches!(
        session
            .prepare_replacement(id, first.hash().unwrap(), bump)
            .await,
        Err(AccountError::FeeLimit)
    ));
    let bump = ReplacementPolicy {
        fees: FeePolicy {
            max_gas_price: U256::from(10),
            max_total_fee: U256::from(300000),
            ..policy()
        },
        ..bump
    };
    let prepared = session
        .prepare_replacement(id, first.hash().unwrap(), bump)
        .await
        .unwrap();
    let second = session.sign_replacement(&prepared).unwrap();
    let observed = session.observe_candidates(id).await.unwrap();
    assert_eq!(observed.candidates.len(), 3);
    assert!(observed.canonical.is_none());
    mock.mode.store(1, Ordering::SeqCst);
    assert!(
        matches!(session.broadcast_candidate(id,second.hash().unwrap()).await,Err(AccountError::Broadcast(error)) if error.acceptance_is_ambiguous())
    );
    let expected = vec![
        root.hash().unwrap(),
        first.hash().unwrap(),
        second.hash().unwrap(),
    ];
    let mut reopened = SqliteStore::open(directory.0.join("wallet.sqlite"), store.scope()).unwrap();
    assert!(reopened.release_unsigned(id).is_err());
    assert_eq!(
        reopened.signed_payload(id).unwrap().unwrap(),
        root.signed_bytes().unwrap()
    );
    let mut recovered = AccountSession::new(&provider, &signer, &mut reopened).unwrap();
    assert_eq!(
        recovered
            .signed_candidates(id)
            .unwrap()
            .iter()
            .map(|s| s.hash().unwrap())
            .collect::<Vec<_>>(),
        expected
    );
    mock.mode.store(0, Ordering::SeqCst);
    assert_eq!(
        recovered
            .broadcast_candidate(id, second.hash().unwrap())
            .await
            .unwrap()
            .transaction_hash,
        second.hash().unwrap()
    );
    assert_eq!(
        reopened.reserved_nonce(id).unwrap().unwrap().1,
        root.transaction().nonce
    );
}

#[tokio::test]
async fn cross_zone_account_prepare_and_unsigned_restart_keep_exact_destination() {
    let (directory, mock, provider, signer, mut store) = setup();
    let id = ReservationId([97; 16]);
    let mut send = intent();
    send.to = "0x0100000000000000000000000000000000000001"
        .parse()
        .unwrap();
    let mut session = AccountSession::new(&provider, &signer, &mut store).unwrap();
    assert!(session.prepare(id, send.clone(), policy()).await.is_err());
    assert!(
        session
            .prepare_cross_zone(id, intent(), policy())
            .await
            .is_err()
    );
    let prepared = session
        .prepare_cross_zone(id, send.clone(), policy())
        .await
        .unwrap();
    let transaction = prepared.transaction().clone();
    assert_eq!(transaction.to, Some(send.to.address()));
    assert_eq!(transaction.data, send.data.bytes());
    let path = directory.0.join("wallet.sqlite");
    let mut reopened = SqliteStore::open(path, store.scope()).unwrap();
    let mut recovered = AccountSession::new(&provider, &signer, &mut reopened).unwrap();
    let resumed = recovered
        .prepare_cross_zone_reserved(id, send, policy())
        .await
        .unwrap();
    assert_eq!(resumed.transaction(), &transaction);
    let signed = recovered.sign(&resumed).unwrap();
    assert_eq!(
        recovered.broadcast(id).await.unwrap().transaction_hash,
        signed.hash().unwrap()
    );
    assert!(
        mock.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, _)| m == "quai_estimateGas")
            .all(|(_, p)| p[0]["to"] == "0x0100000000000000000000000000000000000001")
    );
}

#[tokio::test]
async fn settlement_reconstructs_durable_candidate_and_invalidates_stale_cache_on_error() {
    use quai_sdk::settlement::{SettlementKind, cached_settlement, track_settlement};
    let (directory, _mock, provider, signer, mut store) = setup();
    let id = ReservationId([98; 16]);
    let mut send = intent();
    send.to = "0x0100000000000000000000000000000000000001"
        .parse()
        .unwrap();
    let mut session = AccountSession::new(&provider, &signer, &mut store).unwrap();
    let prepared = session
        .prepare_cross_zone(id, send, policy())
        .await
        .unwrap();
    let signed = session.sign(&prepared).unwrap();
    let hash = signed.hash().unwrap();
    // A direct Cyprus-1 provider cannot attest Cyprus-2. Failed observation is
    // persisted as an invalidation, never as completed or dropped settlement.
    let request = quai_sdk::provider::EtxScanRequest {
        zone: Zone::Cyprus2,
        from: 1,
        to: 2,
        max_transactions_per_block: 16,
        max_total_transactions: 32,
        preceding_block: None,
    };
    assert!(
        track_settlement(
            &provider,
            &mut store,
            id,
            hash,
            SettlementKind::CrossZoneQuai,
            request,
            16
        )
        .await
        .is_err()
    );
    let mut reopened = SqliteStore::open(directory.0.join("wallet.sqlite"), store.scope()).unwrap();
    let cache = reopened.observation_cache(id, hash, 0).unwrap().unwrap();
    assert_eq!(cache.revision, 1);
    assert!(cached_settlement(&cache).unwrap().is_none());
    assert_eq!(
        reopened.signed_payload(id).unwrap().unwrap(),
        signed.signed_bytes().unwrap()
    );
    assert!(reopened.release_unsigned(id).is_err());
}

#[tokio::test]
async fn deployment_tracking_rejects_noncreation_and_invalidates_only_public_cache() {
    let (_directory, _mock, provider, signer, mut store) = setup();
    let id = ReservationId([93; 16]);
    let mut session = AccountSession::new(&provider, &signer, &mut store).unwrap();
    let prepared = session.prepare(id, intent(), policy()).await.unwrap();
    let signed = session.sign(&prepared).unwrap();
    let hash = signed.hash().unwrap();
    store
        .compare_exchange_observation(id, hash, 0, None, Some(b"{\"version\":1}"))
        .unwrap();
    assert!(
        quai_sdk::deployments::track_deployment(&provider, &mut store, id, hash, None)
            .await
            .is_err()
    );
    let cache = store.observation_cache(id, hash, 0).unwrap().unwrap();
    assert_eq!(cache.revision, 2);
    assert!(cache.payload.is_none());
    assert_eq!(
        store.signed_payload(id).unwrap().unwrap(),
        signed.signed_bytes().unwrap()
    );
    assert_eq!(
        store.reservation(id).unwrap().unwrap().state,
        ReservationState::Signed
    );
    assert!(store.release_unsigned(id).is_err());
}

#[path = "support/recovery.rs"]
mod recovery_support;
#[tokio::test]
async fn recovery_binds_account_receipt_and_rechecks_confirmation_head_before_writing() {
    use quai_sdk::recovery::{OperationObservation, reconcile_operation};
    use recovery_support::{RecoveryMock, receipt};
    use std::sync::atomic::AtomicBool;
    let (directory, mock, provider, signer, mut store) = setup();
    let id = ReservationId([94; 16]);
    let mut session = AccountSession::new(&provider, &signer, &mut store).unwrap();
    let prepared = session.prepare(id, intent(), policy()).await.unwrap();
    let signed = session.sign(&prepared).unwrap();
    let original = receipt(
        signed.hash().unwrap().to_string(),
        0,
        Some(signed.from().address().to_string()),
        signed.transaction().to.map(|a| a.to_string()),
    );
    for mode in 0..4 {
        let mut result = original.clone();
        match mode {
            0 => result["type"] = json!("0x2"),
            1 => result["from"] = json!("0x0000000000000000000000000000000000000001"),
            2 => result["to"] = json!("0x0000000000000000000000000000000000000001"),
            _ => (),
        }
        let transport = RecoveryMock {
            base: mock.clone(),
            receipt: result,
            change_head: mode == 3,
            head_rechecked: Arc::new(AtomicBool::new(false)),
        };
        let observed = Provider::new(
            transport.clone(),
            Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
            store.scope().chain_id,
        );
        assert!(
            reconcile_operation(&observed, &mut store, id)
                .await
                .is_err()
        );
        assert_eq!(
            store.reservation(id).unwrap().unwrap().state,
            ReservationState::Signed
        );
        if mode == 3 {
            assert!(transport.head_rechecked.load(Ordering::SeqCst));
        }
    }
    let observed = Provider::new(
        RecoveryMock {
            base: mock,
            receipt: original,
            change_head: false,
            head_rechecked: Arc::new(AtomicBool::new(false)),
        },
        Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
        store.scope().chain_id,
    );
    assert!(matches!(
        reconcile_operation(&observed, &mut store, id)
            .await
            .unwrap(),
        OperationObservation::Included {
            confirmations: 2,
            ..
        }
    ));
    let mut reopened = SqliteStore::open(directory.0.join("wallet.sqlite"), store.scope()).unwrap();
    assert_eq!(
        reopened.reservation(id).unwrap().unwrap().state,
        ReservationState::Confirmed
    );
    assert!(reopened.release_unsigned(id).is_err());
}

// Inject a second SQLite writer exactly while a recovery RPC is outstanding.
type RecoveryHook = Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>>;
#[derive(Clone)]
struct FamilyRace {
    base: recovery_support::RecoveryMock<Mock>,
    hook: RecoveryHook,
}
impl Transport for FamilyRace {
    async fn request(&self, e: &Endpoint, m: &str, p: Value) -> Result<Value, RpcError> {
        if m == "quai_getTransactionReceipt"
            && let Some(hook) = self.hook.lock().unwrap().take()
        {
            hook();
        }
        self.base.request(e, m, p).await
    }
}
#[tokio::test]
async fn family_recovery_persists_replacement_winner_and_rejects_concurrent_candidates() {
    use quai_sdk::recovery::{CandidateObservation, track_family};
    use quai_sdk::wallet::storage::StorageError;
    use recovery_support::{RecoveryMock, receipt};
    use std::sync::atomic::AtomicBool;
    let (directory, mock, provider, signer, mut store) = setup();
    let id = ReservationId([95; 16]);
    let mut session = AccountSession::new(&provider, &signer, &mut store).unwrap();
    let prepared = session.prepare(id, intent(), policy()).await.unwrap();
    let root = session.sign(&prepared).unwrap();
    let root_hash = root.hash().unwrap();
    let mut tx = root.transaction().clone();
    tx.gas_price += U256::from(1);
    let replacement = signer.sign_quai(&tx).unwrap();
    let replacement_hash = replacement.hash().unwrap();
    store
        .commit_quai_replacement(id, root_hash, &replacement)
        .unwrap();
    let mut result = receipt(
        replacement_hash.to_string(),
        0,
        Some(root.from().address().to_string()),
        tx.to.map(|a| a.to_string()),
    );
    result["status"] = json!("0x0");
    let base = RecoveryMock {
        base: mock.clone(),
        receipt: result.clone(),
        change_head: false,
        head_rechecked: Arc::new(AtomicBool::new(false)),
    };
    let observed = Provider::new(
        base.clone(),
        Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
        store.scope().chain_id,
    );
    let update = track_family(&observed, &mut store, id).await.unwrap();
    assert_eq!(update.canonical, Some(replacement_hash));
    assert!(matches!(
        update.candidates[0].1,
        CandidateObservation::NotObserved
    ));
    assert!(matches!(
        update.candidates[1].1,
        CandidateObservation::Included {
            outcome: quai_sdk::provider::ReceiptOutcome::Failed,
            confirmations: 2,
            ..
        }
    ));
    let path = directory.0.join("wallet.sqlite");
    let scope = store.scope();
    let mut reopened = SqliteStore::open(&path, scope).unwrap();
    let cache = reopened
        .observation_cache(id, root_hash, u16::MAX)
        .unwrap()
        .unwrap();
    let payload: Value = serde_json::from_slice(cache.payload.as_deref().unwrap()).unwrap();
    assert_eq!(payload["canonical"], replacement_hash.to_string());
    assert!(reopened.release_unsigned(id).is_err());
    tx.gas_price += U256::from(1);
    let third = signer.sign_quai(&tx).unwrap();
    let third_hash = third.hash().unwrap();
    let race = FamilyRace {
        base: base.clone(),
        hook: Arc::new(Mutex::new(Some(Box::new(move || {
            let mut writer = SqliteStore::open(path, scope).unwrap();
            writer
                .commit_quai_replacement(id, replacement_hash, &third)
                .unwrap();
        })))),
    };
    let raced = Provider::new(
        race,
        Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
        scope.chain_id,
    );
    assert!(matches!(
        track_family(&raced, &mut store, id).await,
        Err(quai_sdk::qi::QiError::Storage(StorageError::Conflict))
    ));
    let invalidated = store
        .observation_cache(id, root_hash, u16::MAX)
        .unwrap()
        .unwrap();
    assert_eq!(invalidated.revision, cache.revision + 1);
    assert!(invalidated.payload.is_none());
    let update = track_family(&observed, &mut store, id).await.unwrap();
    assert_eq!(update.candidates.len(), 3);
    assert_eq!(update.candidates[2].0, third_hash);
    // A newer revision wins even when this observer subsequently sees a changed head.
    let path = directory.0.join("wallet.sqlite");
    let mut changed = base.clone();
    changed.change_head = true;
    let expected = update.revision;
    let race = FamilyRace {
        base: changed,
        hook: Arc::new(Mutex::new(Some(Box::new(move || {
            let mut writer = SqliteStore::open(path, scope).unwrap();
            writer
                .compare_exchange_family_observation(
                    id,
                    &[root_hash, replacement_hash, third_hash],
                    Some(expected),
                    Some(b"newer observation"),
                )
                .unwrap();
        })))),
    };
    let raced = Provider::new(
        race,
        Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
        scope.chain_id,
    );
    assert!(track_family(&raced, &mut store, id).await.is_err());
    assert_eq!(
        store
            .observation_cache(id, root_hash, u16::MAX)
            .unwrap()
            .unwrap()
            .payload
            .as_deref(),
        Some(b"newer observation".as_slice())
    );
    // Two same-nonce candidates cannot both be canonical. The old cache is invalidated.
    let root_receipt = receipt(
        root_hash.to_string(),
        0,
        Some(root.from().address().to_string()),
        root.transaction().to.map(|a| a.to_string()),
    );
    let mut conflicting = base.clone();
    conflicting.receipt =
        json!({root_hash.to_string():root_receipt,replacement_hash.to_string():result});
    let observed = Provider::new(
        conflicting,
        Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
        scope.chain_id,
    );
    assert!(track_family(&observed, &mut store, id).await.is_err());
    assert!(
        store
            .observation_cache(id, root_hash, u16::MAX)
            .unwrap()
            .unwrap()
            .payload
            .is_none()
    );
    // The largest supported family fits the bounded 4096-byte cache.
    let mut parent = third_hash;
    for _ in 3..33 {
        tx.gas_price += U256::from(1);
        let next = signer.sign_quai(&tx).unwrap();
        store.commit_quai_replacement(id, parent, &next).unwrap();
        parent = next.hash().unwrap();
    }
    let observed = Provider::new(
        base,
        Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
        scope.chain_id,
    );
    let update = track_family(&observed, &mut store, id).await.unwrap();
    assert_eq!(update.candidates.len(), 33);
    assert!(
        store
            .observation_cache(id, root_hash, u16::MAX)
            .unwrap()
            .unwrap()
            .payload
            .unwrap()
            .len()
            <= 4096
    );
    assert!(
        store
            .compare_exchange_family_observation(id, &[root_hash], Some(update.revision), None)
            .is_err()
    );
    assert_eq!(
        store.reserved_nonce(id).unwrap().unwrap().1,
        root.transaction().nonce
    );
}

#[derive(Clone)]
struct AccessDiscoveryMock {
    base: Mock,
    failure: u8,
}
impl Transport for AccessDiscoveryMock {
    async fn request(
        &self,
        endpoint: &Endpoint,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        if method != "quai_createAccessList" {
            return self.base.request(endpoint, method, params).await;
        }
        self.base
            .calls
            .lock()
            .unwrap()
            .push((method.into(), params.clone()));
        if self.failure == 1 || (self.failure == 4 && params[0]["nonce"] == "0x5") {
            return Ok(
                json!({"accessList":[],"gasUsed":"0x5208","error":"fixture execution failed"}),
            );
        }
        if self.failure == 2 {
            return Ok(json!({"accessList":[],"gasUsed":"0x5208"}));
        }
        if self.failure == 3 {
            self.base.mode.store(6, Ordering::SeqCst);
        }
        let mut entries = params[0]["accessList"].as_array().unwrap().clone();
        if self.failure == 5 {
            entries[0]["storageKeys"] = json!([]);
        }
        let nonce = u8::from_str_radix(
            params[0]["nonce"]
                .as_str()
                .unwrap()
                .trim_start_matches("0x"),
            16,
        )
        .unwrap();
        entries.push(json!({"address":"0x000000000000000000000000000000000000000b","storageKeys":[format!("0x{}{:02x}","00".repeat(31),nonce)]}));
        Ok(json!({"accessList":entries,"gasUsed":"0x5208"}))
    }
}
fn access_intent() -> AccountIntent {
    let mut call = intent();
    call.access_list = vec![quai_sdk::consensus::AccessTuple {
        address: "0x000000000000000000000000000000000000000a"
            .parse()
            .unwrap(),
        storage_keys: vec![quai_sdk::primitives::Hash32::from_bytes([1; 32])],
    }];
    call
}
#[tokio::test]
async fn discovered_access_uses_reserved_nonce_and_survives_signing_restart_and_broadcast() {
    use quai_sdk::accounts::{AccountAccessListPolicy, AccountObservationPolicy};
    let (directory, mock, _, signer, mut store) = setup();
    mock.mode.store(5, Ordering::SeqCst);
    let provider = Provider::new(
        AccessDiscoveryMock {
            base: mock.clone(),
            failure: 0,
        },
        Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
        store.scope().chain_id,
    );
    let sender = signer.address().try_into().unwrap();
    store
        .reserve_nonce(ReservationId([110; 16]), sender, 4)
        .unwrap();
    let id = ReservationId([111; 16]);
    let original = access_intent();
    let mut session = AccountSession::new(&provider, &signer, &mut store)
        .unwrap()
        .with_observation_policy(AccountObservationPolicy::PinnedLatest)
        .with_access_list_policy(AccountAccessListPolicy::Discover);
    let prepared = session
        .prepare(id, original.clone(), policy())
        .await
        .unwrap();
    assert_eq!(prepared.transaction().nonce, 5);
    assert_eq!(prepared.transaction().access_list.len(), 2);
    assert_eq!(
        prepared.transaction().access_list[0],
        original.access_list[0]
    );
    assert_eq!(
        prepared.transaction().access_list[1].storage_keys[0].bytes()[31],
        5
    );
    assert_eq!(prepared.transaction().data, original.data.bytes());
    let signed = session.sign(&prepared).unwrap();
    let calls = mock.calls.lock().unwrap().clone();
    let discovery: Vec<_> = calls
        .iter()
        .filter(|(method, _)| method == "quai_createAccessList")
        .collect();
    assert_eq!(discovery.len(), 2);
    for (i, (_, params)) in discovery.iter().enumerate() {
        assert_eq!(params[1], "0x10");
        assert_eq!(params[0]["nonce"], format!("0x{:x}", 4 + i));
        assert_eq!(params[0]["accessList"].as_array().unwrap().len(), 1); // Rebuild from caller requirements.
    }
    let estimates: Vec<_> = calls
        .iter()
        .filter(|(method, _)| method == "quai_estimateGas")
        .collect();
    assert_eq!(estimates.len(), 2);
    assert!(
        estimates
            .iter()
            .all(|(_, p)| p[0]["accessList"].as_array().unwrap().len() == 2)
    );
    let mut reopened = SqliteStore::open(directory.0.join("wallet.sqlite"), store.scope()).unwrap();
    let mut session = AccountSession::new(&provider, &signer, &mut reopened)
        .unwrap()
        .with_access_list_policy(AccountAccessListPolicy::Discover);
    assert_eq!(
        session.broadcast(id).await.unwrap().transaction_hash,
        signed.hash().unwrap()
    );
    assert_eq!(
        reopened.signed_payload(id).unwrap().unwrap(),
        signed.signed_bytes().unwrap()
    );
    assert_eq!(
        mock.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, _)| m == "quai_createAccessList")
            .count(),
        2
    );
}
#[tokio::test]
async fn access_discovery_errors_cannot_discard_requirements_or_leave_signed_payloads() {
    use quai_sdk::accounts::{AccountAccessListPolicy, AccountObservationPolicy};
    for failure in 1..=5 {
        let (_directory, mock, _, signer, mut store) = setup();
        mock.mode.store(5, Ordering::SeqCst);
        let provider = Provider::new(
            AccessDiscoveryMock {
                base: mock.clone(),
                failure,
            },
            Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
            store.scope().chain_id,
        );
        if failure == 4 {
            store
                .reserve_nonce(
                    ReservationId([110; 16]),
                    signer.address().try_into().unwrap(),
                    4,
                )
                .unwrap();
        }
        let id = ReservationId([112; 16]);
        let mut session = AccountSession::new(&provider, &signer, &mut store)
            .unwrap()
            .with_observation_policy(AccountObservationPolicy::PinnedLatest)
            .with_access_list_policy(AccountAccessListPolicy::Discover);
        let result = session.prepare(id, access_intent(), policy()).await;
        assert!(result.is_err());
        if failure == 4 {
            assert_eq!(
                store.reservation(id).unwrap().unwrap().state,
                ReservationState::Reserved
            );
            assert_eq!(store.reserved_nonce(id).unwrap().unwrap().1, 5);
        } else {
            assert!(store.reservation(id).unwrap().is_none());
        }
        assert!(store.signed_payload(id).unwrap().is_none());
        assert!(
            !mock
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|(m, _)| m == "quai_sendRawTransaction")
        );
    }
}

#[cfg(feature = "abi")]
#[tokio::test]
async fn deployment_access_discovery_preserves_create_identity_and_mandatory_address() {
    use quai_sdk::accounts::{AccountAccessListPolicy, AccountObservationPolicy};
    for failure in [0, 2] {
        let (_directory, mock, _, signer, mut store) = setup();
        mock.mode.store(5, Ordering::SeqCst);
        let provider = Provider::new(
            AccessDiscoveryMock {
                base: mock.clone(),
                failure,
            },
            Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
            store.scope().chain_id,
        );
        let id = ReservationId([113; 16]);
        let mut session = AccountSession::new(&provider, &signer, &mut store)
            .unwrap()
            .with_observation_policy(AccountObservationPolicy::PinnedLatest)
            .with_access_list_policy(AccountAccessListPolicy::Discover);
        let nonce = session.reserve_deployment_nonce(id).await.unwrap();
        let deployment = quai_sdk::contracts::prepare_deployment(
            &quai_sdk::abi::AbiInterface::from_json(b"[]").unwrap(),
            &[0x60, 0, 0x60, 0, 0xf3],
            &[],
            signer.address().try_into().unwrap(),
            signer.chain_id(),
            nonce,
            U256::ZERO,
            quai_sdk::contracts::DeploymentSearch {
                start_salt: 0,
                max_attempts: 10000,
            },
            || false,
        )
        .unwrap();
        let predicted = deployment.address();
        let init = deployment.init_data().to_vec();
        let result = session.prepare_deployment(id, deployment, policy()).await;
        if failure == 0 {
            let prepared = result.unwrap();
            assert_eq!(prepared.created_address(), Some(predicted));
            assert_eq!(prepared.transaction().data, init);
            assert_eq!(prepared.transaction().nonce, nonce);
            assert_eq!(prepared.transaction().access_list.len(), 2);
            assert_eq!(
                prepared.transaction().access_list[0].address,
                predicted.address()
            );
            let signed = session.sign(&prepared).unwrap();
            assert_eq!(
                signed.transaction().access_list,
                prepared.transaction().access_list
            );
        } else {
            assert!(matches!(result, Err(AccountError::InvalidOperation)));
            assert_eq!(
                store.reservation(id).unwrap().unwrap().state,
                ReservationState::Reserved
            );
            assert!(store.signed_payload(id).unwrap().is_none());
        }
        let calls = mock.calls.lock().unwrap();
        let (_, args) = calls
            .iter()
            .find(|(m, _)| m == "quai_createAccessList")
            .unwrap();
        assert_eq!(args[1], "0x10");
        assert!(args[0].get("to").is_none());
        assert_eq!(args[0]["nonce"], "0x4");
        assert_eq!(args[0]["input"], RpcData::new(init).unwrap().to_hex());
        assert_eq!(args[0]["accessList"][0]["address"], predicted.to_string());
    }
}

#[tokio::test]
async fn external_signing_commits_only_exact_live_preparation_and_survives_restart() {
    let (directory, _, provider, local, mut store) = setup();
    let watch = quai_sdk::signer::WatchOnlySigner::new(local.address(), local.chain_id()).unwrap();
    let id = ReservationId([91; 16]);
    let mut session = AccountSession::new(&provider, &watch, &mut store).unwrap();
    let prepared = session.prepare(id, intent(), policy()).await.unwrap();
    assert!(matches!(
        session.sign(&prepared),
        Err(AccountError::Signer(SignerError::WatchOnly))
    ));
    let mut changed = prepared.transaction().clone();
    changed.value += U256::from(1);
    let wrong = local.sign_quai(&changed).unwrap();
    assert!(matches!(
        session.commit_external_signature(&prepared, &wrong),
        Err(AccountError::PayloadMismatch)
    ));
    let signed = local.sign_quai(prepared.transaction()).unwrap();
    session
        .commit_external_signature(&prepared, &signed)
        .unwrap();
    assert!(
        session
            .commit_external_signature(&prepared, &signed)
            .is_err()
    );
    let scope = store.scope();
    drop(store);
    let mut reopened = SqliteStore::open(directory.0.join("wallet.sqlite"), scope).unwrap();
    assert_eq!(
        reopened.signed_payload(id).unwrap().unwrap(),
        signed.signed_bytes().unwrap()
    );
    assert_eq!(
        reopened.reservation(id).unwrap().unwrap().state,
        ReservationState::Signed
    );
}

#[tokio::test]
async fn conversion_budget_rejects_low_caps_and_missing_quotes_before_reservation() {
    for missing_quote in [false, true] {
        let (_directory, mock, provider, signer, mut store) = setup();
        mock.mode
            .store(if missing_quote { 7 } else { 5 }, Ordering::SeqCst);
        let id = ReservationId([100; 16]);
        let result = AccountSession::new(&provider, &signer, &mut store)
            .unwrap()
            .with_observation_policy(quai_sdk::accounts::AccountObservationPolicy::PinnedLatest)
            .prepare_conversion(
                id,
                "0x0080000000000000000000000000000000000001"
                    .parse()
                    .unwrap(),
                U256::from(quai_sdk::consensus::MIN_QUAI_CONVERSION_VALUE),
                quai_sdk::consensus::ConversionSlippage::new(100).unwrap(),
                policy(),
            )
            .await;
        if missing_quote {
            assert!(matches!(result, Err(AccountError::Provider(_))));
        } else {
            assert!(matches!(result, Err(AccountError::FeeLimit)));
        }
        assert!(store.reservation(id).unwrap().is_none());
        assert!(
            !mock
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|(m, _)| m == "quai_sendRawTransaction")
        );
    }
}
