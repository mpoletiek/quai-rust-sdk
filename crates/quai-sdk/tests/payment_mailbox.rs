//! Pelagus mailbox calldata and result handling against pinned quais.js encodings.
#![cfg(all(feature = "abi", feature = "payments"))]
#[cfg(not(target_arch = "wasm32"))]
use quai_sdk::BlockTag;
use quai_sdk::payment_mailbox::*;
use quai_sdk::payments::PaymentCode;
use quai_sdk::rpc::{Endpoint, RpcError, Transport};
use quai_sdk::{Provider, Routing, U256, Zone};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

fn vectors() -> Value {
    serde_json::from_str(include_str!("mailbox-calls.json")).unwrap()
}

/// Answers `quai_call` with a fixed ABI result and records the request.
#[derive(Clone)]
struct Mock(Arc<Mutex<Vec<Value>>>, String);
impl Transport for Mock {
    async fn request(&self, _: &Endpoint, method: &str, params: Value) -> Result<Value, RpcError> {
        match method {
            "quai_chainId" => Ok(json!("0x9")),
            "quai_call" => {
                self.0.lock().unwrap().push(params);
                Ok(json!(self.1))
            }
            _ => panic!("unexpected method {method}"),
        }
    }
}

fn provider(result: &str) -> (Provider<Mock>, Arc<Mutex<Vec<Value>>>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    (
        Provider::new(
            Mock(calls.clone(), result.to_owned()),
            Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
            U256::from(9),
        ),
        calls,
    )
}

#[test]
fn notify_calldata_matches_pelagus_and_quais() {
    let v = vectors();
    let (provider, _) = provider("0x");
    let mailbox = PaymentMailbox::new(PELAGUS_MAILBOX_ADDRESS.parse().unwrap(), &provider).unwrap();
    let sender = PaymentCode::from_base58(v["sender"].as_str().unwrap()).unwrap();
    let receiver = PaymentCode::from_base58(v["receiver"].as_str().unwrap()).unwrap();
    let call = mailbox.notify(&sender, &receiver).unwrap();
    assert_eq!(call.data().to_hex(), v["notifyData"]);
    assert_eq!(call.value(), U256::ZERO);
    assert_eq!(
        call.destination(),
        v["mailbox"].as_str().unwrap().parse().unwrap()
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn notifications_are_validated_deduplicated_and_bounded() {
    let v = vectors();
    let (provider, calls) = provider(v["notificationsResult"].as_str().unwrap());
    let mailbox = PaymentMailbox::new(PELAGUS_MAILBOX_ADDRESS.parse().unwrap(), &provider).unwrap();
    let sender = PaymentCode::from_base58(v["sender"].as_str().unwrap()).unwrap();
    let receiver = PaymentCode::from_base58(v["receiver"].as_str().unwrap()).unwrap();
    let caller = "0x0006506bDE7140b85DED58a40D7444F84cde4821"
        .parse()
        .unwrap();
    let found = mailbox
        .notifications(caller, &receiver, BlockTag::Latest)
        .await
        .unwrap();
    assert_eq!(found.senders, vec![sender.clone()]);
    assert_eq!(found.invalid, vec!["not-a-payment-code".to_string()]);
    assert_eq!(found.duplicates, 1);
    assert_eq!(
        calls.lock().unwrap()[0][0]["input"],
        v["getNotificationsData"]
    );
    assert!(
        mailbox
            .is_notified(caller, &sender, &receiver, BlockTag::Latest)
            .await
            .unwrap()
    );
    assert!(
        !mailbox
            .is_notified(caller, &receiver, &receiver, BlockTag::Latest)
            .await
            .unwrap()
    );

    let (empty, _) = self::provider(v["emptyResult"].as_str().unwrap());
    let mailbox = PaymentMailbox::new(PELAGUS_MAILBOX_ADDRESS.parse().unwrap(), &empty).unwrap();
    assert_eq!(
        mailbox
            .notifications(caller, &receiver, BlockTag::Latest)
            .await
            .unwrap(),
        MailboxNotifications::default()
    );
    // A caller outside the mailbox zone is rejected before any request.
    let other_zone = "0x0106506bde7140b85ded58a40d7444f84cde4821"
        .parse()
        .unwrap();
    assert!(
        mailbox
            .notifications(other_zone, &receiver, BlockTag::Latest)
            .await
            .is_err()
    );
}

/// Serves `NotificationSent` logs for requested ranges, refusing as too large
/// any multi-block range that covers the dense block.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
struct Logs {
    entries: Arc<Vec<(u64, Value)>>,
    dense: u64,
    ranges: Arc<Mutex<Vec<(u64, u64)>>>,
}
#[cfg(not(target_arch = "wasm32"))]
impl Transport for Logs {
    async fn request(&self, _: &Endpoint, method: &str, params: Value) -> Result<Value, RpcError> {
        let number = |v: &Value| {
            u64::from_str_radix(v.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
        };
        match method {
            "quai_chainId" => Ok(json!("0x9")),
            "quai_getLogs" => {
                let (from, to) = (
                    number(&params[0]["fromBlock"]),
                    number(&params[0]["toBlock"]),
                );
                self.ranges.lock().unwrap().push((from, to));
                if from < to && (from..=to).contains(&self.dense) {
                    return Err(RpcError::ResponseTooLarge);
                }
                Ok(Value::Array(
                    self.entries
                        .iter()
                        .filter(|(block, _)| (from..=to).contains(block))
                        .map(|(_, log)| log.clone())
                        .collect(),
                ))
            }
            _ => panic!("unexpected method {method}"),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn log_announcements_are_filtered_bounded_and_split_when_too_large() {
    use quai_sdk::payments::PrivatePaymentCode;
    let code = |n: u8| {
        PrivatePaymentCode::from_seed(&[n; 32], 0)
            .unwrap()
            .public_code()
            .clone()
    };
    let (receiver, sender, other, stranger) = (code(1), code(2), code(3), code(4));
    let (plain, _) = provider("0x");
    let mailbox = PaymentMailbox::new(PELAGUS_MAILBOX_ADDRESS.parse().unwrap(), &plain).unwrap();
    let event = mailbox
        .contract()
        .interface()
        .event("NotificationSent")
        .unwrap()
        .clone();
    let log = |block: u64, index: u64, from: &str, to: &str, removed: bool| {
        let (topics, data) = event.encode_log(&[json!(from), json!(to)]).unwrap();
        let mut hash = [0u8; 32];
        hash[24..].copy_from_slice(&block.to_be_bytes());
        (
            block,
            json!({
                "address": PELAGUS_MAILBOX_ADDRESS,
                "topics": topics.iter().map(ToString::to_string).collect::<Vec<_>>(),
                "data": format!("0x{}", data.iter().map(|b| format!("{b:02x}")).collect::<String>()),
                "blockHash": quai_sdk::primitives::Hash32::from_bytes(hash).to_string(),
                "blockNumber": format!("{block:#x}"),
                "transactionHash": quai_sdk::primitives::Hash32::from_bytes([7; 32]).to_string(),
                "transactionIndex": "0x0",
                "logIndex": format!("{index:#x}"),
                "removed": removed,
            }),
        )
    };
    let (r, s, o, x) = (
        receiver.to_base58(),
        sender.to_base58(),
        other.to_base58(),
        stranger.to_base58(),
    );
    let entries = vec![
        log(5, 0, &s, &r, false),
        log(4_321, 0, &x, &o, false), // to another receiver: not ours
        log(4_321, 1, "not-a-payment-code", &r, false),
        log(4_321, 2, &s, &r, false), // repeated sender collapses
        log(15_000, 0, &x, &r, true), // reorged out
        log(25_000, 0, &o, &r, false),
    ];
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let provider = Provider::new(
        Logs {
            entries: Arc::new(entries),
            dense: 4_321,
            ranges: ranges.clone(),
        },
        Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
        U256::from(9),
    );
    let mailbox = PaymentMailbox::new(PELAGUS_MAILBOX_ADDRESS.parse().unwrap(), &provider).unwrap();
    let found = mailbox
        .notifications_in_blocks(&receiver, 0, 30_000)
        .await
        .unwrap();
    assert_eq!(found.senders, vec![sender.clone(), other.clone()]);
    assert_eq!(found.invalid, vec!["not-a-payment-code".to_string()]);
    assert_eq!(found.duplicates, 1);
    // Every request stays within the provider's limit; the dense block is
    // finally read alone; successful reads cover 0..=30,000 exactly once, in order.
    let ranges = ranges.lock().unwrap().clone();
    assert!(ranges.iter().all(|(f, t)| t - f < MAILBOX_LOG_BLOCKS));
    assert!(ranges.contains(&(4_321, 4_321)));
    let read: Vec<_> = ranges
        .iter()
        .filter(|(f, t)| f == t || !(*f..=*t).contains(&4_321))
        .collect();
    assert_eq!(read.first().unwrap().0, 0);
    assert_eq!(read.last().unwrap().1, 30_000);
    assert!(read.windows(2).all(|w| w[1].0 == w[0].1 + 1));
    // An empty range is refused before any request.
    assert!(
        mailbox
            .notifications_in_blocks(&receiver, 10, 9)
            .await
            .is_err()
    );
}
