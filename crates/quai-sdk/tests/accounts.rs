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
        Ok(match method {
            "quai_chainId" => json!("0x3a98"),
            "quai_getHeaderByNumber" => {
                json!({"woHeader":{"hash":GENESIS,"number":"0x0","location":"0x","parentHash":format!("0x{}","00".repeat(32))}})
            }
            "quai_getTransactionCount" => json!("0x4"),
            "quai_gasPrice" => json!("0x2"),
            "quai_estimateGas" => {
                if self.mode.load(Ordering::SeqCst) == 3 && params[0]["nonce"] != "0x4" {
                    json!("0xffff")
                } else {
                    json!("0x5208")
                }
            }
            "quai_getBalance" => json!("0xffffffffffff"),
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
    let mut session = AccountSession::new(&provider, &signer, &mut store).unwrap();
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
