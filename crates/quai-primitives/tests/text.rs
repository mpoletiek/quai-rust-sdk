//! Explicit Unicode normalization and immutable UUID formatting reference parity.
use quai_primitives::{
    EncodingError, MAX_ENCODING_BYTES, UTF8_UNICODE_VERSION, Utf8Normalization, to_utf8_bytes,
    to_utf8_code_points, to_utf8_string, uuid_v4,
};
use serde_json::Value;
fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "fixtures/shared/compatibility/fixtures/text-crypto.json"
    ))
    .unwrap()
}
fn form(value: &Value) -> Option<Utf8Normalization> {
    match value.as_str() {
        None => None,
        Some("NFC") => Some(Utf8Normalization::Nfc),
        Some("NFD") => Some(Utf8Normalization::Nfd),
        Some("NFKC") => Some(Utf8Normalization::Nfkc),
        Some("NFKD") => Some(Utf8Normalization::Nfkd),
        _ => panic!("fixture form"),
    }
}
#[test]
fn normalization_bytes_and_scalar_values_match_pinned_reference() {
    let data = fixture();
    for row in data["texts"].as_array().unwrap() {
        let text = row["text"].as_str().unwrap();
        let form = form(&row["form"]);
        let encoded = to_utf8_bytes(text, form).unwrap();
        assert_eq!(
            quai_primitives::hexlify(&encoded).unwrap(),
            row["bytes"],
            "{row}"
        );
        let expected: Vec<u32> = row["points"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u32)
            .collect();
        assert_eq!(to_utf8_code_points(text, form).unwrap(), expected, "{row}");
        assert_eq!(
            to_utf8_bytes(to_utf8_string(&encoded).unwrap(), form).unwrap(),
            encoded
        );
    }
    assert_eq!(UTF8_UNICODE_VERSION, (17, 0, 0));
}
#[test]
fn strict_decoding_rejects_invalid_scalars_overlongs_and_truncation() {
    let data = fixture();
    for row in data["decoding"].as_array().unwrap() {
        let bytes = quai_primitives::get_bytes(row["hex"].as_str().unwrap()).unwrap();
        if row["error"] == true {
            assert_eq!(to_utf8_string(&bytes), Err(EncodingError::Invalid));
        } else {
            assert_eq!(
                to_utf8_string(&bytes).unwrap(),
                row["text"].as_str().unwrap()
            );
        }
    }
    let mut unpaired = 0;
    for row in data["surrogates"].as_array().unwrap() {
        let units: Vec<u16> = row["units"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u16)
            .collect();
        match String::from_utf16(&units) {
            Ok(text) => assert_eq!(
                quai_primitives::hexlify(&to_utf8_bytes(&text, None).unwrap()).unwrap(),
                row["bytes"]
            ),
            Err(_) => {
                unpaired += 1;
                if let Some(hex) = row["bytes"].as_str() {
                    assert!(to_utf8_string(&quai_primitives::get_bytes(hex).unwrap()).is_err());
                }
            }
        }
    }
    assert_eq!(unpaired, 4);
}
#[test]
fn unicode_input_and_expanded_output_limits_apply_without_implicit_normalization() {
    assert_eq!(to_utf8_bytes("e\u{301}", None).unwrap(), b"e\xcc\x81");
    assert_eq!(
        to_utf8_bytes("e\u{301}", Some(Utf8Normalization::Nfc)).unwrap(),
        b"\xc3\xa9"
    );
    let exact = "a".repeat(MAX_ENCODING_BYTES);
    assert_eq!(
        to_utf8_bytes(&exact, None).unwrap().len(),
        MAX_ENCODING_BYTES
    );
    assert_eq!(
        to_utf8_code_points(&exact, None).unwrap().len(),
        MAX_ENCODING_BYTES
    );
    assert_eq!(
        to_utf8_bytes(&(exact + "a"), None),
        Err(EncodingError::TooLarge)
    );
    assert_eq!(
        to_utf8_string(&vec![0; MAX_ENCODING_BYTES + 1]),
        Err(EncodingError::TooLarge)
    );
    let expansion = "\u{fdfa}".repeat(MAX_ENCODING_BYTES / 3);
    assert!(expansion.len() <= MAX_ENCODING_BYTES);
    assert_eq!(
        to_utf8_bytes(&expansion, Some(Utf8Normalization::Nfkd)),
        Err(EncodingError::TooLarge)
    );
    assert_eq!(
        to_utf8_code_points(&expansion, Some(Utf8Normalization::Nfkd)),
        Err(EncodingError::TooLarge)
    );
}
#[test]
fn uuid_sets_version_and_variant_without_mutating_callers_entropy() {
    for row in fixture()["uuids"].as_array().unwrap() {
        let bytes: [u8; 16] = quai_primitives::get_bytes(row["input"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        let before = bytes;
        let uuid = uuid_v4(&bytes);
        assert_eq!(uuid, row["uuid"]);
        assert_eq!(bytes, before);
        assert_eq!(&uuid[14..15], "4");
        assert!(["8", "9", "a", "b"].contains(&&uuid[19..20]));
    }
}
