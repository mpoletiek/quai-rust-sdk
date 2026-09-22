//! Independent cryptographic vectors and boundary checks.

use quai_crypto::{
    CryptoError, PublicKey, RecoverableSignature, SchnorrPublicKey, SchnorrSignature, SecretKey,
    hash_message, hmac_sha256, hmac_sha512, keccak256, sha256, sha512,
};
use serde_json::Value;

fn decode(input: &str) -> Vec<u8> {
    let input = input.strip_prefix("0x").unwrap_or(input);
    assert_eq!(input.len() % 2, 0);
    (0..input.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&input[i..i + 2], 16).unwrap())
        .collect()
}
fn fixed<const N: usize>(input: &str) -> [u8; N] {
    decode(input).try_into().unwrap()
}
fn one() -> SecretKey {
    let mut bytes = [0; 32];
    bytes[31] = 1;
    SecretKey::from_bytes(&bytes).unwrap()
}

#[test]
fn pinned_quais_ecdsa_and_public_key_vectors() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/quais-crypto.json")).unwrap();
    assert_eq!(fixture["reference"], "quais@1.0.0-alpha.57");
    let vectors = fixture["vectors"].as_array().unwrap();
    assert_eq!(vectors.len(), 24);
    for vector in vectors {
        let text = |key: &str| vector[key].as_str().unwrap();
        let key = SecretKey::from_bytes(&fixed(text("publicTestSecret"))).unwrap();
        let public = key.public_key();
        assert_eq!(public.to_compressed(), fixed::<33>(text("compressed")));
        assert_eq!(public.to_uncompressed(), fixed::<65>(text("uncompressed")));
        assert_eq!(public.address().to_checksum(), text("address"));
        assert_eq!(
            PublicKey::from_sec1_bytes(&public.to_compressed()).unwrap(),
            public
        );
        assert_eq!(
            PublicKey::from_sec1_bytes(&public.to_uncompressed()).unwrap(),
            public
        );
        let digest = fixed(text("digest"));
        let signature = key.sign_prehash(&digest).unwrap();
        assert_eq!(
            signature.to_quais_bytes().unwrap(),
            fixed::<65>(text("signature")),
            "digest {} for {}",
            text("digest"),
            text("address")
        );
        assert_eq!(signature, key.sign_prehash(&digest).unwrap());
        assert_eq!(signature.recover_prehash(&digest).unwrap(), public);
        signature.verify_prehash(&digest, &public).unwrap();
        assert_eq!(
            RecoverableSignature::from_quais_bytes(&signature.to_quais_bytes().unwrap()).unwrap(),
            signature
        );
        let mut parity_form = signature.to_quais_bytes().unwrap();
        parity_form[64] -= 27;
        assert_eq!(
            RecoverableSignature::from_quais_bytes(&parity_form).unwrap(),
            signature
        );
        assert_eq!(signature.r().as_slice(), &signature.to_compact()[..32]);
        assert_eq!(signature.s().as_slice(), &signature.to_compact()[32..]);
    }
}

#[test]
fn personal_message_hash_is_byte_exact_and_uses_ethereum_prefix() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/quais-crypto.json")).unwrap();
    for vector in fixture["messages"].as_array().unwrap() {
        let text = vector["input"].as_str().unwrap();
        let bytes = if vector["encoding"] == "utf8" {
            text.as_bytes().to_vec()
        } else {
            decode(text)
        };
        assert_eq!(
            hash_message(&bytes),
            fixed::<32>(vector["expected"].as_str().unwrap())
        );
    }
    assert_ne!(hash_message(b"0x4243"), hash_message(b"BC"));
    assert_ne!(hash_message(b""), keccak256(b"\x19Quai Signed Message:\n0"));
}

#[test]
fn official_bip340_verification_vectors() {
    let mut count = 0;
    for line in include_str!("fixtures/bip340.csv").lines().skip(1) {
        let fields: Vec<_> = line.split(',').collect();
        let public = SchnorrPublicKey::from_bytes(&fixed(fields[2]));
        let signature = SchnorrSignature::from_bytes(&fixed(fields[5]));
        let valid = match (public, signature) {
            (Ok(public), Ok(signature)) => public.verify(&decode(fields[4]), &signature).is_ok(),
            _ => false,
        };
        assert_eq!(valid, fields[6] == "TRUE", "BIP340 index {}", fields[0]);
        count += 1;
    }
    assert_eq!(count, 19);
}

#[test]
fn schnorr_os_randomized_signing_verifies_exact_bytes() {
    let key = one();
    let public = key.schnorr_public_key();
    for message in [
        &b""[..],
        &b"raw BIP340 message"[..],
        &[0; 32][..],
        &[0xff; 100][..],
    ] {
        let a = key.sign_schnorr(message).unwrap();
        let b = key.sign_schnorr(message).unwrap();
        public.verify(message, &a).unwrap();
        public.verify(message, &b).unwrap();
        assert_eq!(SchnorrSignature::from_bytes(&a.to_bytes()).unwrap(), a);
        assert_eq!(
            SchnorrPublicKey::from_bytes(&public.to_bytes()).unwrap(),
            public
        );
        assert!(public.verify(b"different message", &a).is_err());
    }
}

#[test]
fn rejects_secret_and_public_key_boundaries() {
    for bytes in [
        [0; 32],
        [0xff; 32],
        fixed("fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141"),
    ] {
        assert!(matches!(
            SecretKey::from_bytes(&bytes),
            Err(CryptoError::InvalidSecretKey)
        ));
    }
    assert!(
        SecretKey::from_bytes(&fixed(
            "fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364140"
        ))
        .is_ok()
    );
    for bytes in [
        vec![],
        vec![0],
        vec![0; 32],
        vec![2; 32],
        vec![4; 64],
        vec![4; 66],
        vec![2; 65],
        vec![0xff; 33],
        vec![0; 65],
    ] {
        assert_eq!(
            PublicKey::from_sec1_bytes(&bytes),
            Err(CryptoError::InvalidPublicKey)
        );
    }
    let mut off_curve = [0; 65];
    off_curve[0] = 4;
    assert_eq!(
        PublicKey::from_sec1_bytes(&off_curve),
        Err(CryptoError::InvalidPublicKey)
    );
    let mut hybrid = one().public_key().to_uncompressed();
    hybrid[0] = 6;
    assert_eq!(
        PublicKey::from_sec1_bytes(&hybrid),
        Err(CryptoError::InvalidPublicKey)
    );
    assert_eq!(
        SchnorrPublicKey::from_bytes(&[0xff; 32]),
        Err(CryptoError::InvalidPublicKey)
    );
}

#[test]
fn rejects_high_s_and_recovery_corruption() {
    let key = one();
    let digest = keccak256(b"test digest");
    let signature = key.sign_prehash(&digest).unwrap();
    assert!(
        signature
            .verify_prehash(&keccak256(b"wrong digest"), &key.public_key())
            .is_err()
    );
    let other_id = signature.recovery_id() ^ 1;
    let wrong = RecoverableSignature::from_compact(&signature.to_compact(), other_id).unwrap();
    assert!(wrong.verify_prehash(&digest, &key.public_key()).is_err());
    assert_eq!(
        RecoverableSignature::from_compact(&signature.to_compact(), 4),
        Err(CryptoError::InvalidRecoveryId)
    );
    assert_eq!(
        RecoverableSignature::from_compact(&[0; 64], 0),
        Err(CryptoError::InvalidSignature)
    );
    let mut high = signature.to_compact();
    high[32..].copy_from_slice(&fixed::<32>(
        "fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364140",
    ));
    assert_eq!(
        RecoverableSignature::from_compact(&high, 0),
        Err(CryptoError::HighS)
    );
    let reduced_x = RecoverableSignature::from_compact(&signature.to_compact(), 2).unwrap();
    assert_eq!(
        reduced_x.to_quais_bytes(),
        Err(CryptoError::UnsupportedRecoveryId)
    );
    for v in [2, 3, 26, 29, 35, 36, 255] {
        let mut bytes = signature.to_quais_bytes().unwrap();
        bytes[64] = v;
        assert_eq!(
            RecoverableSignature::from_quais_bytes(&bytes),
            Err(CryptoError::UnsupportedRecoveryId)
        );
    }
}

#[test]
fn standard_hash_and_rfc4231_hmac_vectors() {
    assert_eq!(
        keccak256(b""),
        fixed::<32>("c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470")
    );
    assert_eq!(
        sha256(b"abc"),
        fixed::<32>("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
    );
    assert_eq!(
        sha512(b"abc"),
        fixed::<64>(
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        )
    );
    assert_eq!(
        hmac_sha256(&[0x0b; 20], b"Hi There"),
        fixed::<32>("b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7")
    );
    assert_eq!(
        hmac_sha512(&[0x0b; 20], b"Hi There"),
        fixed::<64>(
            "87aa7cdea5ef619d4ff0b4241a1d6cb02379f4e2ce4ec2787ad0b30545e17cdedaa833b7d6b8a702038b274eaea3f4e4be9d914eeb61f1702e696c203a126854"
        )
    );
}

#[test]
fn secret_diagnostics_are_redacted_and_generated_keys_work() {
    fn assert_zeroizes<T: zeroize::ZeroizeOnDrop>() {}
    assert_zeroizes::<SecretKey>();
    assert_eq!(format!("{:?}", one()), "SecretKey([REDACTED])");
    let generated = SecretKey::generate().unwrap();
    let digest = [0x5a; 32];
    generated
        .sign_prehash(&digest)
        .unwrap()
        .verify_prehash(&digest, &generated.public_key())
        .unwrap();
}

#[test]
fn hmac_verification_rejects_changes() {
    let tag256 = hmac_sha256(b"public test key", b"message");
    quai_crypto::verify_hmac_sha256(b"public test key", b"message", &tag256).unwrap();
    assert!(quai_crypto::verify_hmac_sha256(b"different key", b"message", &tag256).is_err());
    let mut modified = tag256;
    modified[31] ^= 1;
    assert!(quai_crypto::verify_hmac_sha256(b"public test key", b"message", &modified).is_err());
    let tag512 = hmac_sha512(b"public test key", b"message");
    quai_crypto::verify_hmac_sha512(b"public test key", b"message", &tag512).unwrap();
    assert!(quai_crypto::verify_hmac_sha512(b"public test key", b"different", &tag512).is_err());
}

#[test]
fn explicit_export_is_guarded_and_key_diagnostics_remain_redacted() {
    use zeroize::Zeroize;
    let mut bytes = [0; 32];
    bytes[31] = 1;
    let key = quai_crypto::SecretKey::from_bytes(&bytes).unwrap();
    let before = key.public_key();
    let mut exported = key.export_bytes();
    assert_eq!(format!("{exported:?}"), "SecretBytes([REDACTED])");
    assert_eq!(*exported, bytes);
    exported.zeroize();
    assert_eq!(*exported, [0; 32]);
    assert_eq!(key.public_key(), before);
    assert_eq!(format!("{key:?}"), "SecretKey([REDACTED])");
}

#[test]
fn guarded_ecdh_and_additive_tweaks_match_known_curve_points() {
    let key = |n: u8| {
        let mut bytes = [0; 32];
        bytes[31] = n;
        quai_crypto::SecretKey::from_bytes(&bytes).unwrap()
    };
    let one = key(1);
    let two = key(2);
    let three = key(3);
    let x = one.ecdh_shared_x(&two.public_key());
    assert_eq!(x.as_slice(), &two.public_key().to_compressed()[1..]);
    assert_eq!(*x, *two.ecdh_shared_x(&one.public_key()));
    assert_eq!(
        one.add_tweak(&two).unwrap().public_key(),
        three.public_key()
    );
    assert_eq!(
        one.public_key().add_tweak(&two).unwrap(),
        three.public_key()
    );
    let order_minus_one = [
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xfe, 0xba, 0xae, 0xdc, 0xe6, 0xaf, 0x48, 0xa0, 0x3b, 0xbf, 0xd2, 0x5e, 0x8c, 0xd0, 0x36,
        0x41, 0x40,
    ];
    let minus_one = quai_crypto::SecretKey::from_bytes(&order_minus_one).unwrap();
    assert!(minus_one.add_tweak(&one).is_err());
    assert!(minus_one.public_key().add_tweak(&one).is_err());
}

#[test]
fn a_digest_above_the_group_order_signs_as_noble_and_recovers_the_signer() {
    // noble-curves `secp256k1.sign("ff".repeat(32), "07".repeat(32), {lowS: true})`,
    // the backend quais uses. The digest exceeds n, so RFC 6979 reduces it;
    // signing now also recovers the signer before returning.
    let key = SecretKey::from_bytes(&[7; 32]).unwrap();
    let digest = [0xff; 32];
    let signature = key.sign_prehash(&digest).unwrap();
    let hex: String = signature
        .to_compact()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(
        hex,
        "62776b3e9ff45aa5f72b4d941f7a7c3f40a2342a2551eaee384ae32df1fc2d5c\
         53215fe89f507446b72bdbd2ceae1c256ea7c76d04203f6f12bdf870c6be11a1"
    );
    assert_eq!(signature.recovery_id(), 1);
    assert_eq!(
        signature.recover_prehash(&digest).unwrap(),
        key.public_key()
    );
}
