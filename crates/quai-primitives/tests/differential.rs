//! Offline comparison with the pinned quais.js oracle's address operations.

use quai_primitives::{Address, Ledger, QiAddress, QuaiAddress, Zone};
use serde_json::Value;

#[test]
fn address_operations_match_pinned_javascript_oracle() {
    let fixture: Value = serde_json::from_str(include_str!(
        "fixtures/shared/compatibility/fixtures/primitives.json"
    ))
    .expect("valid compatibility fixture");
    assert_eq!(fixture["schemaVersion"], 1);
    assert_eq!(fixture["reference"], "quais@1.0.0-alpha.57");
    let vectors = fixture["vectors"].as_array().expect("vectors array");
    let mut accepted = 0;
    let mut rejected = 0;
    for vector in vectors.iter().filter(|v| v["operation"] == "address") {
        let id = vector["id"].as_str().expect("vector id");
        let input = vector["input"]["address"].as_str().expect("address input");
        let expected = &vector["expected"];
        let result = input.parse::<Address>();
        if expected.get("error").is_some() {
            assert_eq!(expected["error"]["code"], "INVALID_ARGUMENT", "{id}");
            assert!(result.is_err(), "oracle rejects {id}: {input}");
            assert!(input.parse::<QuaiAddress>().is_err(), "{id}");
            assert!(input.parse::<QiAddress>().is_err(), "{id}");
            rejected += 1;
            continue;
        }
        let address = result.unwrap_or_else(|error| panic!("oracle accepts {id}: {error}"));
        let checksum = expected["checksum"].as_str().expect("checksum output");
        assert_eq!(address.to_checksum(), checksum, "{id}");
        assert_eq!(checksum.parse::<Address>().unwrap(), address, "{id}");
        let ledger = match expected["ledger"].as_str().expect("ledger output") {
            "quai" => Ledger::Quai,
            "qi" => Ledger::Qi,
            other => panic!("unexpected oracle ledger {other}"),
        };
        assert_eq!(address.ledger(), ledger, "{id}");
        let zone = match &expected["zone"] {
            Value::Null => None,
            Value::String(value) => Some(value.parse::<Zone>().expect("oracle zone")),
            other => panic!("invalid oracle zone {other}"),
        };
        assert_eq!(address.zone().ok(), zone, "{id}");
        assert_eq!(
            input.parse::<QuaiAddress>().is_ok(),
            zone.is_some() && ledger == Ledger::Quai,
            "{id}"
        );
        assert_eq!(
            input.parse::<QiAddress>().is_ok(),
            zone.is_some() && ledger == Ledger::Qi,
            "{id}"
        );
        accepted += 1;
    }
    // Prevent a changed operation name or empty corpus from silently passing.
    assert!(accepted >= 24, "missing positive address cases");
    assert!(rejected >= 3, "missing negative address cases");
}
