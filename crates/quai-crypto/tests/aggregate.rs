//! Ordered local aggregation vectors independently verified with JS and Go.
use quai_crypto::{
    CryptoError, MAX_AGGREGATE_KEYS, OrderedKeyAggregate, PublicKey, SchnorrSignature, SecretKey,
};
use serde_json::Value;

fn decode(value: &str) -> Vec<u8> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).unwrap())
        .collect()
}
fn key(n: u8) -> SecretKey {
    let mut bytes = [0; 32];
    bytes[31] = n;
    SecretKey::from_bytes(&bytes).unwrap()
}

#[test]
fn ordered_duplicate_key_and_signature_vectors_match_js_and_go() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/ordered-musig.json")).unwrap();
    let vectors = fixture["vectors"].as_array().unwrap();
    assert_eq!(vectors.len(), 11);
    for vector in vectors {
        let secrets: Vec<_> = vector["publicTestSecrets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                SecretKey::from_bytes(&decode(s.as_str().unwrap()).try_into().unwrap()).unwrap()
            })
            .collect();
        let publics: Vec<_> = vector["publicKeys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| PublicKey::from_sec1_bytes(&decode(p.as_str().unwrap())).unwrap())
            .collect();
        let context = OrderedKeyAggregate::new(&publics).unwrap();
        assert_eq!(
            context.public_key().to_compressed().as_slice(),
            decode(vector["aggregatePublicKey"].as_str().unwrap())
        );
        let digest: [u8; 32] = decode(vector["digest"].as_str().unwrap())
            .try_into()
            .unwrap();
        let signature = SchnorrSignature::from_bytes(
            &decode(vector["jsSignature"].as_str().unwrap())
                .try_into()
                .unwrap(),
        )
        .unwrap();
        context.verify_prehash(&digest, &signature).unwrap();
        let signature = context
            .sign_local(&secrets.iter().collect::<Vec<_>>(), &digest)
            .unwrap();
        context.verify_prehash(&digest, &signature).unwrap();
        assert!(context.verify_prehash(&[0; 32], &signature).is_err());
        let mut altered = signature.to_bytes();
        altered[63] ^= 1;
        if let Ok(altered) = SchnorrSignature::from_bytes(&altered) {
            assert!(context.verify_prehash(&digest, &altered).is_err());
        }
    }
}

#[test]
fn rejects_missing_extra_wrong_and_reordered_secrets() {
    let a = key(1);
    let b = key(2);
    let c = key(3);
    let context = OrderedKeyAggregate::new(&[a.public_key(), b.public_key()]).unwrap();
    for keys in [vec![&a], vec![&a, &b, &c], vec![&a, &c], vec![&b, &a]] {
        assert_eq!(
            context.sign_local(&keys, &[0; 32]),
            Err(CryptoError::InvalidSecretKeySet)
        );
    }
    let reverse = OrderedKeyAggregate::new(&[b.public_key(), a.public_key()]).unwrap();
    assert_ne!(context.public_key(), reverse.public_key());
    let signature = context.sign_local(&[&a, &b], &[0; 32]).unwrap();
    assert!(reverse.verify_prehash(&[0; 32], &signature).is_err());
}

#[test]
fn rejects_unsupported_counts_before_backend_panics_or_allocates() {
    let public = key(1).public_key();
    for keys in [vec![], vec![public], vec![public; MAX_AGGREGATE_KEYS + 1]] {
        assert!(matches!(
            OrderedKeyAggregate::new(&keys),
            Err(CryptoError::InvalidKeyCount)
        ));
    }
}
