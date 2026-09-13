//! Independent readable declaration, formatting and hostile-input checks.
use quai_abi::{AbiError, AbiInterface, MAX_SCHEMA_BYTES};
use serde_json::json;
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn pinned_readable_fragments_format_and_roundtrip_with_exact_selectors() {
    let fixture: serde_json::Value =
        serde_json::from_slice(include_bytes!("fixtures/human-abi.json")).unwrap();
    for row in fixture["vectors"].as_array().unwrap() {
        let text = row["text"].as_str().unwrap();
        let iface =
            AbiInterface::from_human_readable(&[text]).unwrap_or_else(|e| panic!("{text}: {e}"));
        for minimal in [false, true] {
            let expected = row[if minimal { "minimal" } else { "full" }]
                .as_str()
                .unwrap();
            assert_eq!(
                iface.format_human_readable(minimal).unwrap(),
                [expected],
                "{text}"
            );
            let round = AbiInterface::from_human_readable(&[expected]).unwrap();
            assert_eq!(
                round.format_human_readable(true).unwrap(),
                iface.format_human_readable(true).unwrap()
            );
        }
        let reference =
            AbiInterface::from_json(&serde_json::to_vec(&json!([row["abi"]])).unwrap()).unwrap();
        assert_eq!(
            reference.format_human_readable(false).unwrap(),
            iface.format_human_readable(false).unwrap(),
            "JSON {text}"
        );
        let exported = iface.format_json().unwrap();
        let round = AbiInterface::from_json(exported.as_bytes()).unwrap();
        assert_eq!(round.format_json().unwrap(), exported);
        assert_eq!(
            round.format_human_readable(false).unwrap(),
            iface.format_human_readable(false).unwrap()
        );
        if let Some(signature) = row["signature"].as_str() {
            let actual = match row["abi"]["type"].as_str().unwrap() {
                "function" => format!(
                    "0x{}",
                    iface
                        .function(signature)
                        .unwrap()
                        .selector()
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>()
                ),
                "error" => format!(
                    "0x{}",
                    iface
                        .error(signature)
                        .unwrap()
                        .selector()
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>()
                ),
                "event" => iface.event(signature).unwrap().topic_hash().to_string(),
                _ => unreachable!(),
            };
            assert_eq!(actual, row["selector"]);
        }
    }
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn malformed_or_unsupported_fragments_fail_as_a_whole_with_bounded_work() {
    let fixture: serde_json::Value =
        serde_json::from_slice(include_bytes!("fixtures/human-abi.json")).unwrap();
    for row in fixture["reject"].as_array().unwrap() {
        let text = row.as_str().unwrap();
        assert!(
            AbiInterface::from_human_readable(&["function valid()", text]).is_err(),
            "{text}"
        );
    }
    assert_eq!(
        AbiInterface::from_human_readable(&[&" ".repeat(4097)]).unwrap_err(),
        AbiError::Limit
    );
    assert_eq!(
        AbiInterface::from_human_readable(&vec!["function a()"; 1025]).unwrap_err(),
        AbiError::Limit
    );
    assert_eq!(
        AbiInterface::from_human_readable(
            &[" ".repeat(4096).as_str(); MAX_SCHEMA_BYTES / 4096 + 1]
        )
        .unwrap_err(),
        AbiError::Limit
    );
    let deep = format!("function a({}uint{} value)", "(".repeat(65), ")".repeat(65));
    assert_eq!(
        AbiInterface::from_human_readable(&[&deep]).unwrap_err(),
        AbiError::Limit
    );
    let broad = format!("function a({})", vec!["()"; 1025].join(","));
    assert_eq!(
        AbiInterface::from_human_readable(&[&broad]).unwrap_err(),
        AbiError::Limit
    );
    assert_eq!(
        AbiInterface::from_human_readable(&["function a(uint)", "function a(uint256)"])
            .unwrap_err(),
        AbiError::Ambiguous
    );
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn metadata_export_retains_order_names_internal_types_and_overloads() {
    let abi = json!([
        {"type":"event","name":"Z","inputs":[{"type":"tuple[]","name":"rows","indexed":true,"components":[{"type":"uint","name":"value","internalType":"uint256"}]}]},
        {"type":"function","name":"f","constant":true,"inputs":[{"type":"address","name":"who","internalType":"contract Receiver"}],"outputs":[{"type":"uint","name":"balance"}]},
        {"type":"function","name":"f","stateMutability":"nonpayable","inputs":[],"outputs":[]}
    ]);
    let iface = AbiInterface::from_json(&serde_json::to_vec(&abi).unwrap()).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&iface.format_json().unwrap()).unwrap(),
        abi
    );
    assert_eq!(
        iface.format_human_readable(false).unwrap(),
        [
            "event Z((uint256 value)[] indexed rows)",
            "function f(address who) view returns (uint256 balance)",
            "function f()"
        ]
    );
    assert_eq!(iface.function("f").unwrap_err(), AbiError::Ambiguous);
    let bare = AbiInterface::from_human_readable(&[
        "f(address)",
        "function g(address payable[] recipients)",
    ])
    .unwrap();
    assert_eq!(
        bare.format_human_readable(true).unwrap(),
        ["function f(address)", "function g(address[])"]
    );
    assert_eq!(AbiInterface::default().format_json().unwrap(), "[]");
}
