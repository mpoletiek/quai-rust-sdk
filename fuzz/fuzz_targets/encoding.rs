#![no_main]
use libfuzzer_sys::fuzz_target;
use quai_consensus::U256;
use quai_primitives::*;
fuzz_target!(|data: &[u8]| {
    use quai_primitives::numeric::*;
    if let Ok(value)=uint_from_be_bytes(data){assert_eq!(uint_from_be_bytes(&uint_to_be_array(value)).unwrap(),value);assert_eq!(parse_uint(&uint_to_quantity(value)).unwrap(),value);assert_eq!(parse_uint(&uint_to_be_hex(value,None).unwrap()).unwrap(),value);}
    if data.len()>=8 {let bits=u64::from_be_bytes(data[..8].try_into().unwrap());let number=f64::from_bits(bits);if let Ok(integer)=integer_from_safe_number(number){assert_eq!(integer_to_safe_number(integer).unwrap(),number);}}

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
    let _ = to_utf8_string(data);
    if let Ok(text) = std::str::from_utf8(data) {
        if let Ok(integer)=parse_integer(text){let canonical=integer.format(Unit::new(0).unwrap());assert_eq!(parse_integer(&canonical).unwrap(),integer);if let Ok(value)=integer.try_u256(){assert_eq!(parse_uint(text).unwrap(),value);}}
        if is_hex_string(text,HexFormat::Bytes){let bytes=get_bytes(text).unwrap();assert!(is_hex_string(text,HexFormat::Exact(bytes.len())));assert!(is_hex_string(text,HexFormat::Any));}

        use quai_sdk::rpc::fetch::*;
        let _=data_resource(text);
        let _=ipfs_resource(text,"https://gateway.invalid/ipfs/");
        if let Ok(mut request)=FetchRequest::new(text) {assert_eq!(request.url(),text);let _=request.validate();let _=request.redirect(text);let _=request.set_text(text);assert!(request.body().map_or(true,|b|b.len()<=MAX_FETCH_BYTES));}
        if let Ok(value)=serde_json::from_str::<serde_json::Value>(text){
            if let Ok(network)=quai_provider::Network::from_json(&value){assert_eq!(quai_provider::Network::from_json(&network.to_json()).unwrap(),network);let mut registry=quai_provider::NetworkRegistry::new(3).unwrap();registry.register(network.clone()).unwrap();assert_eq!(registry.by_chain_id(network.chain_id()),network);}

            let mut request=FetchRequest::new("https://example.invalid/").unwrap();
            if request.set_json(&value).is_ok(){assert_eq!(serde_json::from_slice::<serde_json::Value>(request.body().unwrap()).unwrap(),value);}
            if let (Some(name),Some(value))=(value["header"].as_str(),value["value"].as_str()){let mut h=FetchHeaders::default();if h.set(name,value).is_ok(){assert_eq!(h.get(name),Some(value));}}
        }

        for form in [None,Some(Utf8Normalization::Nfc),Some(Utf8Normalization::Nfd),Some(Utf8Normalization::Nfkc),Some(Utf8Normalization::Nfkd)] {
            if let Ok(bytes) = to_utf8_bytes(text,form) {
                let normalized = to_utf8_string(&bytes).unwrap();
                assert_eq!(to_utf8_bytes(normalized,form).unwrap(),bytes);
                assert_eq!(to_utf8_code_points(text,form).unwrap(),normalized.chars().map(u32::from).collect::<Vec<_>>());
            }
        }
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
