//! Differential signed/unsigned wire vectors and hostile decoder inputs.
use quai_consensus::{AccessTuple, QuaiTransaction, SignedQuaiTransaction, TransactionError, U256};
use quai_crypto::SecretKey;
use serde_json::Value;

fn bytes(s: &str) -> Vec<u8> {
    s.strip_prefix("0x")
        .unwrap()
        .as_bytes()
        .chunks_exact(2)
        .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
        .collect()
}
fn fixtures() -> Vec<Value> {
    serde_json::from_str::<Value>(include_str!(
        "../../../compatibility/fixtures/transactions.json"
    ))
    .unwrap()["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["kind"] == "quai")
        .cloned()
        .collect()
}
fn transaction(v: &Value) -> QuaiTransaction {
    let t = &v["input"];
    let num = |name: &str| U256::from_str_radix(t[name].as_str().unwrap(), 10).unwrap();
    QuaiTransaction {
        chain_id: num("chainId"),
        nonce: t["nonce"].as_u64().unwrap(),
        to: t["to"].as_str().map(|a| a.parse().unwrap()),
        value: num("value"),
        gas_limit: t["gasLimit"].as_str().unwrap().parse().unwrap(),
        gas_price: num("gasPrice"),
        data: bytes(t["data"].as_str().unwrap()),
        access_list: t["accessList"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|a| AccessTuple {
                        address: a["address"].as_str().unwrap().parse().unwrap(),
                        storage_keys: a["storageKeys"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|k| k.as_str().unwrap().parse().unwrap())
                            .collect(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

#[test]
fn signed_bytes_digests_ids_and_recovery_match_published_sdk() {
    for v in fixtures() {
        let tx = transaction(&v);
        let unsigned = bytes(v["unsigned"].as_str().unwrap());
        assert_eq!(tx.unsigned_bytes().unwrap(), unsigned, "{}", v["id"]);
        assert_eq!(QuaiTransaction::decode_unsigned(&unsigned).unwrap(), tx);
        assert_eq!(tx.signing_digest().unwrap().to_string(), v["digest"]);
        let key = SecretKey::from_bytes(
            &bytes(v["publicTestSecret"].as_str().unwrap())
                .try_into()
                .unwrap(),
        )
        .unwrap();
        let signed = tx.sign(&key).unwrap();
        let expected = bytes(v["signed"].as_str().unwrap());
        assert_eq!(signed.signed_bytes().unwrap(), expected, "{}", v["id"]);
        assert_eq!(signed.hash().unwrap().to_string(), v["hash"]);
        let decoded = SignedQuaiTransaction::decode(&expected).unwrap();
        assert_eq!(decoded.transaction(), &tx);
        assert_eq!(decoded.from().to_string(), v["input"]["from"]);
    }
}

#[test]
fn ambiguous_and_truncated_wire_data_fail_closed() {
    let v = &fixtures()[1];
    let good = bytes(v["signed"].as_str().unwrap());
    for end in 0..good.len() {
        assert!(
            SignedQuaiTransaction::decode(&good[..end]).is_err(),
            "prefix {end}"
        );
    }
    let mut duplicate = good.clone();
    duplicate.extend([8, 0]);
    assert!(matches!(
        SignedQuaiTransaction::decode(&duplicate),
        Err(TransactionError::InvalidEncoding)
    ));
    let mut unknown = good.clone();
    unknown.extend([0xf8, 0x07, 0]);
    assert!(matches!(
        SignedQuaiTransaction::decode(&unknown),
        Err(TransactionError::InvalidEncoding)
    ));
    let mut overlong = good.clone();
    overlong.splice(1..2, [0x80, 0]);
    assert!(SignedQuaiTransaction::decode(&overlong).is_err());
    assert!(QuaiTransaction::decode_unsigned(&good).is_err());
    assert!(SignedQuaiTransaction::decode(&bytes(v["unsigned"].as_str().unwrap())).is_err());
}

#[test]
fn native_nonce_covers_node_u64_range_without_js_number_narrowing() {
    let mut tx = transaction(&fixtures()[0]);
    tx.nonce = u64::MAX;
    tx.gas_limit = u64::MAX;
    assert_eq!(
        QuaiTransaction::decode_unsigned(&tx.unsigned_bytes().unwrap()).unwrap(),
        tx
    );
}

#[test]
fn changing_payload_does_not_mutate_existing_signature_and_zero_chain_cannot_sign() {
    let v = &fixtures()[0];
    let mut tx = transaction(v);
    let key = SecretKey::from_bytes(
        &bytes(v["publicTestSecret"].as_str().unwrap())
            .try_into()
            .unwrap(),
    )
    .unwrap();
    let signed = tx.sign(&key).unwrap();
    let original = signed.hash().unwrap();
    tx.value = U256::from(42);
    assert_eq!(signed.hash().unwrap(), original);
    assert_ne!(tx.sign(&key).unwrap().hash().unwrap(), original);
    tx.chain_id = U256::ZERO;
    assert!(tx.sign(&key).is_err());
}
