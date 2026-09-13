//! Published public-toy signature and curve vectors plus strict boundary policies.
use quai_crypto::{
    CryptoError, PublicKey, RecoverableSignature, SecretKey, SignatureMetadata, U256,
    legacy_chain_id, legacy_chain_v, normalized_v,
};
use serde_json::Value;
fn fixture() -> Value {
    serde_json::from_slice(include_bytes!(
        "fixtures/shared/compatibility/fixtures/crypto-utils.json"
    ))
    .unwrap()
}
fn bytes<const N: usize>(v: &Value) -> [u8; N] {
    quai_primitives::get_bytes(v.as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap()
}
fn integer(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn exact_eip2098_and_legacy_metadata_match_public_reference_vectors() {
    let f = fixture();
    let mut parities = [false; 2];
    for row in f["signatures"].as_array().unwrap() {
        let key = SecretKey::from_bytes(&bytes(
            &f["keys"][row["key"].as_u64().unwrap() as usize]["privateKey"],
        ))
        .unwrap();
        let digest = bytes(&row["digest"]);
        let sig = key.sign_prehash(&digest).unwrap();
        assert_eq!(
            sig.to_quais_bytes().unwrap(),
            bytes::<65>(&row["serialized"])
        );
        assert_eq!(sig.to_eip2098().unwrap(), bytes::<64>(&row["compact"]));
        assert_eq!(
            sig.to_eip2098().unwrap()[32..],
            bytes::<32>(&row["yParityAndS"])
        );
        assert_eq!(
            RecoverableSignature::from_eip2098(&bytes(&row["compact"])).unwrap(),
            sig
        );
        assert_eq!(sig.recover_prehash(&digest).unwrap(), key.public_key());
        parities[usize::from(sig.recovery_id())] = true;
        let legacy =
            SignatureMetadata::from_rs_v(sig.r(), sig.s(), integer(&row["networkV"])).unwrap();
        assert_eq!(legacy.signature(), sig);
        assert_eq!(legacy.network_v(), Some(integer(&row["networkV"])));
        assert_eq!(legacy.legacy_chain_id(), Some(integer(&row["chainId"])));
        assert_eq!(legacy.v(), row["v"].as_u64().unwrap() as u8);
        assert_eq!(
            serde_json::from_str::<Value>(&legacy.to_json()).unwrap(),
            row["legacyJson"]
        );
        let plain = SignatureMetadata::from_signature(sig).unwrap();
        assert_eq!(plain.legacy_chain_id(), None);
        assert_eq!(plain.network_v(), None);
        assert_eq!(
            serde_json::from_str::<Value>(&plain.to_json()).unwrap(),
            row["json"]
        );
        assert_eq!(
            SignatureMetadata::from_rs_v(sig.r(), sig.s(), U256::from(sig.recovery_id())).unwrap(),
            plain
        );
    }
    assert_eq!(parities, [true, true]);
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn legacy_v_helpers_reject_invalid_values_and_exact_overflow() {
    let f = fixture();
    for row in f["vValues"].as_array().unwrap() {
        let v = integer(&row["v"]);
        assert_eq!(
            normalized_v(v).unwrap(),
            row["normalized"].as_u64().unwrap() as u8
        );
        if row["chain"].is_null() {
            assert!(legacy_chain_id(v).is_err());
        } else {
            assert_eq!(legacy_chain_id(v).unwrap(), integer(&row["chain"]));
        }
    }
    for v in [2, 5, 26, 29, 34] {
        assert!(normalized_v(U256::from(v)).is_err());
        assert!(legacy_chain_id(U256::from(v)).is_err());
    }
    for v in [0, 1, 5, 26, 29, 255] {
        assert!(legacy_chain_v(U256::from(9), v).is_err());
    }
    for parity in [27, 28] {
        let max = (U256::MAX - U256::from(35 + parity - 27)) / U256::from(2);
        let v = legacy_chain_v(max, parity).unwrap();
        assert_eq!(legacy_chain_id(v).unwrap(), max);
        assert_eq!(normalized_v(v).unwrap(), parity);
        assert!(legacy_chain_v(max + U256::from(1), parity).is_err());
    }
    assert!(legacy_chain_v(U256::MAX, 27).is_err());
    assert_eq!(legacy_chain_v(U256::ZERO, 27).unwrap(), U256::from(35));
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn compact_signatures_reject_zero_high_s_and_unrepresentable_recovery_bits() {
    assert_eq!(
        RecoverableSignature::from_eip2098(&[0; 64]).unwrap_err(),
        CryptoError::InvalidSignature
    );
    assert!(SignatureMetadata::from_rs_v([0; 32], [0; 32], U256::from(27)).is_err());
    let order = U256::from_str_radix(
        "fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141",
        16,
    )
    .unwrap();
    let mut r = [0; 32];
    r[31] = 1;
    let s = (order / U256::from(2) + U256::from(1)).to_be_bytes::<32>();
    let mut compact = [0; 64];
    compact[..32].copy_from_slice(&r);
    compact[32..].copy_from_slice(&s);
    assert_eq!(
        RecoverableSignature::from_eip2098(&compact).unwrap_err(),
        CryptoError::HighS
    );
    assert_eq!(
        SignatureMetadata::from_rs_v(r, s, U256::from(27)).unwrap_err(),
        CryptoError::HighS
    );
    let mut s = [0; 32];
    s[31] = 1;
    compact[32..].copy_from_slice(&s);
    for recovery in [2, 3] {
        let sig = RecoverableSignature::from_compact(&compact, recovery).unwrap();
        assert_eq!(
            sig.to_eip2098().unwrap_err(),
            CryptoError::UnsupportedRecoveryId
        );
        assert_eq!(
            SignatureMetadata::from_signature(sig).unwrap_err(),
            CryptoError::UnsupportedRecoveryId
        );
    }
    let invalid_r = order.to_be_bytes::<32>();
    assert!(SignatureMetadata::from_rs_v(invalid_r, s, U256::from(27)).is_err());
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn full_ecdh_and_public_addition_match_and_secret_outputs_redact() {
    let f = fixture();
    for row in f["pairs"].as_array().unwrap() {
        let a = SecretKey::from_bytes(&bytes(
            &f["keys"][row["a"].as_u64().unwrap() as usize]["privateKey"],
        ))
        .unwrap();
        let b = SecretKey::from_bytes(&bytes(
            &f["keys"][row["b"].as_u64().unwrap() as usize]["privateKey"],
        ))
        .unwrap();
        let shared = a.ecdh_shared_point(&b.public_key());
        assert_eq!(shared.as_bytes(), &bytes::<65>(&row["shared"]));
        assert_eq!(
            shared.as_bytes(),
            b.ecdh_shared_point(&a.public_key()).as_bytes()
        );
        assert_eq!(
            &shared.as_bytes()[1..33],
            a.ecdh_shared_x(&b.public_key()).as_bytes()
        );
        assert_eq!(format!("{shared:?}"), "SecretBytes([REDACTED])");
        let sum = a.public_key().add_point(b.public_key()).unwrap();
        assert_eq!(sum.to_compressed(), bytes::<33>(&row["sumCompressed"]));
        assert_eq!(sum.to_uncompressed(), bytes::<65>(&row["sumUncompressed"]));
        assert_eq!(sum, b.public_key().add_point(a.public_key()).unwrap());
        assert_eq!(sum, a.add_tweak(&b).unwrap().public_key());
    }
    let p = PublicKey::from_sec1_bytes(&bytes::<33>(&f["keys"][0]["compressed"])).unwrap();
    let mut negative = p.to_compressed();
    negative[0] ^= 1;
    let negative = PublicKey::from_sec1_bytes(&negative).unwrap();
    assert_eq!(
        p.add_point(negative).unwrap_err(),
        CryptoError::InvalidPublicKey
    );
    assert_eq!(
        p.add_point(p).unwrap().to_compressed(),
        bytes::<33>(&f["keys"][1]["compressed"])
    );
    assert!(PublicKey::from_sec1_bytes(&[0; 33]).is_err());
    assert!(SecretKey::from_bytes(&[0; 32]).is_err());
}
