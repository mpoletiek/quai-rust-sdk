#![no_main]
use libfuzzer_sys::fuzz_target;
use quai_consensus::{
    MAX_TRANSACTION_BYTES, QiConversionTransaction, QiTransaction, QiWrappingTransaction,
    QuaiToQiTransaction, QuaiTransaction, SignedQiConversionTransaction, SignedQiOperation,
    SignedQiTransaction, SignedQuaiTransaction,
};
fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_TRANSACTION_BYTES + 1 {
        return;
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
