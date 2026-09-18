//! How a background sync loop is told to react to each failure.
#![cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
use quai_sdk::primitives::{ErrorClass, Hash32};
use quai_sdk::provider::BroadcastError;
use quai_sdk::qi::QiError;
use quai_sdk::rpc::{Endpoint, RpcError, Transport};
use quai_sdk::wallet::discovery::{DiscoveryError, NetworkScope, ObservationSource};
use quai_sdk::wallet::storage::StorageError;
use quai_sdk::{Provider, ProviderError, Routing, U256, Zone};
use serde_json::{Value, json};

#[test]
fn classes_separate_retry_reobserve_stop_and_reconcile() {
    for (error, class) in [
        (RpcError::Timeout, ErrorClass::Transient),
        (RpcError::HttpStatus(429), ErrorClass::Transient),
        (RpcError::HttpStatus(503), ErrorClass::Transient),
        (RpcError::HttpStatus(404), ErrorClass::Invalid),
        (RpcError::SubscriptionLagged, ErrorClass::Stale),
        (RpcError::InvalidResponse("x"), ErrorClass::Invalid),
    ] {
        assert_eq!(error.class(), class, "{error:?}");
    }
    let wrong_chain = ProviderError::ChainMismatch {
        expected: U256::from(1),
        actual: U256::from(2),
    };
    assert_eq!(wrong_chain.class(), ErrorClass::NetworkMismatch);
    assert_eq!(
        QiError::Provider(wrong_chain).class(),
        ErrorClass::NetworkMismatch
    );
    assert_eq!(
        QiError::NetworkMismatch.class(),
        ErrorClass::NetworkMismatch
    );
    assert_eq!(QiError::StaleSnapshot.class(), ErrorClass::Stale);
    let ambiguous = BroadcastError::Ambiguous {
        transaction_hash: Hash32::from_bytes([1; 32]),
        zone: Zone::Cyprus1,
        source: RpcError::Timeout,
    };
    // A timeout is transient on a read, but ambiguous after a submit.
    assert_eq!(ambiguous.class(), ErrorClass::Ambiguous);
    assert_eq!(QiError::Broadcast(ambiguous).class(), ErrorClass::Ambiguous);
    assert_eq!(StorageError::Database.class(), ErrorClass::Storage);
    assert_eq!(StorageError::Conflict.class(), ErrorClass::Invalid);
    assert_eq!(
        DiscoveryError::SourceUnavailable.class(),
        ErrorClass::Transient
    );
}

#[tokio::test]
async fn a_wrong_chain_endpoint_is_a_network_mismatch_not_an_outage() {
    // Every provider failure used to become SourceUnavailable, so a sync loop
    // pointed at the wrong chain would retry it forever.
    #[derive(Clone)]
    struct OtherChain;
    impl Transport for OtherChain {
        async fn request(&self, _: &Endpoint, method: &str, _: Value) -> Result<Value, RpcError> {
            assert_eq!(method, "quai_chainId");
            Ok(json!("0x9"))
        }
    }
    let provider = Provider::new(
        OtherChain,
        Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
        U256::from(15000),
    );
    let scope = NetworkScope {
        chain_id: U256::from(15000),
        genesis: Hash32::from_bytes([1; 32]),
        zone: Zone::Cyprus1,
    };
    let source = quai_sdk::discovery::AccountRpcSource::new(&provider);
    let error = source.tip(scope).await.unwrap_err();
    assert_eq!(error, DiscoveryError::NetworkMismatch);
    assert_eq!(error.class(), ErrorClass::NetworkMismatch);
}
