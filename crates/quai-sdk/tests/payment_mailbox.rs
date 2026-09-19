//! Pelagus mailbox calldata and result handling against pinned quais.js encodings.
#![cfg(all(feature = "abi", feature = "payments"))]
#[cfg(not(target_arch = "wasm32"))]
use quai_sdk::BlockTag;
use quai_sdk::payment_mailbox::*;
use quai_sdk::payments::PaymentCode;
#[cfg(not(target_arch = "wasm32"))]
use quai_sdk::primitives::Hash32;
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

#[cfg(not(target_arch = "wasm32"))]
type Ranges = Arc<Mutex<Vec<(u64, u64)>>>;

/// Serves `NotificationSent` logs for requested ranges, refusing as too large
/// any multi-block range that covers the dense block, and zone headers whose
/// hashes change after `reorg_after` header reads, to model a reorg.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
struct Logs {
    entries: Arc<Vec<(u64, Value)>>,
    dense: u64,
    ranges: Ranges,
    tip: u64,
    reorg_after: Option<usize>,
    headers: Arc<Mutex<usize>>,
    /// Head of the backend that answers batches, when it lags the tip.
    batch_head: Option<u64>,
}
#[cfg(not(target_arch = "wasm32"))]
impl Transport for Logs {
    async fn request(&self, _: &Endpoint, method: &str, params: Value) -> Result<Value, RpcError> {
        let number = |v: &Value| {
            u64::from_str_radix(v.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
        };
        match method {
            "quai_chainId" => Ok(json!("0x9")),
            "quai_getHeaderByNumber" => {
                let n = if params[0] == "latest" {
                    self.tip
                } else {
                    number(&params[0])
                };
                let mut served = self.headers.lock().unwrap();
                *served += 1;
                let salt = u8::from(self.reorg_after.is_some_and(|after| *served > after));
                let mut hash = [salt; 32];
                hash[24..].copy_from_slice(&(n + 1).to_be_bytes());
                Ok(
                    json!({"woHeader":{"hash":Hash32::from_bytes(hash).to_string(),
                    "parentHash":Hash32::from_bytes([9; 32]).to_string(),
                    "number":format!("{n:#x}"),"location":"0x0000","primeTerminusNumber":"0x1"},
                    "gasLimit":"0x1","stateLimit":"0x1"}),
                )
            }
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
    async fn request_batch(
        &self,
        e: &Endpoint,
        requests: Vec<(&str, Value)>,
    ) -> Option<quai_sdk::rpc::BatchResult> {
        let mut out = Vec::with_capacity(requests.len());
        // Only the backend answering log batches lags; header checks reach a
        // healthy one, as behind a load balancer.
        let logs = requests.iter().any(|(method, _)| *method == "quai_getLogs");
        for (method, params) in requests {
            let lagging = logs
                && self.batch_head.is_some_and(|head| {
                    method == "quai_getHeaderByNumber"
                        && params[0] != "latest"
                        && u64::from_str_radix(
                            params[0].as_str().unwrap().trim_start_matches("0x"),
                            16,
                        )
                        .unwrap()
                            > head
                });
            match self.request(e, method, params).await {
                // One oversized member fails the whole response.
                Err(RpcError::ResponseTooLarge) => return Some(Err(RpcError::ResponseTooLarge)),
                _ if lagging => out.push(Ok(Value::Null)),
                result => out.push(result),
            }
        }
        Some(Ok(out))
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn log_announcements_are_filtered_bounded_settled_and_split_when_too_large() {
    use quai_sdk::contracts::ContractError;
    use quai_sdk::payments::PrivatePaymentCode;
    use quai_sdk::provider::ProviderError;
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
    // `undecodable` corrupts the sender string into non-UTF-8, which Solidity
    // accepts and which cannot decode.
    let log = |block: u64, index: u64, from: &str, to: &str, removed: bool, undecodable: bool| {
        let (topics, mut data) = event.encode_log(&[json!(from), json!(to)]).unwrap();
        if undecodable {
            data[0x60] = 0xff;
        }
        let mut hash = [0u8; 32];
        hash[24..].copy_from_slice(&block.to_be_bytes());
        (
            block,
            json!({
                "address": PELAGUS_MAILBOX_ADDRESS,
                "topics": topics.iter().map(ToString::to_string).collect::<Vec<_>>(),
                "data": format!("0x{}", data.iter().map(|b| format!("{b:02x}")).collect::<String>()),
                "blockHash": Hash32::from_bytes(hash).to_string(),
                "blockNumber": format!("{block:#x}"),
                "transactionHash": Hash32::from_bytes([7; 32]).to_string(),
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
    let entries = Arc::new(vec![
        log(5, 0, &s, &r, false, false),
        log(4_321, 0, &x, &o, false, false), // to another receiver: not ours
        log(4_321, 1, "not-a-payment-code", &r, false, false),
        log(4_321, 2, &s, &r, false, false), // repeated sender collapses
        log(15_000, 0, &x, &r, true, false), // reorged out
        log(20_000, 0, &x, &r, false, true), // does not decode: listed, not fatal
        log(25_000, 0, &o, &r, false, false),
    ]);
    let backend = |tip: u64, reorg_after: Option<usize>, batch_head, ranges: &Ranges| {
        Provider::new(
            Logs {
                entries: entries.clone(),
                dense: 4_321,
                ranges: ranges.clone(),
                tip,
                reorg_after,
                headers: Default::default(),
                batch_head,
            },
            Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
            U256::from(9),
        )
    };
    let logs = |tip, reorg_after, ranges: &Ranges| backend(tip, reorg_after, None, ranges);
    let settled = 30_000 + MAILBOX_SETTLED_DEPTH;

    let ranges = Ranges::default();
    let provider = logs(settled, None, &ranges);
    let mailbox = PaymentMailbox::new(PELAGUS_MAILBOX_ADDRESS.parse().unwrap(), &provider).unwrap();
    let found = mailbox
        .notifications_in_blocks(&receiver, 0, 30_000)
        .await
        .unwrap();
    assert_eq!(found.senders, vec![sender.clone(), other.clone()]);
    assert_eq!(found.invalid.len(), 2);
    assert_eq!(found.invalid[0], "not-a-payment-code");
    assert!(found.invalid[1].starts_with("undecodable log "));
    assert_eq!(found.duplicates, 1);
    // Every request stays within the provider's limit, the dense block is
    // finally read alone, and successful reads cover 0..=30,000 once, in order.
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

    // A range ending too close to the tip is refused, retryably, before any
    // log request: a shallow reorg could still hide an announcement in it.
    let near = Ranges::default();
    let provider = logs(settled - 1, None, &near);
    let mailbox = PaymentMailbox::new(PELAGUS_MAILBOX_ADDRESS.parse().unwrap(), &provider).unwrap();
    assert!(matches!(
        mailbox.notifications_in_blocks(&receiver, 0, 30_000).await,
        Err(ContractError::Provider(ProviderError::ObservationChanged))
    ));
    assert!(near.lock().unwrap().is_empty());

    // A backend lagging behind the range answers getLogs with what it has.
    // The header in the same batch exposes it, so the short answer is refused.
    let lag = Ranges::default();
    let provider = backend(settled, None, Some(24_999), &lag);
    let mailbox = PaymentMailbox::new(PELAGUS_MAILBOX_ADDRESS.parse().unwrap(), &provider).unwrap();
    assert!(matches!(
        mailbox.notifications_in_blocks(&receiver, 0, 30_000).await,
        Err(ContractError::Provider(ProviderError::ObservationChanged))
    ));
    // It failed at a log batch past the lagging head, not before any.
    assert!(lag.lock().unwrap().iter().any(|(_, to)| *to > 24_999));

    // If the end block changes during the read, the result is not trusted.
    let moved = Ranges::default();
    let provider = logs(settled, Some(2), &moved);
    let mailbox = PaymentMailbox::new(PELAGUS_MAILBOX_ADDRESS.parse().unwrap(), &provider).unwrap();
    assert!(matches!(
        mailbox.notifications_in_blocks(&receiver, 0, 30_000).await,
        Err(ContractError::Provider(ProviderError::ObservationChanged))
    ));
}
