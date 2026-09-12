//! Decimal boundary checks and exact-code deployment-address regressions.
use quai_primitives::{
    Address, AmountError, Hash32, SignedUnits, Unit, contract_address, create2_address,
    format_units, parse_signed_units, parse_units,
};
use ruint::aliases::{U256, U512};
use serde_json::Value;

#[test]
fn signed_decimal_units_match_reference_and_chain_amounts_reject_negatives() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../compatibility/fixtures/amounts.json")).unwrap();
    for vector in fixture["vectors"].as_array().unwrap() {
        let unit = Unit::new(vector["decimals"].as_u64().unwrap() as u8).unwrap();
        let actual = parse_signed_units(vector["input"].as_str().unwrap(), unit);
        if vector["expected"]["error"] == true {
            assert!(actual.is_err(), "{vector}");
            continue;
        }
        let actual = actual.unwrap();
        assert_eq!(
            actual.format(unit),
            vector["expected"]["formatted"].as_str().unwrap(),
            "{vector}"
        );
        assert_eq!(
            actual.format(Unit::BASE),
            vector["expected"]["integer"].as_str().unwrap()
        );
    }
    assert_eq!(parse_units("-0.001", Unit::QI), Err(AmountError::Negative));
    assert_eq!(parse_units("-0", Unit::QI).unwrap(), U256::ZERO);
    assert!(parse_units(&(U512::from(1) << 256usize).to_string(), Unit::BASE).is_err());
    assert_eq!(
        parse_units(&"0".repeat(513), Unit::BASE),
        Err(AmountError::InputTooLong)
    );
    assert!(Unit::new(81).is_err());
    assert!(Unit::named("QUAI").is_err());
    assert_eq!(Unit::named("quai").unwrap(), Unit::QUAI);
}

#[test]
fn amount_boundary_roundtrips_without_floating_point() {
    for decimals in 0..=80 {
        let unit = Unit::new(decimals).unwrap();
        for bit in 0..256 {
            let value = U256::from(1) << bit;
            assert_eq!(
                parse_units(&format_units(value, unit), unit).unwrap(),
                value
            );
        }
        assert_eq!(
            parse_units(&format_units(U256::MAX, unit), unit).unwrap(),
            U256::MAX
        );
    }
    let boundary = U512::from(1) << 511;
    assert!(SignedUnits::new(false, boundary).is_err());
    assert!(SignedUnits::new(true, boundary).is_ok());
    assert!(SignedUnits::new(true, boundary + U512::from(1)).is_err());
}

fn bytes(input: &str) -> Vec<u8> {
    input.as_bytes()[2..]
        .chunks_exact(2)
        .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn contract_predictions_preserve_exact_code_and_record_legacy_difference() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../compatibility/fixtures/amounts.json")).unwrap();
    let mut corrected = 0;
    for v in fixture["contracts"].as_array().unwrap() {
        let sender: Address = v["sender"].as_str().unwrap().parse().unwrap();
        let exact = contract_address(
            sender,
            v["nonce"].as_str().unwrap().parse().unwrap(),
            &bytes(v["code"].as_str().unwrap()),
        );
        assert_eq!(exact.to_string(), v["exact"].as_str().unwrap());
        if v["exact"] != v["legacy"] {
            corrected += 1;
        }
        let salt: Hash32 = v["salt"].as_str().unwrap().parse().unwrap();
        let hash: Hash32 = v["codeHash"].as_str().unwrap().parse().unwrap();
        assert_eq!(
            create2_address(sender, salt, hash).to_string(),
            v["create2"].as_str().unwrap()
        );
    }
    assert_eq!(corrected, 18);
}
