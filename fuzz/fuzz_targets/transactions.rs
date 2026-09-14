#![no_main]
use libfuzzer_sys::fuzz_target;
use quai_consensus::{
    MAX_TRANSACTION_BYTES, QiConversionTransaction, QiTransaction, QiWrappingTransaction,
    QuaiToQiTransaction, QuaiTransaction, SignedQiConversionTransaction, SignedQiOperation,
    SignedQiTransaction, SignedQuaiTransaction,
};
use quai_consensus::document::{TransactionDocument, access_list_from_json, access_list_to_json};
fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_TRANSACTION_BYTES + 1 {
        return;
    }
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(data) {
        if let Ok(tx) = TransactionDocument::from_json(&value) {
            let exported = tx.to_json().unwrap();
            assert_eq!(TransactionDocument::from_json(&exported).unwrap().to_bytes().unwrap(), tx.to_bytes().unwrap());
            let _ = tx.origin_zone().unwrap(); let _ = tx.destination_zones(); let _ = tx.has_cross_zone_outputs();
        }
        if let Ok(list) = access_list_from_json(&value) {
            assert_eq!(access_list_from_json(&access_list_to_json(&list).unwrap()).unwrap(), list);
        }
        if let Ok(tx) = quai_provider::Transaction::try_from(value.clone()) {
            let exported = tx.to_rpc_json().unwrap();
            assert_eq!(quai_provider::Transaction::try_from(exported).unwrap(), tx);
        }
        if let Ok(receipt) = quai_provider::Receipt::try_from(value.clone()) {
            let exported = receipt.to_rpc_json().unwrap();
            assert_eq!(quai_provider::Receipt::try_from(exported).unwrap(), receipt);
            let _ = receipt.fee();
        }
        if let Ok(log) = quai_provider::Log::try_from(value) {
            let exported = log.to_rpc_json().unwrap();
            assert_eq!(quai_provider::Log::try_from(exported).unwrap(), log);
        }
    }
    if let Ok(proto) = quai_consensus::decode_proto_transaction(data) {
        assert_eq!(quai_consensus::encode_proto_transaction(&proto).unwrap(), data);
    }
    if let Ok(tx) = TransactionDocument::decode(data) {
        assert_eq!(tx.to_bytes().unwrap(), data);
        assert_eq!(TransactionDocument::from_json(&tx.to_json().unwrap()).unwrap().to_bytes().unwrap(), data);
        let proto = tx.to_proto(false).unwrap();
        assert!(!TransactionDocument::from_proto(&proto).unwrap().is_signed());
    }
    if let Ok(tx) = QuaiTransaction::decode_unsigned(data) {
        assert_eq!(tx.unsigned_bytes().unwrap(), data);
    }
    if let Ok(tx) = QiTransaction::decode_unsigned(data) {
        assert_eq!(tx.unsigned_bytes().unwrap(), data);
    }
    if let Ok(tx) = SignedQuaiTransaction::decode(data) {
        assert_eq!(tx.signed_bytes().unwrap(), data);
        tx.hash().unwrap();
    }
    if let Ok(tx) = SignedQiTransaction::decode(data) {
        assert_eq!(tx.signed_bytes().unwrap(), data);
        tx.hash().unwrap();
    }
    if let Ok(tx) = QiConversionTransaction::decode_unsigned(data) {
        assert_eq!(tx.unsigned_bytes().unwrap(), data);
    }
    if let Ok(tx) = SignedQiConversionTransaction::decode(data) {
        assert_eq!(tx.signed_bytes().unwrap(), data);
        tx.hash().unwrap();
    }
    if let Ok(tx) = QiWrappingTransaction::decode_unsigned(data) {
        assert_eq!(tx.unsigned_bytes().unwrap(), data);
    }
    if let Ok(tx) = SignedQiOperation::decode(data) {
        assert_eq!(tx.signed_bytes().unwrap(), data);
        tx.hash().unwrap();
    }
    if let Ok(tx) = QuaiToQiTransaction::decode_unsigned(data) {
        assert_eq!(tx.unsigned_bytes().unwrap(), data);
    }
});
