//! Provider routing, protocol validation and fail-closed read regressions.
use quai_primitives::{Address, QuaiAddress, Shard, Zone};
use quai_provider::{BlockTag, Provider, ProviderError};
use quai_rpc::{Endpoint, RouteError, Routing, RpcError, Transport, U256};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

const LOCAL: &str = "http://127.0.0.1:9200/private-prefix?fixture=public";
const CYPRUS: Shard = Shard::Zone(Zone::Cyprus1);

#[derive(Clone, Debug, PartialEq)]
struct Request {
    endpoint: String,
    method: String,
    params: Value,
}

#[derive(Clone, Default)]
struct MockTransport {
    requests: Arc<Mutex<Vec<Request>>>,
    responses: Arc<Mutex<VecDeque<Result<Value, RpcError>>>>,
}

impl MockTransport {
    fn with_responses(responses: impl IntoIterator<Item = Result<Value, RpcError>>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into_iter().collect())),
            ..Self::default()
        }
    }

    fn calls(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }

    fn assert_drained(&self) {
        assert!(
            self.responses.lock().unwrap().is_empty(),
            "expected response not consumed"
        );
    }
}

impl Transport for MockTransport {
    async fn request(
        &self,
        endpoint: &Endpoint,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        self.requests.lock().unwrap().push(Request {
            endpoint: endpoint.as_str().into(),
            method: method.into(),
            params,
        });
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected network request")
    }
}

fn provider(transport: &MockTransport, routing: Routing) -> Provider<MockTransport> {
    Provider::new(transport.clone(), routing, U256::from(1337))
}

fn account(zone: Zone) -> QuaiAddress {
    let mut bytes = [0x35; 20];
    bytes[0] = zone.byte();
    QuaiAddress::try_from(Address::from_bytes(bytes)).unwrap()
}

fn request(endpoint: &str, method: &str, params: Value) -> Request {
    Request {
        endpoint: endpoint.into(),
        method: method.into(),
        params,
    }
}

#[tokio::test]
async fn chain_mismatch_prevents_state_query() {
    let transport = MockTransport::with_responses([Ok(json!("0x3a98"))]);
    let provider = provider(&transport, Routing::direct(LOCAL, CYPRUS).unwrap());
    let result = provider
        .balance(account(Zone::Cyprus1), BlockTag::Latest)
        .await;
    assert!(
        matches!(result, Err(ProviderError::ChainMismatch { expected, actual })
        if expected == U256::from(1337) && actual == U256::from(15000))
    );
    assert_eq!(
        transport.calls(),
        vec![request(LOCAL, "quai_chainId", json!([]))]
    );
    transport.assert_drained();
}

#[tokio::test]
async fn invalid_chain_response_and_transport_error_stop_state_reads() {
    for response in [Ok(json!(1337)), Ok(json!("0x0539")), Err(RpcError::Timeout)] {
        let transport = MockTransport::with_responses([response]);
        let provider = provider(&transport, Routing::direct(LOCAL, CYPRUS).unwrap());
        assert!(provider.block_number(CYPRUS).await.is_err());
        assert_eq!(
            transport.calls(),
            vec![request(LOCAL, "quai_chainId", json!([]))]
        );
    }
}

#[tokio::test]
async fn unavailable_address_zone_never_falls_back_or_contacts_default() {
    let transport = MockTransport::default();
    let provider = provider(&transport, Routing::direct(LOCAL, CYPRUS).unwrap());
    let result = provider
        .balance(account(Zone::Hydra3), BlockTag::Latest)
        .await;
    assert!(matches!(
        result,
        Err(ProviderError::Route(RouteError::ShardUnavailable(
            Shard::Zone(Zone::Hydra3)
        )))
    ));
    assert!(transport.calls().is_empty());
}

#[tokio::test]
async fn missing_prime_route_prevents_discovery_request() {
    let transport = MockTransport::default();
    let provider = provider(&transport, Routing::direct(LOCAL, CYPRUS).unwrap());
    assert!(matches!(
        provider.running_zones().await,
        Err(ProviderError::Route(RouteError::ShardUnavailable(
            Shard::Prime
        )))
    ));
    assert!(transport.calls().is_empty());
}

#[tokio::test]
async fn account_zone_routes_chain_check_and_balance_to_same_explicit_endpoint() {
    let transport = MockTransport::with_responses([Ok(json!("0x539")), Ok(json!("0xa"))]);
    let destination = "http://127.0.0.1:9242/custom/hydra3";
    let routes = Routing::explicit([
        (CYPRUS, Endpoint::parse(LOCAL).unwrap()),
        (
            Shard::Zone(Zone::Hydra3),
            Endpoint::parse(destination).unwrap(),
        ),
    ])
    .unwrap();
    let provider = provider(&transport, routes);
    let address = account(Zone::Hydra3);
    assert_eq!(
        provider.balance(address, BlockTag::Latest).await.unwrap(),
        U256::from(10)
    );
    assert_eq!(
        transport.calls(),
        vec![
            request(destination, "quai_chainId", json!([])),
            request(
                destination,
                "quai_getBalance",
                json!([address.to_string(), "latest"])
            ),
        ]
    );
}

#[tokio::test]
async fn block_selectors_use_exact_node_parameters() {
    for (block, expected) in [
        (BlockTag::Latest, "latest"),
        (BlockTag::Pending, "pending"),
        (BlockTag::Number(U256::ZERO), "0x0"),
        (BlockTag::Number(U256::from(26)), "0x1a"),
        (
            BlockTag::Number(U256::from(i64::MAX as u64)),
            "0x7fffffffffffffff",
        ),
    ] {
        let transport = MockTransport::with_responses([Ok(json!("0x539")), Ok(json!("0x0"))]);
        let provider = provider(&transport, Routing::direct(LOCAL, CYPRUS).unwrap());
        let address = account(Zone::Cyprus1);
        assert_eq!(provider.balance(address, block).await.unwrap(), U256::ZERO);
        assert_eq!(
            transport.calls(),
            vec![
                request(LOCAL, "quai_chainId", json!([])),
                request(
                    LOCAL,
                    "quai_getBalance",
                    json!([address.to_string(), expected])
                ),
            ]
        );
    }
}

#[tokio::test]
async fn oversized_block_numbers_are_rejected_before_rpc_and_cannot_become_hashes() {
    // Pinned go-quai rpc/types.go treats any 66-character selector as a HASH,
    // while numeric selectors must fit signed int64. MAX must not change meaning.
    for number in [U256::from(i64::MAX as u64) + U256::from(1), U256::MAX] {
        let transport = MockTransport::default();
        let provider = provider(&transport, Routing::direct(LOCAL, CYPRUS).unwrap());
        assert!(matches!(
            provider
                .balance(account(Zone::Cyprus1), BlockTag::Number(number))
                .await,
            Err(ProviderError::BlockNumberOutOfRange)
        ));
        assert!(transport.calls().is_empty());
    }
}

#[tokio::test]
async fn account_amounts_retain_all_256_bits() {
    let maximum = format!("0x{}", "f".repeat(64));
    let transport = MockTransport::with_responses([Ok(json!("0x539")), Ok(json!(maximum))]);
    let provider = provider(&transport, Routing::direct(LOCAL, CYPRUS).unwrap());
    assert_eq!(
        provider
            .balance(account(Zone::Cyprus1), BlockTag::Latest)
            .await
            .unwrap(),
        U256::MAX
    );
    transport.assert_drained();
}

#[tokio::test]
async fn amounts_reject_non_quantity_and_overflow_results() {
    for response in [
        json!(null),
        json!(1),
        json!(true),
        json!("0x"),
        json!("0x01"),
        json!("-1"),
        json!(format!("0x1{}", "0".repeat(64))),
    ] {
        let transport = MockTransport::with_responses([Ok(json!("0x539")), Ok(response)]);
        let provider = provider(&transport, Routing::direct(LOCAL, CYPRUS).unwrap());
        assert!(
            provider
                .balance(account(Zone::Cyprus1), BlockTag::Latest)
                .await
                .is_err()
        );
        transport.assert_drained();
    }
}

#[tokio::test]
async fn block_number_preserves_method_and_direct_no_pathing_endpoint() {
    let transport = MockTransport::with_responses([Ok(json!("0x539")), Ok(json!("0x76bf37"))]);
    let provider = provider(
        &transport,
        Routing::with_pathing(LOCAL, CYPRUS, false).unwrap(),
    );
    assert_eq!(
        provider.block_number(CYPRUS).await.unwrap(),
        U256::from(0x76bf37)
    );
    assert_eq!(
        transport.calls(),
        vec![
            request(LOCAL, "quai_chainId", json!([])),
            request(LOCAL, "quai_blockNumber", json!([])),
        ]
    );
}

#[tokio::test]
async fn running_zones_uses_prime_and_does_not_add_advertised_routes() {
    let transport =
        MockTransport::with_responses([Ok(json!("0x539")), Ok(json!([[2, 2], [0, 0], [1, 2]]))]);
    let provider = provider(
        &transport,
        Routing::gateway("https://gateway.example/rpc", [Shard::Prime]).unwrap(),
    );
    assert_eq!(
        provider.running_zones().await.unwrap(),
        vec![Zone::Hydra3, Zone::Cyprus1, Zone::Paxos3]
    );
    assert!(matches!(
        provider.block_number(CYPRUS).await,
        Err(ProviderError::Route(RouteError::ShardUnavailable(CYPRUS)))
    ));
    assert_eq!(
        transport.calls(),
        vec![
            request(
                "https://gateway.example/rpc/prime",
                "quai_chainId",
                json!([])
            ),
            request(
                "https://gateway.example/rpc/prime",
                "quai_listRunningChains",
                json!([])
            ),
        ]
    );
}

#[tokio::test]
async fn malformed_running_zones_are_rejected_as_a_whole() {
    for response in [
        json!(null),
        json!({}),
        json!([0, 0]),
        json!([[0]]),
        json!([[0, 0, 0]]),
        json!([[true, 0]]),
        json!([[0.0, 0]]),
        json!([["0", 0]]),
        json!([[-1, 0]]),
        json!([[3, 0]]),
        json!([[0, 3]]),
        json!([[0, 0], [0, 0]]),
        json!([[0, 0], [2, 256]]),
    ] {
        let transport = MockTransport::with_responses([Ok(json!("0x539")), Ok(response.clone())]);
        let provider = provider(
            &transport,
            Routing::direct("http://localhost:9001", Shard::Prime).unwrap(),
        );
        assert!(
            matches!(
                provider.running_zones().await,
                Err(ProviderError::InvalidResult(_))
            ),
            "accepted {response}"
        );
        transport.assert_drained();
    }
}

#[tokio::test]
async fn empty_running_zones_is_valid_without_inventing_default_zone() {
    let transport = MockTransport::with_responses([Ok(json!("0x539")), Ok(json!([]))]);
    let provider = provider(
        &transport,
        Routing::direct("http://localhost:9001", Shard::Prime).unwrap(),
    );
    assert!(provider.running_zones().await.unwrap().is_empty());
}

#[tokio::test]
async fn genesis_reads_root_header_on_zone_route_with_chain_preflight() {
    for (fixture, expected) in [
        (
            include_str!("fixtures/lan-mainnet-genesis.json"),
            "0xac81c28f1a72591b87b5f16c9793cdc0e87c45c6d426d1a364c3b8f6386b5b8b",
        ),
        (
            include_str!("fixtures/orchard-genesis.json"),
            "0x663a73416275109a01aad3a4c29ea9e310aded63c5eea491243b7312ad8cd16b",
        ),
    ] {
        let transport = MockTransport::with_responses([
            Ok(json!("0x539")),
            Ok(serde_json::from_str(fixture).unwrap()),
        ]);
        let p = provider(&transport, Routing::direct(LOCAL, CYPRUS).unwrap());
        assert_eq!(
            p.genesis_hash(Zone::Cyprus1).await.unwrap().to_string(),
            expected
        );
        assert_eq!(
            transport.calls(),
            vec![
                request(LOCAL, "quai_chainId", json!([])),
                request(LOCAL, "quai_getHeaderByNumber", json!(["0x0"]))
            ]
        );
        transport.assert_drained();
    }
    let transport = MockTransport::with_responses([Ok(json!("0x9"))]);
    let p = provider(&transport, Routing::direct(LOCAL, CYPRUS).unwrap());
    assert!(matches!(
        p.genesis_hash(Zone::Cyprus1).await,
        Err(ProviderError::ChainMismatch { .. })
    ));
    assert_eq!(transport.calls().len(), 1);
}

#[tokio::test]
async fn genesis_rejects_missing_or_wrong_height_location_parent_and_hash() {
    let valid: Value =
        serde_json::from_str(include_str!("fixtures/lan-mainnet-genesis.json")).unwrap();
    let mut cases = vec![Value::Null, json!({})];
    for (field, value) in [
        ("number", json!("0x1")),
        ("number", json!("0x00")),
        ("location", json!("0x0000")),
        ("location", json!("0x00")),
        ("parentHash", json!(format!("0x{}", "11".repeat(32)))),
        ("hash", json!("0x1")),
        ("hash", json!(format!("0x{}", "00".repeat(32)))),
    ] {
        let mut invalid = valid.clone();
        invalid["woHeader"][field] = value;
        cases.push(invalid);
    }
    for invalid in cases {
        let transport = MockTransport::with_responses([Ok(json!("0x539")), Ok(invalid)]);
        let p = provider(&transport, Routing::direct(LOCAL, CYPRUS).unwrap());
        assert!(p.genesis_hash(Zone::Cyprus1).await.is_err());
    }
}
