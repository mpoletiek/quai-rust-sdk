//! Published SDK Qi wire/signature vectors, mutation and scope tests.
use quai_consensus::{Denomination, QiTransaction, SignedQiTransaction, TransactionError};
use quai_crypto::{SchnorrSignature, SecretKey};
use serde_json::Value;
fn bytes(s: &str) -> Vec<u8> {
    s.strip_prefix("0x")
        .unwrap()
        .as_bytes()
        .chunks_exact(2)
        .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
        .collect()
}
fn vectors() -> Vec<Value> {
    serde_json::from_str::<Value>(include_str!(
        "fixtures/shared/compatibility/fixtures/transactions.json"
    ))
    .unwrap()["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["kind"] == "qi")
        .cloned()
        .collect()
}

#[test]
fn single_input_unsigned_signed_digest_and_id_match_reference() {
    for v in vectors() {
        let unsigned = bytes(v["unsigned"].as_str().unwrap());
        let tx = QiTransaction::decode_unsigned(&unsigned).unwrap();
        assert_eq!(tx.unsigned_bytes().unwrap(), unsigned);
        assert_eq!(tx.signing_digest().unwrap().to_string(), v["digest"]);
        let signature = SchnorrSignature::from_bytes(
            &bytes(v["input"]["signature"].as_str().unwrap())
                .try_into()
                .unwrap(),
        )
        .unwrap();
        if !tx.data.is_empty() {
            // Wire-only fixtures: the pinned node rejects data lengths 1 and 2.
            assert!(tx.attach_single_signature(signature).is_err());
            assert!(SignedQiTransaction::decode(&bytes(v["signed"].as_str().unwrap())).is_err());
            continue;
        }
        let signed = tx.attach_signature(signature).unwrap();
        assert_eq!(
            signed.signed_bytes().unwrap(),
            bytes(v["signed"].as_str().unwrap())
        );
        assert_eq!(signed.hash().unwrap().to_string(), v["hash"]);
        assert_eq!(
            SignedQiTransaction::decode(&signed.signed_bytes().unwrap())
                .unwrap()
                .transaction(),
            &tx
        );
        let raw_keys: Vec<_> = if let Some(keys) = v["publicTestSecrets"].as_array() {
            keys.iter().map(|k| k.as_str().unwrap()).collect()
        } else {
            vec![v["publicTestSecret"].as_str().unwrap()]
        };
        let keys: Vec<_> = raw_keys
            .iter()
            .map(|k| SecretKey::from_bytes(&bytes(k).try_into().unwrap()).unwrap())
            .collect();
        let randomized = tx.sign_local(&keys.iter().collect::<Vec<_>>()).unwrap();
        assert_eq!(
            SignedQiTransaction::decode(&randomized.signed_bytes().unwrap())
                .unwrap()
                .transaction(),
            &tx
        );
    }
}

#[test]
fn changing_authorized_payload_invalidates_qi_signature() {
    let v = &vectors()[0];
    let signed = SignedQiTransaction::decode(&bytes(v["signed"].as_str().unwrap())).unwrap();
    let mut tx = signed.transaction().clone();
    tx.data.push(7);
    assert!(tx.attach_single_signature(signed.signature()).is_err());
    let wrong = SecretKey::from_bytes(&[1; 32]).unwrap();
    assert!(tx.sign_single(&wrong).is_err());
}

#[test]
fn malformed_wire_duplicate_spends_and_output_reuse_are_rejected() {
    let v = &vectors()[0];
    let good = bytes(v["signed"].as_str().unwrap());
    for end in 0..good.len() {
        assert!(SignedQiTransaction::decode(&good[..end]).is_err());
    }
    let signed = SignedQiTransaction::decode(&good).unwrap();
    let mut tx = signed.transaction().clone();
    tx.inputs.push(tx.inputs[0].clone());
    assert!(tx.unsigned_bytes().is_err());
    let mut tx = signed.transaction().clone();
    tx.outputs.push(tx.outputs[0].clone());
    assert!(tx.unsigned_bytes().is_err());
    let mut tx = signed.transaction().clone();
    tx.outputs[0].address = tx.inputs[0].public_key.address();
    assert!(tx.unsigned_bytes().is_err());
    let mut tx = signed.transaction().clone();
    let mut h = tx.inputs[0].previous_output.transaction_hash.into_bytes();
    h[2] = 0x10;
    tx.inputs[0].previous_output.transaction_hash = h.into();
    assert!(tx.unsigned_bytes().is_err());
    let mut unknown = good.clone();
    unknown.extend([0xf8, 0x07, 0]);
    assert!(SignedQiTransaction::decode(&unknown).is_err());
    let mut tx = signed.transaction().clone();
    tx.inputs.clear();
    assert!(tx.unsigned_bytes().is_err());
}

#[test]
fn all_denomination_indices_have_exact_qit_values() {
    for i in 0..=255u8 {
        if i < 15 {
            let d = Denomination::new(i).unwrap();
            assert_eq!(d.index(), i);
            assert_eq!(d.value(), Denomination::VALUES[usize::from(i)]);
        } else {
            assert!(Denomination::new(i).is_err());
        }
    }
}

#[test]
fn nested_message_budget_precedes_protobuf_heap_allocation() {
    let count = quai_consensus::MAX_TRANSACTION_MESSAGES + 1;
    let mut raw = vec![0x82, 0x01]; // Transaction.tx_outs
    let mut len = count * 2;
    while len >= 128 {
        raw.push(((len & 127) as u8) | 128);
        len >>= 7;
    }
    raw.push(len as u8);
    for _ in 0..count {
        raw.extend([0x0a, 0]);
    } // Empty Output messages
    assert!(raw.len() < quai_consensus::MAX_TRANSACTION_BYTES);
    assert!(matches!(
        QiTransaction::decode_unsigned(&raw),
        Err(TransactionError::TooLarge)
    ));
}
