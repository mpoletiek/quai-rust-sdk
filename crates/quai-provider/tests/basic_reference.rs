//! Independent pinned RPC mapping checks for existing basic provider reads.
use quai_primitives::{Shard, Zone};
use quai_provider::Provider;
use quai_rpc::{Endpoint, Routing, RpcError, Transport, U256};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
#[derive(Clone)]
struct Mock {
    calls: Arc<Mutex<Vec<(String, Value)>>>,
    response: Value,
}
impl Transport for Mock {
    async fn request(&self, _: &Endpoint, method: &str, params: Value) -> Result<Value, RpcError> {
        self.calls.lock().unwrap().push((method.to_owned(), params));
        if method == "quai_chainId" {
            Ok(json!("0x9"))
        } else {
            Ok(self.response.clone())
        }
    }
}
#[tokio::test]
async fn typed_basic_reads_match_pinned_rpc_actions_and_keep_full_width_values() {
    let fixture: Value =
        serde_json::from_slice(include_bytes!("fixtures/provider-basics.json")).unwrap();
    for method in [
        "chainId",
        "getBlockNumber",
        "getGasPrice",
        "getRunningLocations",
    ] {
        let row = fixture["rpcActions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["action"]["method"] == method)
            .unwrap();
        let mock = Mock {
            calls: Default::default(),
            response: if method == "getRunningLocations" {
                json!([[0, 0], [2, 2]])
            } else {
                json!(format!("{:#x}", U256::MAX))
            },
        };
        let shard = if method == "getRunningLocations" {
            Shard::Prime
        } else {
            Zone::Cyprus1.into()
        };
        let provider = Provider::new(
            mock.clone(),
            Routing::direct("http://127.0.0.1:9200", shard).unwrap(),
            U256::from(9),
        );
        match method {
            "chainId" => assert_eq!(provider.chain_id(shard).await.unwrap(), U256::from(9)),
            "getBlockNumber" => assert_eq!(provider.block_number(shard).await.unwrap(), U256::MAX),
            "getGasPrice" => {
                assert_eq!(provider.gas_price(Zone::Cyprus1).await.unwrap(), U256::MAX)
            }
            "getRunningLocations" => assert_eq!(
                provider.running_zones().await.unwrap(),
                [Zone::Cyprus1, Zone::Hydra3]
            ),
            _ => unreachable!(),
        }
        let calls = mock.calls.lock().unwrap();
        assert_eq!(
            calls.last().unwrap(),
            &(
                row["rpc"]["method"].as_str().unwrap().to_owned(),
                row["rpc"]["args"].clone()
            )
        );
        assert_eq!(calls.len(), if method == "chainId" { 1 } else { 2 });
        assert_eq!(calls[0], ("quai_chainId".into(), json!([])));
    }
}
