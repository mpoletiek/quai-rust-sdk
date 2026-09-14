//! Exact numeric interchange and explicit signer recovery against source fixtures.
#![cfg(all(feature = "wallet", feature = "abi"))]
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
use quai_sdk::crypto::{RecoverableSignature, recover_message_signer};
use quai_sdk::primitives::{EncodingError, SignedUnits, Unit, get_bytes, hexlify, numeric::*};
use quai_sdk::{Address, U256};
use serde_json::Value;
fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "fixtures/shared/compatibility/fixtures/final-utilities.json"
    ))
    .unwrap()
}
fn sig(v: &Value) -> RecoverableSignature {
    RecoverableSignature::from_quais_bytes(
        &get_bytes(v.as_str().unwrap()).unwrap().try_into().unwrap(),
    )
    .unwrap()
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn signed_integer_grammar_matches_supported_reference_range() {
    for row in fixture()["integers"].as_array().unwrap() {
        let parsed = parse_integer(row["text"].as_str().unwrap());
        if row["rustReject"] == true || row["source"]["error"] == true {
            assert!(parsed.is_err(), "{}", row["text"]);
        } else {
            assert_eq!(
                parsed.unwrap().format(Unit::new(0).unwrap()),
                row["source"]["value"]
            );
        }
    }
    assert_eq!(
        parse_integer(&"0".repeat(MAX_INTEGER_TEXT + 1)),
        Err(EncodingError::TooLarge)
    );
    assert!(!parse_integer("-0").unwrap().is_negative());
    assert!(parse_uint("-1").is_err());
    assert_eq!(parse_uint("0xFF").unwrap(), U256::from(255));
    assert_eq!(parse_uint(&U256::MAX.to_string()).unwrap(), U256::MAX);
    assert!(parse_uint(&format!("{}0", U256::MAX)).is_err());
    for s in ["+1", "\n0", "0X1", "１２", "-+1", "0o8"] {
        assert!(parse_integer(s).is_err());
    }
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn explicit_number_bridges_reject_lossy_values() {
    for row in fixture()["numbers"].as_array().unwrap() {
        let value = row["input"].as_f64().unwrap();
        let integer = integer_from_safe_number(value);
        if row["source"]["error"] == true {
            assert!(integer.is_err());
        } else {
            assert_eq!(integer_to_safe_number(integer.unwrap()).unwrap(), value);
        }
    }
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.5, -0.5] {
        assert!(integer_from_safe_number(value).is_err());
    }
    assert!(!integer_from_safe_number(-0.0).unwrap().is_negative());
    assert!(
        !integer_to_safe_number(integer_from_safe_number(-0.0).unwrap())
            .unwrap()
            .is_sign_negative()
    );
    assert!(integer_to_safe_number(parse_integer("9007199254740992").unwrap()).is_err());
    assert!(integer_to_safe_number(SignedUnits::INT256_MIN).is_err());
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn unsigned_byte_and_quantity_forms_match_without_losing_large_values() {
    for row in fixture()["unsigned"].as_array().unwrap() {
        let value = parse_uint(row["value"].as_str().unwrap()).unwrap();
        let bytes = uint_to_be_array(value);
        assert_eq!(hexlify(&bytes).unwrap(), row["array"]);
        assert_eq!(uint_from_be_bytes(&bytes).unwrap(), value);
        assert_eq!(uint_to_quantity(value), row["quantity"]);
        assert_eq!(
            quai_sdk::parse_quantity(&uint_to_quantity(value)).unwrap(),
            value
        );
        for width in row["widths"].as_array().unwrap() {
            let output = uint_to_be_hex(value, width["width"].as_u64().map(|n| n as usize));
            if width["source"]["error"] == true {
                assert!(output.is_err());
            } else {
                assert_eq!(output.unwrap(), width["source"]["value"]);
            }
        }
    }
    assert_eq!(uint_from_be_bytes(&[]).unwrap(), U256::ZERO);
    assert_eq!(uint_from_be_bytes(&[0; 64]).unwrap(), U256::ZERO);
    let mut padded = vec![0; 100];
    padded[99] = 1;
    assert_eq!(uint_from_be_bytes(&padded).unwrap(), U256::from(1));
    assert_eq!(uint_from_be_bytes(&[1; 33]), Err(EncodingError::Bounds));
    assert_eq!(
        uint_to_be_hex(U256::from(1), Some(usize::MAX)),
        Err(EncodingError::TooLarge)
    );
    assert_eq!(
        uint_from_be_bytes(&vec![0; quai_sdk::primitives::MAX_ENCODING_BYTES + 1]),
        Err(EncodingError::TooLarge)
    );
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn hex_validation_is_allocation_free_and_has_explicit_shape() {
    for row in fixture()["hex"].as_array().unwrap() {
        let text = row["text"].as_str().unwrap();
        assert_eq!(
            is_hex_string(text, HexFormat::Any),
            row["any"].as_bool().unwrap()
        );
        assert_eq!(
            is_hex_string(text, HexFormat::Bytes),
            row["bytes"].as_bool().unwrap()
        );
        for (n, expected) in row["exact"].as_array().unwrap().iter().enumerate() {
            assert_eq!(
                is_hex_string(text, HexFormat::Exact(n)),
                expected.as_bool().unwrap()
            );
        }
    }
    assert!(!is_hex_string("0x", HexFormat::Exact(usize::MAX)));
    assert!(!is_hex_string("0xπ", HexFormat::Any));
    assert!(!is_hex_string(
        &format!(
            "0x{}",
            "0".repeat(quai_sdk::primitives::MAX_ENCODING_BYTES * 2 + 1)
        ),
        HexFormat::Any
    ));
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn message_recovery_returns_an_address_requiring_authorization_comparison() {
    for row in fixture()["messages"].as_array().unwrap() {
        let bytes = get_bytes(row["bytes"].as_str().unwrap()).unwrap();
        let signature = sig(&row["signature"]);
        let expected: Address = row["address"].as_str().unwrap().parse().unwrap();
        assert_eq!(
            recover_message_signer(&bytes, &signature).unwrap(),
            expected
        );
        let mut changed = bytes.clone();
        changed.push(0);
        if let Ok(recovered) = recover_message_signer(&changed, &signature) {
            assert_ne!(recovered, expected);
        }
    }
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn typed_data_recovery_binds_the_exact_document_and_domain() {
    for row in fixture()["typed"].as_array().unwrap() {
        let document =
            quai_sdk::abi::TypedData::from_json(&serde_json::to_vec(&row["document"]).unwrap())
                .unwrap();
        let signature = sig(&row["signature"]);
        let expected: Address = row["address"].as_str().unwrap().parse().unwrap();
        assert_eq!(
            quai_sdk::recover_typed_data_signer(&document, &signature).unwrap(),
            expected
        );
        let mut changed = row["document"].clone();
        changed["domain"]["chainId"] = Value::String("777".into());
        let changed =
            quai_sdk::abi::TypedData::from_json(&serde_json::to_vec(&changed).unwrap()).unwrap();
        if let Ok(recovered) = quai_sdk::recover_typed_data_signer(&changed, &signature) {
            assert_ne!(recovered, expected);
        }
    }
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn signed_256_constants_match_reference_without_float_conversions() {
    let f = fixture();
    let unit = Unit::new(0).unwrap();
    assert_eq!(
        SignedUnits::INT256_MIN.format(unit),
        f["constants"]["minInt256"]
    );
    assert_eq!(
        SignedUnits::INT256_MAX.format(unit),
        f["constants"]["maxInt256"]
    );
    assert_eq!(quai_sdk::primitives::QUAIS_SYMBOL, f["constants"]["symbol"]);
    assert_eq!(
        quai_sdk::primitives::from_twos(U256::from(1) << 255, 256).unwrap(),
        SignedUnits::INT256_MIN
    );
}

#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn shard_metadata_matches_every_published_label_without_confusing_levels() {
    use quai_sdk::{Shard, Zone};
    let f = fixture();
    assert_eq!(f["shards"].as_array().unwrap().len(), Shard::ALL.len());
    for (shard, row) in Shard::ALL.into_iter().zip(f["shards"].as_array().unwrap()) {
        let m = shard.metadata();
        assert_eq!(
            serde_json::json!({"name":m.name,"nickname":m.nickname,"shard":m.shard,"context":m.context,"byte":m.byte}),
            *row
        );
        for label in [m.name, m.nickname, m.shard, m.byte] {
            assert_eq!(label.parse::<Shard>().unwrap(), shard);
        }
    }
    for (zone, row) in Zone::ALL.into_iter().zip(f["zones"].as_array().unwrap()) {
        assert_eq!(zone.metadata(), Shard::Zone(zone).metadata());
        for field in ["name", "nickname", "shard", "byte"] {
            assert_eq!(row[field].as_str().unwrap().parse::<Zone>().unwrap(), zone);
        }
    }
    assert_ne!(
        "0x0".parse::<Shard>().unwrap(),
        "0x00".parse::<Shard>().unwrap()
    );
    assert!("0x0".parse::<Zone>().is_err());
    assert!("0x03".parse::<Shard>().is_err());
}
