//! One provider type over transports chosen at runtime.
#![cfg(not(target_arch = "wasm32"))]
use quai_primitives::Zone;
use quai_provider::Provider;
use quai_rpc::{BatchResult, DynTransport, Endpoint, Routing, RpcError, Transport, U256};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering::SeqCst},
};

/// Answers every call; counts single requests and batches separately.
#[derive(Clone, Default)]
struct Counting {
    batching: bool,
    singles: Arc<AtomicUsize>,
    batches: Arc<AtomicUsize>,
}
fn answer(method: &str) -> Value {
    match method {
        "quai_chainId" => json!("0x9"),
        "quai_blockNumber" => json!("0x2a"),
        _ => panic!("unexpected {method}"),
    }
}
impl Transport for Counting {
    async fn request(&self, _: &Endpoint, method: &str, _: Value) -> Result<Value, RpcError> {
        self.singles.fetch_add(1, SeqCst);
        Ok(answer(method))
    }
    async fn request_batch(
        &self,
        _: &Endpoint,
        requests: Vec<(&str, Value)>,
    ) -> Option<BatchResult> {
        if !self.batching {
            return None;
        }
        self.batches.fetch_add(1, SeqCst);
        Some(Ok(requests.iter().map(|(m, _)| Ok(answer(m))).collect()))
    }
}

/// A second, unrelated transport type.
struct Fixed;
impl Transport for Fixed {
    async fn request(&self, _: &Endpoint, method: &str, _: Value) -> Result<Value, RpcError> {
        Ok(answer(method))
    }
}

fn provider(transport: DynTransport) -> Provider<DynTransport> {
    Provider::new(
        transport,
        Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
        U256::from(9),
    )
}

#[tokio::test]
async fn different_transports_share_one_provider_type_and_keep_batching() {
    let batching = Counting {
        batching: true,
        ..Default::default()
    };
    let plain = Counting::default();
    let providers = [
        provider(DynTransport::new(batching.clone())),
        provider(DynTransport::new(plain.clone())),
        provider(DynTransport::new(Fixed)),
    ];
    for provider in &providers {
        let read = provider.block_number(Zone::Cyprus1.into());
        fn send<F: Send>(future: F) -> F {
            future
        }
        assert_eq!(send(read).await.unwrap(), U256::from(42));
    }
    // The guarded read stays one batch through the erased type...
    assert_eq!(batching.batches.load(SeqCst), 1);
    assert_eq!(batching.singles.load(SeqCst), 0);
    // ...and a non-batching transport still falls back to guard then call.
    assert_eq!(plain.singles.load(SeqCst), 2);
    // Cheap clones share the wrapped transport.
    let clone = providers[0].clone();
    clone.block_number(Zone::Cyprus1.into()).await.unwrap();
    assert_eq!(batching.batches.load(SeqCst), 2);
}

#[test]
fn the_erased_type_is_send_sync_and_wraps_the_native_transports() {
    fn bounds<T: Send + Sync + Clone + std::fmt::Debug + 'static>() {}
    bounds::<DynTransport>();
    bounds::<Provider<DynTransport>>();
    #[cfg(feature = "http")]
    let _ = |t: quai_rpc::HttpTransport| DynTransport::new(t);
    #[cfg(feature = "ws")]
    let _ = |t: quai_rpc::WsTransport| DynTransport::new(t);
}
