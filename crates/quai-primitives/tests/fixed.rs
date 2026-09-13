//! Published-reference arithmetic, corrected rounding, and field boundaries.
use quai_primitives::*;
use ruint::aliases::{U256, U512};
use serde_json::Value;
fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "fixtures/shared/compatibility/fixtures/fixed.json"
    ))
    .unwrap()
}
fn value(text: &str, format: &str) -> FixedPoint {
    FixedPoint::parse(text, format.parse().unwrap()).unwrap()
}
fn check(actual: Result<FixedPoint, FixedError>, expected: &Value, context: &Value) {
    if expected["error"] == true {
        assert!(actual.is_err(), "{context}: {actual:?}");
    } else {
        let actual = actual.unwrap_or_else(|e| panic!("{context}: {e}"));
        // Compare exact numeric values independently of cosmetic decimal formatting.
        assert_eq!(
            actual,
            FixedPoint::parse(expected["value"].as_str().unwrap(), actual.format()).unwrap(),
            "{context}"
        );
    }
}
#[test]
fn fixed_point_matches_pinned_parse_arithmetic_and_byte_vectors() {
    let vectors = fixture();
    for v in vectors["parse"].as_array().unwrap() {
        check(
            FixedPoint::parse(
                v["input"].as_str().unwrap(),
                v["format"].as_str().unwrap().parse().unwrap(),
            ),
            &v["result"],
            v,
        );
    }
    for v in vectors["bytes"].as_array().unwrap() {
        let actual = FixedPoint::from_bytes(
            &get_bytes(v["input"].as_str().unwrap()).unwrap(),
            v["format"].as_str().unwrap().parse().unwrap(),
        );
        check(actual, &v["result"], v);
        if let Ok(actual) = actual {
            assert_eq!(
                FixedPoint::from_bytes(&actual.to_bytes(), actual.format()).unwrap(),
                actual
            );
        }
    }
    for v in vectors["arithmetic"].as_array().unwrap() {
        let a = value(v["a"].as_str().unwrap(), v["format"].as_str().unwrap());
        let b = value(v["b"].as_str().unwrap(), v["format"].as_str().unwrap());
        let actual = match v["operation"].as_str().unwrap() {
            "add" => a.checked_add(b),
            "sub" => a.checked_sub(b),
            "mul" => a.checked_mul(b, Rounding::TowardZero),
            "div" => a.checked_div(b, Rounding::TowardZero),
            "mulSignal" => a.checked_mul(b, Rounding::Exact),
            "divSignal" => a.checked_div(b, Rounding::Exact),
            _ => unreachable!(),
        };
        check(actual, &v["result"], v);
    }
}
#[test]
fn mathematical_rounding_corrects_recorded_reference_defects() {
    let vectors = fixture();
    let mut differences = [0; 3];
    for v in vectors["rounding"].as_array().unwrap() {
        let input = value(v["input"].as_str().unwrap(), "fixed16x2");
        for (index, actual, correct, reference) in [
            (0, input.floor().unwrap(), "correctFloor", "referenceFloor"),
            (
                1,
                input.ceiling().unwrap(),
                "correctCeiling",
                "referenceCeiling",
            ),
            (
                2,
                input.round(0, Rounding::NearestTiesPositive).unwrap(),
                "correctRound",
                "referenceRound",
            ),
        ] {
            assert_eq!(
                actual,
                value(v[correct].as_str().unwrap(), "fixed16x2"),
                "{v}"
            );
            if actual != value(v[reference].as_str().unwrap(), "fixed16x2") {
                differences[index] += 1;
            }
        }
    }
    assert!(differences.iter().all(|n| *n > 0));
    for (input, expected) in [("2.5", "2"), ("3.5", "4"), ("-2.5", "-2"), ("-3.5", "-4")] {
        assert_eq!(
            value(input, "fixed16x2")
                .round(0, Rounding::NearestTiesEven)
                .unwrap(),
            value(expected, "fixed16x2")
        );
    }
    assert_eq!(
        value("-1.01", "fixed16x2").round(0, Rounding::Exact),
        Err(FixedError::PrecisionLoss)
    );
    assert_eq!(
        value("-327.68", "fixed16x2").floor(),
        Err(FixedError::Overflow)
    );
    assert_eq!(
        value("327.67", "fixed16x2").ceiling(),
        Err(FixedError::Overflow)
    );
}
#[test]
fn formats_rescaling_order_and_chain_units_preserve_exact_values() {
    for invalid in [
        "fixed0x0",
        "fixed7x0",
        "fixed264x0",
        "fixed8x81",
        "fixed8x-1",
        "fixed+8x0",
        "fixed8",
        "Fixed8x0",
        "fixed8x0x0",
    ] {
        assert!(invalid.parse::<FixedFormat>().is_err());
    }
    assert_eq!(
        "fixed".parse::<FixedFormat>().unwrap(),
        FixedFormat::default()
    );
    assert_eq!(
        "ufixed".parse::<FixedFormat>().unwrap().to_string(),
        "ufixed128x18"
    );
    let a = value("1.25", "fixed16x2");
    assert_eq!(a, value("1.2500", "ufixed32x4"));
    assert!(a > value("1.2499", "fixed32x4"));
    assert!(value("-1.25", "fixed16x2") < value("-1.2499", "fixed32x4"));
    assert_eq!(
        a.checked_add(value("1.25", "fixed32x4")),
        Err(FixedError::FormatMismatch)
    );
    assert_eq!(
        a.with_format("fixed16x1".parse().unwrap(), Rounding::Exact),
        Err(FixedError::PrecisionLoss)
    );
    assert_eq!(
        a.with_format("fixed16x1".parse().unwrap(), Rounding::NearestTiesEven)
            .unwrap(),
        value("1.2", "fixed16x1")
    );
    assert_eq!(
        a.to_chain_units(Unit::QUAI).unwrap(),
        U256::from(1_250_000_000_000_000_000u64)
    );
    assert!(value("-1", "fixed16x2").to_chain_units(Unit::QUAI).is_err());
    let units = SignedUnits::new(false, U512::from(12500)).unwrap();
    assert_eq!(
        FixedPoint::from_scaled_units(units, Unit::new(4).unwrap(), a.format()).unwrap(),
        a
    );
    assert_eq!(
        value("-128", "fixed8x0")
            .checked_sub(value("-128", "fixed8x0"))
            .unwrap(),
        value("0", "fixed8x0")
    );
    assert!(!value("-0", "fixed8x0").is_negative());
}
#[test]
fn wide_intermediates_handle_full_fields_and_extreme_scales() {
    let format = FixedFormat::new(false, 256, 80).unwrap();
    let max = FixedPoint::from_bytes(&[255; 32], format).unwrap();
    assert_eq!(max.to_chain_units(format.unit()).unwrap(), U256::MAX);
    assert_eq!(FixedPoint::parse(&max.to_string(), format).unwrap(), max);
    assert_eq!(
        max.checked_div(max, Rounding::Exact),
        Err(FixedError::Overflow)
    ); // 1.0 cannot fit 256 bits at scale 80.
    let plain =
        FixedPoint::from_bytes(&[255; 32], FixedFormat::new(false, 256, 0).unwrap()).unwrap();
    assert!(plain > max);
    assert_eq!(
        plain.with_format(format, Rounding::Exact),
        Err(FixedError::Overflow)
    );
    let a = value("1000000000000000000000000000000", "ufixed256x18");
    let b = value("0.000000000000000001", "ufixed256x18");
    assert_eq!(
        a.checked_div(b, Rounding::Exact).unwrap().to_string(),
        "1000000000000000000000000000000000000000000000000.0"
    );
    assert!(max.checked_mul(max, Rounding::TowardZero).is_ok()); // 512-bit intermediate before division by 10^80.
    let signed_min = FixedPoint::from_bytes(
        &[0x80].into_iter().chain([0; 31]).collect::<Vec<_>>(),
        FixedFormat::new(true, 256, 0).unwrap(),
    )
    .unwrap();
    assert_eq!(
        signed_min.checked_div(value("-1", "fixed256x0"), Rounding::Exact),
        Err(FixedError::Overflow)
    );
    assert_eq!(signed_min.to_bytes()[0], 0x80);
}
