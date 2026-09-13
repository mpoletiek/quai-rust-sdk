//! Exact selector routing and hostile block-association checks.
use quai_primitives::{Hash32, Zone};
use quai_provider::{MinedBlock, Provider, ProviderError};
use quai_rpc::{Endpoint, Routing, RpcError, Transport, U256};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
#[derive(Clone)]
struct Mock {
    value: Value,
    calls: Arc<Mutex<Vec<(String, Value)>>>,
    fail: bool,
}
impl Transport for Mock {
    async fn request(&self, _: &Endpoint, method: &str, params: Value) -> Result<Value, RpcError> {
        self.calls.lock().unwrap().push((method.into(), params));
        if method == "quai_chainId" {
            return Ok(json!("0x539"));
        }
        if self.fail {
            return Err(RpcError::Timeout);
        }
        Ok(self.value.clone())
    }
}
fn setup(value: Value) -> (Provider<Mock>, Mock) {
    let mock = Mock {
        value,
        calls: Default::default(),
        fail: false,
    };
    (
        Provider::new(
            mock.clone(),
            Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
            U256::from(1337),
        ),
        mock,
    )
}
fn block() -> Value {
    serde_json::from_str(include_str!("fixtures/conversion-block16.json")).unwrap()
}
fn hash(v: &Value) -> Hash32 {
    v["hash"].as_str().unwrap().parse().unwrap()
}
fn header(v: &Value) -> Value {
    json!({"woHeader":v["woHeader"],"gasLimit":"0x2faf080","stateLimit":"0x2faf080"})
}
#[tokio::test]
async fn exact_full_block_selectors_validate_and_preserve_executed_order() {
    let v = block();
    let id = hash(&v);
    for (selector, method, param) in [
        (
            MinedBlock::Number(16),
            "quai_getBlockByNumber",
            json!("0x10"),
        ),
        (MinedBlock::Latest, "quai_getBlockByNumber", json!("latest")),
        (
            MinedBlock::Hash(id),
            "quai_getBlockByHash",
            json!(id.to_string()),
        ),
    ] {
        let (p, m) = setup(v.clone());
        let b = p
            .mined_block(Zone::Cyprus1, selector, 16)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(b.block.hash, id);
        assert_eq!(b.block.number, 16);
        assert_eq!(
            b.transactions.len(),
            v["transactions"].as_array().unwrap().len()
        );
        for (i, tx) in b.transactions.iter().enumerate() {
            assert_eq!(tx.inclusion.unwrap().transaction_index, i as u64);
        }
        assert_eq!(
            m.calls.lock().unwrap()[1],
            (method.into(), json!([param, true]))
        );
        assert!(b.extensions.contains_key("outboundEtxs"));
    }
    let (p, m) = setup(header(&v));
    let h = p.header_by_hash(Zone::Cyprus1, id).await.unwrap().unwrap();
    assert_eq!(h.hash, id);
    assert_eq!(h.number, 16);
    assert_eq!(
        m.calls.lock().unwrap()[1],
        ("quai_getHeaderByHash".into(), json!([id.to_string()]))
    );
}
#[tokio::test]
async fn hash_only_blocks_keep_order_and_reject_duplicate_or_malformed_identities() {
    let mut v = block();
    let id = hash(&v);
    v["transactions"] = Value::Array(
        v["transactions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["hash"].clone())
            .collect(),
    );
    let (p, m) = setup(v.clone());
    let b = p
        .block_hashes(Zone::Cyprus1, MinedBlock::Hash(id), 16)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        b.transactions
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        v["transactions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        m.calls.lock().unwrap()[1],
        ("quai_getBlockByHash".into(), json!([id.to_string(), false]))
    );
    for replacement in [
        json!([v["transactions"][0], v["transactions"][0]]),
        json!([Hash32::ZERO.to_string()]),
        json!([{}]),
        json!(null),
    ] {
        let mut invalid = v.clone();
        invalid["transactions"] = replacement;
        assert!(
            setup(invalid)
                .0
                .block_hashes(Zone::Cyprus1, MinedBlock::Hash(id), 16)
                .await
                .is_err()
        );
    }
    assert!(
        setup(v)
            .0
            .block_hashes(Zone::Cyprus1, MinedBlock::Hash(id), 1)
            .await
            .is_err()
    );
}
#[tokio::test]
async fn wrong_hash_height_zone_and_inclusion_fail_closed() {
    let v = block();
    let id = hash(&v);
    let other = Hash32::from_bytes([0x11; 32]);
    for path in [
        vec!["hash"],
        vec!["woHeader", "hash"],
        vec!["woHeader", "location"],
        vec!["woHeader", "number"],
    ] {
        let mut bad = v.clone();
        let mut field = &mut bad;
        for p in &path {
            field = &mut field[*p];
        }
        *field = match *path.last().unwrap() {
            "location" => json!("0x0100"),
            "number" => json!("0x0"),
            _ => json!(other.to_string()),
        };
        assert!(
            setup(bad)
                .0
                .mined_block(Zone::Cyprus1, MinedBlock::Hash(id), 16)
                .await
                .is_err()
        );
    }
    assert!(
        setup(v.clone())
            .0
            .mined_block(Zone::Cyprus1, MinedBlock::Hash(other), 16)
            .await
            .is_err()
    );
    assert!(
        setup(v.clone())
            .0
            .mined_block(Zone::Cyprus1, MinedBlock::Number(15), 16)
            .await
            .is_err()
    );
    let mut bad = v.clone();
    bad["transactions"][0]["transactionIndex"] = json!("0x1");
    assert!(
        setup(bad)
            .0
            .mined_block(Zone::Cyprus1, MinedBlock::Hash(id), 16)
            .await
            .is_err()
    );
    assert!(
        setup(header(&v))
            .0
            .header_by_hash(Zone::Cyprus1, other)
            .await
            .is_err()
    );
}
#[tokio::test]
async fn request_bounds_precede_io_and_unavailable_is_not_rejection() {
    let (p, m) = setup(Value::Null);
    for selector in [
        MinedBlock::Number(0),
        MinedBlock::Number(u64::MAX),
        MinedBlock::Hash(Hash32::ZERO),
    ] {
        assert!(p.mined_block(Zone::Cyprus1, selector, 16).await.is_err());
        assert!(p.block_hashes(Zone::Cyprus1, selector, 16).await.is_err());
    }
    for budget in [0, 4097] {
        assert!(
            p.mined_block(Zone::Cyprus1, MinedBlock::Latest, budget)
                .await
                .is_err()
        );
    }
    assert!(p.header_by_hash(Zone::Cyprus1, Hash32::ZERO).await.is_err());
    assert!(m.calls.lock().unwrap().is_empty());
    assert!(
        p.mined_block(Zone::Cyprus1, MinedBlock::Latest, 16)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        p.block_hashes(Zone::Cyprus1, MinedBlock::Latest, 16)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        p.header_by_hash(Zone::Cyprus1, hash(&block()))
            .await
            .unwrap()
            .is_none()
    );
    let mut failing = m;
    failing.fail = true;
    let p = Provider::new(
        failing,
        Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
        U256::from(1337),
    );
    assert!(matches!(
        p.mined_block(Zone::Cyprus1, MinedBlock::Latest, 16).await,
        Err(ProviderError::Rpc(RpcError::Timeout))
    ));
}
