//! Canonical code observations and checked wrapper bindings, without transaction sends.
#![cfg(feature = "abi")]
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
use quai_sdk::contracts::ContractError;
use quai_sdk::primitives::Hash32;
use quai_sdk::provider::ProviderError;
use quai_sdk::rpc::{Endpoint, RpcError, Transport};
use quai_sdk::wrappers::{WQI_ADDRESS, WQUAI_ADDRESS, WrappedQi, WrappedQuai};
use quai_sdk::{BlockTag, Provider, Routing, U256, Zone};
use serde_json::{Value, json};
#[cfg(target_arch = "wasm32")]
type Shared = std::rc::Rc<std::cell::RefCell<State>>;
#[cfg(not(target_arch = "wasm32"))]
type Shared = std::sync::Arc<std::sync::Mutex<State>>;
#[derive(Clone, Default)]
struct Mock(Shared);
#[derive(Default)]
struct State {
    mode: u8,
    code: String,
    calls: Vec<(String, Value)>,
    genesis_reads: u8,
    headers: u8,
    active: bool,
}
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
    fn new(mode: u8, code: &str) -> Self {
        let m = Self::default();
        {
            let mut s = m.state();
            s.mode = mode;
            s.code = code.into();
        }
        m
    }
    fn provider(&self) -> Provider<Self> {
        Provider::new(
            self.clone(),
            Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
            U256::from(9),
        )
    }
}
fn hash(n: u8) -> Hash32 {
    Hash32::from_bytes([n; 32])
}
fn genesis(n: u8) -> Value {
    json!({"woHeader":{"hash":hash(n).to_string(),"number":"0x0","location":"0x","parentHash":Hash32::ZERO.to_string()}})
}
fn header(n: u8) -> Value {
    json!({"gasLimit":"0x10000","stateLimit":"0x10000","woHeader":{"hash":hash(n).to_string(),"parentHash":hash(0).to_string(),"number":"0x10","primeTerminusNumber":"0x4","location":"0x0000"}})
}
struct Flight(Mock);
impl Drop for Flight {
    fn drop(&mut self) {
        self.0.state().active = false;
    }
}
impl Transport for Mock {
    async fn request(&self, _: &Endpoint, method: &str, params: Value) -> Result<Value, RpcError> {
        let wait;
        {
            let mut s = self.state();
            s.calls.push((method.into(), params.clone()));
            match method {
                "quai_chainId" => return Ok(json!(if s.mode == 5 { "0x3a98" } else { "0x9" })),
                "quai_getHeaderByNumber" if params[0] == "0x0" => {
                    s.genesis_reads += 1;
                    return Ok(genesis(if s.mode == 2 && s.genesis_reads > 1 {
                        9
                    } else {
                        1
                    }));
                }
                "quai_getHeaderByNumber" => {
                    s.headers += 1;
                    assert!(params[0] == "latest" || params[0] == "0x10");
                    if (s.mode == 3 && s.headers == 1) || (s.mode == 4 && s.headers == 2) {
                        return Ok(Value::Null);
                    }
                    return Ok(header(if s.mode == 1 && s.headers == 2 { 9 } else { 2 }));
                }
                "quai_getCode" => {
                    assert_eq!(params[1], "0x10");
                    if s.mode == 6 {
                        return Err(RpcError::Transport);
                    }
                    wait = s.mode == 7;
                    if !wait {
                        return Ok(json!(s.code));
                    }
                    s.active = true;
                }
                _ => panic!("unexpected RPC method"),
            }
        }
        assert!(wait);
        let _flight = Flight(self.clone());
        std::future::pending().await
    }
}
fn vectors() -> Vec<Value> {
    serde_json::from_slice::<Value>(include_bytes!(
        "fixtures/shared/test-infra/fixtures/contract-code.json"
    ))
    .unwrap()["vectors"]
        .as_array()
        .unwrap()
        .clone()
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn code_hashes_match_pinned_reference_at_exact_rechecked_block() {
    for row in vectors() {
        for at in [BlockTag::Latest, BlockTag::Number(U256::from(16))] {
            let code = row["code"].as_str().unwrap();
            let expected: Hash32 = row["hash"].as_str().unwrap().parse().unwrap();
            let m = Mock::new(0, code);
            let p = m.provider();
            let address = WQI_ADDRESS.parse().unwrap();
            let o = p
                .observe_contract_code(address, at, Some(expected))
                .await
                .unwrap();
            assert_eq!(o.chain_id, U256::from(9));
            assert_eq!(o.genesis, hash(1));
            assert_eq!(o.address, address);
            assert_eq!(o.block.number, 16);
            assert_eq!(o.block.hash, hash(2));
            assert_eq!(o.code.bytes.to_hex(), code);
            assert_eq!(o.code.hash, expected);
            assert_eq!(o.code.matches_expected, Some(true));
            assert_eq!(m.state().calls.len(), 10);
            assert_eq!(
                m.state().calls[5],
                ("quai_getCode".into(), json!([WQI_ADDRESS, "0x10"]))
            );
        }
    }
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn checked_wrappers_reject_empty_code_wrong_genesis_and_runtime_hash() {
    let rows = vectors();
    let good = rows[2]["code"].as_str().unwrap();
    let expected = rows[2]["hash"].as_str().unwrap().parse().unwrap();
    for qi in [false, true] {
        for case in 0..4 {
            let m = Mock::new(0, if case == 0 { "0x" } else { good });
            let p = m.provider();
            let genesis = if case == 1 { hash(9) } else { hash(1) };
            let runtime = Some(if case == 2 { hash(9) } else { expected });
            let result = if qi {
                WrappedQi::new_verified(
                    WQI_ADDRESS.parse().unwrap(),
                    &p,
                    genesis,
                    runtime,
                    BlockTag::Latest,
                )
                .await
                .map(|(_, o)| o)
            } else {
                WrappedQuai::new_verified(
                    WQUAI_ADDRESS.parse().unwrap(),
                    &p,
                    genesis,
                    runtime,
                    BlockTag::Latest,
                )
                .await
                .map(|(_, o)| o)
            };
            match case {
                0 => assert!(matches!(result, Err(ContractError::MissingCode))),
                1 => assert!(matches!(result, Err(ContractError::GenesisMismatch))),
                2 => assert!(matches!(result, Err(ContractError::RuntimeMismatch))),
                _ => assert_eq!(result.unwrap().code.hash, expected),
            }
        }
    }
    let m = Mock::new(0, "0x00");
    let p = m.provider();
    assert!(matches!(
        WrappedQuai::new_verified(
            WQUAI_ADDRESS.parse().unwrap(),
            &p,
            Hash32::ZERO,
            None,
            BlockTag::Latest
        )
        .await,
        Err(ContractError::InvalidDeployment)
    ));
    assert!(m.state().calls.is_empty());
    let (_, o) = WrappedQuai::new_verified(
        WQUAI_ADDRESS.parse().unwrap(),
        &p,
        hash(1),
        None,
        BlockTag::Latest,
    )
    .await
    .unwrap();
    assert_eq!(o.code.matches_expected, None);
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn changed_or_missing_anchors_and_chain_mismatch_never_become_verified_code() {
    for mode in 1..=6 {
        let m = Mock::new(mode, "0x00");
        let p = m.provider();
        let result = p
            .observe_contract_code(WQI_ADDRESS.parse().unwrap(), BlockTag::Latest, None)
            .await;
        match mode {
            1..=4 => assert!(matches!(result, Err(ProviderError::ObservationChanged))),
            5 => {
                assert!(matches!(result, Err(ProviderError::ChainMismatch { .. })));
                assert_eq!(m.state().calls.len(), 1);
            }
            _ => assert!(matches!(
                result,
                Err(ProviderError::Rpc(RpcError::Transport))
            )),
        }
        assert!(m.state().calls.len() <= 10);
    }
    let m = Mock::new(0, "0x00");
    let p = m.provider();
    for at in [
        BlockTag::Pending,
        BlockTag::Number(U256::ZERO),
        BlockTag::Number(U256::MAX),
    ] {
        assert!(matches!(
            p.observe_contract_code(WQI_ADDRESS.parse().unwrap(), at, None)
                .await,
            Err(ProviderError::InvalidRequest(_))
        ));
    }
    assert!(m.state().calls.is_empty());
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn cancelled_code_read_drops_transport_without_retry_or_later_observation() {
    use std::{future::Future, task::Poll};
    let m = Mock::new(7, "0x00");
    let p = m.provider();
    let mut pending =
        Box::pin(p.observe_contract_code(WQI_ADDRESS.parse().unwrap(), BlockTag::Latest, None));
    std::future::poll_fn(|cx| {
        assert!(pending.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    assert!(m.state().active);
    let calls = m.state().calls.len();
    drop(pending);
    assert!(!m.state().active);
    assert_eq!(m.state().calls.len(), calls);
}
#[cfg(all(feature = "http", not(target_arch = "wasm32")))]
#[tokio::test]
#[ignore = "requires explicit QUAI_RPC_URL, QUAI_EXPECTED_CHAIN_ID and QUAI_EXPECTED_GENESIS; read-only"]
async fn explicit_endpoint_wrapper_code_availability() {
    let url = std::env::var("QUAI_RPC_URL").unwrap();
    let chain =
        U256::from_str_radix(&std::env::var("QUAI_EXPECTED_CHAIN_ID").unwrap(), 10).unwrap();
    let genesis: Hash32 = std::env::var("QUAI_EXPECTED_GENESIS")
        .unwrap()
        .parse()
        .unwrap();
    let p = Provider::new(
        quai_sdk::HttpTransport::new(quai_sdk::HttpConfig::default()).unwrap(),
        Routing::direct(&url, Zone::Cyprus1.into()).unwrap(),
        chain,
    );
    for (kind, address) in [("WQI", WQI_ADDRESS), ("WQUAI", WQUAI_ADDRESS)] {
        let address = address.parse().unwrap();
        let o = p
            .observe_contract_code(address, BlockTag::Latest, None)
            .await
            .unwrap();
        assert_eq!(o.genesis, genesis);
        let result = if kind == "WQI" {
            WrappedQi::new_verified(
                address,
                &p,
                genesis,
                None,
                BlockTag::Number(U256::from(o.block.number)),
            )
            .await
            .map(|(_, o)| o)
        } else {
            WrappedQuai::new_verified(
                address,
                &p,
                genesis,
                None,
                BlockTag::Number(U256::from(o.block.number)),
            )
            .await
            .map(|(_, o)| o)
        };
        if o.code.bytes.bytes().is_empty() {
            assert!(matches!(result, Err(ContractError::MissingCode)));
        } else {
            assert_eq!(result.unwrap().code.hash, o.code.hash);
        }
        println!(
            "{}",
            json!({"kind":kind,"chainId":chain.to_string(),"genesis":genesis.to_string(),"address":address.to_string(),"block":o.block.number,"blockHash":o.block.hash.to_string(),"runtimeBytes":o.code.bytes.bytes().len(),"runtimeKeccak256":o.code.hash.to_string(),"checkedBindingAccepted":!o.code.bytes.bytes().is_empty()})
        );
    }
}
