//! Network binding, watch-only rejection, and personal-message recovery.
use quai_consensus::{QuaiTransaction, U256};
use quai_crypto::{SecretKey, hash_message};
use quai_signer::{DomainPolicy, LocalSigner, Signer, SignerError, TypedData, WatchOnlySigner};

fn signer() -> LocalSigner {
    let mut key = [0u8; 32];
    key[30] = 3;
    key[31] = 0x25; // Public pinned test key, never fund.
    LocalSigner::new(SecretKey::from_bytes(&key).unwrap(), U256::from(15000)).unwrap()
}
fn transaction() -> QuaiTransaction {
    QuaiTransaction {
        chain_id: U256::from(15000),
        nonce: 0,
        to: Some(
            "0x0011223344556677889900112233445566778899"
                .parse()
                .unwrap(),
        ),
        value: U256::from(1),
        gas_limit: 21000,
        gas_price: U256::from(1000000000),
        data: vec![],
        access_list: vec![],
    }
}
#[test]
fn transaction_cannot_escape_signer_network() {
    let signer = signer();
    let mut tx = transaction();
    let signed = signer.sign_quai(&tx).unwrap();
    assert_eq!(signed.from().address(), signer.address());
    tx.chain_id = U256::from(9);
    assert!(matches!(
        signer.sign_quai(&tx),
        Err(SignerError::ChainMismatch)
    ));
}
#[test]
fn watch_only_never_signs_and_retains_public_identity() {
    let local = signer();
    let watch = WatchOnlySigner::new(local.address(), local.chain_id()).unwrap();
    assert_eq!(watch.address(), local.address());
    assert!(matches!(
        watch.sign_quai(&transaction()),
        Err(SignerError::WatchOnly)
    ));
    assert!(matches!(
        watch.sign_message(b"hello"),
        Err(SignerError::WatchOnly)
    ));
    assert!(WatchOnlySigner::new(local.address(), U256::ZERO).is_err());
}
#[test]
fn personal_message_recovers_signer_without_leaking_key_in_debug() {
    let local = signer();
    let message = "Quai wallet: café 🐬".as_bytes();
    let signature = local.sign_message(message).unwrap();
    assert_eq!(
        signature
            .recover_prehash(&hash_message(message))
            .unwrap()
            .address(),
        local.address()
    );
    let debug = format!("{local:?}");
    assert!(!debug.contains("0000000000000325"));
    assert!(debug.contains("address"));
}

#[test]
fn typed_data_requires_explicit_chain_policy_and_cannot_escape_network() {
    let local = signer();
    let make = |domain| {
        TypedData::from_json(
            &serde_json::to_vec(&serde_json::json!({
                "types":{"Transfer":[{"name":"amount","type":"uint256"}]},
                "primaryType":"Transfer","domain":domain,"message":{"amount":"1000"}
            }))
            .unwrap(),
        )
        .unwrap()
    };
    for chain in [
        serde_json::json!(15000),
        serde_json::json!("15000"),
        serde_json::json!("0x3a98"),
    ] {
        let data = make(serde_json::json!({"name":"Quai SDK test","chainId":chain}));
        let signature = local
            .sign_typed_data(&data, DomainPolicy::RequireChainId)
            .unwrap();
        assert_eq!(
            signature
                .recover_prehash(data.signing_hash().bytes())
                .unwrap()
                .address(),
            local.address()
        );
        let watch = WatchOnlySigner::new(local.address(), local.chain_id()).unwrap();
        assert!(matches!(
            watch.sign_typed_data(&data, DomainPolicy::AllowUnbound),
            Err(SignerError::WatchOnly)
        ));
    }
    let wrong = make(serde_json::json!({"chainId":9}));
    for policy in [DomainPolicy::RequireChainId, DomainPolicy::AllowUnbound] {
        assert!(matches!(
            local.sign_typed_data(&wrong, policy),
            Err(SignerError::ChainMismatch)
        ));
    }
    for domain in [serde_json::json!({}), serde_json::json!({"chainId":null})] {
        let unbound = make(domain);
        assert!(matches!(
            local.sign_typed_data(&unbound, DomainPolicy::RequireChainId),
            Err(SignerError::ChainMismatch)
        ));
        assert!(
            local
                .sign_typed_data(&unbound, DomainPolicy::AllowUnbound)
                .is_ok()
        );
    }
}

#[test]
fn qi_messages_match_published_wallet_and_bind_full_public_key() {
    use quai_crypto::{PublicKey, SchnorrSignature, keccak256};
    use quai_primitives::QiAddress;
    use quai_signer::verify_qi_message;
    fn bytes(text: &str) -> Vec<u8> {
        let text = text.strip_prefix("0x").unwrap();
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "fixtures/shared/compatibility/fixtures/qi-messages.json"
    ))
    .unwrap();
    for case in fixture["vectors"].as_array().unwrap() {
        let secret: [u8; 32] = bytes(case["privateKey"].as_str().unwrap())
            .try_into()
            .unwrap();
        let signer =
            LocalSigner::new(SecretKey::from_bytes(&secret).unwrap(), U256::from(15000)).unwrap();
        let address: QiAddress = case["address"].as_str().unwrap().parse().unwrap();
        let public_key =
            PublicKey::from_sec1_bytes(&bytes(case["publicKey"].as_str().unwrap())).unwrap();
        assert_eq!(public_key, signer.public_key());
        let message = bytes(case["message"].as_str().unwrap());
        assert_eq!(
            keccak256(&message).as_slice(),
            bytes(case["digest"].as_str().unwrap())
        );
        let signature = SchnorrSignature::from_bytes(
            &bytes(case["signature"].as_str().unwrap())
                .try_into()
                .unwrap(),
        )
        .unwrap();
        verify_qi_message(address, &public_key, &message, &signature).unwrap();
        let signed = signer.sign_qi_message(&message).unwrap();
        verify_qi_message(address, &public_key, &message, &signed).unwrap();
        let mut modified = message.clone();
        modified.push(0);
        assert!(verify_qi_message(address, &public_key, &modified, &signature).is_err());
        assert!(
            verify_qi_message(address, &public_key, &hash_message(&message), &signature).is_err()
        );
        // Negating a point preserves the x-only verifier, but changes its address.
        let mut opposite = public_key.to_compressed();
        opposite[0] ^= 1;
        let opposite = PublicKey::from_sec1_bytes(&opposite).unwrap();
        assert!(matches!(
            verify_qi_message(address, &opposite, &message, &signature),
            Err(SignerError::InvalidAddress)
        ));
        let watch = WatchOnlySigner::new(address.address(), U256::from(15000)).unwrap();
        assert!(matches!(
            watch.sign_qi_message(&message),
            Err(SignerError::WatchOnly)
        ));
    }
    assert!(matches!(
        signer().sign_qi_message(b"hello"),
        Err(SignerError::InvalidAddress)
    ));
}

#[test]
fn a_qi_message_that_is_an_unsigned_spend_is_refused() {
    // Message and single-input spend signatures are both BIP340 over Keccak of
    // the raw bytes, so signing a transaction's unsigned bytes "as a message"
    // would authorize that transaction.
    use quai_consensus::{Denomination, OutPoint, QiInput, QiOutput, QiTransaction};
    use quai_primitives::{Hash32, Ledger, Zone};
    use quai_signer::sign_qi_message;
    let qi_key = |seed: u8| {
        (1u32..)
            .find_map(|n| {
                let mut b = [seed; 32];
                b[28..].copy_from_slice(&n.to_be_bytes());
                let key = SecretKey::from_bytes(&b).ok()?;
                let address = key.public_key().address();
                (address.ledger() == Ledger::Qi && address.zone() == Ok(Zone::Cyprus1))
                    .then_some(key)
            })
            .unwrap()
    };
    let (owner, recipient) = (qi_key(7), qi_key(9));
    let mut hash = [0x11; 32];
    hash[0] = Zone::Cyprus1.byte();
    hash[2] = Zone::Cyprus1.byte();
    let spend = QiTransaction {
        chain_id: U256::from(9000),
        inputs: vec![QiInput {
            previous_output: OutPoint {
                transaction_hash: Hash32::from_bytes(hash),
                index: 0,
            },
            public_key: owner.public_key(),
        }],
        outputs: vec![QiOutput {
            address: recipient.public_key().address(),
            denomination: Denomination::new(10).unwrap(),
        }],
        data: vec![],
    };
    let unsigned = spend.unsigned_bytes().unwrap();
    // Guard the premise: the transaction really is signable with this key.
    assert!(spend.sign_single(&owner).is_ok());
    assert!(matches!(
        sign_qi_message(&owner, &unsigned),
        Err(SignerError::QiTransactionMessage)
    ));
    // Ordinary messages, including empty and protobuf-looking ones, still sign.
    for message in [&b""[..], b"hello", b"z", &unsigned[1..]] {
        assert!(sign_qi_message(&owner, message).is_ok(), "{message:?}");
    }
}
