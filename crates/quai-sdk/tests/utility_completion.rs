//! Exact network metadata, raw public derivation and guarded standalone KDF utilities.
#![cfg(all(feature = "wallet", feature = "keystore"))]
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
use quai_sdk::U256;
use quai_sdk::crypto::PublicKey;
use quai_sdk::keystore::derive::*;
use quai_sdk::primitives::{get_bytes, hexlify};
use quai_sdk::provider::{FeeData, Network, NetworkError, NetworkMatch, NetworkRegistry};
use quai_sdk::wallet::ExtendedPublicKey;
use serde_json::{Value, json};
fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "fixtures/shared/compatibility/fixtures/utility-completion.json"
    ))
    .unwrap()
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn network_metadata_is_exact_and_matching_intent_is_explicit() {
    for row in fixture()["networks"].as_array().unwrap() {
        let n = Network::from_json(row).unwrap();
        assert_eq!(n.to_json(), *row);
        assert!(n.matches(NetworkMatch::Name(row["name"].as_str().unwrap())));
        assert!(n.matches(NetworkMatch::ChainId(n.chain_id())));
        assert!(n.matches(NetworkMatch::Descriptor(
            &Network::new("other", n.chain_id()).unwrap()
        )));
        assert!(!n.matches(NetworkMatch::Name("other")));
        assert_eq!(n.clone(), n);
    }
    assert!(Network::from_json(&json!({"name":"x","chainId":1.5})).is_err());
    assert!(Network::from_json(&json!({"name":"x","chainId":-1})).is_err());
    assert!(Network::from_json(&json!({"name":"x","chainId":"9","extra":0})).is_err());
    assert!(Network::new(&"x".repeat(129), U256::from(9)).is_err());
    assert!(Network::new("bad\nname", U256::from(9)).is_err());
    let named = Network::new("9", U256::from(17)).unwrap();
    assert!(named.matches(NetworkMatch::Name("9")));
    assert!(!named.matches(NetworkMatch::ChainId(U256::from(9))));
    assert_eq!(
        Network::from_json(&json!({"name":"zero","chainId":"0x0"}))
            .unwrap()
            .chain_id(),
        U256::ZERO
    );
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn registry_limits_and_conflicts_leave_both_maps_unchanged() {
    let mut registry = NetworkRegistry::new(3).unwrap();
    assert!(registry.is_empty());
    let a = Network::new("mainnet", U256::from(9)).unwrap();
    registry.register(a.clone()).unwrap();
    assert_eq!(registry.len(), 2);
    assert_eq!(
        registry.register(Network::new("other", U256::from(9)).unwrap()),
        Err(NetworkError::Conflict)
    );
    assert_eq!(registry.by_name("other"), Err(NetworkError::Unknown));
    assert_eq!(registry.len(), 2);
    registry.register_alias("quai", &a).unwrap();
    assert_eq!(registry.by_name("quai").unwrap(), a);
    assert_eq!(registry.by_chain_id(U256::from(9)), a);
    assert_eq!(registry.len(), 3);
    assert_eq!(
        registry.register(Network::new("new", U256::from(99)).unwrap()),
        Err(NetworkError::Limit)
    );
    assert_eq!(registry.by_name("new"), Err(NetworkError::Unknown));
    assert_eq!(registry.by_chain_id(U256::from(99)).name(), "unknown");
    assert_eq!(registry.len(), 3);
    assert_eq!(
        registry.register_alias("quai", &a),
        Err(NetworkError::Conflict)
    );
    assert!(NetworkRegistry::new(257).is_err());
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn detached_fee_view_preserves_null_and_full_precision_without_display_marker() {
    for row in fixture()["fees"].as_array().unwrap() {
        let gas_price = row["gasPrice"]
            .as_str()
            .map(|s| U256::from_str_radix(s, 10).unwrap());
        let out = FeeData { gas_price }.to_json();
        assert_eq!(out["gasPrice"], row["gasPrice"]);
        assert!(out.get("_type").is_none());
    }
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn public_point_chain_roots_and_full_metadata_match_published_child_vectors() {
    let f = fixture();
    let n = &f["publicNode"];
    let public =
        PublicKey::from_sec1_bytes(&get_bytes(n["publicKey"].as_str().unwrap()).unwrap()).unwrap();
    let chain: [u8; 32] = get_bytes(n["chainCode"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let synthetic = ExtendedPublicKey::from_public_key_chain_code(public, chain).unwrap();
    assert_eq!(synthetic.depth(), 0);
    assert!(synthetic.export().starts_with("xpub"));
    let account = ExtendedPublicKey::import(n["accountXpub"].as_str().unwrap()).unwrap();
    let restored = ExtendedPublicKey::from_components(public, account.metadata()).unwrap();
    assert_eq!(restored.export(), account.export());
    assert_eq!(restored.depth(), 3);
    for row in n["children"].as_array().unwrap() {
        for root in [&synthetic, &restored] {
            let child = root
                .derive_child(row["change"].as_u64().unwrap() as u32, false)
                .unwrap()
                .derive_child(row["index"].as_u64().unwrap() as u32, false)
                .unwrap();
            let expected = &row["expected"];
            assert_eq!(
                hexlify(&child.public_key().unwrap().to_compressed()).unwrap(),
                expected["publicKey"]
            );
            assert_eq!(
                hexlify(&child.metadata().chain_code).unwrap(),
                expected["chainCode"]
            );
            assert_eq!(
                child.public_key().unwrap().address().to_string(),
                expected["address"]
            );
        }
    }
    let mut metadata = account.metadata();
    metadata.fingerprint[0] ^= 1;
    assert!(ExtendedPublicKey::from_components(public, metadata).is_err());
    metadata = account.metadata();
    metadata.depth = 0;
    assert!(ExtendedPublicKey::from_components(public, metadata).is_err());
    metadata = account.metadata();
    metadata.depth = 255;
    let terminal = ExtendedPublicKey::from_components(public, metadata).unwrap();
    assert!(terminal.derive_child(0, false).is_err());
    assert!(synthetic.derive_child(0, true).is_err());
}
fn params(row: &Value) -> DeriveParams {
    if row["kind"] == "scrypt" {
        DeriveParams::Scrypt {
            log_n: row["logN"].as_u64().unwrap() as u8,
            r: row["r"].as_u64().unwrap() as u32,
            p: row["p"].as_u64().unwrap() as u32,
        }
    } else {
        DeriveParams::Pbkdf2 {
            rounds: row["rounds"].as_u64().unwrap() as u32,
            hash: if row["hash"] == "sha512" {
                Pbkdf2Hash::Sha512
            } else {
                Pbkdf2Hash::Sha256
            },
        }
    }
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn standalone_kdf_vectors_cover_multiple_output_blocks_and_both_hashes() {
    let f = fixture();
    assert_eq!(f["kdfs"].as_array().unwrap().len(), 16);
    for row in f["kdfs"].as_array().unwrap() {
        let password = get_bytes(row["password"].as_str().unwrap()).unwrap();
        let salt = get_bytes(row["salt"].as_str().unwrap()).unwrap();
        let mut progress = vec![];
        let key = derive_key_with_progress(
            &password,
            &salt,
            params(row),
            row["length"].as_u64().unwrap() as usize,
            DeriveLimits::default().with_max_output_bytes(128),
            |p| {
                progress.push(p);
                true
            },
        )
        .unwrap();
        assert_eq!(hexlify(key.as_bytes()).unwrap(), row["output"]);
        assert_eq!(
            progress,
            [DeriveProgress::Started, DeriveProgress::Completed]
        );
        assert_eq!(progress[0].fraction(), 0.0);
        assert_eq!(progress[1].fraction(), 1.0);
        assert_eq!(format!("{key:?}"), "DerivedKey([REDACTED])");
    }
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn standalone_kdf_preflight_accounts_for_output_work_and_checkpoints() {
    assert_eq!(
        DeriveParams::Scrypt {
            log_n: 0,
            r: 1,
            p: 1
        }
        .validate(32, DeriveLimits::default()),
        Err(DeriveError::Invalid)
    );
    assert_eq!(
        DeriveParams::Scrypt {
            log_n: 16,
            r: 1,
            p: 1
        }
        .validate(32, DeriveLimits::default()),
        Err(DeriveError::Invalid)
    );
    let params = DeriveParams::Pbkdf2 {
        rounds: 100,
        hash: Pbkdf2Hash::Sha256,
    };
    let limits = DeriveLimits::default()
        .with_max_output_bytes(128)
        .with_max_pbkdf2_work(299);
    let mut called = 0;
    assert!(matches!(
        derive_key_with_progress(b"PUBLIC", b"SALT", params, 96, limits, |_| {
            called += 1;
            true
        }),
        Err(DeriveError::Limit)
    ));
    assert_eq!(called, 0);
    assert!(matches!(
        derive_key(&[0; 1025], b"", params, 32, DeriveLimits::default()),
        Err(DeriveError::Limit)
    ));
    assert!(matches!(
        derive_key(b"", b"", params, 0, DeriveLimits::default()),
        Err(DeriveError::Invalid)
    ));
    assert!(
        DeriveParams::Scrypt {
            log_n: 63,
            r: u32::MAX,
            p: u32::MAX
        }
        .validate(32, DeriveLimits::default())
        .is_err()
    );
    assert!(matches!(
        derive_key_with_progress(b"", b"", params, 32, DeriveLimits::default(), |_| false),
        Err(DeriveError::Cancelled)
    ));
    let mut phases = vec![];
    assert!(matches!(
        derive_key_with_progress(b"", b"", params, 32, DeriveLimits::default(), |p| {
            phases.push(p);
            p == DeriveProgress::Started
        }),
        Err(DeriveError::Cancelled)
    ));
    assert_eq!(phases, [DeriveProgress::Started, DeriveProgress::Completed]);
}
