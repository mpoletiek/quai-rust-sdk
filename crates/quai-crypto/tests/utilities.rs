//! Exported RIPEMD/text-hash primitives and OS-backed bounded entropy.
use quai_crypto::{CryptoError, MAX_RANDOM_BYTES, fill_random, keccak256, ripemd160};
use serde_json::Value;
#[test]
fn ripemd160_and_utf8_identifier_hashes_match_pinned_reference() {
    let data: Value = serde_json::from_str(include_str!(
        "fixtures/shared/compatibility/fixtures/text-crypto.json"
    ))
    .unwrap();
    for row in data["hashes"].as_array().unwrap() {
        let bytes = quai_primitives::get_bytes(row["hex"].as_str().unwrap()).unwrap();
        assert_eq!(
            quai_primitives::hexlify(&ripemd160(&bytes)).unwrap(),
            row["ripemd160"]
        );
    }
    for row in data["ids"].as_array().unwrap() {
        assert_eq!(
            quai_primitives::hexlify(&keccak256(row["text"].as_str().unwrap().as_bytes())).unwrap(),
            row["id"]
        );
    }
    assert_eq!(
        quai_primitives::hexlify(&ripemd160(b"abc")).unwrap(),
        "0x8eb208f7e05d987a9b044a8e98c6b087f15a0bfc"
    );
}
#[test]
fn bounded_entropy_supports_empty_and_maximum_requests_and_leaves_oversize_untouched() {
    fill_random(&mut []).unwrap();
    let mut first = [0; 32];
    let mut second = [0; 32];
    fill_random(&mut first).unwrap();
    fill_random(&mut second).unwrap();
    assert!(first != second); // Usability check, not statistical entropy certification.
    let mut maximum = vec![0; MAX_RANDOM_BYTES];
    fill_random(&mut maximum).unwrap();
    assert!(maximum.iter().any(|b| *b != 0));
    let mut oversized = vec![17; MAX_RANDOM_BYTES + 1];
    assert_eq!(
        fill_random(&mut oversized),
        Err(CryptoError::RandomRequestTooLarge)
    );
    assert!(oversized.iter().all(|b| *b == 17));
}
