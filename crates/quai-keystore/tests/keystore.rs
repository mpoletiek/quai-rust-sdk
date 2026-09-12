//! Public fixture interchange, bounded hostile inputs and legacy metadata tampering.
use quai_keystore::{KdfLimits, Keystore, KeystoreError, Password};
use serde_json::{Value, json};
fn fixtures() -> Vec<Value> {
    serde_json::from_str::<Value>(include_str!("fixtures/keystores.json")).unwrap()["vectors"]
        .as_array()
        .unwrap()
        .clone()
}
fn decode(s: &str) -> Vec<u8> {
    s.trim_start_matches("0x")
        .as_bytes()
        .chunks_exact(2)
        .map(|x| u8::from_str_radix(std::str::from_utf8(x).unwrap(), 16).unwrap())
        .collect()
}
fn parsed(v: &Value) -> Result<Keystore, KeystoreError> {
    Keystore::from_json(&serde_json::to_vec(v).unwrap(), KdfLimits::default())
}
#[test]
fn all_pinned_js_key_kdfs_password_forms_and_mnemonic_languages_match() {
    let vectors = fixtures();
    assert_eq!(vectors.len(), 15);
    for v in vectors {
        let store = parsed(&v["json"]).unwrap();
        let raw = v["password"]["bytes"].as_str().map(decode);
        let password = if let Some(s) = v["password"]["text"].as_str() {
            Password::Text(s)
        } else {
            Password::Bytes(raw.as_ref().unwrap())
        };
        let account = store.decrypt(password, KdfLimits::default()).unwrap();
        assert_eq!(account.address().to_string(), v["expected"]["address"]);
        assert_eq!(
            account.secret_key().export_bytes().as_ref(),
            decode(v["expected"]["privateKey"].as_str().unwrap())
        );
        assert!(!format!("{account:?}").contains(v["expected"]["privateKey"].as_str().unwrap()));
        assert_eq!(
            account.mnemonic().is_some(),
            v["expected"].get("mnemonic").is_some()
        );
        if let Some(m) = account.mnemonic() {
            assert_eq!(
                m.path(),
                v["expected"]["mnemonic"]["path"].as_str().unwrap()
            );
        }
    }
}
#[test]
fn wrong_password_ciphertext_iv_address_and_unmaced_mnemonic_tampering_fail() {
    let fixtures = fixtures();
    let raw = &fixtures[0]["json"];
    assert_eq!(
        parsed(raw)
            .unwrap()
            .decrypt(Password::Text("wrong"), KdfLimits::default())
            .unwrap_err(),
        KeystoreError::Authentication
    );
    for pointer in [
        "/Crypto/ciphertext",
        "/Crypto/mac",
        "/Crypto/cipherparams/iv",
        "/address",
    ] {
        let mut modified = raw.clone();
        let text = modified
            .pointer(pointer)
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned();
        let changed = format!(
            "{}{}",
            if text.starts_with('0') { "1" } else { "0" },
            &text[1..]
        );
        *modified.pointer_mut(pointer).unwrap() = json!(changed);
        assert_eq!(
            parsed(&modified)
                .unwrap()
                .decrypt(Password::Text("PUBLIC password"), KdfLimits::default())
                .unwrap_err(),
            KeystoreError::Authentication
        );
    }
    let mnemonic = &fixtures
        .iter()
        .find(|x| x["name"] == "mnemonic-en")
        .unwrap()["json"];
    for (pointer, value) in [
        ("/x-quais/path", json!("m/44'/969'/1'/1/13")),
        ("/x-quais/locale", json!("fr")),
        ("/x-quais/mnemonicCounter", json!("55".repeat(16))),
        ("/x-quais/mnemonicCiphertext", json!("00".repeat(16))),
    ] {
        let mut modified = mnemonic.clone();
        *modified.pointer_mut(pointer).unwrap() = value;
        assert_eq!(
            parsed(&modified)
                .unwrap()
                .decrypt(Password::Text("PUBLIC mnemonic"), KdfLimits::default())
                .unwrap_err(),
            KeystoreError::Authentication
        );
    }
}
#[test]
fn resource_and_format_rejections_happen_before_kdf() {
    let raw = fixtures()[0]["json"].clone();
    for (pointer, value) in [
        ("/version", json!(4)),
        ("/Crypto/kdfparams/n", json!(u64::MAX)),
        ("/Crypto/kdfparams/n", json!(1u64 << 63)),
        ("/Crypto/kdfparams/r", json!(u32::MAX)),
        ("/Crypto/kdfparams/p", json!(0)),
        ("/Crypto/kdfparams/dklen", json!(64)),
        ("/Crypto/cipher", json!("aes-256-cbc")),
        ("/Crypto/ciphertext", json!("00".repeat(33))),
    ] {
        let mut modified = raw.clone();
        *modified.pointer_mut(pointer).unwrap() = value;
        assert!(parsed(&modified).is_err());
    }
    assert!(Keystore::from_json(&vec![b' '; 65_537], KdfLimits::default()).is_err());
    let mut modified = raw.clone();
    modified["x-ethers"] = json!({});
    assert!(parsed(&modified).is_err());
    let text = serde_json::to_string(&raw).unwrap();
    for doc in [
        text.replacen("\"Crypto\":", "\"crypto\":{},\"Crypto\":", 1),
        text.replacen("\"iv\":", "\"IV\":\"00\",\"iv\":", 1),
    ] {
        assert!(Keystore::from_json(doc.as_bytes(), KdfLimits::default()).is_err());
    }
    let store = parsed(&raw).unwrap();
    assert!(matches!(
        store.decrypt(
            Password::Text("PUBLIC password"),
            KdfLimits {
                max_memory_bytes: 1,
                ..Default::default()
            }
        ),
        Err(KeystoreError::Limit)
    ));
    assert!(matches!(
        store.decrypt(Password::Bytes(&[0; 1025]), KdfLimits::default()),
        Err(KeystoreError::Limit)
    ));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn native_export_uses_fresh_entropy_fixed_scrypt_and_roundtrips() {
    let mut scalar = [0; 32];
    scalar[30..].copy_from_slice(&805u16.to_be_bytes());
    let key = quai_crypto::SecretKey::from_bytes(&scalar).unwrap();
    let first = quai_keystore::encrypt(&key, Password::Text("PUBLIC new export")).unwrap();
    let second = quai_keystore::encrypt(&key, Password::Text("PUBLIC new export")).unwrap();
    let a: Value = serde_json::from_str(first.as_json()).unwrap();
    let b: Value = serde_json::from_str(second.as_json()).unwrap();
    for pointer in [
        "/id",
        "/Crypto/cipherparams/iv",
        "/Crypto/kdfparams/salt",
        "/Crypto/ciphertext",
    ] {
        assert_ne!(a.pointer(pointer), b.pointer(pointer));
    }
    assert_eq!(a["Crypto"]["kdfparams"]["n"], 131072);
    assert_eq!(a["Crypto"]["kdfparams"]["r"], 8);
    assert_eq!(a["Crypto"]["kdfparams"]["p"], 1);
    let restored = Keystore::from_json(first.as_json().as_bytes(), KdfLimits::default())
        .unwrap()
        .decrypt(Password::Text("PUBLIC new export"), KdfLimits::default())
        .unwrap();
    assert_eq!(restored.address(), key.public_key().address());
    assert!(!format!("{first:?}").contains("PUBLIC"));
    assert!(
        quai_keystore::encrypt_with_mnemonic(
            &key,
            Password::Text("PUBLIC"),
            &[0; 16],
            quai_wallet::Language::English,
            "m/44'/994'/0'/0/0"
        )
        .is_err()
    );
}

#[test]
fn scrypt_budget_covers_downstream_parallel_feature_unification() {
    let mut raw = fixtures()[0]["json"].clone();
    raw["Crypto"]["kdfparams"]["n"] = json!(16);
    raw["Crypto"]["kdfparams"]["r"] = json!(1);
    raw["Crypto"]["kdfparams"]["p"] = json!(4);
    // Sequential B+V+T = 2688 bytes; four parallel V/T workspaces need 9216.
    let limits = KdfLimits {
        max_memory_bytes: 3000,
        ..Default::default()
    };
    assert!(matches!(
        Keystore::from_json(&serde_json::to_vec(&raw).unwrap(), limits),
        Err(KeystoreError::Limit)
    ));
    let limits = KdfLimits {
        max_memory_bytes: 9216,
        ..Default::default()
    };
    assert!(Keystore::from_json(&serde_json::to_vec(&raw).unwrap(), limits).is_ok());
}

#[test]
fn hostile_json_aliases_depth_numeric_tokens_and_trailing_data_fail_closed() {
    let raw = fixtures()[0]["json"].clone();
    let text = serde_json::to_string(&raw).unwrap();
    let escaped_alias = text.replacen("\"Crypto\":", r#""Cr\u0079pto":{},"Crypto":"#, 1);
    assert!(Keystore::from_json(escaped_alias.as_bytes(), KdfLimits::default()).is_err());
    for replacement in ["3.0", "3e0", "-3", "\"3\""] {
        let changed = text.replace("\"version\":3", &format!("\"version\":{replacement}"));
        assert!(Keystore::from_json(changed.as_bytes(), KdfLimits::default()).is_err());
    }
    assert!(Keystore::from_json(format!("{text} {{}}").as_bytes(), KdfLimits::default()).is_err());
    let nested = format!("{{\"id\":{}0{}}}", "[".repeat(17), "]".repeat(17));
    assert!(Keystore::from_json(nested.as_bytes(), KdfLimits::default()).is_err());
}
