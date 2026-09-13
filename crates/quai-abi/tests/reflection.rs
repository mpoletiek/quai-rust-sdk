//! Published metadata/encoding vectors and traversal/name/resource boundary checks.
use quai_abi::{
    AbiError, AbiEventValue, AbiFormat, AbiInterface, AbiParameter, AbiResult, MAX_DATA_BYTES,
    MAX_DEPTH, MAX_FIELDS, MAX_SCHEMA_BYTES, MAX_VALUE_NODES,
};
use serde_json::{Value, json};
fn fixture() -> Value {
    serde_json::from_slice(include_bytes!(
        "fixtures/shared/compatibility/fixtures/abi-reflection.json"
    ))
    .unwrap()
}
fn bytes(value: &Value) -> Vec<u8> {
    quai_primitives::get_bytes(value.as_str().unwrap()).unwrap()
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn published_parameter_formats_and_array_metadata_roundtrip() {
    for row in fixture()["parameters"].as_array().unwrap() {
        let p = AbiParameter::from_human_readable(row["text"].as_str().unwrap(), true).unwrap();
        for (style, key) in [
            (AbiFormat::Full, "full"),
            (AbiFormat::Minimal, "minimal"),
            (AbiFormat::Signature, "signature"),
        ] {
            assert_eq!(p.format(style).unwrap(), row[key]);
        }
        let json = p.format(AbiFormat::Json).unwrap();
        let round = AbiParameter::from_json(json.as_bytes(), true).unwrap();
        assert_eq!(round, p);
        let published =
            AbiParameter::from_json(&serde_json::to_vec(&row["json"]).unwrap(), true).unwrap();
        assert_eq!(published.abi_type(), p.abi_type());
    }
    let p = AbiParameter::from_json(br#"{"type":"tuple[][2]","name":"rows","indexed":true,"internalType":"struct X[][2]","components":[{"type":"uint","name":"value","internalType":"uint256"}]}"#,true).unwrap();
    assert_eq!(p.array_length(), Some(Some(2)));
    assert!(p.is_array());
    assert!(!p.is_tuple());
    assert!(p.components().is_empty());
    let child = p.array_child().unwrap();
    assert_eq!(child.array_length(), Some(None));
    assert_eq!(child.name(), "");
    assert_eq!(child.indexed(), None);
    let tuple = child.array_child().unwrap();
    assert!(tuple.is_tuple());
    assert_eq!(tuple.array_length(), None);
    assert_eq!(tuple.components()[0].internal_type(), Some("uint256"));
    assert_eq!(p.internal_type(), Some("struct X[][2]"));
    let exported: Value = serde_json::from_str(&p.format(AbiFormat::Json).unwrap()).unwrap();
    assert_eq!(exported["indexed"], true);
    assert_eq!(
        AbiParameter::from_json(&serde_json::to_vec(&exported).unwrap(), true).unwrap(),
        p
    );
    for text in [
        "uint) returns (uint",
        "uint, bool",
        "uint) @100",
        "uint indexed",
        "(uint x,uint x)",
        "uint x garbage",
        "",
        "uint) payable (",
    ] {
        assert!(
            AbiParameter::from_human_readable(text, false).is_err(),
            "{text}"
        );
    }
    assert!(AbiParameter::from_json(br#"{"type":"uint","type":"bool"}"#, false).is_err());
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn gas_hints_are_exact_metadata_and_never_affect_selectors_or_encoding() {
    for row in fixture()["gas"].as_array().unwrap() {
        let interface =
            AbiInterface::from_human_readable(&[row["text"].as_str().unwrap()]).unwrap();
        assert_eq!(
            interface.format_human_readable(false).unwrap(),
            [row["full"].as_str().unwrap()]
        );
        assert_eq!(
            interface.format_human_readable(true).unwrap(),
            [row["minimal"].as_str().unwrap()]
        );
        let hint = if let Some(c) = interface.constructor() {
            assert_eq!(c.input_names(), ["x"]);
            c.gas_hint()
        } else {
            interface.function("f").unwrap().gas_hint()
        };
        assert_eq!(hint.unwrap().to_string(), row["gas"]);
        let exported = interface.format_json().unwrap();
        assert_eq!(
            AbiInterface::from_json(exported.as_bytes())
                .unwrap()
                .format_json()
                .unwrap(),
            exported
        );
    }
    let plain = AbiInterface::from_human_readable(&["f(uint x)"]).unwrap();
    let hint = AbiInterface::from_json(
        br#"[{"type":"function","name":"f","inputs":[{"type":"uint","name":"x"}],"gas":"0xffff"}]"#,
    )
    .unwrap();
    assert_eq!(
        hint.function("f").unwrap().selector(),
        plain.function("f").unwrap().selector()
    );
    assert_eq!(
        hint.function("f")
            .unwrap()
            .encode_call(&[json!(7)])
            .unwrap(),
        plain
            .function("f")
            .unwrap()
            .encode_call(&[json!(7)])
            .unwrap()
    );
    for gas in [
        json!(-1),
        json!(1.5),
        json!(true),
        json!(""),
        json!("-1"),
        json!("+1"),
        json!("1e3"),
        json!("0x"),
        json!(format!("1{}", "0".repeat(78))),
    ] {
        assert!(
            AbiInterface::from_json(
                &serde_json::to_vec(&json!([{"type":"function","name":"f","gas":gas}])).unwrap()
            )
            .is_err()
        );
    }
    for text in [
        "f() @",
        "f() @-1",
        "f() @1 @2",
        "f() @1 payable",
        "event E() @1",
        "error E() @1",
        "fallback() @1",
        "receive() payable @1",
    ] {
        assert!(
            AbiInterface::from_human_readable(&[text]).is_err(),
            "{text}"
        );
    }
    for gas in [Value::Null, json!(0), json!(9007199254740993_u64)] {
        assert!(
            AbiInterface::from_json(
                &serde_json::to_vec(&json!([{"type":"constructor","gas":gas}])).unwrap()
            )
            .is_ok()
        );
    }
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn named_decoding_retains_call_return_error_and_indexed_event_identity() {
    let f = fixture();
    let abi: Vec<_> = f["abi"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let interface = AbiInterface::from_human_readable(&abi).unwrap();
    let function = interface.function("f").unwrap();
    let call = function
        .decode_call_named(&bytes(&f["call"]["data"]))
        .unwrap();
    assert_eq!(call.values(), f["call"]["values"].as_array().unwrap());
    assert_eq!(call.get_value("length"), Some(&json!("42")));
    assert_eq!(call.to_object().unwrap()["payload"], &json!("0x1234"));
    let result = function
        .decode_returns_named(&bytes(&f["returns"]["data"]))
        .unwrap();
    assert_eq!(result.get_value("then"), Some(&json!("99")));
    assert_eq!(result.values(), f["returns"]["values"].as_array().unwrap());
    assert_eq!(function.output_names(), ["then", "detail"]);
    assert_eq!(
        function.output_parameters()[1].components()[1].name(),
        "who"
    );
    let error = interface.error("Denied").unwrap();
    assert_eq!(error.input_names(), ["code", "reason"]);
    assert_eq!(error.input_parameters()[0].name(), "code");
    assert_eq!(
        error
            .decode_named(&bytes(&f["error"]["data"]))
            .unwrap()
            .get_value("code"),
        Some(&json!("3"))
    );
    let event = interface.event("Changed").unwrap();
    let topics: Vec<_> = f["event"]["topics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().parse().unwrap())
        .collect();
    let result = event
        .decode_log_named(&topics, &bytes(&f["event"]["data"]))
        .unwrap();
    assert_eq!(
        result.get_value("label"),
        Some(&AbiEventValue::IndexedHash(topics[1]))
    );
    assert_eq!(
        result.get_value("value"),
        Some(&AbiEventValue::Value(json!("8")))
    );
    let mut corrupt = bytes(&f["call"]["data"]);
    corrupt[0] ^= 1;
    assert_eq!(
        function.decode_call_named(&corrupt).unwrap_err(),
        AbiError::Selector
    );
    let mut corrupt = bytes(&f["returns"]["data"]);
    corrupt.push(0);
    assert!(function.decode_returns_named(&corrupt).is_err());
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn result_names_are_unique_bounded_and_free_of_object_prototype_collisions() {
    let r = AbiResult::from_items(
        vec![1, 2, 3, 4],
        vec![
            Some("same".into()),
            Some("same".into()),
            Some("length".into()),
            Some("__proto__".into()),
        ],
    )
    .unwrap();
    assert_eq!(r.get_value("same"), None);
    assert_eq!(r.get_value("length"), Some(&3));
    assert_eq!(r.get_value("__proto__"), Some(&4));
    assert_eq!(r.to_object().unwrap_err(), AbiError::Value);
    let filtered = r.filter(|i, _| i > 1);
    assert_eq!(filtered.get_value("__proto__"), Some(&4));
    assert_eq!(r.filter(|i, _| i == 0).names(), [None]);
    let s = r.slice(2..4).unwrap();
    assert_eq!(s.to_object().unwrap()["__proto__"], &4);
    assert_eq!(s.to_object().unwrap().len(), 2);
    assert!(r.slice(4..5).is_err());
    assert!(r.slice(std::ops::Range { start: 3, end: 1 }).is_err());
    assert_eq!(r.slice(0..1).unwrap().get_value("same"), None);
    assert_eq!(
        AbiResult::from_items(vec![0], vec![Some(String::new())])
            .unwrap()
            .names(),
        [None]
    );
    assert_eq!(
        AbiResult::from_items(vec![0], vec![]).unwrap_err(),
        AbiError::Value
    );
    assert_eq!(
        AbiResult::from_items(vec![0; MAX_FIELDS + 1], vec![None; MAX_FIELDS + 1]).unwrap_err(),
        AbiError::Limit
    );
    assert_eq!(
        AbiResult::from_items(vec![0], vec![Some("x".repeat(MAX_SCHEMA_BYTES + 1))]).unwrap_err(),
        AbiError::Limit
    );
    assert_eq!(s.into_values(), [3, 4]);
}
fn ready<F: std::future::Future>(future: F) -> F::Output {
    let mut f = std::pin::pin!(future);
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    match f.as_mut().poll(&mut context) {
        std::task::Poll::Ready(value) => value,
        _ => panic!("test future must finish synchronously"),
    }
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn traversal_validates_complete_shape_then_visits_sync_and_non_send_async_leaves() {
    let parameter =
        AbiParameter::from_human_readable("(uint count,(string label,bool enabled)[] rows)", false)
            .unwrap();
    let object =
        json!({"count":7,"rows":[{"label":"a","enabled":true},{"label":"b","enabled":false}]});
    let positional = json!([7, [["a", true], ["b", false]]]);
    let mut seen = Vec::new();
    assert_eq!(
        parameter
            .walk(&object, |ty, v| {
                seen.push(ty.canonical_name());
                Ok(v.clone())
            })
            .unwrap(),
        positional
    );
    assert_eq!(seen, ["uint256", "string", "bool", "string", "bool"]);
    let shared = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let result = ready(parameter.walk_async(&object, |ty, v| {
        let shared = shared.clone();
        async move {
            shared.borrow_mut().push(ty.canonical_name());
            Ok(v.clone())
        }
    }))
    .unwrap();
    assert_eq!(result, positional);
    assert_eq!(*shared.borrow(), seen);
    let mut called = 0;
    for bad in [
        json!([7, [["a", true], ["b"]]]),
        json!({"count":7,"rows":[],"extra":false}),
        json!({"count":7}),
        json!([7, {}]),
    ] {
        assert_eq!(
            parameter
                .walk(&bad, |_, v| {
                    called += 1;
                    Ok(v.clone())
                })
                .unwrap_err(),
            AbiError::Value
        );
    }
    assert_eq!(called, 0);
    let fixed = AbiParameter::from_human_readable("uint[2]", false).unwrap();
    assert!(fixed.walk(&json!([1]), |_, v| Ok(v.clone())).is_err());
    let mut calls = 0;
    assert_eq!(
        parameter
            .walk(&object, |_, _| {
                calls += 1;
                Err(AbiError::NotFound)
            })
            .unwrap_err(),
        AbiError::NotFound
    );
    assert_eq!(calls, 1);
    let empty = AbiParameter::from_human_readable("()", false).unwrap();
    assert_eq!(empty.walk(&json!({}), |_, _| panic!()).unwrap(), json!([]));
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn traversal_enforces_combined_output_and_input_budgets() {
    let parameter = AbiParameter::from_human_readable("uint[]", false).unwrap();
    let mut called = 0;
    let too_many = json!(vec![0; MAX_VALUE_NODES]);
    assert_eq!(
        parameter
            .walk(&too_many, |_, v| {
                called += 1;
                Ok(v.clone())
            })
            .unwrap_err(),
        AbiError::Limit
    );
    assert_eq!(called, 0);
    let mut deep = json!(0);
    for _ in 0..MAX_DEPTH + 1 {
        deep = json!([deep]);
    }
    assert_eq!(
        parameter.walk(&deep, |_, v| Ok(v.clone())).unwrap_err(),
        AbiError::Limit
    );
    let mut count = 0;
    assert_eq!(
        parameter
            .walk(&json!([1, 2, 3]), |_, _| {
                count += 1;
                Ok(json!("x".repeat(MAX_DATA_BYTES / 2 + 1)))
            })
            .unwrap_err(),
        AbiError::Limit
    );
    assert_eq!(count, 2);
    assert_eq!(
        parameter
            .walk(&json!([1, 2]), |_, _| Ok(json!(vec![
                0;
                MAX_VALUE_NODES / 2
            ])))
            .unwrap_err(),
        AbiError::Limit
    );
    assert_eq!(
        parameter
            .walk(&json!([1]), |_, _| Ok(deep.clone()))
            .unwrap_err(),
        AbiError::Limit
    );
}

#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn individual_fragment_formats_preserve_normalized_metadata_and_order() {
    let interface = AbiInterface::from_human_readable(&[
        "event E((uint n)[] indexed rows)",
        "function f(uint x) view returns (uint y) @100",
        "constructor(address owner) payable @200",
        "error E(uint reason)",
        "fallback() payable",
        "receive() payable",
    ])
    .unwrap();
    assert_eq!(interface.declarations().len(), 6);
    assert_eq!(interface.declarations()[0]["type"], "event");
    for style in [AbiFormat::Full, AbiFormat::Minimal, AbiFormat::Json] {
        let formatted = [
            interface.event("E").unwrap().format(style).unwrap(),
            interface.function("f").unwrap().format(style).unwrap(),
            interface.constructor().unwrap().format(style).unwrap(),
            interface.error("E").unwrap().format(style).unwrap(),
        ];
        for (index, text) in formatted.iter().enumerate() {
            if style == AbiFormat::Json {
                let value: Value = serde_json::from_str(text).unwrap();
                let restored =
                    AbiInterface::from_json(&serde_json::to_vec(&json!([value])).unwrap()).unwrap();
                assert_eq!(
                    restored.format_human_readable(false).unwrap()[0],
                    interface.format_human_readable(false).unwrap()[index]
                );
            } else {
                assert_eq!(
                    text,
                    &interface
                        .format_human_readable(style == AbiFormat::Minimal)
                        .unwrap()[index]
                );
            }
        }
    }
    assert_eq!(
        interface
            .function("f")
            .unwrap()
            .format(AbiFormat::Signature)
            .unwrap(),
        "f(uint256)"
    );
    assert_eq!(
        interface
            .event("E")
            .unwrap()
            .format(AbiFormat::Signature)
            .unwrap(),
        "E((uint256)[])"
    );
    assert_eq!(
        interface
            .error("E")
            .unwrap()
            .format(AbiFormat::Signature)
            .unwrap(),
        "E(uint256)"
    );
    assert_eq!(
        interface
            .constructor()
            .unwrap()
            .format(AbiFormat::Signature)
            .unwrap_err(),
        AbiError::Schema
    );
}
