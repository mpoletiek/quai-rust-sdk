//! Typed response lookup/export tests using retained public-node observations.
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
use quai_sdk::primitives::Hash32;
use quai_sdk::provider::{
    BlockTransactionId as Id, Log, MinedBlock, OutboundBlockTransaction, Receipt, ReceiptOutcome,
    Transaction, TransactionDetails,
};
use quai_sdk::rpc::{Endpoint, RpcError, Transport};
use quai_sdk::{Provider, Routing, U256, Zone};
use serde_json::{Value, json};
fn records() -> Vec<Value> {
    let mut records = Vec::new();
    for fixture in [
        include_str!("fixtures/shared/crates/quai-provider/tests/fixtures/orchard.json"),
        include_str!("fixtures/shared/crates/quai-provider/tests/fixtures/lan-mainnet.json"),
    ] {
        records.extend(
            serde_json::from_str::<Value>(fixture).unwrap()["records"]
                .as_array()
                .unwrap()
                .clone(),
        );
    }
    // Shape-only public synthetic Qi DTO; not a claimed mined or validly signed transaction.
    let hash = Hash32::from_bytes([128; 32]).to_string();
    records.push(json!({"transaction":{"blockHash":null,"blockNumber":null,"transactionIndex":null,"hash":hash,"type":"0x2","gas":"0x0","nonce":"0x0","chainId":"0x9","input":"0x","inputs":[{"previousOutPoint":{"txHash":hash,"index":"0xffff"},"pubKey":format!("0x02{}","11".repeat(32))}],"outputs":[{"denomination":"0xe","address":"0x0080000000000000000000000000000000000000","lock":null}],"utxoSignature":format!("0x{}","00".repeat(64))},"receipt":null}));
    records
}
fn block() -> Value {
    serde_json::from_str(include_str!(
        "fixtures/shared/crates/quai-provider/tests/fixtures/conversion-block16.json"
    ))
    .unwrap()
}
#[derive(Clone)]
struct Mock {
    block: Value,
    transaction: Value,
}
impl Transport for Mock {
    async fn request(&self, _: &Endpoint, method: &str, _: Value) -> Result<Value, RpcError> {
        Ok(match method {
            "quai_chainId" => json!("0x539"),
            "quai_getBlockByNumber" | "quai_getBlockByHash" => self.block.clone(),
            "quai_getTransactionByHash" => self.transaction.clone(),
            _ => panic!("unexpected {method}"),
        })
    }
}
fn provider(block: Value, transaction: Value) -> Provider<Mock> {
    Provider::new(
        Mock { block, transaction },
        Routing::direct("https://fixture.invalid", Zone::Cyprus1.into()).unwrap(),
        U256::from(1337),
    )
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn all_retained_transaction_and_receipt_variants_round_trip_with_exact_quantities() {
    let mut kinds = std::collections::BTreeSet::new();
    for row in records() {
        let mut input = row["transaction"].clone();
        input["futureField"] = json!({"exact":"9007199254740993"});
        let t = Transaction::try_from(input).unwrap();
        kinds.insert(format!("{:?}", t.kind()));
        assert_eq!(Transaction::try_from(t.to_rpc_json().unwrap()).unwrap(), t);
        if !row["receipt"].is_null() {
            let mut v = row["receipt"].clone();
            v["futureField"] = json!([1, 2, 3]);
            let r = Receipt::try_from(v).unwrap();
            let exported = r.to_rpc_json().unwrap();
            assert_eq!(Receipt::try_from(exported).unwrap(), r);
            assert_eq!(
                r.fee().unwrap(),
                r.effective_gas_price * U256::from(r.gas_used)
            );
            for log in &r.logs {
                assert_eq!(Log::try_from(log.to_rpc_json().unwrap()).unwrap(), *log);
            }
        }
    }
    assert_eq!(kinds.len(), 3);
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn receipt_export_preserves_all_outcomes_and_rejects_mutated_associations() {
    let row = records()
        .into_iter()
        .find(|r| !r["receipt"].is_null())
        .unwrap();
    let mut r = Receipt::try_from(row["receipt"].clone()).unwrap();
    for outcome in [
        ReceiptOutcome::Failed,
        ReceiptOutcome::Succeeded,
        ReceiptOutcome::Locked,
        ReceiptOutcome::PostState(Hash32::from_bytes([7; 32])),
    ] {
        r.outcome = outcome;
        assert_eq!(Receipt::try_from(r.to_rpc_json().unwrap()).unwrap(), r);
    }
    r.effective_gas_price = U256::MAX;
    r.gas_used = 2;
    assert!(r.fee().is_err());
    let mut with_log = records()
        .into_iter()
        .filter_map(|r| Receipt::try_from(r["receipt"].clone()).ok())
        .find(|r| !r.logs.is_empty())
        .unwrap();
    with_log.logs[0].transaction_hash = Hash32::ZERO;
    assert!(with_log.to_rpc_json().is_err());
    let mut log = with_log.logs[0].clone();
    log.topics = vec![Hash32::ZERO; 5];
    assert!(log.to_rpc_json().is_err());
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn invalid_mutated_qi_and_quai_fields_cannot_export_as_valid_node_dtos() {
    for row in records() {
        let mut t = Transaction::try_from(row["transaction"].clone()).unwrap();
        match &mut t.details {
            TransactionDetails::Qi(q) => {
                q.outputs[0].denomination = 15;
                assert!(t.to_rpc_json().is_err());
            }
            TransactionDetails::Quai(q) => {
                q.access_list.push(quai_sdk::provider::AccessListItem {
                    address: q.from.address(),
                    storage_keys: vec![Hash32::ZERO; 65537],
                });
                assert!(t.to_rpc_json().is_err());
            }
            _ => (),
        }
    }
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn full_blocks_lookup_exact_hash_and_preserve_metadata_and_outbound_separation() {
    let raw = block();
    let p = provider(raw.clone(), Value::Null);
    let b = p
        .mined_block(Zone::Cyprus1, MinedBlock::Number(16), 4096)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(b.to_rpc_json().unwrap(), raw);
    for (i, t) in b.transactions.iter().enumerate() {
        assert_eq!(b.transaction(Id::Hash(t.hash)), Some(t));
        assert_eq!(b.transaction(Id::Index(i)), Some(t));
    }
    assert!(b.transaction(Id::Hash(Hash32::ZERO)).is_none());
    assert!(b.transaction(Id::Index(usize::MAX)).is_none());
    let meta = b.metadata();
    assert!(meta.header().unwrap().is_some());
    assert!(meta.work_header().unwrap().is_some());
    assert_eq!(meta.size().unwrap(), Some(U256::from(0x8e7)));
    assert!(meta.total_entropy().unwrap().unwrap() > U256::from(u64::MAX));
    assert!(meta.timestamp_seconds().unwrap().unwrap() > U256::ZERO);
    assert_eq!(meta.interlink_hashes().unwrap(), Some(vec![]));
    assert_eq!(meta.sub_manifest().unwrap(), Some(vec![]));
    assert!(meta.uncles().unwrap().is_some());
    assert!(meta.work_shares().unwrap().is_some());
    let out = meta.outbound_etxs(4096).unwrap().unwrap();
    assert_eq!(out.len(), raw["outboundEtxs"].as_array().unwrap().len());
    for item in &out {
        assert!(matches!(item, OutboundBlockTransaction::Prefetched(_)));
        assert_eq!(
            out.iter().find(|t| t.hash() == item.hash()).unwrap().hash(),
            item.hash()
        );
    }
    let mut changed = b.clone();
    changed.block.number += 1;
    assert!(changed.to_rpc_json().is_err());
    changed = b.clone();
    changed.transactions[0]
        .inclusion
        .as_mut()
        .unwrap()
        .transaction_index = 99;
    assert!(changed.to_rpc_json().is_err());
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn hash_block_lookup_fetches_only_member_and_rejects_wrong_inclusion() {
    let mut raw = block();
    let first = raw["transactions"][0].clone();
    raw["transactions"] = json!(
        raw["transactions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["hash"].clone())
            .collect::<Vec<_>>()
    );
    let p = provider(raw.clone(), first.clone());
    let b = p
        .block_hashes(Zone::Cyprus1, MinedBlock::Number(16), 4096)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(b.to_rpc_json().unwrap(), raw);
    let hash = b.transactions[0];
    assert_eq!(b.transaction_hash(Id::Hash(hash)), Some(hash));
    assert!(
        b.transaction(&p, Id::Index(usize::MAX))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        b.transaction(&p, Id::Hash(hash))
            .await
            .unwrap()
            .unwrap()
            .hash,
        hash
    );
    let mut wrong = first;
    wrong["transactionIndex"] = json!("0x99");
    assert!(
        b.transaction(&provider(raw.clone(), wrong), Id::Hash(hash))
            .await
            .is_err()
    );
    assert!(
        b.transaction(&provider(raw, Value::Null), Id::Index(0))
            .await
            .unwrap()
            .is_none()
    );
}
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
async fn metadata_bounds_and_malformed_outbound_items_are_explicit() {
    let raw = block();
    let p = provider(raw.clone(), Value::Null);
    let mut b = p
        .mined_block(Zone::Cyprus1, MinedBlock::Number(16), 4096)
        .await
        .unwrap()
        .unwrap();
    assert!(b.metadata().outbound_etxs(0).is_err());
    let external = raw["outboundEtxs"][0].clone();
    for value in [
        json!([external.clone(), external.clone()]),
        json!([Hash32::ZERO.to_string()]),
        json!([records()
            .into_iter()
            .find(|r| r["transaction"]["type"] == "0x0")
            .unwrap()["transaction"]
            .clone()]),
        json!("bad"),
    ] {
        b.extensions.insert("outboundEtxs".into(), value);
        assert!(b.metadata().outbound_etxs(4096).is_err());
    }
    b.extensions
        .insert("outboundEtxs".into(), json!([external["hash"].clone()]));
    assert!(matches!(
        &b.metadata().outbound_etxs(1).unwrap().unwrap()[0],
        OutboundBlockTransaction::Hash(_)
    ));
    b.extensions
        .insert("workShares".into(), json!(["conflict"]));
    assert!(b.metadata().work_shares().is_err());
    b.extensions.insert(
        "interlinkHashes".into(),
        json!(vec![Hash32::ZERO.to_string(); 8193]),
    );
    assert!(b.metadata().interlink_hashes().is_err());
    b.extensions.insert("size".into(), json!(-1));
    assert!(b.metadata().size().is_err());
    b.extensions.get_mut("woHeader").unwrap()["timestamp"] = json!("0x20000000000001");
    assert_eq!(
        b.metadata().timestamp_seconds().unwrap(),
        Some(U256::from(9007199254740993u64))
    );
    b.extensions.remove("header");
    assert!(b.metadata().header().unwrap().is_none());
    b.extensions.insert("header".into(), json!(true));
    assert!(b.metadata().header().is_err());
}
