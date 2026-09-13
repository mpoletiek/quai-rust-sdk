//! Differential packed byte layout, strict values and ambiguous dynamic fields.
use quai_abi::{
    AbiError, AbiType, MAX_DATA_BYTES, solidity_packed, solidity_packed_keccak256,
    solidity_packed_sha256,
};
use serde_json::{Value, json};
fn types(values: &[&str]) -> Vec<AbiType> {
    values.iter().map(|s| s.parse().unwrap()).collect()
}
#[test]
fn all_pinned_packed_encodings_and_hashes_match_or_reject_documented_coercions() {
    let data: Value = serde_json::from_str(include_str!(
        "fixtures/shared/compatibility/fixtures/packed.json"
    ))
    .unwrap();
    let mut matched = 0;
    let mut stricter = 0;
    for row in data["vectors"].as_array().unwrap() {
        let parsed: Result<Vec<AbiType>, _> = row["types"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().parse())
            .collect();
        let values = row["values"].as_array().unwrap();
        let result = parsed
            .as_ref()
            .map_err(|_| AbiError::Schema)
            .and_then(|types| solidity_packed(types, values));
        if row["rust"] == "reject" || row["error"] == true {
            assert!(result.is_err(), "unexpected acceptance: {row}");
            if row["error"] != true {
                stricter += 1;
            }
        } else {
            let types = parsed.unwrap();
            assert_eq!(
                quai_primitives::hexlify(&result.unwrap()).unwrap(),
                row["packed"],
                "{row}"
            );
            assert_eq!(
                quai_primitives::hexlify(&solidity_packed_keccak256(&types, values).unwrap())
                    .unwrap(),
                row["keccak256"],
                "{row}"
            );
            assert_eq!(
                quai_primitives::hexlify(&solidity_packed_sha256(&types, values).unwrap()).unwrap(),
                row["sha256"],
                "{row}"
            );
            matched += 1;
        }
    }
    assert!(matched > 300 && stricter > 50);
}
#[test]
fn declared_array_integer_widths_sign_extension_and_bytes_padding_are_exact() {
    let values = [
        json!(["-128", "127"]),
        json!(["0x0102", "0x0000"]),
        json!([true, false]),
    ];
    let bytes = solidity_packed(&types(&["int8[]", "bytes2[2]", "bool[]"]), &values).unwrap();
    assert_eq!(bytes.len(), 192);
    assert_eq!(&bytes[..31], &[255; 31]);
    assert_eq!(bytes[31], 128);
    assert_eq!(&bytes[32..63], &[0; 31]);
    assert_eq!(bytes[63], 127);
    assert_eq!(&bytes[64..66], &[1, 2]);
    assert_eq!(&bytes[66..96], &[0; 30]);
    assert_eq!(bytes[159], 1);
    assert_eq!(bytes[191], 0);
    for (ty, v) in [
        ("uint8[]", json!(["256"])),
        ("int8[]", json!(["-129"])),
        ("bool[]", json!([1])),
    ] {
        assert!(solidity_packed(&types(&[ty]), &[v]).is_err());
    }
}
#[test]
fn packed_values_keep_global_resource_limits_and_reject_tuples_even_in_empty_arrays() {
    for ty in ["()", "(uint8)[]", "(uint8)[0]", "((bool))[][]"] {
        assert!(solidity_packed(&types(&[ty]), &[json!([])]).is_err());
    }
    assert_eq!(
        solidity_packed(
            &vec!["bool".parse().unwrap(); 1025],
            &vec![json!(true); 1025]
        ),
        Err(AbiError::Limit)
    );
    assert!(solidity_packed(&types(&["uint8[]"]), &[json!(vec!["1"; 32768])]).is_err());
    assert!(
        solidity_packed(
            &types(&["string", "string"]),
            &[
                json!("a".repeat(MAX_DATA_BYTES / 2 + 1)),
                json!("b".repeat(MAX_DATA_BYTES / 2 + 1))
            ]
        )
        .is_err()
    );
    let max = "a".repeat(MAX_DATA_BYTES - 64);
    assert_eq!(
        solidity_packed(&types(&["string"]), &[json!(max)])
            .unwrap()
            .len(),
        MAX_DATA_BYTES - 64
    );
    assert!(solidity_packed(&types(&["bytes"]), &[json!("0x0")]).is_err());
}
#[test]
fn packed_dynamic_fields_are_intentionally_ambiguous_and_reference_extensions_are_explicit() {
    let ty = types(&["string", "string"]);
    assert_eq!(
        solidity_packed(&ty, &[json!("a"), json!("bc")]).unwrap(),
        solidity_packed(&ty, &[json!("ab"), json!("c")]).unwrap()
    );
    assert_eq!(
        solidity_packed(
            &types(&["string[]", "bytes[]"]),
            &[json!(["a", "bc"]), json!(["0x00", "0x01"])]
        )
        .unwrap(),
        b"abc\0\x01"
    );
    let nested = solidity_packed(&types(&["uint8[][]"]), &[json!([["1"], [], ["2"]])]).unwrap();
    assert_eq!(nested.len(), 64);
    assert_eq!(nested[31], 1);
    assert_eq!(nested[63], 2);
}
