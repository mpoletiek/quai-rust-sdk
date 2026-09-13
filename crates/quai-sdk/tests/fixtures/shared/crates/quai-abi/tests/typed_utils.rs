//! Exact published arbitrary-type encoding and schema-aware traversal behavior.
use quai_abi::{
    MAX_DATA_BYTES, MAX_DEPTH, MAX_VALUE_NODES, TypedDataEncoder, TypedDataError, TypedDataTypes,
};
use serde_json::{Value, json};
fn fixture() -> Value {
    serde_json::from_slice(include_bytes!(
        "fixtures/shared/compatibility/fixtures/typed-data-utils.json"
    ))
    .unwrap()
}
fn encoder(types: &Value) -> TypedDataEncoder {
    TypedDataEncoder::new(&serde_json::from_value::<TypedDataTypes>(types.clone()).unwrap())
        .unwrap()
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn all_published_type_encoders_and_hashes_match_and_schema_views_stay_immutable() {
    let f = fixture();
    let e = encoder(&f["types"]);
    assert_eq!(serde_json::to_value(e.types()).unwrap(), f["types"]);
    assert_eq!(e.primary_type(), "Message");
    for row in f["vectors"].as_array().unwrap() {
        let ty = row["type"].as_str().unwrap();
        let resolved = e.encoder(ty).unwrap();
        let expected = quai_primitives::get_bytes(row["encoding"].as_str().unwrap()).unwrap();
        assert_eq!(resolved.encode(&row["value"]).unwrap(), expected, "{ty}");
        assert_eq!(e.encode_data(ty, &row["value"]).unwrap(), expected);
        assert_eq!(
            e.hash_struct(ty, &row["value"]).unwrap().to_string(),
            row["hash"],
            "{ty}"
        );
        assert_eq!(resolved.clone().encode(&row["value"]).unwrap(), expected);
    }
    assert_eq!(
        e.encoder("Missing[]").unwrap_err(),
        TypedDataError::UnknownType
    );
    assert_eq!(
        e.encoder("Missing[0]").unwrap_err(),
        TypedDataError::UnknownType
    );
    for ty in [
        "uint",
        "int",
        "uint7",
        "uint256[01]",
        "bool[1]garbage",
        "Person[32769]",
    ] {
        assert!(e.encoder(ty).is_err(), "{ty}");
    }
    assert_eq!(
        e.encode_data("uint8", &json!(256)).unwrap_err(),
        TypedDataError::Integer
    );
    assert_eq!(
        e.encode_data("int8", &json!(-129)).unwrap_err(),
        TypedDataError::Integer
    );
    assert!(e.encode_data("address", &json!("invalid")).is_err());
    assert!(e.encode_data("bool", &json!(1)).is_err());
    assert_eq!(
        e.encode_data("uint256[2]", &json!([1])).unwrap_err(),
        TypedDataError::ArrayLength
    );
    let mut copy = e.types().clone();
    copy.clear();
    assert_eq!(e.types().len(), 2);
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn nested_visits_match_published_leaf_order_and_preserve_exact_named_objects() {
    let f = fixture();
    let e = encoder(&f["types"]);
    let mut seen = Vec::new();
    let result = e
        .visit(&f["message"], |ty, value| {
            seen.push(json!({"type":ty,"value":value}));
            Ok(if ty == "address" {
                json!(value.as_str().unwrap().to_lowercase())
            } else {
                value.clone()
            })
        })
        .unwrap();
    assert_eq!(json!(seen), f["visited"]);
    assert_eq!(result, f["transformed"]);
    assert_eq!(e.hash(&result).unwrap(), e.hash(&f["message"]).unwrap());
    assert_eq!(
        e.visit_type("uint256[2]", &json!([1, 2]), |ty, v| {
            assert_eq!(ty, "uint256");
            Ok(json!(v.to_string()))
        })
        .unwrap(),
        json!(["1", "2"])
    );
    assert_eq!(
        e.visit_type("Person[0]", &json!([]), |_, _| panic!())
            .unwrap(),
        json!([])
    );
    assert_eq!(
        e.visit_type("uint8", &json!(999), |_, v| Ok(v.clone()))
            .unwrap(),
        json!(999)
    );
    let e = encoder(
        &json!({"T":[{"name":"__proto__","type":"uint256"},{"name":"constructor","type":"string"}]}),
    );
    let object = json!({"__proto__":7,"constructor":"name"});
    assert_eq!(e.visit(&object, |_, v| Ok(v.clone())).unwrap(), object);
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn shape_errors_precede_callbacks_and_processing_failures_stop_immediately() {
    let e = encoder(
        &json!({"T":[{"name":"first","type":"uint256"},{"name":"later","type":"uint256[2]"}]}),
    );
    let mut called = 0;
    for value in [
        json!({"first":1,"later":[1]}),
        json!({"first":1,"later":[1,2],"extra":0}),
        json!({"first":1}),
        json!([1, [1, 2]]),
    ] {
        assert!(
            e.visit(&value, |_, v| {
                called += 1;
                Ok(v.clone())
            })
            .is_err()
        );
    }
    assert_eq!(called, 0);
    let value = json!({"first":1,"later":[1,2]});
    assert_eq!(
        e.visit(&value, |_, _| {
            called += 1;
            Err(TypedDataError::Bytes)
        })
        .unwrap_err(),
        TypedDataError::Bytes
    );
    assert_eq!(called, 1);
    assert_eq!(
        e.visit_type("Missing[]", &json!([]), |_, _| panic!())
            .unwrap_err(),
        TypedDataError::UnknownType
    );
    let empty = encoder(&json!({"Empty":[]}));
    assert_eq!(empty.visit(&json!({}), |_, _| panic!()).unwrap(), json!({}));
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn visits_and_type_encoders_share_bounded_input_and_combined_output_limits() {
    let e = encoder(&json!({"T":[{"name":"items","type":"uint256[]"}]}));
    let mut calls = 0;
    assert_eq!(
        e.visit_type("uint256[]", &json!(vec![0; MAX_VALUE_NODES]), |_, v| {
            calls += 1;
            Ok(v.clone())
        })
        .unwrap_err(),
        TypedDataError::Limit
    );
    assert_eq!(calls, 0);
    assert_eq!(
        e.encode_data("uint256[]", &json!(vec![0; MAX_VALUE_NODES]))
            .unwrap_err(),
        TypedDataError::Limit
    );
    assert_eq!(
        e.visit_type("uint256[]", &json!([1, 2, 3]), |_, _| {
            calls += 1;
            Ok(json!("x".repeat(MAX_DATA_BYTES / 2 + 1)))
        })
        .unwrap_err(),
        TypedDataError::Limit
    );
    assert_eq!(calls, 2);
    assert_eq!(
        e.visit_type("uint256[]", &json!([1, 2]), |_, _| Ok(json!(vec![
            0;
            MAX_VALUE_NODES
                / 2
        ])))
        .unwrap_err(),
        TypedDataError::Limit
    );
    let mut deep = json!(0);
    for _ in 0..MAX_DEPTH + 1 {
        deep = json!([deep]);
    }
    assert_eq!(
        e.visit_type("uint256", &deep, |_, v| Ok(v.clone()))
            .unwrap_err(),
        TypedDataError::Limit
    );
    assert_eq!(
        e.visit_type("uint256[]", &json!([1]), |_, _| Ok(deep.clone()))
            .unwrap_err(),
        TypedDataError::Limit
    );
}
