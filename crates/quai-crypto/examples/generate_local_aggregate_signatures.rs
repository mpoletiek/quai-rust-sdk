//! Emit public toy-key local aggregate signatures for independent Go verification.
use quai_crypto::{OrderedKeyAggregate, SecretKey};
use serde_json::Value;

fn decode(value: &str) -> Vec<u8> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).expect("public fixture hex"))
        .collect()
}
fn main() {
    let mut fixture: Value =
        serde_json::from_str(include_str!("../tests/fixtures/ordered-musig.json"))
            .expect("public fixture");
    for vector in fixture["vectors"].as_array_mut().expect("vector array") {
        let keys: Vec<_> = vector["publicTestSecrets"]
            .as_array()
            .expect("toy keys")
            .iter()
            .map(|s| {
                SecretKey::from_bytes(
                    &decode(s.as_str().expect("key string"))
                        .try_into()
                        .expect("32 bytes"),
                )
                .expect("valid toy key")
            })
            .collect();
        let context =
            OrderedKeyAggregate::new(&keys.iter().map(SecretKey::public_key).collect::<Vec<_>>())
                .expect("public context");
        let digest = decode(vector["digest"].as_str().expect("digest string"))
            .try_into()
            .expect("32 bytes");
        let signature = context
            .sign_local(&keys.iter().collect::<Vec<_>>(), &digest)
            .expect("sign public test digest");
        vector["rustSignature"] = format!(
            "0x{}",
            signature
                .to_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        )
        .into();
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&fixture).expect("serialize fixture")
    );
}
