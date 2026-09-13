#![no_main]
use libfuzzer_sys::fuzz_target;
use quai_consensus::U256;
use quai_primitives::*;
fuzz_target!(|data: &[u8]| {
    if let Ok(encoded) = hexlify(data) {
        assert_eq!(get_bytes(&encoded).unwrap(), data);
    }
    if let Ok(encoded) = encode_base64(data) {
        assert_eq!(decode_base64(&encoded).unwrap(), data);
    }
    if data.len() <= 256 {
        let encoded = encode_base58(data).unwrap();
        assert_eq!(decode_base58_bytes(&encoded).unwrap(), data);
    }
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = get_bytes(text);
        let _ = decode_base64(text);
        let _ = decode_base58_bytes(text);
        let _ = decode_base58(text);
        if let Ok(encoded) = encode_bytes32(text) {
            assert_eq!(
                decode_bytes32(&encoded).unwrap(),
                text.trim_end_matches('\0')
            );
        }
    }
    if let Ok(word) = <&[u8; 32]>::try_from(data) {
        let _ = decode_bytes32(word);
    }
    if data.len() >= 34 {
        let width = u16::from_be_bytes([data[0], data[1]]);
        let value = U256::from_be_slice(&data[2..34]);
        if let Ok(signed) = from_twos(value, width) {
            assert_eq!(to_twos(signed, width).unwrap(), value);
        }
        if let Ok(masked) = mask(value, width) {
            assert_eq!(mask(masked, width).unwrap(), masked);
        }
    }
});
