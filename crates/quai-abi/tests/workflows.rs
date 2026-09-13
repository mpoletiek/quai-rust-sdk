//! Reference event filters and strict call/log/revert selection under shared bounds.
use quai_abi::{
    AbiCoder, AbiError, AbiEventValue, AbiFilterTopic, AbiFilterValue, AbiInterface, AbiType,
    MAX_DATA_BYTES, ParsedRevert,
};
use quai_primitives::{Hash32, get_bytes, hexlify};
use serde_json::{Value, json};
fn fixture() -> Value {
    serde_json::from_slice(include_bytes!(
        "fixtures/shared/compatibility/fixtures/abi-workflows.json"
    ))
    .unwrap()
}
fn interface(value: &Value) -> AbiInterface {
    AbiInterface::from_json(&serde_json::to_vec(value).unwrap()).unwrap()
}
fn topics_json(topics: &[AbiFilterTopic]) -> Value {
    Value::Array(
        topics
            .iter()
            .map(|topic| match topic {
                AbiFilterTopic::Any => Value::Null,
                AbiFilterTopic::Exact(hash) => json!(hash.to_string()),
                AbiFilterTopic::AnyOf(hashes) => {
                    json!(hashes.iter().map(ToString::to_string).collect::<Vec<_>>())
                }
            })
            .collect(),
    )
}
#[test]
fn pinned_filters_use_canonical_words_and_explicit_strict_deviations() {
    let fixture = fixture();
    let mut matched = 0;
    let mut rejected = 0;
    let mut reference_negative = 0;
    for row in fixture["filters"].as_array().unwrap() {
        let abi = interface(&row["abi"]);
        let filters: Vec<_> = row["criteria"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                if c.is_null() {
                    AbiFilterValue::Any
                } else if let Some(value) = c.get("exact") {
                    AbiFilterValue::Exact(value)
                } else {
                    AbiFilterValue::AnyOf(c["anyOf"].as_array().unwrap())
                }
            })
            .collect();
        let result = abi
            .event("Filtered")
            .unwrap()
            .encode_filter_topics(&filters);
        if row["rust"] == "reject" {
            assert!(result.is_err(), "{row}");
            rejected += 1;
            continue;
        }
        let actual = topics_json(&result.unwrap());
        assert_eq!(actual, row["topics"], "{row}");
        if row["referenceError"] == true {
            reference_negative += 1;
        } else {
            assert_eq!(actual, row["reference"], "{row}");
        }
        matched += 1;
    }
    assert_eq!((matched, rejected, reference_negative), (174, 72, 64));
}
#[test]
fn filters_bound_all_alternatives_before_hashing_and_distinguish_arrays_from_or() {
    let abi = interface(
        &json!([{"type":"event","name":"Array","inputs":[{"type":"uint256[]","indexed":true}]},{"type":"event","name":"Text","inputs":[{"type":"string","indexed":true}]},{"type":"event","name":"Anonymous","anonymous":true,"inputs":[{"type":"int8","indexed":true},{"type":"uint8","indexed":false},{"type":"bytes2","indexed":true}]}]),
    );
    let event = abi.event("Array").unwrap();
    let array = json!(["1", "2"]);
    // Indexed arrays have no length prefix: an independent canonical element sequence
    // supplies the preimage, rather than reusing the indexed-event implementation.
    let preimage = AbiCoder::encode(
        &vec![AbiType::parse("uint256").unwrap(); 2],
        &[json!("1"), json!("2")],
    )
    .unwrap();
    use tiny_keccak::{Hasher, Keccak};
    let mut h = Keccak::v256();
    h.update(&preimage);
    let mut hash = [0; 32];
    h.finalize(&mut hash);
    let expected = vec![
        AbiFilterTopic::Exact(event.topic_hash()),
        AbiFilterTopic::Exact(Hash32::from_bytes(hash)),
    ];
    assert_eq!(
        event
            .encode_filter_topics(&[AbiFilterValue::Exact(&array)])
            .unwrap(),
        expected
    );
    let alternatives = vec![array.clone(), json!([])];
    let or = event
        .encode_filter_topics(&[AbiFilterValue::AnyOf(&alternatives)])
        .unwrap();
    assert!(
        matches!(&or[1],AbiFilterTopic::AnyOf(hashes) if hashes.len()==2 && hashes[0]==Hash32::from_bytes(hash))
    );
    assert_eq!(
        event.encode_filter_topics(&[AbiFilterValue::AnyOf(&[])]),
        Err(AbiError::Value)
    );
    let alternatives = vec![json!(vec!["0"; 254]); 128];
    assert!(
        event
            .encode_filter_topics(&[AbiFilterValue::AnyOf(&alternatives)])
            .is_ok()
    );
    let encoded_overflow = vec![json!(vec!["0"; 255]); 128];
    assert_eq!(
        event.encode_filter_topics(&[AbiFilterValue::AnyOf(&encoded_overflow)]),
        Err(AbiError::Limit)
    );
    let node_overflow = vec![json!(vec!["0"; 256]); 128];
    assert_eq!(
        event.encode_filter_topics(&[AbiFilterValue::AnyOf(&node_overflow)]),
        Err(AbiError::Limit)
    );
    let text_overflow = vec![json!("x".repeat(MAX_DATA_BYTES / 2 + 1)); 2];
    assert_eq!(
        abi.event("Text")
            .unwrap()
            .encode_filter_topics(&[AbiFilterValue::AnyOf(&text_overflow)]),
        Err(AbiError::Limit)
    );
    let event = abi.event("Anonymous").unwrap();
    assert!(
        event
            .encode_filter_topics(&[AbiFilterValue::Any])
            .unwrap()
            .is_empty()
    );
    let topics = event
        .encode_filter_topics(&[
            AbiFilterValue::Any,
            AbiFilterValue::Any,
            AbiFilterValue::Exact(&json!("0xabcd")),
        ])
        .unwrap();
    assert_eq!(topics.len(), 2);
    assert_eq!(topics[0], AbiFilterTopic::Any);
    assert!(
        matches!(&topics[1],AbiFilterTopic::Exact(hash) if hash.bytes()[..2]==[0xab,0xcd] && hash.bytes()[2..]==[0;30])
    );
}
#[test]
fn pinned_calls_logs_and_builtin_and_custom_reverts_decode_canonically() {
    let fixture = fixture();
    let abi = interface(&fixture["abi"]);
    for row in fixture["calls"].as_array().unwrap() {
        let data = get_bytes(row["data"].as_str().unwrap()).unwrap();
        let call = abi.parse_call(&data).unwrap();
        assert_eq!(
            call.function.signature(),
            row["signature"].as_str().unwrap()
        );
        assert_eq!(json!(call.arguments), row["arguments"]);
        assert_eq!(
            hexlify(&call.function.encode_call(&call.arguments).unwrap()).unwrap(),
            row["data"]
        );
        let mut trailing = data.clone();
        trailing.push(0);
        assert!(matches!(abi.parse_call(&trailing), Err(AbiError::Encoding)));
    }
    for row in fixture["reverts"].as_array().unwrap() {
        let data = get_bytes(row["data"].as_str().unwrap()).unwrap();
        let revert = abi.parse_revert(&data).unwrap();
        assert_eq!(revert.signature(), row["signature"].as_str().unwrap());
        assert_eq!(
            revert.name(),
            row["signature"]
                .as_str()
                .unwrap()
                .split('(')
                .next()
                .unwrap()
        );
        assert_eq!(revert.selector(), data[..4]);
        match revert {
            ParsedRevert::Error(reason) => {
                assert_eq!(row["signature"], "Error(string)");
                assert_eq!(json!([reason]), row["arguments"]);
            }
            ParsedRevert::Panic(code) => {
                assert_eq!(row["signature"], "Panic(uint256)");
                assert_eq!(json!([code.to_string()]), row["arguments"]);
            }
            ParsedRevert::Custom { error, arguments } => {
                assert_eq!(error.signature(), row["signature"].as_str().unwrap());
                assert_eq!(json!(arguments), row["arguments"]);
            }
        }
        let mut trailing = data.clone();
        trailing.push(0);
        assert!(matches!(
            abi.parse_revert(&trailing),
            Err(AbiError::Encoding)
        ));
        assert!(abi.parse_revert(&data[..data.len() - 1]).is_err());
    }
    for row in fixture["logs"].as_array().unwrap() {
        let topics: Vec<Hash32> = row["topics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().parse().unwrap())
            .collect();
        let data = get_bytes(row["data"].as_str().unwrap()).unwrap();
        let log = abi.parse_log(&topics, &data).unwrap();
        assert_eq!(log.event.signature(), row["signature"].as_str().unwrap());
        let values: Vec<_> = log
            .values
            .iter()
            .map(|v| match v {
                AbiEventValue::Value(value) => json!({"value":value}),
                AbiEventValue::IndexedHash(hash) => json!({"hash":hash.to_string()}),
            })
            .collect();
        assert_eq!(json!(values), row["values"]);
        assert!(abi.parse_log(&topics[..topics.len() - 1], &data).is_err());
        let mut trailing = data.clone();
        trailing.push(0);
        assert!(matches!(
            abi.parse_log(&topics, &trailing),
            Err(AbiError::Encoding)
        ));
    }
}
#[test]
fn automatic_decoding_never_guesses_unknown_anonymous_or_colliding_declarations() {
    let abi = AbiInterface::default();
    for bytes in [&[][..], &[0; 3][..]] {
        assert!(matches!(abi.parse_call(bytes), Err(AbiError::Encoding)));
        assert!(matches!(abi.parse_revert(bytes), Err(AbiError::Encoding)));
    }
    assert!(matches!(abi.parse_call(&[0; 4]), Err(AbiError::NotFound)));
    assert!(matches!(abi.parse_revert(&[0; 4]), Err(AbiError::NotFound)));
    assert!(matches!(abi.parse_log(&[], &[]), Err(AbiError::NotFound)));
    let oversized = vec![0; MAX_DATA_BYTES + 5];
    assert!(matches!(abi.parse_call(&oversized), Err(AbiError::Limit)));
    assert!(matches!(abi.parse_revert(&oversized), Err(AbiError::Limit)));
    assert!(matches!(
        abi.parse_log(&[], &oversized),
        Err(AbiError::Limit)
    ));
    assert!(matches!(
        abi.parse_log(&[Hash32::ZERO; 5], &[]),
        Err(AbiError::Limit)
    ));
    let abi = interface(
        &json!([{"type":"event","name":"Hidden","anonymous":true,"inputs":[{"type":"bytes32","indexed":true}]}]),
    );
    let event = abi.event("Hidden").unwrap();
    assert!(event.decode_log(&[event.topic_hash()], &[]).is_ok());
    assert!(matches!(
        abi.parse_log(&[event.topic_hash()], &[]),
        Err(AbiError::NotFound)
    ));
    for kind in ["function", "error"] {
        let mut rows = json!([{"type":kind,"name":"burn","inputs":[{"type":"uint256"}]},{"type":kind,"name":"collate_propagate_storage","inputs":[{"type":"bytes16"}]}]);
        if kind == "function" {
            for row in rows.as_array_mut().unwrap() {
                row["outputs"] = json!([]);
                row["stateMutability"] = json!("nonpayable");
            }
        }
        let abi = interface(&rows);
        if kind == "function" {
            let data = abi
                .function("burn")
                .unwrap()
                .encode_call(&[json!("1")])
                .unwrap();
            assert!(matches!(abi.parse_call(&data), Err(AbiError::Ambiguous)));
        } else {
            let data = abi.error("burn").unwrap().encode(&[json!("1")]).unwrap();
            assert!(matches!(abi.parse_revert(&data), Err(AbiError::Ambiguous)));
        }
    }
    // Explicit builtin declarations preserve the same builtin representation.
    let abi = interface(&json!([{"type":"error","name":"Error","inputs":[{"type":"string"}]}]));
    let data = abi
        .error("Error")
        .unwrap()
        .encode(&[json!("reason")])
        .unwrap();
    assert!(matches!(abi.parse_revert(&data),Ok(ParsedRevert::Error(reason)) if reason=="reason"));
}
