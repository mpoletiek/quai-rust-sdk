//! Verify signed custody from node RPC fields, including retained mainnet records.
use quai_primitives::Hash32;
use quai_provider::{Transaction, TransactionDetails};
use quai_rpc::U256;
use serde_json::{Value, json};
fn fixtures() -> Vec<Value> {
    serde_json::from_str::<Value>(include_str!("fixtures/lan-mainnet.json")).unwrap()["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["transaction"].clone())
        .collect()
}
#[test]
fn captured_mainnet_account_transaction_reconstructs_and_verifies_exact_identity() {
    let mut verified = 0;
    for value in fixtures() {
        let tx = Transaction::try_from(value).unwrap();
        if matches!(tx.details, TransactionDetails::Quai(_)) {
            let signed = tx.verified_quai().unwrap();
            assert_eq!(signed.hash().unwrap(), tx.hash);
            assert_eq!(
                quai_consensus::SignedQuaiTransaction::decode(&signed.signed_bytes().unwrap())
                    .unwrap()
                    .hash()
                    .unwrap(),
                tx.hash
            );
            verified += 1;
        } else {
            assert!(tx.verified_quai().is_err());
        }
    }
    assert!(verified > 0);
}
#[test]
fn changed_rpc_fields_signature_and_oversized_metadata_cannot_be_verified() {
    let value = fixtures().into_iter().find(|v| v["type"] == "0x0").unwrap();
    for field in [
        "nonce", "gas", "gasPrice", "value", "chainId", "r", "s", "v",
    ] {
        let mut changed = value.clone();
        changed[field] = if changed[field] == "0x0" {
            json!("0x1")
        } else {
            json!("0x0")
        };
        assert!(
            Transaction::try_from(changed)
                .unwrap()
                .verified_quai()
                .is_err(),
            "{field}"
        );
    }
    for (field, new) in [
        ("hash", json!(Hash32::ZERO.to_string())),
        ("input", json!("0x0001")),
        ("to", Value::Null),
        ("from", json!("0x0049cda3305ccb9cb23e7ce2528cef555e9a5b32")),
        ("v", json!("0x1b")),
    ] {
        let mut changed = value.clone();
        changed[field] = new;
        assert!(
            Transaction::try_from(changed)
                .unwrap()
                .verified_quai()
                .is_err(),
            "{field}"
        );
    }
    let mut oversized = Transaction::try_from(value).unwrap();
    let TransactionDetails::Quai(fields) = &mut oversized.details else {
        unreachable!()
    };
    fields.access_list = vec![quai_provider::AccessListItem {
        address: fields.from.address(),
        storage_keys: vec![Hash32::ZERO; quai_consensus::MAX_TRANSACTION_MESSAGES],
    }];
    assert!(oversized.verified_quai().is_err());
    if let TransactionDetails::Quai(fields) = &mut oversized.details {
        fields.signature.v = U256::MAX;
    }
    assert!(oversized.verified_quai().is_err());
}
