//! Emit fresh signatures for public toy conversion vectors; never use these keys for funds.
use quai_consensus::{QiConversionTransaction, QuaiToQiTransaction};
use quai_crypto::SecretKey;
use serde_json::Value;
fn bytes(value: &str) -> Vec<u8> {
    value
        .strip_prefix("0x")
        .unwrap()
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
fn hex(bytes: &[u8]) -> String {
    let mut result = String::from("0x");
    for byte in bytes {
        use std::fmt::Write;
        write!(&mut result, "{byte:02x}").unwrap();
    }
    result
}
fn main() {
    let mut fixture: Value =
        serde_json::from_str(include_str!("../tests/conversion-vectors.json")).unwrap();
    fixture["reference"] =
        Value::from("quai-consensus local signing over quais@1.0.0-alpha.57 conversion vectors");
    for vector in fixture["vectors"].as_array_mut().unwrap() {
        let unsigned = bytes(vector["unsigned"].as_str().unwrap());
        let (signed, hash) = if vector["kind"] == "qi" {
            let transaction = QiConversionTransaction::decode_unsigned(&unsigned).unwrap();
            let keys: Vec<_> = vector["publicTestSecrets"]
                .as_array()
                .unwrap()
                .iter()
                .map(|key| {
                    SecretKey::from_bytes(&bytes(key.as_str().unwrap()).try_into().unwrap())
                        .unwrap()
                })
                .collect();
            let signed = transaction
                .sign_local(&keys.iter().collect::<Vec<_>>())
                .unwrap();
            vector["input"]["signature"] = hex(&signed.signature().to_bytes()).into();
            (signed.signed_bytes().unwrap(), signed.hash().unwrap())
        } else {
            let transaction = QuaiToQiTransaction::decode_unsigned(&unsigned).unwrap();
            let key = SecretKey::from_bytes(
                &bytes(vector["publicTestSecret"].as_str().unwrap())
                    .try_into()
                    .unwrap(),
            )
            .unwrap();
            let signed = transaction.sign(&key).unwrap();
            (signed.signed_bytes().unwrap(), signed.hash().unwrap())
        };
        vector["signed"] = hex(&signed).into();
        vector["hash"] = hash.to_string().into();
        vector["input"]["hash"] = hash.to_string().into();
    }
    println!("{}", serde_json::to_string_pretty(&fixture).unwrap());
}
