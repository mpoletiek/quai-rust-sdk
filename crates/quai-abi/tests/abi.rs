//! JS differential ABI/interface vectors and hostile-offset/resource checks.
use quai_abi::{
    AbiCoder, AbiError, AbiEventValue, AbiInterface, AbiType, MAX_DATA_BYTES, MAX_VALUE_NODES,
    StateMutability, function_selector, indexed_event_topic, signature_hash,
};
use quai_primitives::Hash32;
use serde_json::{Value, json};
fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/abi.json")).unwrap()
}
fn types(value: &Value) -> Vec<AbiType> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|v| AbiType::parse(v.as_str().unwrap()).unwrap())
        .collect()
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
fn all_js_encodings_and_supported_decodings_match_and_roundtrip() {
    let fixture = fixture();
    assert_eq!(fixture["vectors"].as_array().unwrap().len(), 45);
    for v in fixture["vectors"].as_array().unwrap() {
        let types = types(&v["types"]);
        assert_eq!(
            types
                .iter()
                .map(AbiType::canonical_name)
                .collect::<Vec<_>>(),
            serde_json::from_value::<Vec<String>>(v["canonicalTypes"].clone()).unwrap()
        );
        let encoded = AbiCoder::encode(&types, v["values"].as_array().unwrap()).unwrap();
        assert_eq!(hex(&encoded), v["encoded"], "{}", v["id"]);
        let decoded = AbiCoder::decode(&types, &encoded).unwrap();
        assert_eq!(json!(decoded), v["decoded"], "{}", v["id"]);
        assert_eq!(AbiCoder::encode(&types, &decoded).unwrap(), encoded);
    }
}
#[test]
fn pinned_js_rejections_and_strict_primitive_words() {
    let fixture = fixture();
    for v in fixture["rejection"].as_array().unwrap() {
        let parsed = v["types"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| AbiType::parse(t.as_str().unwrap()))
            .collect::<Result<Vec<_>, _>>();
        assert!(
            parsed
                .and_then(|t| AbiCoder::encode(&t, v["values"].as_array().unwrap()))
                .is_err()
        );
    }
    for (ty, offset, value) in [
        ("bool", 31, 2),
        ("bool", 0, 1),
        ("address", 11, 1),
        ("uint8", 30, 1),
        ("int8", 31, 128),
        ("bytes2", 2, 1),
    ] {
        let mut raw = [0; 32];
        raw[offset] = value;
        assert_eq!(
            AbiCoder::decode(&[AbiType::parse(ty).unwrap()], &raw),
            Err(AbiError::Encoding),
            "{ty}"
        );
    }
    let ty = [AbiType::parse("int8").unwrap()];
    assert_eq!(
        AbiCoder::decode(&ty, &[255; 32]).unwrap(),
        vec![json!("-1")]
    );
    for ty in [
        "uint7",
        "int264",
        "bytes0",
        "bytes33",
        "uint08",
        "uint8[01]",
        "tupleuint",
        "uint8 garbage",
        "(uint8,)",
        "uint8[-1]",
        "function",
        "fixed",
    ] {
        assert!(AbiType::parse(ty).is_err(), "{ty}");
    }
    assert_eq!(
        AbiType::parse("tuple( uint , bool ) [2]")
            .unwrap()
            .canonical_name(),
        "(uint256,bool)[2]"
    );
}
#[test]
fn canonical_offset_padding_utf8_and_length_rejections() {
    let types = vec![
        AbiType::parse("bytes").unwrap(),
        AbiType::parse("bytes").unwrap(),
    ];
    let good = AbiCoder::encode(&types, &[json!("0xaa"), json!("0xbb")]).unwrap();
    for offset in [0, 1, 31, 32, 63, 65, 96, 160] {
        let mut altered = good.clone();
        altered[31] = offset;
        assert!(
            AbiCoder::decode(&types, &altered).is_err(),
            "offset {offset}"
        );
    }
    let mut alias = good.clone();
    alias[63] = 64;
    assert!(AbiCoder::decode(&types, &alias).is_err());
    let mut gap = good.clone();
    gap[63] = 160;
    gap.extend([0; 32]);
    assert!(AbiCoder::decode(&types, &gap).is_err());
    let mut padding = good.clone();
    padding[127] = 1;
    assert!(AbiCoder::decode(&types, &padding).is_err());
    let mut trailing = good.clone();
    trailing.extend([0; 32]);
    assert!(AbiCoder::decode(&types, &trailing).is_err());
    for end in 0..good.len() {
        assert!(AbiCoder::decode(&types, &good[..end]).is_err());
    }
    let string = [AbiType::parse("string").unwrap()];
    let mut utf8 = AbiCoder::encode(&string, &[json!("a")]).unwrap();
    utf8[64] = 255;
    assert!(AbiCoder::decode(&string, &utf8).is_err());
    let mut enormous = good;
    enormous[64..96].fill(255);
    assert!(AbiCoder::decode(&types, &enormous).is_err());
}
#[test]
fn bounded_zero_size_arrays_and_expansion_attacks() {
    let ty = [AbiType::parse("()[]").unwrap()];
    let value = vec![json!([[], [], []])];
    let bytes = AbiCoder::encode(&ty, &value).unwrap();
    assert_eq!(bytes.len(), 64);
    assert_eq!(AbiCoder::decode(&ty, &bytes).unwrap(), value);
    let mut enormous = bytes.clone();
    enormous[32..64].fill(255);
    assert!(AbiCoder::decode(&ty, &enormous).is_err());
    let mut excessive = bytes;
    let n = (MAX_VALUE_NODES as u64).to_be_bytes();
    excessive[56..64].copy_from_slice(&n);
    assert_eq!(AbiCoder::decode(&ty, &excessive), Err(AbiError::Limit));
    assert!(AbiType::parse("()[32768][32768]").is_err());
    assert!(AbiType::parse(&format!("uint8{}", "[]".repeat(65))).is_err());
    assert_eq!(
        AbiCoder::decode(&[], &vec![0; MAX_DATA_BYTES + 1]),
        Err(AbiError::Limit)
    );
    assert!(
        AbiCoder::encode(
            &[AbiType::parse("string").unwrap()],
            &[json!("x".repeat(MAX_DATA_BYTES))]
        )
        .is_err()
    );
    let mut nested = json!(0);
    for _ in 0..66 {
        nested = json!([nested]);
    }
    assert!(AbiCoder::encode(&ty, &[nested]).is_err());
}
#[test]
fn interface_calls_errors_returns_and_overloads_match_js() {
    let fixture = fixture();
    let iface =
        AbiInterface::from_json(&serde_json::to_vec(&fixture["interface"]).unwrap()).unwrap();
    assert_eq!(iface.functions().count(), 3);
    assert_eq!(iface.errors().count(), 1);
    assert_eq!(iface.events().count(), 3);
    assert_eq!(iface.function("lookup").unwrap_err(), AbiError::Ambiguous);
    assert_eq!(iface.function("absent").unwrap_err(), AbiError::NotFound);
    for v in fixture["calls"].as_array().unwrap() {
        let f = iface.function(v["name"].as_str().unwrap()).unwrap();
        assert_eq!(f.signature(), v["signature"].as_str().unwrap());
        assert_eq!(hex(&f.selector()), v["selector"]);
        let call = f.encode_call(v["args"].as_array().unwrap()).unwrap();
        assert_eq!(hex(&call), v["call"]);
        assert_eq!(f.encode_call(&f.decode_call(&call).unwrap()).unwrap(), call);
        let result = f.encode_returns(v["result"].as_array().unwrap()).unwrap();
        assert_eq!(hex(&result), v["returns"]);
        assert_eq!(
            f.encode_returns(&f.decode_returns(&result).unwrap())
                .unwrap(),
            result
        );
        assert_eq!(
            iface
                .function_by_selector(f.selector())
                .unwrap()
                .signature(),
            f.signature()
        );
        let mut wrong = call;
        wrong[0] ^= 1;
        assert_eq!(f.decode_call(&wrong), Err(AbiError::Selector));
    }
    assert_eq!(
        iface.function("lookup(uint)").unwrap().signature(),
        "lookup(uint256)"
    );
    assert_eq!(
        iface.function("transfer").unwrap().state_mutability(),
        StateMutability::Nonpayable
    );
    for v in fixture["errors"].as_array().unwrap() {
        let error = iface.error(v["name"].as_str().unwrap()).unwrap();
        assert_eq!(hex(&error.selector()), v["selector"]);
        let data = error.encode(v["args"].as_array().unwrap()).unwrap();
        assert_eq!(hex(&data), v["encoded"]);
        assert_eq!(error.encode(&error.decode(&data).unwrap()).unwrap(), data);
        assert_eq!(
            iface
                .error_by_selector(error.selector())
                .unwrap()
                .signature(),
            error.signature()
        );
    }
    assert!(iface.has_receive());
    assert_eq!(iface.fallback_mutability(), Some(StateMutability::Payable));
    assert_eq!(
        iface
            .constructor()
            .unwrap()
            .encode_arguments(&[json!("0x0000000000000000000000000000000000000001")])
            .unwrap()
            .len(),
        32
    );
    assert_eq!(
        hex(&function_selector("transfer(address,uint)").unwrap()),
        "0xa9059cbb"
    );
    assert_eq!(
        hex(&function_selector("baz(uint32,bool)").unwrap()),
        "0xcdcd77c0"
    );
}
#[test]
fn indexed_and_anonymous_event_logs_match_js() {
    let fixture = fixture();
    let iface =
        AbiInterface::from_json(&serde_json::to_vec(&fixture["interface"]).unwrap()).unwrap();
    for v in fixture["events"].as_array().unwrap() {
        let event = iface.event(v["name"].as_str().unwrap()).unwrap();
        assert_eq!(event.topic_hash().to_string(), v["topic"]);
        let (topics, data) = event.encode_log(v["args"].as_array().unwrap()).unwrap();
        assert_eq!(
            json!(topics.iter().map(ToString::to_string).collect::<Vec<_>>()),
            v["log"]["topics"]
        );
        assert_eq!(hex(&data), v["log"]["data"]);
        let result = event.decode_log(&topics, &data).unwrap();
        assert_eq!(result.len(), v["args"].as_array().unwrap().len());
        if event.name() == "Note" {
            assert!(matches!(result[0], AbiEventValue::IndexedHash(_)));
            assert!(matches!(result[1], AbiEventValue::IndexedHash(_)));
        }
        assert!(
            event
                .decode_log(&topics[..topics.len() - 1], &data)
                .is_err()
        );
        let mut extra = topics.clone();
        extra.push(Hash32::ZERO);
        assert!(event.decode_log(&extra, &data).is_err());
        if !event.anonymous() {
            let mut wrong = topics;
            wrong[0] = Hash32::ZERO;
            assert!(event.decode_log(&wrong, &data).is_err());
        }
        assert_eq!(
            iface
                .event_by_topic(event.topic_hash())
                .unwrap()
                .signature(),
            event.signature()
        );
    }
}
#[test]
fn indexed_compound_topics_follow_in_place_padding_rules() {
    let ty = AbiType::parse("(string,uint8[])").unwrap();
    let topic = indexed_event_topic(&ty, &json!(["a", [1, 2]])).unwrap();
    let mut expected = vec![0; 96];
    expected[0] = b'a';
    expected[63] = 1;
    expected[95] = 2;
    assert_eq!(topic.bytes(), &quai_crypto::keccak256(&expected));
    assert_eq!(
        indexed_event_topic(&AbiType::parse("uint8[2]").unwrap(), &json!([1, 2]))
            .unwrap()
            .bytes(),
        &quai_crypto::keccak256(&expected[32..])
    );
    assert_eq!(
        signature_hash("Transfer(address,address,uint)")
            .unwrap()
            .to_string(),
        "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"
    );
}
#[test]
fn interface_rejects_duplicate_keys_declarations_and_invalid_metadata() {
    for source in [
        r#"[{"type":"function","type":"event","name":"f"}]"#,
        r#"[{"type":"receive","stateMutability":"nonpayable"}]"#,
        r#"[{"type":"function","name":"f","unknown":1}]"#,
        r#"[{"type":"function","name":"f","stateMutability":"view","payable":true}]"#,
        r#"[{"type":"function","name":"f","inputs":[{"type":"tuple"}]}]"#,
        r#"[{"type":"function","name":"f","inputs":[{"type":"bool","indexed":false}]}]"#,
    ] {
        assert!(
            AbiInterface::from_json(source.as_bytes()).is_err(),
            "{source}"
        );
    }
    let f = json!({"type":"function","name":"f","inputs":[],"outputs":[]});
    assert_eq!(
        AbiInterface::from_json(&serde_json::to_vec(&json!([f, f])).unwrap()).unwrap_err(),
        AbiError::Ambiguous
    );
    let event = json!({"type":"event","name":"E","inputs":(0..4).map(|i|json!({"name":format!("x{i}"),"type":"uint8","indexed":true})).collect::<Vec<_>>()});
    assert!(AbiInterface::from_json(&serde_json::to_vec(&json!([event])).unwrap()).is_err());
}
#[test]
fn arbitrary_short_bytes_never_panic_and_success_is_canonical() {
    let schemas = [
        vec![],
        vec![AbiType::parse("bool").unwrap()],
        vec![AbiType::parse("(uint8,string[])[]").unwrap()],
        vec![AbiType::parse("()[]").unwrap()],
    ];
    let mut state = 0x123456789abcdef0u64;
    for len in 0..257 {
        let mut bytes = vec![0; len];
        for b in &mut bytes {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *b = state as u8;
        }
        for types in &schemas {
            if let Ok(value) = AbiCoder::decode(types, &bytes) {
                assert_eq!(AbiCoder::encode(types, &value).unwrap(), bytes);
            }
        }
    }
    let encoded = decode("0x");
    assert!(encoded.is_empty());
}

#[test]
fn four_byte_selector_collisions_require_explicit_signature_selection() {
    let abi = json!([
        {"type":"function","name":"burn","inputs":[{"type":"uint256"}]},
        {"type":"function","name":"collate_propagate_storage","inputs":[{"type":"bytes16"}]}
    ]);
    let iface = AbiInterface::from_json(&serde_json::to_vec(&abi).unwrap()).unwrap();
    let first = iface.function("burn(uint256)").unwrap();
    let other = iface
        .function("collate_propagate_storage(bytes16)")
        .unwrap();
    assert_eq!(first.selector(), other.selector());
    assert_eq!(
        iface.function_by_selector(first.selector()).unwrap_err(),
        AbiError::Ambiguous
    );
}

#[test]
fn explicit_strict_decoding_policies_reject_js_permissive_cases() {
    let fixture = fixture();
    for case in fixture["decodePolicy"].as_array().unwrap() {
        assert!(
            AbiCoder::decode(
                &types(&case["types"]),
                &decode(case["encoded"].as_str().unwrap())
            )
            .is_err(),
            "{}",
            case["id"]
        );
    }
}
