//! Captured mainnet headers recompute to the hashes the node reported, and a
//! change to any hashed field is caught.
use quai_primitives::Hash32;
use quai_provider::header_hash::{HeaderHashError, verify_header_hash};
use serde_json::{Value, json};

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "fixtures/shared/test-infra/fixtures/header-hashes-mainnet.json"
    ))
    .unwrap()
}
fn hash(value: &Value) -> Hash32 {
    value.as_str().unwrap().parse().unwrap()
}
/// Flip the last hex digit of a string field.
fn flip(value: &mut Value) {
    let text = value.as_str().unwrap().to_owned();
    let last = if text.ends_with('0') { '1' } else { '0' };
    *value = json!(format!("{}{last}", &text[..text.len() - 1]));
}

#[test]
fn every_captured_header_recomputes_its_header_hash() {
    let fixture = fixture();
    let v1 = fixture["v1"].as_array().unwrap();
    for header in v1 {
        let verified = verify_header_hash(header).unwrap();
        let work = &header["woHeader"];
        assert_eq!(verified.header_hash, hash(&work["headerHash"]));
        assert_eq!(verified.hash, hash(&work["hash"]));
        assert_eq!(verified.evm_root, hash(&header["evmRoot"]));
        assert_eq!(verified.utxo_root, hash(&header["utxoRoot"]));
        assert_eq!(verified.etx_set_root, hash(&header["etxSetRoot"]));
    }
    // Genesis and the two ProgPoW-era blocks recompute the block hash from v1
    // fields; the head is past the fork, where v1 omits the seal's share fields.
    let shape = v1
        .iter()
        .map(|h| {
            let v = verify_header_hash(h).unwrap();
            (v.number, v.seal_hash.is_some(), v.hash_verified)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        shape[..3],
        [
            (0, true, true),
            (1_048_576, true, true),
            (3_000_000, true, true)
        ]
    );
    assert!(!shape[3].1 && !shape[3].2);
}

#[test]
fn a_v2_work_object_recomputes_its_block_hash_through_the_auxpow() {
    let fixture = fixture();
    let work = &fixture["v2"];
    let verified = verify_header_hash(work).unwrap();
    assert_eq!(verified.header_hash, hash(&work["woHeader"]["headerHash"]));
    assert!(verified.seal_hash.is_some());
    assert!(verified.hash_verified);
    assert_eq!(verified.hash, hash(&work["woHeader"]["hash"]));
}

#[test]
fn a_changed_root_or_any_other_hashed_field_is_caught() {
    let fixture = fixture();
    let head = &fixture["v1"][3];
    // The forgery a hash-only comparison misses: evmRoot swapped, woHeader kept.
    let mut forged = head.clone();
    flip(&mut forged["evmRoot"]);
    assert_eq!(
        verify_header_hash(&forged),
        Err(HeaderHashError::HeaderHashMismatch)
    );
    for (key, value) in head.as_object().unwrap() {
        if matches!(key.as_str(), "woHeader" | "size") {
            continue;
        }
        let mut changed = head.clone();
        match value {
            Value::Array(_) => flip(&mut changed[key][0]),
            _ => flip(&mut changed[key]),
        }
        assert!(verify_header_hash(&changed).is_err(), "{key} is not hashed");
    }
    // `size` is reported beside the header, never hashed.
    let mut sized = head.clone();
    flip(&mut sized["size"]);
    assert!(verify_header_hash(&sized).is_ok());
}

#[test]
fn a_changed_seal_field_fails_where_the_block_hash_is_recomputed() {
    let fixture = fixture();
    let pre_fork = &fixture["v1"][2];
    for field in [
        "timestamp",
        "number",
        "parentHash",
        "primaryCoinbase",
        "nonce",
        "mixHash",
    ] {
        let mut changed = pre_fork.clone();
        flip(&mut changed["woHeader"][field]);
        assert_eq!(
            verify_header_hash(&changed),
            Err(HeaderHashError::HashMismatch),
            "{field}"
        );
    }
    // Past the fork a v1 view cannot recompute the block hash, so the seal
    // fields (height and time among them) are the node's report.
    let mut head = fixture["v1"][3].clone();
    flip(&mut head["woHeader"]["timestamp"]);
    assert!(verify_header_hash(&head).is_ok());

    let work = &fixture["v2"];
    let mut changed = work.clone();
    flip(&mut changed["woHeader"]["timestamp"]);
    assert_eq!(
        verify_header_hash(&changed),
        Err(HeaderHashError::SealNotCommitted)
    );
    let mut changed = work.clone();
    flip(&mut changed["woHeader"]["auxpow"]["signature"]);
    assert_eq!(
        verify_header_hash(&changed),
        Err(HeaderHashError::HashMismatch)
    );
}

#[test]
fn an_unknown_or_missing_field_fails_closed() {
    let fixture = fixture();
    let head = &fixture["v1"][3];
    let mut extended = head.clone();
    extended["futureRoot"] = json!(format!("0x{}", "11".repeat(32)));
    assert_eq!(
        verify_header_hash(&extended),
        Err(HeaderHashError::UnknownField("futureRoot".into()))
    );
    let mut extended = head.clone();
    extended["woHeader"]["futureSeal"] = json!("0x1");
    assert_eq!(
        verify_header_hash(&extended),
        Err(HeaderHashError::UnknownField("futureSeal".into()))
    );
    let mut missing = head.clone();
    missing.as_object_mut().unwrap().remove("utxoRoot");
    assert_eq!(
        verify_header_hash(&missing),
        Err(HeaderHashError::Missing("utxoRoot"))
    );
    let mut short = head.clone();
    short["manifestHash"].as_array_mut().unwrap().pop();
    assert_eq!(
        verify_header_hash(&short),
        Err(HeaderHashError::Malformed("manifestHash"))
    );
    for bad in [json!(null), json!([]), json!({"woHeader": 1})] {
        assert!(verify_header_hash(&bad).is_err());
    }
}
