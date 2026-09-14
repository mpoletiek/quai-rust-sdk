//! Explicit modular fixed-point arithmetic and lossy presentation conversion.
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
use quai_sdk::primitives::{FixedError, FixedFormat, FixedPoint, Rounding, Unit};
use serde_json::Value;
fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "fixtures/shared/compatibility/fixtures/fixed-completion.json"
    ))
    .unwrap()
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn modular_operations_match_reference_or_explicit_signed_minimum_corrections() {
    let f = fixture();
    let mut differences = 0;
    let mut operations = 0;
    for row in f["vectors"].as_array().unwrap() {
        let format: FixedFormat = row["format"].as_str().unwrap().parse().unwrap();
        let a = FixedPoint::parse(row["a"].as_str().unwrap(), format).unwrap();
        let b = FixedPoint::parse(row["b"].as_str().unwrap(), format).unwrap();
        for (name, result) in [
            ("add", a.wrapping_add(b)),
            ("sub", a.wrapping_sub(b)),
            ("mul", a.wrapping_mul(b)),
            ("div", a.wrapping_div(b)),
        ] {
            let e = &row["outputs"][name];
            operations += 1;
            if e["correct"]["error"] == true {
                assert_eq!(result, Err(FixedError::DivisionByZero));
                continue;
            }
            let value = result.unwrap();
            let units = value.units().format(Unit::BASE);
            assert_eq!(
                units, e["correct"]["units"],
                "{name}: {} / {} in {}",
                row["a"], row["b"], row["format"]
            );
            assert_eq!(
                FixedPoint::from_bytes(&value.to_bytes(), format).unwrap(),
                value
            );
            if e["source"]["units"] != e["correct"]["units"] {
                differences += 1;
            } else {
                assert_eq!(value.to_string(), e["source"]["display"]);
            }
        }
    }
    assert_eq!(operations, 632);
    assert_eq!(differences, 26);
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn all_eight_bit_pairs_match_independent_machine_integer_modular_arithmetic() {
    for signed in [false, true] {
        let format = FixedFormat::new(signed, 8, 0).unwrap();
        let values: Vec<_> = (0..=255u8)
            .map(|n| FixedPoint::from_bytes(&[n], format).unwrap())
            .collect();
        for a in 0..=255u8 {
            for b in 0..=255u8 {
                let x = values[usize::from(a)];
                let y = values[usize::from(b)];
                assert_eq!(x.wrapping_add(y).unwrap().to_bytes(), [a.wrapping_add(b)]);
                assert_eq!(x.wrapping_sub(y).unwrap().to_bytes(), [a.wrapping_sub(b)]);
                assert_eq!(x.wrapping_mul(y).unwrap().to_bytes(), [a.wrapping_mul(b)]);
                if b != 0 {
                    let quotient = if signed {
                        (i16::from(a as i8) / i16::from(b as i8)) as u8
                    } else {
                        a / b
                    };
                    assert_eq!(x.wrapping_div(y).unwrap().to_bytes(), [quotient]);
                }
            }
        }
    }
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn wrapping_is_explicit_and_signed_minimum_remains_in_range() {
    let f = FixedFormat::new(true, 8, 0).unwrap();
    let min = FixedPoint::parse("-128", f).unwrap();
    let neg = FixedPoint::parse("-1", f).unwrap();
    assert_eq!(min.wrapping_div(neg).unwrap(), min);
    assert_eq!(
        min.checked_div(neg, Rounding::TowardZero),
        Err(FixedError::Overflow)
    );
    assert_eq!(
        FixedPoint::parse("-127", f)
            .unwrap()
            .wrapping_add(neg)
            .unwrap(),
        min
    );
    assert!(min.is_negative());
    assert_eq!(min.to_bytes(), [128]);
    let unsigned = FixedFormat::new(false, 8, 0).unwrap();
    let max = FixedPoint::parse("255", unsigned).unwrap();
    let one = FixedPoint::parse("1", unsigned).unwrap();
    assert!(max.wrapping_add(one).unwrap().is_zero());
    assert_eq!(max.checked_add(one), Err(FixedError::Overflow));
    let small = FixedPoint::parse("0.01", FixedFormat::new(true, 16, 2).unwrap()).unwrap();
    assert!(small.wrapping_mul(small).unwrap().is_zero());
    assert_eq!(
        small.checked_mul(small, Rounding::Exact),
        Err(FixedError::PrecisionLoss)
    );
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn format_mismatch_and_zero_division_never_wrap_into_valid_values() {
    let f = FixedFormat::new(true, 8, 0).unwrap();
    let a = FixedPoint::parse("1", f).unwrap();
    let b = FixedPoint::parse("1", FixedFormat::new(false, 8, 0).unwrap()).unwrap();
    for result in [
        a.wrapping_add(b),
        a.wrapping_sub(b),
        a.wrapping_mul(b),
        a.wrapping_div(b),
    ] {
        assert_eq!(result, Err(FixedError::FormatMismatch));
    }
    assert_eq!(
        a.wrapping_div(FixedPoint::parse("0", f).unwrap()),
        Err(FixedError::DivisionByZero)
    );
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn lossy_float_conversion_matches_ieee_rounding_and_never_mutates_exact_units() {
    for row in fixture()["vectors"].as_array().unwrap() {
        let a = FixedPoint::parse(
            row["a"].as_str().unwrap(),
            row["format"].as_str().unwrap().parse().unwrap(),
        )
        .unwrap();
        let before = a.units();
        let float = a.to_f64_lossy();
        assert!(float.is_finite());
        assert_eq!(format!("{:016x}", float.to_bits()), row["floatBits"]);
        assert_eq!(a.units(), before);
    }
    let a = FixedPoint::parse("9007199254740993", FixedFormat::new(true, 128, 0).unwrap()).unwrap();
    assert_eq!(a.to_f64_lossy(), 9007199254740992.0);
    assert_eq!(a.to_string(), "9007199254740993");
}
