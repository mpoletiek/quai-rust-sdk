//! Published interchange and rejection boundaries shared with browser workers.
use quai_consensus::{
    MAX_TRANSACTION_BYTES, MAX_TRANSACTION_MESSAGES, QuaiTransaction, U256,
    decode_proto_transaction,
    document::{
        MAX_DOCUMENT_BYTES, TransactionDocument as Document, access_list_from_json,
        access_list_to_json,
    },
    encode_proto_transaction, proto,
};
use quai_primitives::{Zone, get_bytes};
use serde_json::{Value, json};
fn vectors() -> Vec<Value> {
    let mut rows = Vec::new();
    for text in [
        include_str!("fixtures/shared/compatibility/fixtures/transactions.json"),
        include_str!("conversion-vectors.json"),
        include_str!("wrapping-vectors.json"),
    ] {
        rows.extend(
            serde_json::from_str::<Value>(text).unwrap()["vectors"]
                .as_array()
                .unwrap()
                .clone(),
        );
    }
    rows
}
fn bytes(v: &Value) -> Vec<u8> {
    get_bytes(v.as_str().unwrap()).unwrap()
}
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn published_documents_roundtrip_all_supported_signed_and_unsigned_operations() {
    let mut count = 0;
    for row in vectors() {
        if row["id"] == "qi-1" || row["id"] == "qi-2" {
            assert!(Document::from_json(&row["input"]).is_err());
            continue;
        }
        let document =
            Document::from_json(&row["input"]).unwrap_or_else(|e| panic!("{}: {e}", row["id"]));
        count += 1;
        assert!(document.is_signed());
        assert_eq!(document.to_bytes().unwrap(), bytes(&row["signed"]));
        assert_eq!(document.unsigned_bytes().unwrap(), bytes(&row["unsigned"]));
        assert_eq!(
            document.signing_digest().unwrap().to_string(),
            row["digest"]
        );
        assert_eq!(document.hash().unwrap().unwrap().to_string(), row["hash"]);
        assert_eq!(document.chain_id().to_string(), row["input"]["chainId"]);
        assert_eq!(document.origin_zone().unwrap(), Some(Zone::Cyprus1));
        let value = document.to_json().unwrap();
        assert_eq!(
            Document::from_json(&value).unwrap().to_bytes().unwrap(),
            bytes(&row["signed"])
        );
        for signed in [false, true] {
            let proto = document.to_proto(signed).unwrap();
            let raw = encode_proto_transaction(&proto).unwrap();
            assert_eq!(decode_proto_transaction(&raw).unwrap(), proto);
            let decoded = Document::from_proto(&proto).unwrap();
            assert_eq!(decoded.is_signed(), signed);
            assert_eq!(
                decoded.to_bytes().unwrap(),
                bytes(&row[if signed { "signed" } else { "unsigned" }])
            );
            assert_eq!(
                Document::from_json(&decoded.to_json().unwrap())
                    .unwrap()
                    .to_bytes()
                    .unwrap(),
                raw
            );
        }
    }
    assert_eq!(count, 24);
}
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn false_identity_signature_lock_and_inexact_quantity_claims_reject() {
    let rows = vectors();
    for row in rows
        .iter()
        .filter(|r| r["id"] == "quai-1" || r["id"] == "qi-0")
    {
        let original = &row["input"];
        let mut v = original.clone();
        v["hash"] = json!(format!("0x{}", "ff".repeat(32)));
        assert!(Document::from_json(&v).is_err());
        let mut v = original.clone();
        v["signature"] = Value::Null;
        assert!(Document::from_json(&v).is_err());
        let mut v = original.clone();
        v["extra"] = json!(1);
        assert!(Document::from_json(&v).is_err());
        for bad in [
            json!(-1),
            json!(1.5),
            json!(1.0),
            json!("-1"),
            json!("+1"),
            json!(" 1"),
            json!("1e2"),
            json!(format!("0x1{}", "00".repeat(32))),
        ] {
            let mut v = original.clone();
            v["chainId"] = bad;
            assert!(Document::from_json(&v).is_err());
        }
    }
    let quai = &rows.iter().find(|r| r["id"] == "quai-1").unwrap()["input"];
    let mut v = quai.clone();
    v["from"] = json!("0x0011223344556677889900112233445566778899");
    assert!(Document::from_json(&v).is_err());
    let mut v = quai.clone();
    v["signature"]["yParity"] = json!(2);
    assert!(Document::from_json(&v).is_err());
    let mut v = quai.clone();
    v["signature"]["networkV"] = json!(27);
    assert!(Document::from_json(&v).is_err());
    let qi = &rows.iter().find(|r| r["id"] == "qi-0").unwrap()["input"];
    let mut v = qi.clone();
    v["signature"] = json!(format!("0x{}", "00".repeat(64)));
    assert!(Document::from_json(&v).is_err());
    let mut v = qi.clone();
    v["txOutputs"][0]["lock"] = json!("0x01");
    assert!(Document::from_json(&v).is_err());
    let mut p = decode_proto_transaction(&bytes(
        &rows.iter().find(|r| r["id"] == "qi-0").unwrap()["signed"],
    ))
    .unwrap();
    p.tx_outs.as_mut().unwrap().tx_outs[0].lock = Some(vec![1]);
    let raw = encode_proto_transaction(&p).unwrap();
    assert_eq!(decode_proto_transaction(&raw).unwrap(), p);
    assert!(Document::from_proto(&p).is_err());
}
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn explicit_unsigned_claims_full_u64_and_all_qi_destination_zones() {
    let rows = vectors();
    let mut v = rows.iter().find(|r| r["id"] == "quai-1").unwrap()["input"].clone();
    v["signature"] = Value::Null;
    v["hash"] = Value::Null;
    v["nonce"] = json!(u64::MAX.to_string());
    v["gasLimit"] = json!(u64::MAX.to_string());
    let doc = Document::from_json(&v).unwrap();
    assert_eq!(doc.to_json().unwrap()["nonce"], u64::MAX.to_string());
    assert_eq!(doc.origin_zone().unwrap(), Some(Zone::Cyprus1));
    let decoded = Document::decode(&doc.to_bytes().unwrap()).unwrap();
    assert_eq!(decoded.origin_zone().unwrap(), None);
    assert_eq!(decoded.is_external().unwrap(), None);
    assert_eq!(decoded.type_id(), 0);
    assert_eq!(decoded.type_name(), "standard");
    let mut v = rows.iter().find(|r| r["id"] == "qi-0").unwrap()["input"].clone();
    v["signature"] = Value::Null;
    v["hash"] = Value::Null;
    v["txOutputs"]
        .as_array_mut()
        .unwrap()
        .push(json!({"address":"0x0180000000000000000000000000000000000000","denomination":0}));
    let doc = Document::from_json(&v).unwrap();
    assert_eq!(doc.type_id(), 2);
    assert_eq!(doc.type_name(), "utxo");
    assert_eq!(doc.destination_zone().unwrap(), Some(Zone::Cyprus1));
    assert_eq!(doc.is_external().unwrap(), Some(false));
    assert_eq!(doc.has_cross_zone_outputs().unwrap(), Some(true));
    assert_eq!(
        doc.destination_zones().unwrap(),
        vec![Zone::Cyprus1, Zone::Cyprus2]
    );
}
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn access_list_arrays_retain_order_and_maps_normalize_exact_bytes() {
    let address = "0x0011223344556677889900112233445566778899";
    let slot = format!("0x{}", "ab".repeat(32));
    let upper = format!("0x{}", "AB".repeat(32));
    let list = access_list_from_json(&json!([[address, [upper, slot, upper]]])).unwrap();
    assert_eq!(list[0].storage_keys.len(), 3);
    let object = access_list_to_json(&list).unwrap();
    assert_eq!(access_list_from_json(&object).unwrap(), list);
    let map = access_list_from_json(&json!({address:[upper,slot,upper]})).unwrap();
    assert_eq!(map[0].storage_keys.len(), 1);
    assert!(access_list_from_json(&json!([[address, [], 1]])).is_err());
    assert!(
        access_list_from_json(&json!([{ "address":address,"storageKeys":[],"ignored":true}]))
            .is_err()
    );
    let mut too_many = list[0].clone();
    too_many.storage_keys = vec![list[0].storage_keys[0]; MAX_TRANSACTION_MESSAGES];
    assert!(access_list_to_json(&[too_many]).is_err());
}
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn raw_protobuf_and_json_resource_bounds_precede_conversion() {
    assert!(Document::from_json(&json!("x".repeat(MAX_DOCUMENT_BYTES + 1))).is_err());
    assert!(access_list_from_json(&json!(vec![Value::Null; 65_536])).is_err());
    let mut deep = Value::Null;
    for _ in 0..66 {
        deep = json!([deep]);
    }
    assert!(Document::from_json(&deep).is_err());
    assert!(
        encode_proto_transaction(&proto::Transaction {
            data: Some(vec![0; MAX_TRANSACTION_BYTES]),
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        encode_proto_transaction(&proto::Transaction {
            tx_outs: Some(proto::Outputs {
                tx_outs: vec![proto::Output::default(); MAX_TRANSACTION_MESSAGES]
            }),
            ..Default::default()
        })
        .is_err()
    );
    assert!(decode_proto_transaction(&[0x08, 0x80, 0x00]).is_err());
    assert!(decode_proto_transaction(&[0x08, 0, 0x08, 0]).is_err());
    assert!(decode_proto_transaction(&[0xf8, 0x07, 0]).is_err());
    let document = Document::UnsignedQuai {
        transaction: QuaiTransaction {
            chain_id: U256::ZERO,
            nonce: 0,
            to: None,
            value: U256::ZERO,
            gas_limit: 0,
            gas_price: U256::ZERO,
            data: vec![],
            access_list: vec![],
        },
        claimed_sender: None,
    };
    assert!(!document.is_signed());
    assert_eq!(document.hash().unwrap(), None);
    assert_eq!(document.is_external().unwrap(), Some(false));
}
