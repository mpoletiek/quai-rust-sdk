//! Published factory payload parity and hostile artifact parsing regressions.
use quai_abi::{AbiError, MAX_DATA_BYTES, SolidityArtifact};
use quai_primitives::get_bytes;
use serde_json::{Value, json};
fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../compatibility/fixtures/artifacts.json"
    ))
    .unwrap()
}
#[test]
fn compiler_artifacts_preserve_exact_creation_and_constructor_bytes() {
    for v in fixture()["vectors"].as_array().unwrap() {
        let parsed = SolidityArtifact::from_json(&serde_json::to_vec(&v["output"]).unwrap());
        if v["strictReject"] == true {
            assert!(parsed.is_err(), "{v}");
            continue;
        }
        let encoded = parsed.and_then(|a| a.init_data(v["args"].as_array().unwrap()));
        if v["reference"]["error"] == true {
            assert!(encoded.is_err(), "{v}");
        } else {
            assert_eq!(
                encoded.unwrap(),
                get_bytes(v["reference"]["data"].as_str().unwrap()).unwrap(),
                "{v}"
            );
        }
    }
    let artifact = SolidityArtifact::from_json(br#"{"abi":[],"bytecode":"00006000"}"#).unwrap();
    assert_eq!(artifact.init_code(), &[0, 0, 0x60, 0]);
    assert!(artifact.interface().constructor().is_none());
    assert!(!format!("{artifact:?}").contains("00006000"));
    assert_eq!(artifact.into_parts().1, [0, 0, 0x60, 0]);
}
#[test]
fn compiler_output_requires_explicit_source_and_contract_selection() {
    let output = json!({"contracts":{"A.sol":{"A":{"abi":[],"evm":{"bytecode":{"object":"6000"}}},"B":{"abi":[],"bytecode":"6001"}}},"sources":{"A.sol":{"id":0}}});
    let bytes = serde_json::to_vec(&output).unwrap();
    assert_eq!(
        SolidityArtifact::from_compilation_json(&bytes, "A.sol", "B")
            .unwrap()
            .init_code(),
        [0x60, 1]
    );
    assert!(matches!(
        SolidityArtifact::from_compilation_json(&bytes, "B.sol", "A"),
        Err(AbiError::NotFound)
    ));
    assert!(SolidityArtifact::from_compilation_json(&bytes, "", "A").is_err());
    assert!(SolidityArtifact::from_json(&bytes).is_err());
}
#[test]
fn duplicate_fields_resource_bounds_and_constructor_arity_fail_closed() {
    for bytes in [
        br#"{"abi":[],"bytecode":"6000","bytecode":"6001"}"#.as_slice(),
        br#"{"abi":[],"evm":{"bytecode":{"object":"6000","object":"6001"}}}"#,
        br#"{"abi":[],"bytecode":"6000","ignored":{"a":1,"a":2}}"#,
        br#"{"abi":[],"bytecode":"0X6000"}"#,
        br#"{"abi":[],"bytecode":"600"}"#,
    ] {
        assert!(SolidityArtifact::from_json(bytes).is_err());
    }
    let mut oversized = vec![b' '; MAX_DATA_BYTES + 1];
    oversized[..2].copy_from_slice(b"{}");
    assert!(matches!(
        SolidityArtifact::from_json(&oversized),
        Err(AbiError::Limit)
    ));
    let artifact = SolidityArtifact::from_json(br#"{"abi":[],"bytecode":"6000"}"#).unwrap();
    assert!(artifact.init_data(&[json!(1)]).is_err());
    let a=SolidityArtifact::from_json(br#"{"abi":[{"type":"constructor","stateMutability":"nonpayable","inputs":[{"name":"a","type":"bytes"}]}],"bytecode":"6000"}"#).unwrap();
    assert!(
        a.init_data(&[json!(format!("0x{}", "00".repeat(MAX_DATA_BYTES / 2)))])
            .is_err()
    );
}
