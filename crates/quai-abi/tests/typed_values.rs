//! Published encoder parity, immediate validation and bounded typed defaults.
use quai_abi::{AbiCoder, AbiError, AbiType, AbiValue};
use serde_json::{Value, json};
fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "fixtures/shared/compatibility/fixtures/typed-values.json"
    ))
    .unwrap()
}
fn bytes(text: &str) -> Vec<u8> {
    quai_primitives::get_bytes(text).unwrap()
}
#[test]
fn typed_values_validate_all_integer_widths_and_byte_lengths_before_encoding() {
    let fixture = fixture();
    let mut delayed_reference_errors = 0;
    for v in fixture["vectors"].as_array().unwrap() {
        let result = AbiValue::new(
            v["type"].as_str().unwrap().parse().unwrap(),
            v["value"].clone(),
        );
        if v["encoded"]["error"] == true {
            assert!(result.is_err(), "{v}");
            if v["constructor"]["value"].is_string() {
                delayed_reference_errors += 1;
            }
        } else {
            let result = result.unwrap_or_else(|e| panic!("{v}: {e}"));
            assert_eq!(
                result.encode().unwrap(),
                bytes(v["encoded"]["value"].as_str().unwrap()),
                "{v}"
            );
            assert_eq!(result.expect_type(result.abi_type()).unwrap(), &v["value"]);
            let decoded = AbiCoder::decode(
                std::slice::from_ref(result.abi_type()),
                &result.encode().unwrap(),
            )
            .unwrap();
            assert_eq!(
                AbiValue::new(result.abi_type().clone(), decoded[0].clone())
                    .unwrap()
                    .encode()
                    .unwrap(),
                result.encode().unwrap()
            );
        }
    }
    assert!(delayed_reference_errors > 100);
    for v in fixture["bounds"].as_array().unwrap() {
        let ty: AbiType = v["type"].as_str().unwrap().parse().unwrap();
        let (min, max) = ty.integer_bounds().unwrap();
        assert_eq!(min, v["min"]);
        assert_eq!(max, v["max"]);
        assert!(ty.is_integer());
        assert!(!ty.is_bytes());
        assert!(!ty.is_string());
        assert_eq!(v["referenceMax"], 0);
        assert_ne!(max, "0"); // Explicit correction of JS stub.
    }
}
#[test]
fn defaults_match_solidity_types_and_metadata_remains_explicit() {
    for (ty, expected) in [
        ("bool", json!(false)),
        ("int8", json!("0")),
        ("bytes2", json!("0x0000")),
        ("bytes", json!("0x")),
        ("string", json!("")),
        (
            "address",
            json!("0x0000000000000000000000000000000000000000"),
        ),
        ("uint8[]", json!([])),
        ("uint8[2]", json!(["0", "0"])),
        (
            "(bool,string,bytes2[2])",
            json!([false, "", ["0x0000", "0x0000"]]),
        ),
        ("uint8[0]", json!([])),
        ("bytes32[32767][0]", json!([])),
    ] {
        let value = AbiValue::default_for(ty.parse().unwrap()).unwrap();
        assert_eq!(value.value(), &expected, "{ty}");
        let encoded = value.encode().unwrap();
        assert_eq!(
            AbiCoder::decode(std::slice::from_ref(value.abi_type()), &encoded).unwrap(),
            vec![expected]
        );
    }
    let ty: AbiType = "uint8[2]".parse().unwrap();
    let (element, length) = ty.array_info().unwrap();
    assert_eq!(element.canonical_name(), "uint8");
    assert_eq!(length, Some(2));
    assert_eq!(
        "uint8[]"
            .parse::<AbiType>()
            .unwrap()
            .array_info()
            .unwrap()
            .1,
        None
    );
    assert_eq!(
        "(bool,uint)"
            .parse::<AbiType>()
            .unwrap()
            .tuple_components()
            .unwrap()[1]
            .canonical_name(),
        "uint256"
    );
    for ty in ["bytes", "bytes1", "bytes32"] {
        assert!(ty.parse::<AbiType>().unwrap().is_bytes());
    }
    assert!("string".parse::<AbiType>().unwrap().is_string());
    assert!(
        "bool"
            .parse::<AbiType>()
            .unwrap()
            .integer_bounds()
            .is_none()
    );
    let value = AbiValue::new("string".parse().unwrap(), json!("secret-memo")).unwrap();
    assert!(!format!("{value:?}").contains("secret-memo"));
    assert_eq!(
        value.expect_type(&"bytes".parse().unwrap()),
        Err(AbiError::Value)
    );
    assert_eq!(value.into_parts().1, json!("secret-memo"));
}
#[test]
fn defaults_preflight_text_and_encoding_budgets_and_coercions_are_rejected() {
    // Both type expressions are bounded and valid, but their default values exceed
    // the text or encoded-output budget. Reject before constructing those values.
    for ty in ["bytes32[20000]", "bytes[16384]"] {
        assert!(matches!(
            AbiValue::default_for(ty.parse().unwrap()),
            Err(AbiError::Limit)
        ));
    }
    for (ty, value) in [
        ("bool", json!(1)),
        ("uint8", json!(1.25)),
        ("int8", json!("128")),
        ("bytes2", json!("0x01")),
        ("uint8[2]", json!([1])),
        ("uint8", json!("1e2")),
    ] {
        assert!(AbiValue::new(ty.parse().unwrap(), value).is_err());
    }
}
