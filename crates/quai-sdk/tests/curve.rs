//! Shared native/worker curve adapter differential and algebraic regressions.
#![cfg(feature = "wallet")]
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
use quai_sdk::U256;
use quai_sdk::crypto::{CryptoError, PublicKey, SecretKey, curve::*};
use quai_sdk::primitives::{get_bytes, hexlify};
use serde_json::Value;
fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "fixtures/shared/compatibility/fixtures/curve.json"
    ))
    .unwrap()
}
fn bytes(v: &Value) -> [u8; 32] {
    get_bytes(v.as_str().unwrap()).unwrap().try_into().unwrap()
}
fn point(v: &Value) -> PublicKey {
    PublicKey::from_sec1_bytes(&get_bytes(v.as_str().unwrap()).unwrap()).unwrap()
}
fn scalar(n: U256) -> CurveScalar {
    CurveScalar::from_bytes(&n.to_be_bytes()).unwrap()
}
fn encoded(p: Result<PublicKey, CryptoError>) -> Value {
    p.map_or(Value::Null, |p| {
        Value::String(hexlify(&p.to_compressed()).unwrap())
    })
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn canonical_scalar_arithmetic_matches_all_reference_pairs() {
    for row in fixture()["scalars"].as_array().unwrap() {
        let a = CurveScalar::from_bytes(&bytes(&row["a"])).unwrap();
        let b = CurveScalar::from_bytes(&bytes(&row["b"])).unwrap();
        assert_eq!(a.add(&b).export_bytes().as_bytes(), &bytes(&row["add"]));
        assert_eq!(
            a.multiply(&b).export_bytes().as_bytes(),
            &bytes(&row["multiply"])
        );
        assert_eq!(a.negate().export_bytes().as_bytes(), &bytes(&row["negate"]));
        assert!(a.add(&a.negate()).is_zero());
    }
    for row in fixture()["reductions"].as_array().unwrap() {
        assert_eq!(
            CurveScalar::reduce_bytes(&bytes(&row["input"]))
                .export_bytes()
                .as_bytes(),
            &bytes(&row["output"])
        );
    }
    assert_eq!(
        CurveScalar::from_bytes(&CURVE_ORDER.to_be_bytes()).unwrap_err(),
        CryptoError::InvalidScalar
    );
    assert!(CurveScalar::from_bytes(&[255; 32]).is_err());
    let zero = scalar(U256::ZERO);
    assert!(zero.is_zero());
    assert!(SecretKey::from_bytes(zero.export_bytes().as_bytes()).is_err());
    assert_eq!(format!("{zero:?}"), "CurveScalar([REDACTED])");
    assert_eq!(
        format!("{:?}", zero.export_bytes()),
        "SecretBytes([REDACTED])"
    );
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn public_point_operations_match_reference_and_preserve_encoding() {
    for row in fixture()["points"].as_array().unwrap() {
        let p = point(&row["point"]);
        let other = point(&row["other"]);
        let s = CurveScalar::from_bytes(&bytes(&row["scalar"])).unwrap();
        assert_eq!(hexlify(&p.to_uncompressed()).unwrap(), row["uncompressed"]);
        assert_eq!(p.x_coordinate(), bytes(&row["x"]));
        assert_eq!(p.has_even_y(), row["even"].as_bool().unwrap());
        assert_eq!(hexlify(&p.negate().to_compressed()).unwrap(), row["negate"]);
        assert_eq!(encoded(p.multiply(&s)), row["multiply"]);
        assert_eq!(encoded(p.multiply_add(&s, &other)), row["multiplyAdd"]);
        assert_eq!(encoded(p.add_point(other)), row["add"]);
        assert_eq!(
            encoded(SecretKey::from_bytes(&bytes(&row["scalar"])).and_then(|s| p.add_tweak(&s))),
            row["tweak"]
        );
        assert_eq!(
            SecretKey::from_bytes(&bytes(&row["key"]))
                .unwrap()
                .public_key(),
            p
        );
        let lift = PublicKey::lift_x(&p.x_coordinate()).unwrap();
        assert!(lift.has_even_y());
        assert_eq!(lift, if p.has_even_y() { p } else { p.negate() });
    }
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn infinity_and_noncanonical_coordinates_are_rejected() {
    let one = scalar(U256::from(1));
    let zero = scalar(U256::ZERO);
    let p = SecretKey::from_bytes(one.export_bytes().as_bytes())
        .unwrap()
        .public_key();
    assert!(p.add_point(p.negate()).is_err());
    assert!(p.multiply(&zero).is_err());
    assert!(p.multiply_add(&one, &p.negate()).is_err());
    assert_eq!(p.multiply_add(&zero, &p.negate()).unwrap(), p.negate());
    assert_eq!(p.negate().negate(), p);
    assert!(PublicKey::lift_x(&FIELD_PRIME.to_be_bytes()).is_err());
    assert!(PublicKey::lift_x(&[255; 32]).is_err());
    assert!(PublicKey::lift_x(&[0; 32]).is_err());
    let mut bad = p.to_compressed();
    bad[0] = 6;
    assert!(PublicKey::from_sec1_bytes(&bad).is_err());
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn public_field_math_matches_reference_and_terminates_at_prime() {
    for row in fixture()["fields"].as_array().unwrap() {
        let x = U256::from_be_bytes(bytes(&row["x"]));
        assert_eq!(public_curve_rhs(x).to_be_bytes::<32>(), bytes(&row["rhs"]));
        assert_eq!(
            public_jacobi_symbol(x),
            row["symbol"].as_i64().unwrap() as i8
        );
    }
    assert_eq!(public_jacobi_symbol(FIELD_PRIME), 0);
    assert_eq!(public_curve_rhs(FIELD_PRIME), U256::from(7));
    assert_eq!(
        public_jacobi_symbol(U256::MAX),
        public_jacobi_symbol(U256::MAX - FIELD_PRIME)
    );
    for x in [U256::from(2), U256::from(805), U256::MAX] {
        assert_eq!(public_jacobi_symbol(x.mul_mod(x, FIELD_PRIME)), 1);
    }
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn multipart_and_tagged_hashes_match_exact_source_bytes() {
    for row in fixture()["hashes"].as_array().unwrap() {
        let owned: Vec<_> = row["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| get_bytes(p.as_str().unwrap()).unwrap())
            .collect();
        let parts: Vec<_> = owned.iter().map(Vec::as_slice).collect();
        assert_eq!(sha256_parts(&parts).unwrap(), bytes(&row["sha256"]));
        assert_eq!(
            quais_tagged_sha256(row["tag"].as_str().unwrap(), &parts).unwrap(),
            bytes(&row["tagged"])
        );
    }
    assert_ne!(
        quais_tagged_sha256("π", &[]).unwrap(),
        tagged_sha256("π", &[]).unwrap()
    );
    assert_eq!(
        quais_tagged_sha256("BIP0340/challenge", &[]).unwrap(),
        tagged_sha256("BIP0340/challenge", &[]).unwrap()
    );
    assert_eq!(
        sha256_parts(&[b"a", b"bc"]).unwrap(),
        quai_sdk::crypto::sha256(b"abc")
    );
    assert_ne!(
        tagged_sha256("a", &[b"bc"]).unwrap(),
        tagged_sha256("ab", &[b"c"]).unwrap()
    );
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn hash_limits_include_empty_parts_and_total_bytes() {
    assert!(sha256_parts(&vec![&[] as &[u8]; MAX_HASH_PARTS]).is_ok());
    assert_eq!(
        sha256_parts(&vec![&[] as &[u8]; MAX_HASH_PARTS + 1]),
        Err(CryptoError::HashLimit)
    );
    let buffer = vec![0; MAX_HASH_BYTES];
    assert!(sha256_parts(&[&buffer]).is_ok());
    assert_eq!(sha256_parts(&[&buffer, b"x"]), Err(CryptoError::HashLimit));
    assert!(tagged_sha256(&"x".repeat(1024), &[]).is_ok());
    assert_eq!(
        tagged_sha256(&"x".repeat(1025), &[]),
        Err(CryptoError::HashLimit)
    );
}
