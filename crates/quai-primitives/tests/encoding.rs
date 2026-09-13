//! Published-reference and hostile-input encoding regressions.
use quai_primitives::*;
use ruint::aliases::U256;
use serde_json::Value;

#[test]
fn encodings_match_published_reference_vectors_including_zeroes_and_signed_boundaries() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../compatibility/fixtures/encoding.json"
    ))
    .unwrap();
    for vector in fixture["bytes"].as_array().unwrap() {
        let bytes = get_bytes(vector["input"].as_str().unwrap()).unwrap();
        assert_eq!(hexlify(&bytes).unwrap(), vector["hex"]);
        assert_eq!(encode_base64(&bytes).unwrap(), vector["base64"]);
        assert_eq!(
            decode_base64(vector["base64"].as_str().unwrap()).unwrap(),
            bytes
        );
        if let Some(encoded) = vector["base58"]["value"].as_str() {
            assert_eq!(encode_base58(&bytes).unwrap(), encoded);
            assert_eq!(decode_base58_bytes(encoded).unwrap(), bytes);
            if bytes.len() <= 32 {
                assert_eq!(
                    decode_base58(encoded).unwrap().to_string(),
                    vector["base58Integer"]["value"]
                );
            }
        }
        assert_eq!(
            hexlify(strip_zeros_left(&bytes)).unwrap(),
            vector["stripped"]
        );
        assert_eq!(
            hexlify(&zero_pad_value(&bytes, bytes.len().max(8)).unwrap()).unwrap(),
            vector["left"]
        );
        assert_eq!(
            hexlify(&zero_pad_bytes(&bytes, bytes.len().max(8)).unwrap()).unwrap(),
            vector["right"]
        );
    }
    for vector in fixture["strings"].as_array().unwrap() {
        let actual = encode_bytes32(vector["input"].as_str().unwrap());
        if vector["encoded"]["error"] == true {
            assert!(actual.is_err());
        } else {
            let bytes = actual.unwrap();
            assert_eq!(hexlify(&bytes).unwrap(), vector["encoded"]["value"]);
            assert_eq!(decode_bytes32(&bytes).unwrap(), vector["decoded"]["value"]);
        }
    }
    for vector in fixture["twos"].as_array().unwrap() {
        let value = parse_signed_units(vector["input"].as_str().unwrap(), Unit::BASE).unwrap();
        let bits = vector["bits"].as_u64().unwrap() as u16;
        let actual = to_twos(value, bits);
        if vector["encoded"]["error"] == true {
            assert!(actual.is_err(), "{vector}");
        } else {
            let actual = actual.unwrap();
            assert_eq!(actual.to_string(), vector["encoded"]["value"]);
            assert_eq!(
                from_twos(actual, bits).unwrap().format(Unit::BASE),
                vector["decoded"]["value"]
            );
        }
    }
    for vector in fixture["masks"].as_array().unwrap() {
        assert_eq!(
            mask(
                vector["input"].as_str().unwrap().parse().unwrap(),
                vector["bits"].as_u64().unwrap() as u16
            )
            .unwrap()
            .to_string(),
            vector["expected"]
        );
    }
}

#[test]
fn invalid_encodings_resource_bounds_and_strict_decoding_are_explicit() {
    for text in ["0X00", "0x0", "00", "0xzz", "0xé0"] {
        assert!(get_bytes(text).is_err());
    }
    for text in ["Zg", "Zg=", "Zg===", "Zh==", "Zg==\n", "____"] {
        assert!(decode_base64(text).is_err());
    }
    assert!(get_bytes(&format!("0x{}", "00".repeat(MAX_ENCODING_BYTES + 1))).is_err());
    assert!(encode_base58(&vec![0; MAX_BASE58_BYTES + 1]).is_err());
    assert!(decode_base58_bytes(&"1".repeat(MAX_BASE58_BYTES + 1)).is_err());
    assert!(decode_base58_bytes("0OIl").is_err());
    assert!(decode_base58(&encode_base58(&[0xff; 33]).unwrap()).is_err());
    assert_eq!(decode_base58("111").unwrap(), U256::ZERO);
    assert!(zero_pad_bytes(&[1, 2], 1).is_err());
    assert!(zero_pad_value(&[], MAX_ENCODING_BYTES + 1).is_err());
    assert_eq!(concat_bytes(&[&[0, 1], &[], &[2]]).unwrap(), [0, 1, 2]);
    assert_eq!(data_slice(&[0, 1, 2], 1, 3).unwrap(), [1, 2]);
    assert!(data_slice(&[0], 2, 1).is_err());
    assert!(from_twos(U256::from(256), 8).is_err());
    assert!(from_twos(U256::ZERO, 0).is_err());
    assert!(mask(U256::ZERO, 257).is_err());
    assert!(decode_bytes32(&[1; 32]).is_err());
    let mut invalid = [0; 32];
    invalid[0] = 0xff;
    assert!(decode_bytes32(&invalid).is_err());
}
