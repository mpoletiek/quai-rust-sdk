//! Differential and adversarial typed-data checks against the pinned JS encoder.
use quai_abi::{
    MAX_DATA_BYTES, MAX_DEPTH, MAX_FIELDS, MAX_TYPES, MAX_VALUE_NODES, TypedData, TypedDataEncoder,
    TypedDataError, TypedDataField, TypedDataTypes, hash_domain, hash_typed_data,
};
use quai_crypto::{RecoverableSignature, SecretKey};
use serde_json::{Value, json};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/typed-data.json")).unwrap()
}
fn types(value: &Value) -> TypedDataTypes {
    serde_json::from_value(value.clone()).unwrap()
}
fn field(name: &str, kind: &str) -> TypedDataField {
    TypedDataField {
        name: name.into(),
        type_name: kind.into(),
    }
}
fn schema(kind: &str) -> TypedDataTypes {
    TypedDataTypes::from([("Root".into(), vec![field("value", kind)])])
}
fn hex(bytes: &[u8]) -> String {
    format!(
        "0x{}",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    )
}
fn decode(s: &str) -> Vec<u8> {
    s.as_bytes()[2..]
        .chunks_exact(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn all_pinned_js_hashes_encodings_domains_and_signatures_match() {
    let fixture = fixture();
    let vectors = fixture["vectors"].as_array().unwrap();
    assert_eq!(vectors.len(), 43);
    for vector in vectors {
        let encoder = TypedDataEncoder::new(&types(&vector["types"])).unwrap();
        let primary = vector["primaryType"].as_str().unwrap();
        assert_eq!(encoder.primary_type(), primary);
        assert_eq!(
            encoder.encode_type(primary).unwrap(),
            vector["encodedType"].as_str().unwrap()
        );
        assert_eq!(
            hex(&encoder.encode_data(primary, &vector["value"]).unwrap()),
            vector["encoded"]
        );
        assert_eq!(
            encoder.hash(&vector["value"]).unwrap().to_string(),
            vector["structHash"]
        );
        assert_eq!(
            hash_domain(&vector["domain"]).unwrap().to_string(),
            vector["domainHash"]
        );
        assert_eq!(
            hex(&encoder
                .signing_preimage(&vector["domain"], &vector["value"])
                .unwrap()),
            vector["preimage"]
        );
        let hash = encoder
            .signing_hash(&vector["domain"], &vector["value"])
            .unwrap();
        assert_eq!(hash.to_string(), vector["digest"], "{}", vector["id"]);
        let mut secret = [0; 32];
        secret[31] = 1;
        let key = SecretKey::from_bytes(&secret).unwrap();
        let signed = key.sign_prehash(hash.bytes()).unwrap();
        assert_eq!(hex(&signed.to_quais_bytes().unwrap()), vector["signature"]);
        let reference = RecoverableSignature::from_quais_bytes(
            &decode(vector["signature"].as_str().unwrap())
                .try_into()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            reference
                .recover_prehash(hash.bytes())
                .unwrap()
                .address()
                .to_string(),
            vector["signer"]
        );
        let document = json!({"types":vector["types"],"primaryType":primary,"domain":vector["domain"],"message":vector["value"]});
        let parsed = TypedData::from_json(&serde_json::to_vec(&document).unwrap()).unwrap();
        assert_eq!(parsed.signing_hash(), hash);
        assert_eq!(parsed.primary_type(), primary);
        assert_eq!(parsed.message(), &vector["value"]);
        assert_eq!(parsed.domain(), &vector["domain"]);
        assert_eq!(parsed.encoder().primary_type(), primary);
        assert_eq!(
            parsed.domain_hash().unwrap().to_string(),
            vector["domainHash"]
        );
    }
    assert_eq!(
        vectors[0]["digest"],
        "0xbe609aee343fb3c4b28e1df9e632fca64fcfaede20f02e86244efddf30957bd2"
    );
}

#[test]
fn shared_rejections_and_deliberately_stricter_inputs() {
    let fixture = fixture();
    for key in ["rejects", "strictPolicy"] {
        for case in fixture[key].as_array().unwrap() {
            assert!(
                hash_typed_data(&case["domain"], &types(&case["types"]), &case["value"]).is_err(),
                "{}",
                case["id"]
            );
        }
    }
    for value in [json!(null), json!(1), json!("false"), json!([]), json!({})] {
        assert!(
            TypedDataEncoder::new(&schema("bool"))
                .unwrap()
                .hash(&json!({"value":value}))
                .is_err()
        );
    }
}

#[test]
fn json_rejects_duplicates_mismatched_primary_and_domain_and_trailing_data() {
    let base = r#"{"types":{"Root":[{"name":"value","type":"uint8"}]},"primaryType":"Root","domain":{},"message":{"value":1}}"#;
    assert!(TypedData::from_json(base.as_bytes()).is_ok());
    for altered in [
        base.replace("\"value\":1", "\"value\":1,\"value\":2"),
        base.replace("\"domain\":{}", "\"domain\":{},\"domain\":{}"),
        base.replace("\"primaryType\":\"Root\"", "\"primaryType\":\"Other\""),
        base.replace(
            "\"name\":\"value\"",
            "\"name\":\"value\",\"name\":\"other\"",
        ),
        base.replace("\"message\":", "\"unknown\":"),
        format!("{base} null"),
        base.replace("\"value\":1", "\"value\":1.0"),
    ] {
        assert!(
            TypedData::from_json(altered.as_bytes()).is_err(),
            "{altered}"
        );
    }
    let mut doc: Value = serde_json::from_str(base).unwrap();
    doc["types"]["EIP712Domain"] = json!([]);
    assert!(TypedData::from_json(&serde_json::to_vec(&doc).unwrap()).is_ok());
    doc["types"]["EIP712Domain"] = json!([{"name":"chainId","type":"uint256"}]);
    assert_eq!(
        TypedData::from_json(&serde_json::to_vec(&doc).unwrap()).unwrap_err(),
        TypedDataError::Domain
    );
}

#[test]
fn bounds_schema_depth_nodes_and_bytes_before_encoding() {
    assert_eq!(
        TypedData::from_json(&vec![b' '; MAX_DATA_BYTES + 1]).unwrap_err(),
        TypedDataError::Limit
    );
    let too_many: TypedDataTypes = (0..MAX_TYPES + 1)
        .map(|i| (format!("S{i}"), vec![]))
        .collect();
    assert_eq!(
        TypedDataEncoder::new(&too_many).unwrap_err(),
        TypedDataError::Limit
    );
    let fields = (0..MAX_FIELDS + 1)
        .map(|i| field(&format!("f{i}"), "bool"))
        .collect();
    assert_eq!(
        TypedDataEncoder::new(&TypedDataTypes::from([("Root".into(), fields)])).unwrap_err(),
        TypedDataError::Limit
    );
    let encoder = TypedDataEncoder::new(&schema("string")).unwrap();
    assert_eq!(
        encoder
            .hash(&json!({"value":"x".repeat(MAX_DATA_BYTES)}))
            .unwrap_err(),
        TypedDataError::Limit
    );
    let encoder = TypedDataEncoder::new(&schema("uint8[]")).unwrap();
    assert_eq!(
        encoder
            .hash(&json!({"value":vec![0;MAX_VALUE_NODES]}))
            .unwrap_err(),
        TypedDataError::Limit
    );
    let mut deep = Value::Null;
    for _ in 0..MAX_DEPTH + 2 {
        deep = json!([deep]);
    }
    assert_eq!(encoder.hash(&deep).unwrap_err(), TypedDataError::Limit);
    assert_eq!(
        TypedData::from_json(&serde_json::to_vec(&deep).unwrap()).unwrap_err(),
        TypedDataError::Limit
    );
    let mut chain = TypedDataTypes::new();
    for i in 0..MAX_DEPTH + 2 {
        chain.insert(
            format!("T{i}"),
            if i == MAX_DEPTH + 1 {
                vec![]
            } else {
                vec![field("child", &format!("T{}", i + 1))]
            },
        );
    }
    assert_eq!(
        TypedDataEncoder::new(&chain).unwrap_err(),
        TypedDataError::Limit
    );
    for kind in [
        format!("uint8{}", "[]".repeat(17)),
        format!("uint8[{}]", MAX_VALUE_NODES + 1),
    ] {
        assert_eq!(
            TypedDataEncoder::new(&schema(&kind)).unwrap_err(),
            TypedDataError::Limit
        );
    }
}

#[test]
fn schema_graph_validation_and_unknown_struct_access() {
    let cycle = types(
        &json!({"Root":[{"name":"value","type":"A"}],"A":[{"name":"b","type":"B"}],"B":[{"name":"a","type":"A"}]}),
    );
    assert_eq!(
        TypedDataEncoder::new(&cycle).unwrap_err(),
        TypedDataError::Cycle
    );
    let encoder = TypedDataEncoder::new(&schema("uint8")).unwrap();
    assert_eq!(
        encoder.encode_type("nope"),
        Err(TypedDataError::UnknownType)
    );
    assert_eq!(
        encoder.hash_struct("nope", &json!({})),
        Err(TypedDataError::UnknownType)
    );
    for invalid in ["1bad", "a b", "", "é", "uint8", "bool"] {
        let t = TypedDataTypes::from([(invalid.into(), vec![])]);
        assert!(TypedDataEncoder::new(&t).is_err());
    }
}

#[test]
fn every_integer_width_rejects_both_adjacent_out_of_range_values() {
    fn increment(s: &str) -> String {
        let mut digits = s.as_bytes().to_vec();
        for b in digits.iter_mut().rev() {
            if *b != b'9' {
                *b += 1;
                return String::from_utf8(digits).unwrap();
            }
            *b = b'0';
        }
        digits.insert(0, b'1');
        String::from_utf8(digits).unwrap()
    }
    let fixture = fixture();
    for vector in fixture["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["id"].as_str().unwrap().starts_with("integer-width-"))
    {
        let encoder = TypedDataEncoder::new(&types(&vector["types"])).unwrap();
        for field in ["minimum", "maximum", "unsigned"] {
            let mut value = vector["value"].clone();
            let previous = value[field].as_str().unwrap();
            value[field] = if let Some(magnitude) = previous.strip_prefix('-') {
                format!("-{}", increment(magnitude))
            } else {
                increment(previous)
            }
            .into();
            assert_eq!(
                encoder.hash(&value),
                Err(TypedDataError::Integer),
                "{} {field}",
                vector["id"]
            );
        }
    }
}

#[test]
fn changing_domain_or_declared_field_order_changes_signing_hash() {
    let encoder = TypedDataEncoder::new(&schema("uint8")).unwrap();
    let value = json!({"value":1});
    let base = encoder.signing_hash(&json!({"chainId":1}), &value).unwrap();
    assert_ne!(
        base,
        encoder.signing_hash(&json!({"chainId":2}), &value).unwrap()
    );
    assert_ne!(
        base,
        encoder
            .signing_hash(&json!({"chainId":1,"name":"other"}), &value)
            .unwrap()
    );
    assert_eq!(
        hash_domain(&json!({})).unwrap(),
        hash_domain(&json!({"name":null,"salt":null})).unwrap()
    );
    let mut t = TypedDataTypes::from([(
        "Root".into(),
        vec![field("a", "uint8"), field("b", "uint8")],
    )]);
    let original = TypedDataEncoder::new(&t)
        .unwrap()
        .hash(&json!({"a":1,"b":2}))
        .unwrap();
    t.get_mut("Root").unwrap().reverse();
    assert_ne!(
        original,
        TypedDataEncoder::new(&t)
            .unwrap()
            .hash(&json!({"a":1,"b":2}))
            .unwrap()
    );
}

#[test]
fn rpc_documents_roundtrip_reference_hashes_and_emit_exact_integer_strings() {
    for vector in fixture()["vectors"].as_array().unwrap() {
        let document = json!({"types": vector["types"], "primaryType": vector["primaryType"], "domain": vector["domain"], "message": vector["value"]});
        let data = TypedData::from_json(&serde_json::to_vec(&document).unwrap()).unwrap();
        let encoded = data.to_rpc_json().unwrap();
        let restored = TypedData::from_json(encoded.as_bytes()).unwrap();
        assert_eq!(data.signing_hash(), restored.signing_hash());
        let rpc: Value = serde_json::from_str(&encoded).unwrap();
        assert!(rpc["types"]["EIP712Domain"].is_array());
    }
    let data = TypedData::from_json(br#"{"types":{"Root":[{"name":"value","type":"uint256[]"}]},"primaryType":"Root","domain":{"name":null,"chainId":"18446744073709551615"},"message":{"value":[9007199254740991,"18446744073709551615"]}}"#).unwrap();
    let rpc: Value = serde_json::from_str(&data.to_rpc_json().unwrap()).unwrap();
    assert_eq!(rpc["domain"]["chainId"], "18446744073709551615");
    assert!(rpc["domain"].get("name").is_none());
    assert_eq!(
        rpc["message"]["value"],
        json!(["9007199254740991", "18446744073709551615"])
    );
    assert_eq!(
        TypedData::from_json(&serde_json::to_vec(&rpc).unwrap())
            .unwrap()
            .signing_hash(),
        data.signing_hash()
    );
}
