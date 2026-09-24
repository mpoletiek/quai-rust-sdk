//! `Contract::prove_deployment` against captured mainnet proofs: the pin
//! check with a proven code hash, and the same errors as `verify_deployment`.
#![cfg(all(feature = "abi", not(target_arch = "wasm32")))]
use quai_sdk::abi::AbiInterface;
use quai_sdk::contracts::{Contract, ContractError};
use quai_sdk::primitives::Hash32;
use quai_sdk::provider::ProviderError;
use quai_sdk::rpc::{BatchResult, Endpoint, RpcError, Transport};
use quai_sdk::{BlockTag, Provider, Routing, U256, Zone};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "fixtures/shared/test-infra/fixtures/state-proofs-mainnet.json"
    ))
    .unwrap()
}
fn hash(value: &Value) -> Hash32 {
    value.as_str().unwrap().parse().unwrap()
}
/// The captured address at `index`: WQUAI, an absent account, a funded account.
fn address(index: usize) -> quai_sdk::primitives::QuaiAddress {
    fixture()["proofs"][index]["result"]["address"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

/// A batching node that answers from the fixture, counting batches.
#[derive(Clone)]
struct Node(Arc<Value>, Arc<Mutex<usize>>);
impl Transport for Node {
    async fn request(&self, _: &Endpoint, method: &str, _: Value) -> Result<Value, RpcError> {
        panic!("a batching transport received a single {method} request")
    }
    async fn request_batch(
        &self,
        _: &Endpoint,
        requests: Vec<(&str, Value)>,
    ) -> Option<BatchResult> {
        *self.1.lock().unwrap() += 1;
        let fixture = &self.0;
        Some(Ok(requests
            .into_iter()
            .map(|(method, params)| match method {
                "quai_chainId" => Ok(json!("0x9")),
                "quai_getHeaderByNumber" if params[0] == "0x0" => Ok(fixture["genesis"].clone()),
                "quai_getHeaderByNumber" => Ok(fixture["header"].clone()),
                "quai_getProof" => {
                    let mut result = fixture["proofs"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|entry| &entry["result"])
                        .find(|r| {
                            r["address"]
                                .as_str()
                                .unwrap()
                                .eq_ignore_ascii_case(params[0].as_str().unwrap())
                        })
                        .unwrap()
                        .clone();
                    // An honest node proves only the slots asked for.
                    let asked = params[1].as_array().unwrap().clone();
                    let kept = result["storageProof"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|slot| asked.contains(&slot["key"]))
                        .cloned()
                        .collect::<Vec<_>>();
                    result["storageProof"] = json!(kept);
                    Ok(result)
                }
                _ => panic!("unexpected RPC method {method}"),
            })
            .collect()))
    }
}
fn node() -> (Provider<Node>, Arc<Mutex<usize>>) {
    let batches = Arc::new(Mutex::new(0));
    let provider = Provider::new(
        Node(Arc::new(fixture()), batches.clone()),
        Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
        U256::from(9),
    );
    (provider, batches)
}

#[tokio::test]
async fn a_pinned_contract_is_proven_without_its_bytecode() {
    let fixture = fixture();
    let genesis = hash(&fixture["genesis"]["woHeader"]["hash"]);
    let runtime = hash(&fixture["proofs"][0]["result"]["codeHash"]);
    let (provider, batches) = node();
    let wquai = Contract::new(address(0), AbiInterface::default(), &provider);
    let proven = wquai
        .prove_deployment(genesis, Some(runtime), BlockTag::Latest)
        .await
        .unwrap();
    assert_eq!(proven.code_hash(), runtime);
    assert_eq!(proven.state_root, hash(&fixture["header"]["evmRoot"]));
    assert!(proven.storage.is_empty());
    assert_eq!(*batches.lock().unwrap(), 2);
    assert!(
        wquai
            .prove_deployment(genesis, None, BlockTag::Latest)
            .await
            .is_ok()
    );
    assert!(matches!(
        wquai
            .prove_deployment(genesis, Some(Hash32::from_bytes([9; 32])), BlockTag::Latest)
            .await,
        Err(ContractError::RuntimeMismatch)
    ));
}

#[tokio::test]
async fn absent_or_codeless_accounts_and_wrong_networks_are_refused() {
    let fixture = fixture();
    let genesis = hash(&fixture["genesis"]["woHeader"]["hash"]);
    let (provider, batches) = node();
    for index in [1, 2] {
        let contract = Contract::new(address(index), AbiInterface::default(), &provider);
        let result = contract
            .prove_deployment(genesis, None, BlockTag::Latest)
            .await;
        assert!(matches!(result, Err(ContractError::MissingCode)), "{index}");
    }
    let wquai = Contract::new(address(0), AbiInterface::default(), &provider);
    *batches.lock().unwrap() = 0;
    let wrong = wquai
        .prove_deployment(Hash32::from_bytes([7; 32]), None, BlockTag::Latest)
        .await
        .unwrap_err();
    assert!(matches!(wrong, ContractError::GenesisMismatch));
    assert_eq!(
        wrong.class(),
        quai_sdk::primitives::ErrorClass::NetworkMismatch
    );
    // Stopped after the address-free round.
    assert_eq!(*batches.lock().unwrap(), 1);
    assert!(matches!(
        wquai
            .prove_deployment(Hash32::ZERO, None, BlockTag::Latest)
            .await,
        Err(ContractError::InvalidDeployment)
    ));
    assert_eq!(*batches.lock().unwrap(), 1);
    // A proof failure is a provider error the wallet should not retry.
    let error = ContractError::from(ProviderError::Proof(
        quai_sdk::provider::state_proof::ProofError::HashMismatch,
    ));
    assert_eq!(error.class(), quai_sdk::primitives::ErrorClass::Invalid);
}

#[tokio::test]
async fn contracts_sharing_an_anchor_take_one_round_each() {
    let fixture = fixture();
    let genesis = hash(&fixture["genesis"]["woHeader"]["hash"]);
    let runtime = hash(&fixture["proofs"][0]["result"]["codeHash"]);
    let (provider, batches) = node();
    let anchor = provider
        .state_anchor(genesis, Zone::Cyprus1, BlockTag::Latest)
        .await
        .unwrap();
    assert_eq!(*batches.lock().unwrap(), 1);
    let wquai = Contract::new(address(0), AbiInterface::default(), &provider);
    let proven = wquai
        .prove_deployment_at(&anchor, Some(runtime))
        .await
        .unwrap();
    assert_eq!(proven.block, anchor.block);
    assert_eq!(proven.header_hash, anchor.header.header_hash);
    let codeless = Contract::new(address(2), AbiInterface::default(), &provider);
    assert!(matches!(
        codeless.prove_deployment_at(&anchor, None).await,
        Err(ContractError::MissingCode)
    ));
    assert_eq!(*batches.lock().unwrap(), 3);
}
