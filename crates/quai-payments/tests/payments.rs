//! Pinned JS payment keys/searches and strict ownership/resource checks.
use quai_payments::{
    HARDENED_INDEX, MAX_CHANNEL_BYTES, MAX_SEARCH_ATTEMPTS, PaymentChannel, PaymentCode,
    PaymentDirection, PaymentError, PaymentSearch, PrivatePaymentCode,
};
use quai_primitives::Zone;
use serde_json::{Value, json};
fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/quais-payments.json")).unwrap()
}
fn decode(s: &str) -> Vec<u8> {
    s.as_bytes()[2..]
        .chunks_exact(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}
fn hex(bytes: &[u8]) -> String {
    format!(
        "0x{}",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    )
}
fn pair(v: &Value) -> (PrivatePaymentCode, PrivatePaymentCode) {
    let account = v["account"].as_u64().unwrap() as u32;
    (
        PrivatePaymentCode::from_seed(&decode(v["senderSeed"].as_str().unwrap()), account).unwrap(),
        PrivatePaymentCode::from_seed(&decode(v["receiverSeed"].as_str().unwrap()), account)
            .unwrap(),
    )
}
#[test]
fn exact_codes_notifications_and_bidirectional_keys_match_js() {
    let f = fixture();
    assert_eq!(f["pairs"].as_array().unwrap().len(), 4);
    for v in f["pairs"].as_array().unwrap() {
        let (a, b) = pair(v);
        for (who, code) in [("sender", a.public_code()), ("receiver", b.public_code())] {
            assert_eq!(code.to_base58(), v[format!("{who}Code")]);
            assert_eq!(hex(&code.to_bytes()), v[format!("{who}Payload")]);
            assert_eq!(
                hex(&code.notification_public_key().to_compressed()),
                v[format!("{who}NotificationKey")]
            );
            assert_eq!(PaymentCode::from_base58(&code.to_base58()).unwrap(), *code);
            assert_eq!(PaymentCode::from_bytes(&code.to_bytes()).unwrap(), *code);
            assert_eq!(format!("{code:?}"), "PaymentCode([REDACTED])");
        }
        assert_eq!(
            a.notification_key().unwrap().public_key(),
            a.public_code().notification_public_key()
        );
        for p in v["payments"].as_array().unwrap() {
            let index = p["index"].as_u64().unwrap() as u32;
            let send = a.send_public_key(b.public_code(), index).unwrap();
            let receive = b.receive_key(a.public_code(), index).unwrap();
            assert_eq!(send, receive.public_key());
            assert_eq!(hex(&send.to_compressed()), p["publicKey"]);
            assert_eq!(
                hex(receive.export_bytes().as_bytes()),
                p["publicTestReceiveSecret"]
            );
            assert_eq!(send.address().to_string(), p["address"]);
        }
        assert_eq!(format!("{a:?}"), "PrivatePaymentCode([REDACTED])");
    }
}
#[test]
fn malformed_code_components_checksum_and_index_boundaries_are_rejected() {
    let (a, b) = pair(&fixture()["pairs"][0]);
    let original = a.public_code().to_bytes();
    for (position, value) in [(0, 0), (0, 2), (1, 1), (2, 4), (67, 1), (79, 1)] {
        let mut changed = original;
        changed[position] = value;
        assert!(PaymentCode::from_bytes(&changed).is_err());
        let mut frame = vec![0x47];
        frame.extend(changed);
        assert!(PaymentCode::from_base58(&bs58::encode(frame).with_check().into_string()).is_err());
    }
    let mut point = original;
    point[3..35].fill(255);
    assert!(PaymentCode::from_bytes(&point).is_err());
    let mut frame = vec![0x48];
    frame.extend(original);
    assert!(PaymentCode::from_base58(&bs58::encode(frame).with_check().into_string()).is_err());
    let mut bad = a.public_code().to_base58().into_bytes();
    bad[10] = if bad[10] == b'1' { b'2' } else { b'1' };
    assert!(PaymentCode::from_base58(std::str::from_utf8(&bad).unwrap()).is_err());
    for text in ["".into(), "é".repeat(40), "1".repeat(121)] {
        assert!(PaymentCode::from_base58(&text).is_err());
    }
    for len in [0, 79, 81] {
        assert!(PaymentCode::from_bytes(&vec![0; len]).is_err());
    }
    assert!(PrivatePaymentCode::from_seed(&[0; 15], 0).is_err());
    assert!(PrivatePaymentCode::from_seed(&[0; 65], 0).is_err());
    assert!(PrivatePaymentCode::from_seed(&[0; 16], HARDENED_INDEX).is_err());
    assert_eq!(
        a.send_public_key(b.public_code(), HARDENED_INDEX),
        Err(PaymentError::InvalidIndex)
    );
    assert!(a.receive_key(b.public_code(), HARDENED_INDEX).is_err());
}
#[test]
fn all_standard_seed_lengths_are_supported() {
    for n in 16..=64 {
        assert!(
            PrivatePaymentCode::from_seed(&vec![42; n], 0).is_ok(),
            "length {n}"
        );
    }
}
#[test]
fn first_matching_zone_and_direction_searches_match_js() {
    let f = fixture();
    let (a, b) = pair(&f["pairs"][0]);
    for v in f["searches"].as_array().unwrap() {
        let zone = Zone::from_byte(v["zone"].as_u64().unwrap() as u8).unwrap();
        let direction = if v["direction"] == "send" {
            PaymentDirection::Send
        } else {
            PaymentDirection::Receive
        };
        let index = v["index"].as_u64().unwrap() as u32;
        let result = a
            .search(
                b.public_code(),
                direction,
                PaymentSearch {
                    zone,
                    start_index: 0,
                    max_attempts: index + 1,
                },
                || false,
            )
            .unwrap();
        assert_eq!(result.index, index);
        assert_eq!(result.attempts, index + 1);
        assert_eq!(result.next_index, Some(index + 1));
        assert_eq!(result.address.to_string(), v["address"]);
        assert_eq!(hex(&result.public_key.to_compressed()), v["publicKey"]);
    }
}
#[test]
fn search_cancellation_exhaustion_and_overflow_return_precise_continuations() {
    let f = fixture();
    let (a, b) = pair(&f["pairs"][0]);
    let sample = f["searches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["direction"] == "send" && v["index"].as_u64().unwrap() > 3)
        .unwrap();
    let zone = Zone::from_byte(sample["zone"].as_u64().unwrap() as u8).unwrap();
    let search = PaymentSearch {
        zone,
        start_index: 0,
        max_attempts: 20,
    };
    assert_eq!(
        a.search(b.public_code(), PaymentDirection::Send, search, || true),
        Err(PaymentError::SearchCancelled {
            attempts: 0,
            next_index: Some(0)
        })
    );
    let mut calls = 0;
    assert_eq!(
        a.search(b.public_code(), PaymentDirection::Send, search, || {
            calls += 1;
            calls > 3
        }),
        Err(PaymentError::SearchCancelled {
            attempts: 3,
            next_index: Some(3)
        })
    );
    assert_eq!(
        a.search(
            b.public_code(),
            PaymentDirection::Send,
            PaymentSearch {
                max_attempts: 1,
                ..search
            },
            || false
        ),
        Err(PaymentError::SearchExhausted {
            attempts: 1,
            next_index: Some(1)
        })
    );
    for max_attempts in [0, MAX_SEARCH_ATTEMPTS + 1] {
        assert!(
            a.search(
                b.public_code(),
                PaymentDirection::Send,
                PaymentSearch {
                    max_attempts,
                    ..search
                },
                || false
            )
            .is_err()
        );
    }
    let last = HARDENED_INDEX - 1;
    let public = a.send_public_key(b.public_code(), last).unwrap();
    let other = Zone::ALL
        .into_iter()
        .find(|z| public.address().zone() != Ok(*z))
        .unwrap();
    assert_eq!(
        a.search(
            b.public_code(),
            PaymentDirection::Send,
            PaymentSearch {
                zone: other,
                start_index: last,
                max_attempts: 2
            },
            || false
        ),
        Err(PaymentError::SearchExhausted {
            attempts: 1,
            next_index: None
        })
    );
}
#[test]
fn channel_restore_checks_owner_schema_cursors_and_duplicate_fields() {
    let (a, b) = pair(&fixture()["pairs"][0]);
    let channel = PaymentChannel::new(&a, b.public_code().clone());
    let bytes = channel.to_json().unwrap();
    assert_eq!(format!("{channel:?}"), "PaymentChannel([REDACTED])");
    assert_eq!(
        PaymentChannel::from_json(&a, &bytes)
            .unwrap()
            .to_json()
            .unwrap(),
        bytes
    );
    assert_eq!(
        PaymentChannel::from_json(&b, &bytes).unwrap_err(),
        PaymentError::OwnerMismatch
    );
    let wrong = PrivatePaymentCode::from_seed(&[1; 16], 1).unwrap();
    assert_eq!(
        PaymentChannel::from_json(&wrong, &bytes).unwrap_err(),
        PaymentError::OwnerMismatch
    );
    let original: Value = serde_json::from_slice(&bytes).unwrap();
    for (key, value) in [
        ("scheme", json!("unknown")),
        ("schemaVersion", json!(2)),
        ("account", json!(1)),
        ("unknown", json!(true)),
        ("sendNext", json!([HARDENED_INDEX, 0, 0, 0, 0, 0, 0, 0, 0])),
        ("receiveNext", json!([0])),
    ] {
        let mut altered = original.clone();
        altered[key] = value;
        assert!(PaymentChannel::from_json(&a, &serde_json::to_vec(&altered).unwrap()).is_err());
    }
    let text = std::str::from_utf8(&bytes)
        .unwrap()
        .replace("\"account\":0", "\"account\":0,\"account\":1");
    assert!(PaymentChannel::from_json(&a, text.as_bytes()).is_err());
    assert_eq!(
        PaymentChannel::from_json(&a, &vec![b' '; MAX_CHANNEL_BYTES + 1]).unwrap_err(),
        PaymentError::Limit
    );
}
#[test]
fn explicit_reservation_updates_only_its_cursor_and_survives_restore() {
    let f = fixture();
    let (a, b) = pair(&f["pairs"][0]);
    let mut channel = PaymentChannel::new(&a, b.public_code().clone());
    let sample = f["searches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["direction"] == "send")
        .unwrap();
    let zone = Zone::from_byte(sample["zone"].as_u64().unwrap() as u8).unwrap();
    let index = sample["index"].as_u64().unwrap() as u32;
    let result = channel
        .reserve_next(&a, PaymentDirection::Send, zone, index + 1, || false)
        .unwrap();
    assert_eq!(result.index, index);
    assert_eq!(
        channel.next_index(PaymentDirection::Send, zone),
        Some(index + 1)
    );
    assert_eq!(channel.next_index(PaymentDirection::Receive, zone), Some(0));
    let restored = PaymentChannel::from_json(&a, &channel.to_json().unwrap()).unwrap();
    assert_eq!(
        restored.next_index(PaymentDirection::Send, zone),
        Some(index + 1)
    );
    assert!(
        channel
            .reserve_next(&b, PaymentDirection::Send, zone, 1, || false)
            .is_err()
    );
    assert_eq!(
        channel.next_index(PaymentDirection::Send, zone),
        Some(index + 1)
    );
    let receive = f["searches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["direction"] == "receive")
        .unwrap();
    let key = channel
        .receive_key(&a, receive["index"].as_u64().unwrap() as u32)
        .unwrap();
    assert_eq!(hex(&key.public_key().to_compressed()), receive["publicKey"]);
    assert!(channel.receive_key(&b, 0).is_err());
}

#[test]
fn channel_cancellation_progress_and_exhausted_cursor_survive_backup() {
    let (a, b) = pair(&fixture()["pairs"][0]);
    let zone = Zone::from_byte(0).unwrap();
    let mut channel = PaymentChannel::new(&a, b.public_code().clone());
    assert_eq!(
        channel.reserve_next(&a, PaymentDirection::Send, zone, 1, || false),
        Err(PaymentError::SearchExhausted {
            attempts: 1,
            next_index: Some(1)
        })
    );
    assert_eq!(channel.next_index(PaymentDirection::Send, zone), Some(1));
    let mut calls = 0;
    assert_eq!(
        channel.reserve_next(&a, PaymentDirection::Send, zone, 10, || {
            calls += 1;
            calls > 3
        }),
        Err(PaymentError::SearchCancelled {
            attempts: 3,
            next_index: Some(4)
        })
    );
    let bytes = channel.to_json().unwrap();
    assert_eq!(
        PaymentChannel::from_json(&a, &bytes)
            .unwrap()
            .next_index(PaymentDirection::Send, zone),
        Some(4)
    );
    let mut exhausted: Value = serde_json::from_slice(&bytes).unwrap();
    exhausted["sendNext"][0] = Value::Null;
    let mut channel =
        PaymentChannel::from_json(&a, &serde_json::to_vec(&exhausted).unwrap()).unwrap();
    assert_eq!(
        channel.reserve_next(&a, PaymentDirection::Send, zone, 1, || false),
        Err(PaymentError::SearchExhausted {
            attempts: 0,
            next_index: None
        })
    );
}
