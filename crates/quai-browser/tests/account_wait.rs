//! Dedicated-worker replacement waiting, page bounds and cancellation.
#![cfg(target_arch = "wasm32")]
use quai_browser::{BrowserAccountWaitError, BrowserWaitConfig, wait_for_account_transaction};
use quai_provider::{ReceiptOutcome, ReplacementReason};
use wasm_bindgen_test::*;
#[path = "fixtures/shared/crates/quai-provider/tests/support/account_replacement.rs"]
mod fixture;
use fixture::{Mock, hash, original, replacement};
wasm_bindgen_test_configure!(run_in_dedicated_worker);
fn config() -> BrowserWaitConfig {
    BrowserWaitConfig {
        confirmations: 2,
        timeout_ms: 5000,
        poll_interval_ms: 1,
        max_polls: 10,
    }
}
#[wasm_bindgen_test(async)]
async fn worker_waits_for_unknown_repricing_cancellation_and_changed_recipient() {
    for reason in [
        ReplacementReason::Repriced,
        ReplacementReason::Cancelled,
        ReplacementReason::Replaced,
    ] {
        let tx = replacement(reason);
        let m = Mock::new(tx.clone(), 11);
        let candidate =
            wait_for_account_transaction(&m.provider(), &original(), hash(1), 16, config())
                .await
                .unwrap();
        assert_eq!(candidate.reason, Some(reason));
        assert_eq!(
            candidate.transaction.signed_bytes().unwrap(),
            tx.signed_bytes().unwrap()
        );
        assert_eq!(candidate.receipt.unwrap().outcome, ReceiptOutcome::Failed);
        assert_eq!(candidate.confirmations, 2);
    }
}
#[wasm_bindgen_test(async)]
async fn worker_waiter_drains_pages_and_honors_page_budget_without_skipping() {
    let m = Mock::new(replacement(ReplacementReason::Repriced), 14);
    let candidate = wait_for_account_transaction(
        &m.provider(),
        &original(),
        hash(1),
        16,
        BrowserWaitConfig {
            confirmations: 1,
            ..config()
        },
    )
    .await
    .unwrap();
    assert_eq!(candidate.inclusion.block_number, 272);
    assert_eq!(candidate.confirmations, 1);
    assert!(matches!(
        wait_for_account_transaction(
            &m.provider(),
            &original(),
            hash(1),
            16,
            BrowserWaitConfig {
                confirmations: 1,
                max_polls: 1,
                ..config()
            }
        )
        .await,
        Err(BrowserAccountWaitError::PollLimit {
            polls_completed: 1,
            ..
        })
    ));
}
#[wasm_bindgen_test(async)]
async fn worker_waiter_rejects_forgery_reorg_and_missing_history_without_claiming_cancellation() {
    for mode in [2, 3, 4, 5, 6, 7, 8, 10] {
        let m = Mock::new(original(), mode);
        assert!(
            matches!(
                wait_for_account_transaction(&m.provider(), &original(), hash(1), 16, config())
                    .await,
                Err(BrowserAccountWaitError::Provider { .. })
            ),
            "mode {mode}"
        );
    }
    let m = Mock::new(original(), 1);
    assert!(matches!(
        wait_for_account_transaction(
            &m.provider(),
            &original(),
            hash(1),
            16,
            BrowserWaitConfig {
                max_polls: 1,
                ..config()
            }
        )
        .await,
        Err(BrowserAccountWaitError::PollLimit {
            last_completed_inclusion: None,
            polls_completed: 1,
            ..
        })
    ));
}
#[wasm_bindgen_test(async)]
async fn worker_waiter_deadline_and_drop_stop_stalled_reads_and_invalid_limits_do_no_io() {
    use std::{future::Future, task::Poll};
    let m = Mock::new(original(), 13);
    assert!(matches!(
        wait_for_account_transaction(
            &m.provider(),
            &original(),
            hash(1),
            16,
            BrowserWaitConfig {
                timeout_ms: 10,
                ..config()
            }
        )
        .await,
        Err(BrowserAccountWaitError::Timeout {
            polls_completed: 0,
            ..
        })
    ));
    let m = Mock::new(original(), 13);
    let p = m.provider();
    let original = original();
    let mut future = Box::pin(wait_for_account_transaction(
        &p,
        &original,
        hash(1),
        16,
        config(),
    ));
    assert!(
        std::future::poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    drop(future);
    let count = m.counters.lock().unwrap()[0];
    wasm_bindgen_futures::JsFuture::from(js_sys::Promise::resolve(&wasm_bindgen::JsValue::NULL))
        .await
        .unwrap();
    assert_eq!(m.counters.lock().unwrap()[0], count);
    let m = Mock::new(original.clone(), 0);
    assert!(matches!(
        wait_for_account_transaction(&m.provider(), &original, hash(1), 0, config()).await,
        Err(BrowserAccountWaitError::InvalidConfig)
    ));
    assert!(matches!(
        wait_for_account_transaction(
            &m.provider(),
            &original,
            hash(1),
            16,
            BrowserWaitConfig {
                max_polls: 0,
                ..config()
            }
        )
        .await,
        Err(BrowserAccountWaitError::InvalidConfig)
    ));
    assert_eq!(m.counters.lock().unwrap()[0], 0);
}
