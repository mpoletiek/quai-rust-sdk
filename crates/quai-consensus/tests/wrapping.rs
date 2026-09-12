//! Independent wrapping wire/signature compatibility and hostile-shape regressions.
use quai_consensus::{
    QiConversionTransaction, QiWrappingTransaction, SignedQiOperation, SignedQiTransaction,
    SignedQiWrappingTransaction,
};
use quai_crypto::SecretKey;
use serde_json::Value;
fn bytes(value: &str) -> Vec<u8> {
    value
        .strip_prefix("0x")
        .unwrap()
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
#[test]
fn wrapping_matches_pinned_js_for_single_ordered_reversed_and_repeated_keys() {
    let fixture: Value = serde_json::from_str(include_str!("wrapping-vectors.json")).unwrap();
    for vector in fixture["vectors"].as_array().unwrap() {
        let unsigned = bytes(vector["unsigned"].as_str().unwrap());
        let wire = bytes(vector["signed"].as_str().unwrap());
        let tx = QiWrappingTransaction::decode_unsigned(&unsigned).unwrap();
        assert_eq!(tx.signing_digest().unwrap().to_string(), vector["digest"]);
        assert_eq!(tx.unsigned_bytes().unwrap(), unsigned);
        let signed = SignedQiWrappingTransaction::decode(&wire).unwrap();
        assert_eq!(signed.hash().unwrap().to_string(), vector["hash"]);
        assert_eq!(signed.signed_bytes().unwrap(), wire);
        assert!(matches!(
            SignedQiOperation::decode(&wire).unwrap(),
            SignedQiOperation::Wrapping(_)
        ));
        assert!(SignedQiTransaction::decode(&wire).is_err());
        assert!(QiConversionTransaction::decode_unsigned(&unsigned).is_err());
        let keys: Vec<_> = vector["publicTestSecrets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|key| {
                SecretKey::from_bytes(&bytes(key.as_str().unwrap()).try_into().unwrap()).unwrap()
            })
            .collect();
        let local = tx.sign_local(&keys.iter().collect::<Vec<_>>()).unwrap();
        assert_eq!(
            SignedQiWrappingTransaction::decode(&local.signed_bytes().unwrap())
                .unwrap()
                .transaction(),
            &tx
        );
        for length in [0, 19, 21, 22] {
            let mut bad = tx.transaction().clone();
            bad.data.resize(length, 0);
            assert!(QiWrappingTransaction::from_transaction(bad).is_err());
        }
        let mut bad = tx.transaction().clone();
        bad.data[1] |= 0x80;
        assert!(QiWrappingTransaction::from_transaction(bad).is_err());
        let mut bad = tx.transaction().clone();
        bad.data[0] = 0x10;
        assert!(QiWrappingTransaction::from_transaction(bad).is_err());
        let mut bad = tx.transaction().clone();
        bad.outputs[1].address = "0x0000000000000000000000000000000000000001"
            .parse()
            .unwrap();
        assert!(QiWrappingTransaction::from_transaction(bad).is_err());
        let mut corrupt = wire.clone();
        *corrupt.last_mut().unwrap() ^= 1;
        assert!(SignedQiWrappingTransaction::decode(&corrupt).is_err());
    }
}
