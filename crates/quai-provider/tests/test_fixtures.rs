//! The `test-fixtures` constructors build each observation from outside the
//! crate, the way a consumer's tests do, and put every argument in its field.
#![cfg(feature = "test-fixtures")]
use quai_primitives::{Hash32, QiAddress};
use quai_provider::{
    AddressOutpoint, BlockReference, ConversionEffect, ConversionObservation,
    ConversionOriginObservation, ConversionSpendability, EtxExecutionObservation, EtxScanResult,
    Extensions, ExternalObservation, OutPoint, QiCreditObservation, Receipt, ReceiptOutcome,
    ScanCoverage, Transaction,
};
use quai_rpc::U256;
use serde_json::Value;

fn settlement() -> Value {
    serde_json::from_str(include_str!(
        "fixtures/shared/test-infra/local-chain/conversion-settlement-fixtures.json"
    ))
    .unwrap()
}
fn hash(byte: u8) -> Hash32 {
    Hash32::from_bytes([byte; 32])
}
fn block(number: u64, byte: u8) -> BlockReference {
    BlockReference {
        number,
        hash: hash(byte),
    }
}
fn outpoint() -> AddressOutpoint {
    AddressOutpoint::new(
        OutPoint {
            tx_hash: hash(4),
            index: 3,
        },
        6,
        U256::from(30),
        Extensions::default(),
    )
}
fn execution() -> EtxExecutionObservation {
    let fixture = &settlement()[0];
    EtxExecutionObservation::new(
        Transaction::try_from(fixture["transaction"].clone()).unwrap(),
        Some(Receipt::try_from(fixture["receipt"].clone()).unwrap()),
    )
}
fn scan() -> EtxScanResult {
    EtxScanResult::new(
        ScanCoverage::Unavailable { block_number: 26 },
        Some(block(25, 5)),
        7,
        Some(execution()),
    )
}

#[test]
fn address_outpoint_and_qi_credit_keep_each_argument_in_its_field() {
    let o = outpoint();
    assert_eq!((o.outpoint.tx_hash, o.outpoint.index), (hash(4), 3));
    assert_eq!((o.denomination, o.lock), (6, U256::from(30)));
    assert!(o.extensions.fields().is_empty());

    let beneficiary: QiAddress = "0x0080000000000000000000000000000000000001"
        .parse()
        .unwrap();
    let c = QiCreditObservation::new(
        beneficiary,
        hash(1),
        hash(2),
        block(20, 3),
        block(25, 5),
        vec![o.clone()],
        U256::from(100),
        U256::from(200),
        U256::from(300),
    );
    assert_eq!(c.beneficiary, beneficiary);
    assert_eq!((c.transaction_hash, c.creating_hash), (hash(1), hash(2)));
    assert_eq!((c.execution, c.head), (block(20, 3), block(25, 5)));
    assert_eq!(c.outputs, vec![o]);
    assert_eq!(
        (c.locked_qits, c.unlocked_qits, c.unobserved_qits),
        (U256::from(100), U256::from(200), U256::from(300))
    );
}

#[test]
fn scan_and_execution_keep_each_argument_in_its_field() {
    let fixture = &settlement()[0];
    let e = execution();
    assert_eq!(
        e.transaction.hash.to_string(),
        fixture["transaction"]["hash"]
    );
    assert_eq!(
        e.receipt.as_ref().unwrap().transaction_hash,
        e.transaction.hash
    );
    assert_eq!(
        EtxExecutionObservation::new(e.transaction.clone(), None).receipt,
        None
    );

    let s = scan();
    assert_eq!(s.coverage, ScanCoverage::Unavailable { block_number: 26 });
    assert_eq!(s.last_block, Some(block(25, 5)));
    assert_eq!(s.transactions_examined, 7);
    assert_eq!(s.execution, Some(e));
}

#[test]
fn conversion_and_external_observations_keep_each_argument_in_its_field() {
    let origin = ConversionOriginObservation::Failed {
        block: block(10, 1),
    };
    let c = ConversionObservation::new(
        origin.clone(),
        Some(scan()),
        Some(ConversionEffect::Locked { etx_type: 2 }),
        ConversionSpendability::Unverified,
    );
    assert_eq!(c.origin, origin);
    assert_eq!(c.scan, Some(scan()));
    assert_eq!(c.effect, Some(ConversionEffect::Locked { etx_type: 2 }));
    assert_eq!(c.spendability, ConversionSpendability::Unverified);

    let x = ExternalObservation::new(origin.clone(), None, Some(ReceiptOutcome::Locked));
    assert_eq!(x.origin, origin);
    assert_eq!(x.scan, None);
    assert_eq!(x.outcome, Some(ReceiptOutcome::Locked));
}
