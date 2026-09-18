//! Shared native/worker tests for public address views and atomic usage refresh.
#![cfg(feature = "wallet")]
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
use quai_sdk::crypto::SecretKey;
use quai_sdk::discovery::{
    QiDiscoveryError, refresh_qi_address_book, refresh_qi_address_book_with_use_checker,
};
use quai_sdk::primitives::{Hash32, QiAddress};
use quai_sdk::rpc::{Endpoint, RpcError, Transport};
use quai_sdk::wallet::discovery::{Checkpoint, NetworkScope};
use quai_sdk::wallet::qi_addresses::{
    MAX_QI_ADDRESS_RECORDS, QiAddressBook, QiAddressStatus, QiUsageObservation,
};
use quai_sdk::wallet::{CoinType, HdWallet, Search};
use quai_sdk::{Provider, Routing, U256, Zone};
use serde_json::{Value, json};
use std::collections::BTreeMap;
#[cfg(target_arch = "wasm32")]
type Shared<T> = std::rc::Rc<std::cell::RefCell<T>>;
#[cfg(not(target_arch = "wasm32"))]
type Shared<T> = std::sync::Arc<std::sync::Mutex<T>>;
#[derive(Default)]
struct State<T>(Shared<T>);
impl<T> Clone for State<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<T> State<T> {
    fn write(&self) -> impl std::ops::DerefMut<Target = T> {
        #[cfg(target_arch = "wasm32")]
        {
            self.0.borrow_mut()
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.0.lock().unwrap()
        }
    }
}
fn hash(n: u8) -> Hash32 {
    Hash32::from_bytes([n; 32])
}
fn scope() -> NetworkScope {
    NetworkScope {
        chain_id: U256::from(9),
        genesis: hash(1),
        zone: Zone::Cyprus1,
    }
}
fn checkpoint(n: u64) -> Checkpoint {
    Checkpoint {
        hash: hash(n as u8),
        height: U256::from(n),
    }
}
fn imported() -> SecretKey {
    let mut b = [0; 32];
    b[31] = 130;
    SecretKey::from_bytes(&b).unwrap()
}
fn book() -> QiAddressBook {
    let mut book = QiAddressBook::new(scope()).unwrap();
    book.import_public(&imported().public_key()).unwrap();
    book
}
fn qi() -> QiAddress {
    imported().public_key().address().try_into().unwrap()
}
fn statuses(book: &QiAddressBook) -> Vec<(QiAddress, QiAddressStatus, Option<Checkpoint>)> {
    book.addresses()
        .map(|r| {
            (
                r.public().address().try_into().unwrap(),
                r.status(),
                r.checkpoint(),
            )
        })
        .collect()
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn scoped_origin_filters_match_branches_and_never_invent_imported_ancestry() {
    let mut b = book();
    let wallet = HdWallet::from_seed(&[7; 32], CoinType::Qi).unwrap();
    let account = wallet.account_public(0).unwrap();
    for change in [false, true] {
        let index = account
            .search(
                change,
                Search {
                    zone: Zone::Cyprus1,
                    start_index: 0,
                    max_attempts: 10_000,
                },
                || false,
            )
            .unwrap()
            .address
            .index;
        b.import_hd(&account, change, index).unwrap();
        assert_eq!(b.branch(change).count(), 1);
        assert_eq!(b.gap_branch(change).count(), 0);
    }
    assert_eq!(b.addresses().count(), 3);
    assert_eq!(b.account(0).count(), 2);
    assert_eq!(b.imported().count(), 1);
    let address = b.branch(false).next().unwrap().public().clone();
    assert!(
        b.import_public(
            &quai_sdk::crypto::PublicKey::from_sec1_bytes(address.public_key()).unwrap()
        )
        .is_err()
    );
    let observations: Vec<_> = b
        .addresses()
        .map(|r| QiUsageObservation {
            address: r.public().address().try_into().unwrap(),
            used: false,
        })
        .collect();
    b.record_observations(scope(), checkpoint(10), &observations)
        .unwrap();
    assert_eq!(b.gap_branch(false).count(), 1);
    assert_eq!(b.gap_branch(true).count(), 1);
    let old = statuses(&b);
    b.import_public(&imported().public_key()).unwrap();
    assert_eq!(statuses(&b), old);
    let mut key = [0; 32];
    key[30..].copy_from_slice(&805u16.to_be_bytes());
    assert!(
        b.import_public(&SecretKey::from_bytes(&key).unwrap().public_key())
            .is_err()
    );
    assert_eq!(statuses(&b), old);
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn usage_batches_reject_atomically_and_positive_or_attempted_use_is_not_erased_by_empty_reads() {
    let mut b = book();
    let empty = QiUsageObservation {
        address: qi(),
        used: false,
    };
    b.record_observations(scope(), checkpoint(10), &[empty])
        .unwrap();
    assert_eq!(b.address(qi()).unwrap().status(), QiAddressStatus::Unused);
    b.mark_attempted(qi()).unwrap();
    b.record_observations(scope(), checkpoint(11), &[empty])
        .unwrap();
    assert_eq!(
        b.address(qi()).unwrap().status(),
        QiAddressStatus::AttemptedUse
    );
    b.record_observations(
        scope(),
        checkpoint(12),
        &[QiUsageObservation {
            used: true,
            ..empty
        }],
    )
    .unwrap();
    b.record_observations(scope(), checkpoint(13), &[empty])
        .unwrap();
    b.mark_attempted(qi()).unwrap();
    assert_eq!(b.address(qi()).unwrap().status(), QiAddressStatus::Used);
    let old = statuses(&b);
    for which in 0..6 {
        let mut cp = checkpoint(14);
        let mut sc = scope();
        let mut batch = vec![empty];
        match which {
            0 => cp = checkpoint(12),
            1 => {
                cp = Checkpoint {
                    hash: hash(9),
                    ..checkpoint(13)
                }
            }
            2 => batch.push(empty),
            3 => sc.chain_id = U256::from(10),
            4 => cp.hash = Hash32::ZERO,
            _ => batch = vec![empty; MAX_QI_ADDRESS_RECORDS + 1],
        }
        assert!(b.record_observations(sc, cp, &batch).is_err());
        assert_eq!(statuses(&b), old);
    }
    b.invalidate_observations();
    assert_eq!(b.address(qi()).unwrap().status(), QiAddressStatus::Unknown);
    assert_eq!(b.address(qi()).unwrap().checkpoint(), None);
    b.record_observations(scope(), checkpoint(9), &[empty])
        .unwrap();
    assert_eq!(b.address(qi()).unwrap().status(), QiAddressStatus::Unused);
}
#[cfg(feature = "payments")]
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn verified_payment_receive_views_preserve_account_peer_and_cached_gap() {
    use quai_sdk::payments::{PaymentDirection, PaymentSearch, PrivatePaymentCode};
    let owner = PrivatePaymentCode::from_seed(&[7; 32], 0).unwrap();
    let peer = PrivatePaymentCode::from_seed(&[8; 32], 0).unwrap();
    let found = owner
        .search(
            peer.public_code(),
            PaymentDirection::Receive,
            PaymentSearch {
                zone: Zone::Cyprus1,
                start_index: 0,
                max_attempts: 10_000,
            },
            || false,
        )
        .unwrap();
    let mut b = book();
    b.import_payment_receive(&owner, peer.public_code(), found.index)
        .unwrap();
    assert_eq!(b.payment_channel(peer.public_code()).count(), 1);
    assert_eq!(b.account(0).count(), 1);
    assert_eq!(b.imported().count(), 1);
    assert_eq!(b.gap_payment_channel(peer.public_code()).count(), 0);
    b.record_observations(
        scope(),
        checkpoint(10),
        &[QiUsageObservation {
            address: found.address,
            used: false,
        }],
    )
    .unwrap();
    assert_eq!(b.gap_payment_channel(peer.public_code()).count(), 1);
    b.import_payment_receive(&owner, peer.public_code(), found.index)
        .unwrap();
    assert_eq!(b.gap_payment_channel(peer.public_code()).count(), 1);
    assert_eq!(b.payment_channel(owner.public_code()).count(), 0);
}
#[derive(Clone, Default)]
struct Mock {
    calls: State<usize>,
    headers: State<usize>,
    outputs: State<BTreeMap<String, Value>>,
    mode: u8,
}
impl Mock {
    fn provider(&self) -> Provider<Self> {
        Provider::new(
            self.clone(),
            Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
            scope().chain_id,
        )
    }
}
impl Transport for Mock {
    async fn request(&self, _: &Endpoint, method: &str, params: Value) -> Result<Value, RpcError> {
        *self.calls.write() += 1;
        Ok(match method {
            "quai_chainId" => json!("0x9"),
            "quai_getHeaderByNumber" if params[0] == "0x0" => {
                json!({"woHeader":{"hash":hash(if self.mode==2 {2}else{1}).to_string(),"number":"0x0","location":"0x","parentHash":Hash32::ZERO.to_string()}})
            }
            "quai_getHeaderByNumber" => {
                let mut count = self.headers.write();
                *count += 1;
                json!({"woHeader":{"hash":hash(if self.mode==1 && *count>1 {11}else{10}).to_string(),"number":"0xa","location":"0x0000","parentHash":hash(9).to_string(),"primeTerminusNumber":"0x2"},"gasLimit":"0x10000","stateLimit":"0x10000"})
            }
            "quai_getOutpointsByAddress" => {
                if self.mode == 3 {
                    return std::future::pending().await;
                }
                self.outputs
                    .write()
                    .get(params[0].as_str().unwrap())
                    .cloned()
                    .unwrap_or(json!([]))
            }
            _ => panic!("unexpected {method}"),
        })
    }
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test(async))]
async fn refresh_commits_complete_usage_and_use_hints_short_circuit_nonempty_outputs() {
    let mut b = book();
    let m = Mock::default();
    let p = m.provider();
    let hints = State::<usize>::default();
    let hint_count = hints.clone();
    refresh_qi_address_book_with_use_checker(
        &p,
        &mut b,
        4,
        || false,
        move |sc, a| {
            assert_eq!(sc, scope());
            assert_eq!(a, qi());
            *hint_count.write() += 1;
            std::future::ready(Ok(true))
        },
    )
    .await
    .unwrap();
    assert_eq!(*hints.write(), 1);
    assert_eq!(b.address(qi()).unwrap().status(), QiAddressStatus::Used);
    b.invalidate_observations();
    m.outputs.write().insert(qi().to_string(),json!([{"txHash":format!("0x00000080{}","00".repeat(28)),"index":"0x0","denomination":"0x2","lock":"0x0"}]));
    refresh_qi_address_book_with_use_checker(
        &p,
        &mut b,
        4,
        || false,
        |_, _| std::future::ready(Err(QiDiscoveryError::UseCheckFailed)),
    )
    .await
    .unwrap();
    assert_eq!(b.address(qi()).unwrap().status(), QiAddressStatus::Used);
    assert_eq!(b.address(qi()).unwrap().checkpoint(), Some(checkpoint(10)));
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test(async))]
async fn failed_reorg_cancelled_or_dropped_refresh_leaves_existing_view_intact() {
    let mut b = book();
    b.record_observations(
        scope(),
        checkpoint(8),
        &[QiUsageObservation {
            address: qi(),
            used: false,
        }],
    )
    .unwrap();
    let old = statuses(&b);
    for mode in [1, 2] {
        let m = Mock {
            mode,
            ..Default::default()
        };
        assert!(
            refresh_qi_address_book(&m.provider(), &mut b, 4, || false)
                .await
                .is_err()
        );
        assert_eq!(statuses(&b), old);
    }
    let m = Mock::default();
    let p = m.provider();
    assert!(
        refresh_qi_address_book_with_use_checker(
            &p,
            &mut b,
            4,
            || false,
            |_, _| std::future::ready(Err(QiDiscoveryError::UseCheckFailed))
        )
        .await
        .is_err()
    );
    assert_eq!(statuses(&b), old);
    let calls = *m.calls.write();
    assert!(
        refresh_qi_address_book(&p, &mut b, 0, || false)
            .await
            .is_err()
    );
    assert_eq!(*m.calls.write(), calls);
    assert!(
        refresh_qi_address_book(&p, &mut b, 4, || true)
            .await
            .is_err()
    );
    assert_eq!(*m.calls.write(), calls);
    let stalled = Mock {
        mode: 3,
        ..Default::default()
    };
    let p = stalled.provider();
    let mut work = Box::pin(refresh_qi_address_book(&p, &mut b, 4, || false));
    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);
    assert!(std::future::Future::poll(work.as_mut(), &mut cx).is_pending());
    drop(work);
    assert_eq!(statuses(&b), old);
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test(async))]
async fn multi_address_refresh_rejects_duplicates_budgets_and_partial_failure_without_cache_mutation()
 {
    let mut b = book();
    let wallet = HdWallet::from_seed(&[7; 32], CoinType::Qi).unwrap();
    let account = wallet.account_public(0).unwrap();
    let index = account
        .search(
            false,
            Search {
                zone: Zone::Cyprus1,
                start_index: 0,
                max_attempts: 10_000,
            },
            || false,
        )
        .unwrap()
        .address
        .index;
    b.import_hd(&account, false, index).unwrap();
    let old = statuses(&b);
    let m = Mock::default();
    let p = m.provider();
    let output = json!([{"txHash":format!("0x00000080{}","00".repeat(28)),"index":"0x0","denomination":"0x2","lock":"0x0"}]);
    for r in b.addresses() {
        m.outputs
            .write()
            .insert(r.public().address().to_string(), output.clone());
    }
    assert!(matches!(
        refresh_qi_address_book(&p, &mut b, 1, || false).await,
        Err(QiDiscoveryError::OutputLimit)
    ));
    assert_eq!(statuses(&b), old);
    assert!(matches!(
        refresh_qi_address_book(&p, &mut b, 2, || false).await,
        Err(QiDiscoveryError::InvalidOutputs)
    ));
    assert_eq!(statuses(&b), old);
    m.outputs.write().clear();
    let mut hints = 0;
    assert!(
        refresh_qi_address_book_with_use_checker(
            &p,
            &mut b,
            2,
            || false,
            |_, _| {
                hints += 1;
                std::future::ready(if hints == 1 {
                    Ok(true)
                } else {
                    Err(QiDiscoveryError::UseCheckFailed)
                })
            }
        )
        .await
        .is_err()
    );
    assert_eq!(hints, 2);
    assert_eq!(statuses(&b), old);
    let mut checks = 0;
    assert!(matches!(
        refresh_qi_address_book(&p, &mut b, 2, || {
            checks += 1;
            checks == 3
        })
        .await,
        Err(QiDiscoveryError::Cancelled)
    ));
    assert_eq!(statuses(&b), old);
}

/// Batching transport that records how many payload calls each batch carried.
#[derive(Clone, Default)]
struct BatchingMock {
    batches: State<Vec<usize>>,
    singles: State<usize>,
}
impl quai_sdk::rpc::Transport for BatchingMock {
    async fn request(&self, _: &Endpoint, method: &str, params: Value) -> Result<Value, RpcError> {
        *self.singles.write() += 1;
        Ok(match method {
            "quai_chainId" => json!("0x9"),
            "quai_getHeaderByNumber" if params[0] == "0x0" => {
                json!({"woHeader":{"hash":hash(1).to_string(),"number":"0x0","location":"0x","parentHash":Hash32::ZERO.to_string()}})
            }
            "quai_getHeaderByNumber" => {
                json!({"woHeader":{"hash":hash(10).to_string(),"number":"0xa","location":"0x0000","parentHash":hash(9).to_string(),"primeTerminusNumber":"0x2"},"gasLimit":"0x10000","stateLimit":"0x10000"})
            }
            "quai_getOutpointsByAddress" => json!([]),
            other => panic!("unexpected method {other}"),
        })
    }
    async fn request_batch(
        &self,
        _: &Endpoint,
        requests: Vec<(&str, Value)>,
    ) -> Option<quai_sdk::rpc::BatchResult> {
        let payload = requests.len() - 2;
        // Record only the multi-address reads, not the single bracketed reads
        // that `Provider::read` issues for headers and chain identity.
        if requests
            .iter()
            .filter(|(m, _)| *m == "quai_getOutpointsByAddress")
            .count()
            > 0
        {
            self.batches.write().push(payload);
        }
        let mut responses = vec![Ok(json!("0x9"))];
        for (method, params) in requests.iter().skip(1).take(payload) {
            responses.push(Ok(match *method {
                "quai_chainId" => json!("0x9"),
                "quai_getOutpointsByAddress" => json!([]),
                "quai_getHeaderByNumber" if params[0] == "0x0" => {
                    json!({"woHeader":{"hash":hash(1).to_string(),"number":"0x0","location":"0x","parentHash":Hash32::ZERO.to_string()}})
                }
                "quai_getHeaderByNumber" => {
                    json!({"woHeader":{"hash":hash(10).to_string(),"number":"0xa","location":"0x0000","parentHash":hash(9).to_string(),"primeTerminusNumber":"0x2"},"gasLimit":"0x10000","stateLimit":"0x10000"})
                }
                other => panic!("unexpected batched method {other}"),
            }));
        }
        responses.push(Ok(json!("0x9")));
        Some(Ok(responses))
    }
}

#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test(async))]
async fn an_address_book_refresh_reads_every_address_in_one_batch() {
    // The book is a known, fixed set with no gap rule, so every address is read
    // regardless. Previously that was one sequential round trip per address;
    // now it is one batch. This is the property, so it is asserted directly.
    let mut b = QiAddressBook::new(scope()).unwrap();
    let wallet = HdWallet::from_seed(&[11; 32], CoinType::Qi).unwrap();
    let account = wallet.account_public(0).unwrap();
    let mut expected = 0usize;
    let mut index = 0u32;
    for _ in 0..12 {
        let found = account
            .search(
                false,
                Search {
                    zone: Zone::Cyprus1,
                    start_index: index,
                    max_attempts: 100_000,
                },
                || false,
            )
            .unwrap();
        b.import_hd(&account, false, found.address.index).unwrap();
        index = found.next_index.unwrap();
        expected += 1;
    }
    assert_eq!(expected, 12);

    let mock = BatchingMock::default();
    let provider = Provider::new(
        mock.clone(),
        Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
        scope().chain_id,
    );
    refresh_qi_address_book(&provider, &mut b, 1_000, || false)
        .await
        .unwrap();

    let batches = mock.batches.write().clone();
    assert_eq!(
        batches,
        vec![12],
        "all twelve addresses must be read in one batch, got {batches:?}"
    );
}
