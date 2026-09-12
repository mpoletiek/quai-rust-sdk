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
