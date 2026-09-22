//! Public fixture interchange, bounded hostile inputs and legacy metadata tampering.
use quai_keystore::{KdfLimits, Keystore, KeystoreError, Password};
use serde_json::{Value, json};

/// Limits that accept the reference fixtures' deliberately cheap KDF parameters.
///
/// The pinned JavaScript vectors use `n=16, r=1, p=1` so the suite runs fast.
/// Production defaults reject that as `WeakParameters`; importing a fixture is
/// the intended "I know this is weak" case, so it lowers the floor explicitly
/// rather than the suite disabling the bound globally.
fn fixture_limits() -> KdfLimits {
    KdfLimits::default().without_strength_floors()
}

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
    Keystore::from_json(&serde_json::to_vec(v).unwrap(), fixture_limits())
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
        let account = store.decrypt(password, fixture_limits()).unwrap();
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
            .decrypt(Password::Text("wrong"), fixture_limits())
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
                .decrypt(Password::Text("PUBLIC password"), fixture_limits())
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
                .decrypt(Password::Text("PUBLIC mnemonic"), fixture_limits())
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
    assert!(Keystore::from_json(&vec![b' '; 65_537], fixture_limits()).is_err());
    let mut modified = raw.clone();
    modified["x-ethers"] = json!({});
    assert!(parsed(&modified).is_err());
    let text = serde_json::to_string(&raw).unwrap();
    for doc in [
        text.replacen("\"Crypto\":", "\"crypto\":{},\"Crypto\":", 1),
        text.replacen("\"iv\":", "\"IV\":\"00\",\"iv\":", 1),
    ] {
        assert!(Keystore::from_json(doc.as_bytes(), fixture_limits()).is_err());
    }
    let store = parsed(&raw).unwrap();
    assert!(matches!(
        store.decrypt(
            Password::Text("PUBLIC password"),
            KdfLimits::default().with_max_memory_bytes(1)
        ),
        Err(KeystoreError::Limit)
    ));
    assert!(matches!(
        store.decrypt(Password::Bytes(&[0; 1025]), fixture_limits()),
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
    let restored = Keystore::from_json(first.as_json().as_bytes(), fixture_limits())
        .unwrap()
        .decrypt(Password::Text("PUBLIC new export"), fixture_limits())
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
    // These parameters are far below the production work floor by design, so the
    // floor is lowered here to isolate the memory ceiling under test.
    let limits = fixture_limits().with_max_memory_bytes(3000);
    assert!(matches!(
        Keystore::from_json(&serde_json::to_vec(&raw).unwrap(), limits),
        Err(KeystoreError::Limit)
    ));
    let limits = fixture_limits().with_max_memory_bytes(9216);
    assert!(Keystore::from_json(&serde_json::to_vec(&raw).unwrap(), limits).is_ok());
}

#[test]
fn hostile_json_aliases_depth_numeric_tokens_and_trailing_data_fail_closed() {
    let raw = fixtures()[0]["json"].clone();
    let text = serde_json::to_string(&raw).unwrap();
    let escaped_alias = text.replacen("\"Crypto\":", r#""Cr\u0079pto":{},"Crypto":"#, 1);
    assert!(Keystore::from_json(escaped_alias.as_bytes(), fixture_limits()).is_err());
    for replacement in ["3.0", "3e0", "-3", "\"3\""] {
        let changed = text.replace("\"version\":3", &format!("\"version\":{replacement}"));
        assert!(Keystore::from_json(changed.as_bytes(), fixture_limits()).is_err());
    }
    assert!(Keystore::from_json(format!("{text} {{}}").as_bytes(), fixture_limits()).is_err());
    let nested = format!("{{\"id\":{}0{}}}", "[".repeat(17), "]".repeat(17));
    assert!(Keystore::from_json(nested.as_bytes(), fixture_limits()).is_err());
}

#[test]
fn weak_key_derivation_is_rejected_by_default_and_overridable_explicitly() {
    // A hostile "recovery" document can declare a trivial work factor. It
    // decrypts correctly, which is exactly the problem: it turns an import flow
    // into an offline oracle against the password the user just typed, at
    // roughly one hash per guess, and a short salt makes precomputation across
    // victims viable. Ceilings alone never catch this.
    let mut raw = fixtures()[0]["json"].clone();
    raw["Crypto"]["kdfparams"]["n"] = json!(2);
    raw["Crypto"]["kdfparams"]["r"] = json!(1);
    raw["Crypto"]["kdfparams"]["p"] = json!(1);
    let document = serde_json::to_vec(&raw).unwrap();

    assert!(
        matches!(
            Keystore::from_json(&document, KdfLimits::default()),
            Err(KeystoreError::WeakParameters)
        ),
        "the default policy must reject a trivial work factor"
    );

    // Distinct from Limit, so a caller can accept a weak document deliberately
    // rather than by disabling the bound in both directions.
    assert!(Keystore::from_json(&document, fixture_limits()).is_ok());

    // A one-byte salt is likewise rejected at derivation, where it is used.
    let mut short_salt = fixtures()[0]["json"].clone();
    short_salt["Crypto"]["kdfparams"]["salt"] = json!("11");
    let parsed = Keystore::from_json(
        &serde_json::to_vec(&short_salt).unwrap(),
        fixture_limits().with_min_salt_bytes(16),
    )
    .unwrap();
    assert!(matches!(
        parsed.decrypt(
            Password::Text("PUBLIC password"),
            fixture_limits().with_min_salt_bytes(16)
        ),
        Err(KeystoreError::WeakParameters)
    ));

    // Production exports clear the floor: log_n 17, r 8, p 1 is exactly 2^20
    // work with a 32-byte salt, so the default policy round-trips its own output.
    let key = quai_crypto::SecretKey::from_bytes(&[7u8; 32]).unwrap();
    let exported = quai_keystore::encrypt(&key, Password::Text("PUBLIC export")).unwrap();
    let reimported =
        Keystore::from_json(exported.as_json().as_bytes(), KdfLimits::default()).unwrap();
    assert!(
        reimported
            .decrypt(Password::Text("PUBLIC export"), KdfLimits::default())
            .is_ok(),
        "the default floor must not reject this crate's own exports"
    );
}

#[test]
fn kdf_limits_is_configured_through_builders_and_defaults_stay_protective() {
    // This test lives in an integration test, i.e. a separate crate, so it is
    // held to exactly what an external consumer can do. KdfLimits is
    // #[non_exhaustive]: struct-literal construction is unavailable here, which
    // is what makes a future field addition non-breaking. Adding the strength
    // floors to this type was itself a breaking change; marking it prevents a
    // repeat.
    let defaults = KdfLimits::default();
    assert_eq!(defaults.min_scrypt_work, 1 << 20);
    assert_eq!(defaults.min_pbkdf2_rounds, 100_000);
    assert_eq!(defaults.min_salt_bytes, 16);

    // Builders compose and leave every other bound at its default.
    let relaxed = KdfLimits::default().with_min_scrypt_work(0);
    assert_eq!(relaxed.min_scrypt_work, 0);
    assert_eq!(relaxed.max_memory_bytes, defaults.max_memory_bytes);
    assert_eq!(relaxed.min_salt_bytes, defaults.min_salt_bytes);

    // The named escape hatch drops exactly the three floors and nothing else.
    let floorless = KdfLimits::default().without_strength_floors();
    assert_eq!(
        (
            floorless.min_scrypt_work,
            floorless.min_pbkdf2_rounds,
            floorless.min_salt_bytes
        ),
        (0, 0, 0)
    );
    assert_eq!(floorless.max_memory_bytes, defaults.max_memory_bytes);
    assert_eq!(floorless.max_scrypt_work, defaults.max_scrypt_work);
    assert_eq!(floorless.max_pbkdf2_rounds, defaults.max_pbkdf2_rounds);

    // A caller who relaxes one unrelated ceiling still inherits the floors, so
    // tuning a resource bound cannot accidentally disable the strength policy.
    let tuned = KdfLimits::default().with_max_memory_bytes(64 * 1024 * 1024);
    assert_eq!(tuned.min_scrypt_work, defaults.min_scrypt_work);
    let mut weak = fixtures()[0]["json"].clone();
    weak["Crypto"]["kdfparams"]["n"] = json!(2);
    assert!(matches!(
        Keystore::from_json(&serde_json::to_vec(&weak).unwrap(), tuned),
        Err(KeystoreError::WeakParameters)
    ));
}

#[test]
fn the_salt_floor_is_enforced_on_the_standalone_derivation_path() {
    // Regression. min_salt_bytes is checked inside Kdf::derive, which the
    // standalone derive module deliberately does not use -- it calls the
    // RustCrypto entry points directly -- and DeriveParams::validate has no salt
    // to inspect. So a caller who enabled the floors got the work and round
    // floors enforced and the salt bound silently ignored: a fail-open, and one
    // that hit the caller who followed the documented advice rather than the one
    // who ignored it.
    use quai_keystore::derive::{DeriveError, DeriveLimits, DeriveParams, Pbkdf2Hash, derive_key};

    let password = b"PUBLIC password";
    let strict = DeriveLimits::default().with_kdf(KdfLimits::default().with_strength_floors());
    let params = DeriveParams::Pbkdf2 {
        rounds: 200_000,
        hash: Pbkdf2Hash::Sha256,
    };

    assert_eq!(
        derive_key(password, b"\x01", params, 32, strict).unwrap_err(),
        DeriveError::WeakParameters,
        "a one-byte salt must be rejected when the floors are enabled"
    );
    // At the boundary and above it, the same derivation succeeds.
    assert!(derive_key(password, &[7u8; 16], params, 32, strict).is_ok());

    // The floor is reported as a floor, not as malformed parameters, so the
    // caller is pointed at the policy rather than at its DeriveParams.
    let weak_rounds = DeriveParams::Pbkdf2 {
        rounds: 10,
        hash: Pbkdf2Hash::Sha256,
    };
    assert_eq!(
        derive_key(password, &[7u8; 16], weak_rounds, 32, strict).unwrap_err(),
        DeriveError::WeakParameters
    );

    // The default remains floorless: this is a general-purpose primitive whose
    // parameters come from the caller and whose salt may be protocol-fixed.
    assert!(derive_key(password, b"\x01", weak_rounds, 32, DeriveLimits::default()).is_ok());
}

#[test]
fn this_crates_own_export_clears_the_default_strength_floor() {
    // `seal` validates against ceilings only, because its parameters are this
    // crate's own constants rather than an attacker-authored document. That
    // decoupling is deliberate: coupling export to the floor at runtime would
    // let a future floor increase break exports for every user. The policy
    // relationship is asserted here instead, so it fails at build time.
    //
    // Export uses log_n 17, r 8, p 1 = 2^20 work with a 32-byte salt, which is
    // exactly the default floor. If either side moves, this test says so.
    let key = quai_crypto::SecretKey::from_bytes(&[9u8; 32]).unwrap();
    let exported = quai_keystore::encrypt(&key, Password::Text("PUBLIC export")).unwrap();
    let document: Value = serde_json::from_str(exported.as_json()).unwrap();
    let params = &document["Crypto"]["kdfparams"];
    let work = params["n"].as_u64().unwrap()
        * params["r"].as_u64().unwrap()
        * params["p"].as_u64().unwrap();
    let defaults = KdfLimits::default();
    assert!(
        work >= defaults.min_scrypt_work,
        "export work {work} is below the default floor {}",
        defaults.min_scrypt_work
    );
    assert!(
        decode(params["salt"].as_str().unwrap()).len() >= defaults.min_salt_bytes,
        "export salt is below the default floor"
    );
    // And the default policy round-trips this crate's own output.
    assert!(
        Keystore::from_json(exported.as_json().as_bytes(), KdfLimits::default())
            .unwrap()
            .decrypt(Password::Text("PUBLIC export"), KdfLimits::default())
            .is_ok()
    );
}

#[test]
fn scrypt_cost_beyond_the_rfc_bound_is_refused_like_quais_js() {
    // quais.js's scrypt requires N < 2^(16r); scrypt 0.12 dropped that check,
    // so r = 1 with N = 2^16 used to open here and fail there.
    let mut raw = fixtures()[0]["json"].clone();
    raw["Crypto"]["kdfparams"]["n"] = json!(1 << 16);
    assert!(matches!(parsed(&raw), Err(KeystoreError::Format)));
    raw["Crypto"]["kdfparams"]["n"] = json!(1 << 15);
    assert!(parsed(&raw).is_ok());
}

#[test]
fn default_limits_admit_the_standard_n_2_18_r_8_document() {
    // geth's and MetaMask's standard parameters: 128*r*p*(N+2) is 256 MiB plus
    // 2 KiB, which a flat 256 MiB ceiling refused. Parsing validates the KDF
    // policy without running it.
    let mut raw = fixtures()[0]["json"].clone();
    let params = &mut raw["Crypto"]["kdfparams"];
    params["n"] = json!(1 << 18);
    params["r"] = json!(8);
    params["p"] = json!(1);
    let document = serde_json::to_vec(&raw).unwrap();
    assert!(Keystore::from_json(&document, KdfLimits::default()).is_ok());
    assert!(matches!(
        Keystore::from_json(
            &document,
            KdfLimits::default().with_max_memory_bytes(256 * 1024 * 1024)
        ),
        Err(KeystoreError::Limit)
    ));
}
