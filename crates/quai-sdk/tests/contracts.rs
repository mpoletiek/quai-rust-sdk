//! Contract calldata, mutability, sender scope and typed ERC-20 return validation.
#![cfg(feature = "abi")]
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
use quai_sdk::contracts::{ContractError, DeploymentSearch, Erc20, prepare_deployment};
use quai_sdk::provider::BlockTag;
use quai_sdk::rpc::{RpcError, Transport};
use quai_sdk::{Endpoint, Provider, Routing, U256, Zone};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
#[derive(Clone, Default)]
struct Mock(Arc<Mutex<Vec<(String, Value)>>>);
impl Transport for Mock {
    async fn request(&self, _: &Endpoint, method: &str, params: Value) -> Result<Value, RpcError> {
        self.0.lock().unwrap().push((method.to_owned(), params));
        match method {
            "quai_chainId" => Ok(json!("0x3a98")),
            "quai_call" => Ok(json!(format!("0x{}", "ff".repeat(32)))),
            _ => panic!("unexpected method"),
        }
    }
}
fn provider(mock: Mock) -> Provider<Mock> {
    Provider::new(
        mock,
        Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
        U256::from(15000),
    )
}
const CONTRACT: &str = "0x0011223344556677889900112233445566778899";
const OWNER: &str = "0x0000000000000000000000000000000000000001";
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn readable_abi_survives_metadata_export_and_drives_exact_contract_calls() {
    use quai_sdk::abi::AbiInterface;
    use quai_sdk::contracts::Contract;
    let mock = Mock::default();
    let provider = provider(mock.clone());
    let interface = AbiInterface::from_human_readable(&[
        "function balanceOf(address owner) view returns (uint balance)",
        "event Transfer(address indexed from, address indexed to, uint amount)",
    ])
    .unwrap();
    let exported = interface.format_json().unwrap();
    let restored = AbiInterface::from_json(exported.as_bytes()).unwrap();
    let contract = Contract::new(CONTRACT.parse().unwrap(), restored, &provider);
    let result = contract
        .call(
            OWNER.parse().unwrap(),
            "balanceOf",
            &[json!(OWNER)],
            BlockTag::Number(U256::from(100)),
        )
        .await
        .unwrap();
    assert_eq!(result, [json!(U256::MAX.to_string())]);
    assert_eq!(
        contract.interface().format_human_readable(false).unwrap()[0],
        "function balanceOf(address owner) view returns (uint256 balance)"
    );
    let calls = mock.0.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[1].1[0]["input"],
        format!("0x70a08231{}1", "0".repeat(63))
    );
    assert_eq!(calls[1].1[1], "0x64");
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn token_balance_is_exact_and_account_call_is_block_pinned() {
    let mock = Mock::default();
    let provider = provider(mock.clone());
    let token = Erc20::new(CONTRACT.parse().unwrap(), &provider).unwrap();
    let owner = OWNER.parse().unwrap();
    assert_eq!(
        token
            .balance_of(owner, owner, BlockTag::Number(U256::from(100)))
            .await
            .unwrap(),
        U256::MAX
    );
    let calls = mock.0.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].0, "quai_call");
    assert_eq!(calls[1].1[1], "0x64");
    assert_eq!(
        calls[1].1[0]["to"],
        CONTRACT
            .parse::<quai_sdk::QuaiAddress>()
            .unwrap()
            .to_string()
    );
    assert_eq!(
        calls[1].1[0]["input"],
        format!("0x70a08231{}1", "0".repeat(63))
    );
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn exact_approval_is_offline_and_write_call_requires_explicit_simulation() {
    let mock = Mock::default();
    let provider = provider(mock.clone());
    let token = Erc20::new(CONTRACT.parse().unwrap(), &provider).unwrap();
    let owner = OWNER.parse().unwrap();
    let approval = token.approve(owner, U256::from(7)).unwrap();
    assert_eq!(approval.signature(), "approve(address,uint256)");
    assert_eq!(approval.value(), U256::ZERO);
    assert_eq!(
        approval.arguments().unwrap(),
        vec![
            json!(OWNER.parse::<quai_sdk::QuaiAddress>().unwrap().to_string()),
            json!("7")
        ]
    );
    assert_eq!(
        approval.data().to_hex(),
        format!("0x095ea7b3{}1{}7", "0".repeat(63), "0".repeat(63))
    );
    assert!(mock.0.lock().unwrap().is_empty());
    assert!(matches!(
        token
            .contract()
            .call(
                owner,
                "approve",
                &[json!(OWNER), json!("7")],
                BlockTag::Pending
            )
            .await,
        Err(ContractError::WriteFunction)
    ));
    assert!(matches!(
        token
            .contract()
            .prepare("approve", &[json!(OWNER), json!("7")], U256::from(1)),
        Err(ContractError::Nonpayable)
    ));
    assert!(matches!(
        token.transfer(
            "0x1000000000000000000000000000000000000001"
                .parse()
                .unwrap(),
            U256::from(1)
        ),
        Err(ContractError::ZoneMismatch)
    ));
    assert!(mock.0.lock().unwrap().is_empty());
    // Mock returns a full 0xff word; canonical bool decoding rejects it, never truthy.
    assert!(matches!(
        token
            .contract()
            .simulate(owner, &approval, BlockTag::Pending, Some(50_000))
            .await,
        Err(ContractError::Abi(_))
    ));
}

#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn deployment_grinds_exact_code_and_includes_mandatory_created_address_access() {
    let interface=quai_sdk::abi::AbiInterface::from_json(br#"[{"type":"constructor","stateMutability":"nonpayable","inputs":[{"name":"value","type":"uint256"}]}]"#).unwrap();
    let sender = OWNER.parse::<quai_sdk::QuaiAddress>().unwrap();
    let init = [0, 0x60, 1, 0x60, 0, 0xf3];
    let prepared = prepare_deployment(
        &interface,
        &init,
        &[json!("7")],
        sender,
        U256::from(1337),
        u64::MAX,
        U256::ZERO,
        DeploymentSearch {
            start_salt: 0,
            max_attempts: 10_000,
        },
        || false,
    )
    .unwrap();
    assert_eq!(prepared.address().zone(), sender.zone());
    assert!(prepared.init_data().starts_with(&init));
    assert_eq!(
        &prepared.init_data()[prepared.init_data().len() - 4..],
        &prepared.salt().to_be_bytes()
    );
    let address = prepared.address();
    let attempts = prepared.attempts();
    assert!((1..=10_000).contains(&attempts));
    let tx = prepared.into_transaction(100_000, U256::from(1)).unwrap();
    assert_eq!(tx.to, None);
    assert_eq!(tx.nonce, u64::MAX);
    assert_eq!(tx.access_list.len(), 1);
    assert_eq!(tx.access_list[0].address, address.address());
    assert_eq!(
        quai_sdk::primitives::contract_address(sender.address(), tx.nonce, &tx.data),
        address.address()
    );
    assert!(tx.unsigned_bytes().is_ok());
    assert!(matches!(
        prepare_deployment(
            &interface,
            &init,
            &[json!("7")],
            sender,
            U256::from(1337),
            0,
            U256::ZERO,
            DeploymentSearch {
                start_salt: 0,
                max_attempts: 10_000
            },
            || true
        ),
        Err(ContractError::SearchIncomplete)
    ));
    assert!(matches!(
        prepare_deployment(
            &interface,
            &init,
            &[json!("7")],
            sender,
            U256::from(1337),
            0,
            U256::from(1),
            DeploymentSearch {
                start_salt: 0,
                max_attempts: 10_000
            },
            || false
        ),
        Err(ContractError::Nonpayable)
    ));
}

#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn deployment_builder_matches_actual_local_node_accepted_fixture() {
    let fixture: Value = serde_json::from_str(include_str!(
        "fixtures/shared/test-infra/local-chain/acceptance-fixtures.json"
    ))
    .unwrap();
    let accepted = &fixture["deploymentTransaction"];
    let raw: quai_sdk::provider::RpcData = accepted["input"].as_str().unwrap().parse().unwrap();
    let base = &raw.bytes()[..raw.bytes().len() - 4];
    let sender = accepted["from"].as_str().unwrap().parse().unwrap();
    let prepared = prepare_deployment(
        &quai_sdk::abi::AbiInterface::from_json(b"[]").unwrap(),
        base,
        &[],
        sender,
        U256::from(1337),
        2,
        U256::ZERO,
        DeploymentSearch {
            start_salt: 0,
            max_attempts: 10_000,
        },
        || false,
    )
    .unwrap();
    assert_eq!(prepared.salt(), 55);
    assert_eq!(prepared.init_data(), raw.bytes());
    assert_eq!(
        prepared.address(),
        fixture["deploymentReceipt"]["contractAddress"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap()
    );
    let tx = prepared.into_transaction(150_000, U256::from(1)).unwrap();
    assert_eq!(
        tx.access_list[0].address,
        accepted["accessList"][0]["address"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap()
    );
    assert_eq!(fixture["deploymentReceipt"]["status"], "0x1");
    assert_eq!(fixture["failedDeploymentReceipt"]["status"], "0x0");
}

#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn prepared_access_declarations_survive_simulation_and_reject_oversize() {
    let mock = Mock::default();
    let provider = provider(mock.clone());
    let token = Erc20::new(CONTRACT.parse().unwrap(), &provider).unwrap();
    let owner = OWNER.parse().unwrap();
    let call = token
        .contract()
        .prepare("balanceOf", &[json!(OWNER)], U256::ZERO)
        .unwrap();
    let entry = quai_sdk::consensus::AccessTuple {
        address: "0x000000000000000000000000000000000000000A"
            .parse()
            .unwrap(),
        storage_keys: vec![quai_sdk::primitives::Hash32::from_bytes([7; 32])],
    };
    let list = vec![entry.clone(), entry.clone()]; // Explicit order/duplicates are not silently normalized.
    let prepared = call.clone().with_access_list(list.clone()).unwrap();
    assert_eq!(prepared.access_list(), list);
    token
        .contract()
        .simulate(
            owner,
            &prepared,
            BlockTag::Number(U256::from(100)),
            Some(100000),
        )
        .await
        .unwrap();
    let calls = mock.0.lock().unwrap();
    let params = &calls
        .iter()
        .find(|(name, _)| name == "quai_call")
        .unwrap()
        .1;
    assert_eq!(params[0]["accessList"].as_array().unwrap().len(), 2);
    assert_eq!(
        params[0]["accessList"][0]["address"],
        entry.address.to_string()
    );
    assert_eq!(
        params[0]["accessList"][1]["storageKeys"][0],
        entry.storage_keys[0].to_string()
    );
    assert_eq!(params[1], "0x64");
    assert!(call.with_access_list(vec![entry; 10000]).is_err());
}

#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn packed_encoding_and_hashes_are_available_in_the_portable_facade() {
    use quai_sdk::abi::{
        AbiType, solidity_packed, solidity_packed_keccak256, solidity_packed_sha256,
    };
    let types: Vec<AbiType> = ["int16", "bytes1", "uint16", "string"]
        .iter()
        .map(|s| s.parse().unwrap())
        .collect();
    let values = [
        json!("-1"),
        json!("0x42"),
        json!("3"),
        json!("Hello, world!"),
    ];
    assert_eq!(
        quai_sdk::primitives::hexlify(&solidity_packed(&types, &values).unwrap()).unwrap(),
        "0xffff42000348656c6c6f2c20776f726c6421"
    );
    assert_eq!(
        quai_sdk::primitives::hexlify(&solidity_packed_keccak256(&types, &values).unwrap())
            .unwrap(),
        "0xa61ecacd5de1490dcd3f7dad8f517cb383f00d6839207a7d8587ded6965e7889"
    );
    assert_eq!(
        quai_sdk::primitives::hexlify(&solidity_packed_sha256(&types, &values).unwrap()).unwrap(),
        "0xff14471951451962996f0a30b1545597babb982d18bf27c04e4fa2f0a6b40195"
    );
    let array_types: Vec<AbiType> = ["int8[]", "bytes2[]"]
        .iter()
        .map(|s| s.parse().unwrap())
        .collect();
    let packed = solidity_packed(&array_types, &[json!(["-128"]), json!(["0x0001"])]).unwrap();
    assert_eq!(&packed[..31], &[255; 31]);
    assert_eq!(packed[31], 128);
    assert_eq!(&packed[32..34], &[0, 1]);
    assert_eq!(&packed[34..], &[0; 30]);
}
