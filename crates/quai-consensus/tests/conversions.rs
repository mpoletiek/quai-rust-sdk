//! Explicit conversion vectors and static-policy boundaries; no node acceptance claims.
use quai_consensus::{
    ConversionSlippage, Denomination, MIN_QUAI_CONVERSION_VALUE, QiConversionTransaction,
    QiTransaction, QuaiToQiTransaction, QuaiTransaction, SignedQiConversionTransaction,
    SignedQiTransaction, SignedQuaiTransaction, U256,
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
fn vectors() -> Vec<Value> {
    serde_json::from_str::<Value>(include_str!("conversion-vectors.json")).unwrap()["vectors"]
        .as_array()
        .unwrap()
        .clone()
}
fn qi() -> QiConversionTransaction {
    let vector = &vectors()[0];
    QiConversionTransaction::decode_unsigned(&bytes(vector["unsigned"].as_str().unwrap())).unwrap()
}
fn keys(vector: &Value) -> Vec<SecretKey> {
    vector["publicTestSecrets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|key| {
            SecretKey::from_bytes(&bytes(key.as_str().unwrap()).try_into().unwrap()).unwrap()
        })
        .collect()
}
#[test]
fn pinned_js_conversion_wire_hash_signature_and_intent_match_for_all_local_key_forms() {
    for vector in vectors() {
        let unsigned = bytes(vector["unsigned"].as_str().unwrap());
        let wire = bytes(vector["signed"].as_str().unwrap());
        if vector["kind"] == "qi" {
            let tx = QiConversionTransaction::decode_unsigned(&unsigned).unwrap();
            assert_eq!(tx.unsigned_bytes().unwrap(), unsigned);
            assert_eq!(tx.signing_digest().unwrap().to_string(), vector["digest"]);
            assert_eq!(
                tx.intent().slippage.value(),
                vector["intent"]["slippage"].as_u64().unwrap() as u16
            );
            assert_eq!(tx.intent().refund.to_string(), vector["intent"]["refund"]);
            assert_eq!(
                tx.intent().destination.to_string(),
                vector["intent"]["destination"]
            );
            let signed = SignedQiConversionTransaction::decode(&wire).unwrap();
            assert_eq!(signed.transaction(), &tx);
            assert_eq!(signed.signed_bytes().unwrap(), wire);
            assert_eq!(signed.hash().unwrap().to_string(), vector["hash"]);
            let local_keys = keys(&vector);
            let local = tx
                .sign_local(&local_keys.iter().collect::<Vec<_>>())
                .unwrap();
            assert_eq!(
                SignedQiConversionTransaction::decode(&local.signed_bytes().unwrap())
                    .unwrap()
                    .transaction(),
                &tx
            );
            if local_keys.len() == 1 {
                tx.sign_single(&local_keys[0]).unwrap();
            } else {
                assert!(tx.sign_single(&local_keys[0]).is_err());
            }
            assert!(QiTransaction::decode_unsigned(&unsigned).is_err());
            assert!(SignedQiTransaction::decode(&wire).is_err());
            assert!(
                tx.transaction()
                    .sign_local(&local_keys.iter().collect::<Vec<_>>())
                    .is_err()
            );
            let rebuilt = QiConversionTransaction::new(
                tx.chain_id(),
                tx.transaction().inputs.clone(),
                vec![Denomination::new(2).unwrap(), Denomination::new(1).unwrap()],
                vec![tx.transaction().outputs[2].clone()],
                tx.intent(),
            )
            .unwrap();
            assert_eq!(rebuilt, tx);
        } else {
            let tx = QuaiToQiTransaction::decode_unsigned(&unsigned).unwrap();
            assert_eq!(tx.signing_digest().unwrap().to_string(), vector["digest"]);
            let key = SecretKey::from_bytes(
                &bytes(vector["publicTestSecret"].as_str().unwrap())
                    .try_into()
                    .unwrap(),
            )
            .unwrap();
            let signed = tx.sign(&key).unwrap();
            assert_eq!(signed.signed_bytes().unwrap(), wire);
            assert_eq!(signed.hash().unwrap().to_string(), vector["hash"]);
            assert_eq!(
                SignedQuaiTransaction::decode(&wire).unwrap().transaction(),
                tx.transaction()
            );
        }
    }
}
#[test]
fn conversion_static_rules_reject_wrong_data_scope_destinations_and_reuse() {
    let original = qi().transaction().clone();
    for len in [0, 1, 2, 20, 21, 23, 100] {
        let mut tx = original.clone();
        tx.data.resize(len, 0);
        assert!(QiConversionTransaction::from_transaction(tx).is_err());
    }
    for slippage in [0u16, 1, 29, 9001, u16::MAX] {
        assert!(ConversionSlippage::new(slippage).is_err());
        let mut tx = original.clone();
        tx.data[..2].copy_from_slice(&slippage.to_be_bytes());
        assert!(QiConversionTransaction::from_transaction(tx).is_err());
    }
    for valid in [30, 31, 8999, 9000] {
        assert_eq!(ConversionSlippage::new(valid).unwrap().value(), valid);
    }
    let mut tx = original.clone();
    tx.chain_id = U256::ZERO;
    assert!(QiConversionTransaction::from_transaction(tx).is_err());
    let mut tx = original.clone();
    tx.outputs[1].address = "0x0011223344556677889900112233445566778899"
        .parse()
        .unwrap();
    assert!(QiConversionTransaction::from_transaction(tx).is_err());
    let mut tx = original.clone();
    tx.outputs
        .retain(|output| output.address.ledger() == quai_primitives::Ledger::Qi);
    assert!(QiConversionTransaction::from_transaction(tx).is_err());
    let mut tx = original.clone();
    tx.data[3] &= 0x7f;
    assert!(QiConversionTransaction::from_transaction(tx).is_err());
    let mut tx = original.clone();
    tx.data[2] = 0x10;
    assert!(QiConversionTransaction::from_transaction(tx).is_err());
    let mut tx = original.clone();
    tx.outputs[2].address = "0x0188223344556677889900112233445566778877"
        .parse()
        .unwrap();
    assert!(QiConversionTransaction::from_transaction(tx).is_err());
    let mut tx = original.clone();
    tx.outputs[2].address = tx.inputs[0].public_key.address();
    assert!(QiConversionTransaction::from_transaction(tx).is_err());
    let mut tx = original.clone();
    tx.outputs.push(tx.outputs[2].clone());
    assert!(QiConversionTransaction::from_transaction(tx).is_err());
    let mut tx = original.clone();
    tx.inputs.push(tx.inputs[0].clone());
    assert!(QiConversionTransaction::from_transaction(tx).is_err());
    let mut tx = original.clone();
    tx.outputs[0].address = "0x0111223344556677889900112233445566778899"
        .parse()
        .unwrap();
    tx.outputs[1].address = tx.outputs[0].address;
    assert!(QiConversionTransaction::from_transaction(tx).is_err());
}
#[test]
fn signed_mutations_and_wrong_order_keys_fail_without_changing_authorized_bytes() {
    for vector in vectors()
        .into_iter()
        .filter(|vector| vector["kind"] == "qi")
    {
        let wire = bytes(vector["signed"].as_str().unwrap());
        let signed = SignedQiConversionTransaction::decode(&wire).unwrap();
        for end in 0..wire.len() {
            assert!(SignedQiConversionTransaction::decode(&wire[..end]).is_err());
        }
        let mut altered = wire.clone();
        *altered.last_mut().unwrap() ^= 1;
        assert!(SignedQiConversionTransaction::decode(&altered).is_err());
        let mut unknown = wire.clone();
        unknown.extend_from_slice(&[0xf8, 0x07, 0]);
        assert!(SignedQiConversionTransaction::decode(&unknown).is_err());
        let mut tx = signed.transaction().transaction().clone();
        tx.data[21] ^= 1;
        assert!(
            QiConversionTransaction::from_transaction(tx)
                .unwrap()
                .attach_signature(signed.signature())
                .is_err()
        );
        let wrong = SecretKey::from_bytes(&[1; 32]).unwrap();
        assert!(signed.transaction().sign_local(&[&wrong]).is_err());
        let local = keys(&vector);
        let mut reordered: Vec<_> = local.iter().collect();
        reordered.reverse();
        if local.len() > 1 && local[0].public_key() != local.last().unwrap().public_key() {
            assert!(signed.transaction().sign_local(&reordered).is_err());
        }
        assert_eq!(signed.signed_bytes().unwrap(), wire);
    }
}
#[test]
fn quai_conversion_minimum_data_and_recovered_sender_scope_are_exact() {
    let vector = vectors()
        .into_iter()
        .find(|vector| vector["kind"] == "quai")
        .unwrap();
    let original =
        QuaiTransaction::decode_unsigned(&bytes(vector["unsigned"].as_str().unwrap())).unwrap();
    let key = SecretKey::from_bytes(
        &bytes(vector["publicTestSecret"].as_str().unwrap())
            .try_into()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(original.value, U256::from(MIN_QUAI_CONVERSION_VALUE));
    let mut tx = original.clone();
    tx.value -= U256::from(1);
    assert!(QuaiToQiTransaction::new(tx).is_err());
    for data in [vec![], vec![0, 29], vec![0x23, 0x29], vec![0, 30, 0]] {
        let mut tx = original.clone();
        tx.data = data;
        assert!(QuaiToQiTransaction::new(tx).is_err());
    }
    let mut tx = original.clone();
    tx.to = None;
    assert!(QuaiToQiTransaction::new(tx).is_err());
    let mut tx = original.clone();
    tx.to = Some(
        "0x0011223344556677889900112233445566778899"
            .parse()
            .unwrap(),
    );
    assert!(QuaiToQiTransaction::new(tx).is_err());
    let mut tx = original.clone();
    tx.to = Some(
        "0x0188223344556677889900112233445566778899"
            .parse()
            .unwrap(),
    );
    assert!(QuaiToQiTransaction::new(tx).unwrap().sign(&key).is_err());
}

#[test]
fn batch_discount_matches_the_pinned_cubic_formula_and_mainnet_observations() {
    use quai_consensus::conversion_batch_discount_bps as bps;
    let quai = |n: u64| U256::from(n) * U256::from(10u64).pow(U256::from(18));
    assert_eq!(bps(quai(1), U256::ZERO), None);
    // At or below the flow amount only the 20 basis-point minimum applies.
    assert_eq!(bps(quai(100), quai(125)), Some(20));
    assert_eq!(bps(quai(125), quai(125)), Some(20));
    // Continuous above it: twice the flow is 10 + 10·8 basis points.
    assert_eq!(bps(quai(250), quai(125)), Some(90));
    // Ten times the flow is the largest cubic value; beyond it the node's 10% floor applies.
    assert_eq!(bps(quai(1250), quai(125)), Some(9000));
    assert_eq!(bps(quai(1251), quai(125)), Some(9000));
    // Mainnet 2026-09-14: 200 QUAI shared a batch with ~277 QUAI at flow ~126.04 QUAI
    // and realized 1362 of 1440 quoted Qits (542 bps); two ~253 QUAI competitors
    // (~706 QUAI total) would exceed an 800 basis-point bound and were refunded.
    let flow = U256::from(126_040_000_000_000_000_000u128);
    let one = bps(quai(477), flow).unwrap();
    assert!((530..=560).contains(&one), "{one}");
    assert!(bps(quai(706), flow).unwrap() > 800);
    // Cubing extreme values overflows instead of wrapping.
    assert_eq!(bps(U256::from(1u8) << 201, U256::from(1u8) << 200), None);
}

#[test]
fn the_raw_signing_api_enforces_the_conversion_envelope_not_only_the_typed_builder() {
    // A Qi destination in the sender's own zone is a Quai-to-Qi conversion
    // request, not an ordinary transfer. The envelope was previously enforced
    // only in QuaiToQiTransaction::new, which made it opt-in: this crate is
    // published standalone, so a direct integrator could sign a same-zone Qi
    // destination carrying arbitrary data, value and slippage.
    let vector = vectors()
        .into_iter()
        .find(|vector| vector["kind"] == "quai")
        .unwrap();
    let original =
        QuaiTransaction::decode_unsigned(&bytes(vector["unsigned"].as_str().unwrap())).unwrap();
    let key = SecretKey::from_bytes(
        &bytes(vector["publicTestSecret"].as_str().unwrap())
            .try_into()
            .unwrap(),
    )
    .unwrap();

    // The well-formed conversion still signs, through the raw API.
    assert!(original.sign(&key).is_ok());

    // Empty calldata: the node reads slippage as two big-endian bytes, so this
    // is not an ordinary transfer that happens to target a Qi address.
    let mut bare = original.clone();
    bare.data = vec![];
    assert!(
        bare.sign(&key).is_err(),
        "a bare same-zone Qi destination must not sign as an ordinary transfer"
    );

    // Below the node's conversion minimum.
    let mut small = original.clone();
    small.value -= U256::from(1);
    assert!(small.sign(&key).is_err());

    // Out-of-range slippage is the sharp edge: the node clamps it silently, so
    // accepting it would authorize one slippage and submit another.
    for slippage in [0u16, 29, 9001, u16::MAX] {
        let mut tx = original.clone();
        tx.data = slippage.to_be_bytes().to_vec();
        assert!(
            tx.sign(&key).is_err(),
            "slippage {slippage} is outside 30..=9000 and must not sign"
        );
    }
    for slippage in [30u16, 9000] {
        let mut tx = original.clone();
        tx.data = slippage.to_be_bytes().to_vec();
        assert!(tx.sign(&key).is_ok(), "slippage {slippage} is in range");
    }

    // The pre-existing cross-zone rejection is unchanged.
    let mut cross = original.clone();
    cross.to = Some(
        "0x0188223344556677889900112233445566778899"
            .parse()
            .unwrap(),
    );
    assert!(cross.sign(&key).is_err());
}

#[test]
fn the_kquai_hold_window_covers_exactly_the_interval_after_each_controller_change() {
    use quai_consensus::{
        KAWPOW_FORK_BLOCK, KQUAI_CHANGE_HOLD_INTERVAL, SHA_EQUIVALENT_DIFFICULTY_FORK_BLOCK,
        conversion_held,
    };

    // The pinned node's own values. A drift here is a protocol change, not a
    // refactor, so it is asserted rather than derived.
    assert_eq!(KAWPOW_FORK_BLOCK, 1_171_500);
    assert_eq!(SHA_EQUIVALENT_DIFFICULTY_FORK_BLOCK, 1_755_000);
    assert_eq!(KQUAI_CHANGE_HOLD_INTERVAL, 20_000);

    for fork in [KAWPOW_FORK_BLOCK, SHA_EQUIVALENT_DIFFICULTY_FORK_BLOCK] {
        // Half-open: the fork block itself is held, the first block past the
        // interval is not. An inclusive upper bound would hold one block too
        // long and refuse a conversion the node would have taken.
        assert!(!conversion_held(fork - 1), "before {fork}");
        assert!(conversion_held(fork), "at {fork}");
        assert!(
            conversion_held(fork + KQUAI_CHANGE_HOLD_INTERVAL - 1),
            "last held block after {fork}"
        );
        assert!(
            !conversion_held(fork + KQUAI_CHANGE_HOLD_INTERVAL),
            "first free block after {fork}"
        );
    }

    // Genesis and the current chain are both outside every window; the mainnet
    // prime terminus read on 2026-09-20 was 2,256,896.
    assert!(!conversion_held(0));
    assert!(!conversion_held(2_256_896));
}
