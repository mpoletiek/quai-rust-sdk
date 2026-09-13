//! Portable denomination planning and browser allocation/custody composition.
#![cfg(feature = "backup")]
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
use quai_sdk::consensus::{
    ConversionSlippage, Denomination, OutPoint, QiConversionIntent, QiWrappingIntent,
    SignedQiOperation,
};
use quai_sdk::crypto::SecretKey;
use quai_sdk::primitives::{Hash32, get_bytes};
use quai_sdk::qi_preflight::{
    QiFeeMode, QiIntent, QiOperationIntent, QiPolicy, QiPreflightError, QiQuoteRequest, QiSource,
    quote_qi,
};
use quai_sdk::rpc::{RpcError, Transport};
use quai_sdk::wallet::discovery::{Checkpoint, NetworkScope};
use quai_sdk::wallet::metadata::PublicAddress;
use quai_sdk::wallet::{CandidateCoin, CoinType, HdWallet, Search, SelectionError, SweepMode};
use quai_sdk::{Endpoint, Provider, Routing, U256, Zone};
use serde_json::{Value, json};
use std::collections::BTreeMap;
#[cfg(target_arch = "wasm32")]
type Shared = std::rc::Rc<std::cell::RefCell<State>>;
#[cfg(not(target_arch = "wasm32"))]
type Shared = std::sync::Arc<std::sync::Mutex<State>>;
#[derive(Default)]
struct State {
    mode: u8,
    calls: Vec<(String, Value)>,
    fee_calls: u8,
    stall: bool,
    sends: Vec<String>,
    outpoints: BTreeMap<String, Value>,
}
#[derive(Default, Clone)]
struct Mock(Shared);
impl Mock {
    fn state(&self) -> impl std::ops::DerefMut<Target = State> {
        #[cfg(target_arch = "wasm32")]
        {
            self.0.borrow_mut()
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.0.lock().unwrap()
        }
    }
    fn provider(&self) -> Provider<Self> {
        Provider::new(
            self.clone(),
            Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
            scope().chain_id,
        )
    }
}
fn hash(n: u8) -> Hash32 {
    Hash32::from_bytes([n; 32])
}
fn scope() -> NetworkScope {
    NetworkScope {
        chain_id: U256::from(15000),
        genesis: hash(1),
        zone: Zone::Cyprus1,
    }
}
fn key() -> SecretKey {
    let mut b = [0; 32];
    b[31] = 130;
    SecretKey::from_bytes(&b).unwrap()
}
fn wallet() -> HdWallet {
    HdWallet::from_seed(&[7; 32], CoinType::Qi).unwrap()
}
fn change() -> PublicAddress {
    let a = wallet().account_public(0).unwrap();
    let d = a
        .search(
            true,
            Search {
                zone: scope().zone,
                start_index: 0,
                max_attempts: 4000,
            },
            || false,
        )
        .unwrap()
        .address;
    PublicAddress::derive(&a, true, d.index).unwrap()
}
fn source() -> QiSource {
    let mut h = [0; 32];
    h[3] = 0x80;
    h[31] = 1;
    let owner = PublicAddress::imported(&key().public_key()).unwrap();
    QiSource {
        scope: scope(),
        checkpoint: Checkpoint {
            hash: hash(2),
            height: U256::from(16),
        },
        coins: vec![CandidateCoin {
            outpoint: OutPoint {
                transaction_hash: Hash32::from_bytes(h),
                index: 0,
            },
            address: owner.address().try_into().unwrap(),
            denomination: Denomination::new(2).unwrap(),
            unlock_height: U256::ZERO,
            expires_at: None,
            reserved: false,
        }],
        owners: vec![owner],
    }
}
fn intent() -> QiOperationIntent {
    QiOperationIntent::Transfer(QiIntent {
        amount: U256::from(5),
        destinations: vec![
            "0x0080000000000000000000000000000000000001"
                .parse()
                .unwrap(),
        ],
    })
}
fn policy() -> QiPolicy {
    QiPolicy {
        initial_fee: U256::ZERO,
        max_fee: U256::from(5),
        max_inputs: 4,
        max_outputs: 16,
        max_fee_rounds: 4,
        max_snapshot_age: 2,
    }
}
fn special(wrap: bool) -> QiOperationIntent {
    let destination = "0x0000000000000000000000000000000000000001"
        .parse()
        .unwrap();
    if wrap {
        QiOperationIntent::Wrapping {
            amount: U256::from(5),
            intent: QiWrappingIntent {
                destination,
                owner_contract: "0x002b2596EcF05C93a31ff916E8b456DF6C77c750"
                    .parse()
                    .unwrap(),
            },
        }
    } else {
        QiOperationIntent::Conversion {
            amount: U256::from(5),
            intent: QiConversionIntent {
                destination,
                refund: "0x0080000000000000000000000000000000000002"
                    .parse()
                    .unwrap(),
                slippage: ConversionSlippage::new(1234).unwrap(),
            },
        }
    }
}
impl Transport for Mock {
    async fn request(&self, _: &Endpoint, method: &str, params: Value) -> Result<Value, RpcError> {
        self.state().calls.push((method.into(), params.clone()));
        if method == "quai_estimateFeeForQi" {
            std::future::poll_fn(|_| {
                if self.state().stall {
                    std::task::Poll::Pending
                } else {
                    std::task::Poll::Ready(())
                }
            })
            .await;
        }
        let mut s = self.state();
        Ok(match method {
            "quai_chainId" => json!("0x3a98"),
            "quai_getHeaderByNumber" if params[0] == "0x0" => {
                json!({"woHeader":{"hash":hash(if s.mode==1{9}else{1}).to_string(),"number":"0x0","location":"0x","parentHash":Hash32::ZERO.to_string()}})
            }
            "quai_getHeaderByNumber" => {
                json!({"woHeader":{"hash":hash(if s.mode==2{9}else{2}).to_string(),"number":if params[0]=="latest"&&s.mode==3{"0x11"}else{"0x10"},"location":"0x0000","parentHash":hash(1).to_string(),"primeTerminusNumber":if s.mode==5{"0x1ac778"}else{"0x10"}},"baseFeePerGas":"0x1","gasLimit":"0x100000","stateLimit":"0x100000"})
            }
            "quai_estimateFeeForQi" => {
                s.fee_calls += 1;
                if s.mode == 4 {
                    s.mode = 3;
                }
                json!(if s.mode == 6 { "0x6" } else { "0x5" })
            }
            "quai_getLatestUTXOSetSize" => json!("0x1"),
            "quai_quaiToQi" => json!("0x5"),
            "quai_qiToQuai" => json!("0xffffffffff"),
            "quai_getOutpointsByAddress" => s
                .outpoints
                .get(params[0].as_str().unwrap())
                .cloned()
                .unwrap_or(json!([])),
            "quai_sendRawTransaction" => {
                let wire = params[0].as_str().unwrap().to_owned();
                s.sends.push(wire.clone());
                if s.mode == 7 {
                    return Err(RpcError::Timeout);
                }
                json!(
                    SignedQiOperation::decode(&get_bytes(&wire).unwrap())
                        .unwrap()
                        .hash()
                        .unwrap()
                        .to_string()
                )
            }
            _ => panic!("unexpected RPC {method}"),
        })
    }
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn exact_selection_converges_and_keeps_distinct_recipient_and_change_shapes() {
    let m = Mock::default();
    let source = source();
    let change = [change()];
    let q = quote_qi(
        &m.provider(),
        QiQuoteRequest {
            source: &source,
            intent: intent(),
            policy: policy(),
            fees: QiFeeMode::Node,
            change: &change,
        },
    )
    .await
    .unwrap();
    assert_eq!(q.fee(), U256::from(5));
    assert_eq!(q.selected_inputs(), source.coins);
    assert_eq!(q.recipient_outputs(), 1);
    assert_eq!(q.transaction().outputs.len(), 1);
    assert_eq!(q.transaction().outputs[0].denomination.value(), 5);
    assert_eq!(q.candidate_height(), U256::from(17));
    let s = m.state();
    let calls: Vec<_> = s
        .calls
        .iter()
        .filter(|(method, _)| method == "quai_estimateFeeForQi")
        .collect();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].1[0]["txOut"].as_array().unwrap().len(), 2);
    assert_eq!(calls[1].1[0]["txOut"].as_array().unwrap().len(), 1);
    assert!(s.sends.is_empty());
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn special_fee_modes_preserve_data_and_never_use_ordinary_estimation() {
    let c = [change()];
    for wrap in [false, true] {
        for estimated in [false, true] {
            let m = Mock::default();
            m.state().mode = 5;
            let fees = if estimated {
                QiFeeMode::Profile(quai_sdk::provider::QiFeeProfile::V056ShaAnchored)
            } else {
                QiFeeMode::Explicit(U256::from(5))
            };
            let q = quote_qi(
                &m.provider(),
                QiQuoteRequest {
                    source: &source(),
                    intent: special(wrap),
                    policy: policy(),
                    fees,
                    change: &c,
                },
            )
            .await
            .unwrap();
            assert_eq!(q.transaction().data.len(), if wrap { 20 } else { 22 });
            assert_eq!(q.fee(), U256::from(5));
            assert_eq!(q.fee_quote().is_some(), estimated);
            let signed = if wrap {
                SignedQiOperation::Wrapping(
                    quai_sdk::consensus::QiWrappingTransaction::from_transaction(
                        q.transaction().clone(),
                    )
                    .unwrap()
                    .sign_local(&[&key()])
                    .unwrap(),
                )
            } else {
                SignedQiOperation::Conversion(
                    quai_sdk::consensus::QiConversionTransaction::from_transaction(
                        q.transaction().clone(),
                    )
                    .unwrap()
                    .sign_local(&[&key()])
                    .unwrap(),
                )
            };
            assert_eq!(signed.transaction(), q.transaction());
            assert!(
                !m.state()
                    .calls
                    .iter()
                    .any(|(method, _)| method == "quai_estimateFeeForQi")
            );
        }
    }
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn claims_expiry_fee_bounds_changed_heads_and_invalid_modes_reject_before_custody() {
    let c = [change()];
    for variant in 0..8 {
        let m = Mock::default();
        let mut source = source();
        let mut p = policy();
        let mut fees = QiFeeMode::Node;
        match variant {
            0 => source.coins[0].reserved = true,
            1 => source.coins[0].unlock_height = U256::from(18),
            2 => source.coins[0].expires_at = Some(U256::from(17)),
            3 => m.state().mode = 2,
            4 => {
                m.state().mode = 4;
                source.coins[0].expires_at = Some(U256::from(18));
            }
            5 => m.state().mode = 6,
            6 => p.max_fee_rounds = 1,
            7 => fees = QiFeeMode::Profile(quai_sdk::provider::QiFeeProfile::V056ShaAnchored),
            _ => unreachable!(),
        }
        let result = quote_qi(
            &m.provider(),
            QiQuoteRequest {
                source: &source,
                intent: intent(),
                policy: p,
                fees,
                change: &c,
            },
        )
        .await;
        assert!(result.is_err());
        if variant == 5 {
            assert!(matches!(
                result,
                Err(QiPreflightError::Selection(
                    SelectionError::FeeBudgetExceeded
                ))
            ));
        }
        assert!(m.state().sends.is_empty());
    }
    let m = Mock::default();
    assert!(
        quote_qi(
            &m.provider(),
            QiQuoteRequest {
                source: &source(),
                intent: special(false),
                policy: policy(),
                fees: QiFeeMode::Node,
                change: &c
            }
        )
        .await
        .is_err()
    );
    assert!(m.state().calls.is_empty());
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn explicit_cross_zone_and_sweep_preserve_output_policy() {
    let m = Mock::default();
    let cross = QiIntent {
        amount: U256::from(5),
        destinations: vec![
            "0x0180000000000000000000000000000000000001"
                .parse()
                .unwrap(),
        ],
    };
    assert!(
        quote_qi(
            &m.provider(),
            QiQuoteRequest {
                source: &source(),
                intent: QiOperationIntent::Transfer(cross.clone()),
                policy: policy(),
                fees: QiFeeMode::Explicit(U256::from(5)),
                change: &[]
            }
        )
        .await
        .is_err()
    );
    let q = quote_qi(
        &m.provider(),
        QiQuoteRequest {
            source: &source(),
            intent: QiOperationIntent::CrossZone(cross.clone()),
            policy: policy(),
            fees: QiFeeMode::Explicit(U256::from(5)),
            change: &[],
        },
    )
    .await
    .unwrap();
    assert_eq!(
        q.transaction().outputs[0].address,
        cross.destinations[0].address()
    );
    let sweep = QiOperationIntent::Sweep {
        destinations: vec![
            "0x0080000000000000000000000000000000000001"
                .parse()
                .unwrap(),
        ],
        mode: SweepMode::PreserveDenominations,
    };
    let q = quote_qi(
        &m.provider(),
        QiQuoteRequest {
            source: &source(),
            intent: sweep,
            policy: policy(),
            fees: QiFeeMode::Explicit(U256::from(5)),
            change: &[],
        },
    )
    .await
    .unwrap();
    assert_eq!(q.transaction().outputs.len(), 1);
    assert_eq!(q.transaction().outputs[0].denomination.value(), 5);
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn known_address_extension_is_bounded_and_applies_no_partial_results() {
    use quai_sdk::qi_preflight::include_known_qi_addresses;
    let m = Mock::default();
    let original = source();
    let owner = original.owners[0].clone();
    let point = original.coins[0].outpoint;
    let output = json!({"txHash":point.transaction_hash.to_string(),"index":"0x0","denomination":"0x2","lock":"0x0"});
    m.state()
        .outpoints
        .insert(owner.address().to_string(), json!([output.clone()]));
    let mut empty = source();
    empty.coins.clear();
    empty.owners.clear();
    include_known_qi_addresses(&m.provider(), &mut empty, std::slice::from_ref(&owner), 2)
        .await
        .unwrap();
    assert_eq!(empty.coins, original.coins);
    assert_eq!(empty.owners, original.owners);
    let additional = change();
    m.state()
        .outpoints
        .insert(additional.address().to_string(), json!([output]));
    assert!(
        include_known_qi_addresses(&m.provider(), &mut empty, &[additional], 2)
            .await
            .is_err()
    );
    assert_eq!(empty.coins, original.coins);
    assert_eq!(empty.owners, original.owners);
}
#[cfg(all(target_arch = "wasm32", feature = "browser"))]
mod browser {
    use super::*;
    use quai_sdk::browser_addresses::BrowserAddressBook;
    use quai_sdk::browser_qi::BrowserQiBook;
    use quai_sdk::browser_qi_transactions::{
        BrowserQiChangePool, BrowserQiRequest, BrowserQiSession, BrowserQiTransactionError,
    };
    use quai_sdk::wallet::allocation::AddressAllocationId;
    use quai_sdk::wallet::qi_custody::{ReservationId, ReservationState};
    use quai_sdk::wallet::qi_keys::QiKeyring;
    fn id(n: u8) -> ReservationId {
        ReservationId([n; 16])
    }
    fn name() -> String {
        let mut b = [0; 16];
        quai_sdk::crypto::fill_random(&mut b).unwrap();
        format!("qi-session-{:x}", u128::from_be_bytes(b))
    }
    async fn books() -> (String, BrowserAddressBook, BrowserQiBook) {
        let n = name();
        let addresses = BrowserAddressBook::open(&n, scope(), wallet().account_public(0).unwrap())
            .await
            .unwrap();
        addresses.initialize(0, 0).await.unwrap();
        let b = BrowserQiBook::open(&n, scope(), hash(7)).await.unwrap();
        b.initialize().await.unwrap();
        (n, addresses, b)
    }
    async fn pool(book: &BrowserAddressBook, n: u8) -> BrowserQiChangePool {
        BrowserQiChangePool::allocate(book, &[AddressAllocationId([n; 16])], 4000, || false)
            .await
            .unwrap()
    }
    fn request() -> BrowserQiRequest {
        BrowserQiRequest {
            intent: intent(),
            policy: policy(),
            fees: QiFeeMode::Node,
        }
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn fresh_change_exact_fee_signing_and_restart_replay_keep_claims_and_ranges() {
        let m = Mock::default();
        let p = m.provider();
        let (name, a, b) = books().await;
        let w = wallet();
        let mut keys = QiKeyring::new(Some(&w)).unwrap();
        keys.import(key()).unwrap();
        let session = BrowserQiSession::new(&p, &b, &keys);
        let change = pool(&a, 1).await;
        let revision = b.snapshot().await.unwrap().revision;
        let prepared = session
            .prepare_observed(id(1), revision, source(), request(), change)
            .await
            .unwrap();
        assert_eq!(prepared.fee(), U256::from(5));
        assert_eq!(prepared.recipient_outputs(), 1);
        assert_eq!(a.snapshot().await.unwrap().book.next_index(true), 4000);
        let signed = prepared.sign(&keys).await.unwrap();
        assert_eq!(signed.transaction(), prepared.transaction());
        assert!(
            BrowserQiChangePool::allocate(&a, &[AddressAllocationId([1; 16])], 4000, || false)
                .await
                .is_err()
        );
        m.state().mode = 7;
        assert!(session.broadcast(id(1)).await.is_err());
        m.state().mode = 0;
        let reopened = BrowserQiBook::open(&name, scope(), hash(7)).await.unwrap();
        let resumed = BrowserQiSession::new(&p, &reopened, &keys);
        assert_eq!(
            resumed.broadcast(id(1)).await.unwrap().transaction_hash,
            signed.hash().unwrap()
        );
        {
            let s = m.state();
            assert_eq!(s.sends.len(), 2);
            assert_eq!(s.sends[0], s.sends[1]);
        }
        let snapshot = b.snapshot().await.unwrap();
        assert_eq!(
            snapshot.book.operation(id(1)).unwrap().state,
            ReservationState::Submitted
        );
        let change = pool(&a, 2).await;
        let revision = b.snapshot().await.unwrap().revision;
        assert!(matches!(
            session
                .prepare_observed(id(2), revision, source(), request(), change)
                .await,
            Err(BrowserQiTransactionError::Preflight(
                QiPreflightError::Selection(SelectionError::InsufficientFunds)
            ))
        ));
        assert_eq!(b.snapshot().await.unwrap().revision, revision);
        assert!(b.release_unsigned(id(1)).await.is_err());
        assert_eq!(a.snapshot().await.unwrap().book.next_index(true), 8000);
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn typed_conversion_and_wrapping_prepare_sign_and_submit_exact_data() {
        for wrap in [false, true] {
            let m = Mock::default();
            m.state().mode = 5;
            let p = m.provider();
            let (_, a, b) = books().await;
            let w = wallet();
            let mut keys = QiKeyring::new(Some(&w)).unwrap();
            keys.import(key()).unwrap();
            let session = BrowserQiSession::new(&p, &b, &keys);
            let c = pool(&a, 1).await;
            let revision = b.snapshot().await.unwrap().revision;
            let prepared = session
                .prepare_observed(
                    id(1),
                    revision,
                    source(),
                    BrowserQiRequest {
                        intent: special(wrap),
                        policy: policy(),
                        fees: QiFeeMode::Profile(quai_sdk::provider::QiFeeProfile::V056ShaAnchored),
                    },
                    c,
                )
                .await
                .unwrap();
            let signed = prepared.sign(&keys).await.unwrap();
            assert_eq!(signed.transaction().data.len(), if wrap { 20 } else { 22 });
            assert_eq!(
                session.broadcast(id(1)).await.unwrap().transaction_hash,
                signed.hash().unwrap()
            );
            assert!(
                !m.state()
                    .calls
                    .iter()
                    .any(|(method, _)| method == "quai_estimateFeeForQi")
            );
        }
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn current_hd_discovery_flows_through_exact_planning_and_custody() {
        use quai_sdk::discovery::QiDiscoveryOptions;
        use quai_sdk::wallet::discovery::IndexRange;
        let m = Mock::default();
        let p = m.provider();
        let (_, a, b) = books().await;
        let w = wallet();
        let account = w.account_public(0).unwrap();
        let found = account
            .search(
                false,
                Search {
                    zone: scope().zone,
                    start_index: 0,
                    max_attempts: 4000,
                },
                || false,
            )
            .unwrap()
            .address;
        let point = source().coins[0].outpoint;
        m.state().outpoints.insert(found.address.to_string(),json!([{"txHash":point.transaction_hash.to_string(),"index":"0x0","denomination":"0x2","lock":"0x0"}]));
        let options = QiDiscoveryOptions {
            receive: IndexRange {
                start: found.index,
                end: found.index + 1,
            },
            change: IndexRange { start: 0, end: 0 },
            gap_limit: Some(50),
            max_addresses: 1,
            max_outpoints: 10,
        };
        let session = BrowserQiSession::new(&p, &b, &w);
        let c = pool(&a, 1).await;
        let prepared = session
            .prepare_discovered(id(1), &account, &options, request(), c, || false)
            .await
            .unwrap();
        assert_eq!(
            prepared.transaction().inputs[0].public_key.address(),
            found.address
        );
        prepared.sign(&w).await.unwrap();
        assert!(b.snapshot().await.unwrap().book.claimed(point));
        assert_eq!(quai_sdk::discovery::DEFAULT_QI_GAP, 50);
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn persisted_addresses_beyond_scan_are_included_and_allocator_races_abort_claims() {
        use quai_sdk::discovery::QiDiscoveryOptions;
        use quai_sdk::wallet::discovery::IndexRange;
        use std::{future::Future, pin::Pin, task::Poll};
        async fn once<F: Future>(mut f: Pin<&mut F>) -> Poll<F::Output> {
            std::future::poll_fn(|cx| Poll::Ready(f.as_mut().poll(cx))).await
        }
        let m = Mock::default();
        let p = m.provider();
        let (_, a, b) = books().await;
        let w = wallet();
        let account = w.account_public(0).unwrap();
        a.reserve(AddressAllocationId([10; 16]), false, 4000)
            .await
            .unwrap();
        let known = a
            .allocate(AddressAllocationId([11; 16]), false, 4000, || false)
            .await
            .unwrap();
        let point = source().coins[0].outpoint;
        m.state().outpoints.insert(known.address().to_string(),json!([{"txHash":point.transaction_hash.to_string(),"index":"0x0","denomination":"0x2","lock":"0x0"}]));
        let options = QiDiscoveryOptions {
            receive: IndexRange { start: 0, end: 1 },
            change: IndexRange { start: 0, end: 0 },
            gap_limit: Some(50),
            max_addresses: 1,
            max_outpoints: 10,
        };
        let session = BrowserQiSession::new(&p, &b, &w);
        let prepared = session
            .prepare_discovered(
                id(1),
                &account,
                &options,
                request(),
                pool(&a, 1).await,
                || false,
            )
            .await
            .unwrap();
        assert_eq!(
            prepared.transaction().inputs[0].public_key.address(),
            known.address()
        );
        b.release_unsigned(id(1)).await.unwrap();
        let c = pool(&a, 2).await;
        let revision = b.snapshot().await.unwrap().revision;
        {
            let mut state = m.state();
            state.stall = true;
            state.calls.clear();
        }
        let mut work =
            Box::pin(session.prepare_discovered(id(2), &account, &options, request(), c, || false));
        for _ in 0..6 {
            assert!(once(work.as_mut()).await.is_pending());
            b.snapshot().await.unwrap();
            a.snapshot().await.unwrap();
            if m.state()
                .calls
                .iter()
                .any(|(method, _)| method == "quai_estimateFeeForQi")
            {
                break;
            }
        }
        assert!(
            m.state()
                .calls
                .iter()
                .any(|(method, _)| method == "quai_estimateFeeForQi")
        );
        a.reserve(AddressAllocationId([12; 16]), true, 1)
            .await
            .unwrap();
        m.state().stall = false;
        assert!(matches!(
            work.await,
            Err(BrowserQiTransactionError::Custody(
                quai_sdk::browser_qi::BrowserQiError::Browser(
                    quai_sdk::browser::BrowserError::StorageConflict
                )
            ))
        ));
        assert_eq!(b.snapshot().await.unwrap().revision, revision);
        assert!(b.snapshot().await.unwrap().book.operation(id(2)).is_none());
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn concurrent_input_claim_and_cancelled_quote_cannot_commit_stale_selection() {
        use std::{future::Future, pin::Pin, task::Poll};
        async fn once<F: Future>(mut f: Pin<&mut F>) -> Poll<F::Output> {
            std::future::poll_fn(|cx| Poll::Ready(f.as_mut().poll(cx))).await
        }
        let m = Mock::default();
        let p = m.provider();
        let (_, a, b) = books().await;
        let w = wallet();
        let mut keys = QiKeyring::new(Some(&w)).unwrap();
        keys.import(key()).unwrap();
        let session = BrowserQiSession::new(&p, &b, &keys);
        let c = pool(&a, 1).await;
        let revision = b.snapshot().await.unwrap().revision;
        m.state().stall = true;
        let mut f = Box::pin(session.prepare_observed(id(1), revision, source(), request(), c));
        assert!(once(f.as_mut()).await.is_pending());
        b.snapshot().await.unwrap();
        assert!(once(f.as_mut()).await.is_pending());
        let source = source();
        b.reserve(
            revision,
            id(9),
            source.checkpoint,
            U256::from(17),
            &source.coins,
            &source.owners,
        )
        .await
        .unwrap();
        m.state().stall = false;
        assert!(matches!(
            f.await,
            Err(BrowserQiTransactionError::Custody(
                quai_sdk::browser_qi::BrowserQiError::Browser(
                    quai_sdk::browser::BrowserError::StorageConflict
                )
            ))
        ));
        assert!(b.snapshot().await.unwrap().book.operation(id(1)).is_none());
        b.release_unsigned(id(9)).await.unwrap();
        let c = pool(&a, 2).await;
        let before = b.snapshot().await.unwrap().revision;
        m.state().stall = true;
        let mut f = Box::pin(session.prepare_observed(id(2), before, source, request(), c));
        assert!(once(f.as_mut()).await.is_pending());
        b.snapshot().await.unwrap();
        assert!(once(f.as_mut()).await.is_pending());
        drop(f);
        assert_eq!(b.snapshot().await.unwrap().revision, before);
        assert_eq!(a.snapshot().await.unwrap().book.next_index(true), 8000);
        assert!(m.state().sends.is_empty());
    }
}
