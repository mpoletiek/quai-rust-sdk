//! Pelagus mailbox calldata and result handling against pinned quais.js encodings.
#![cfg(all(feature = "abi", feature = "payments"))]
use quai_sdk::payment_mailbox::*;
use quai_sdk::payments::PaymentCode;
use quai_sdk::rpc::{Endpoint, RpcError, Transport};
use quai_sdk::{BlockTag, Provider, Routing, U256, Zone};
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
