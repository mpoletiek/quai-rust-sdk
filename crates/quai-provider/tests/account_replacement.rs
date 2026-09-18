//! Signed public-toy competitors, bounded coverage and hostile observation tests.
use quai_primitives::Hash32;
use quai_provider::{
    AccountReplacementPoll, AccountReplacementScanRequest, AccountReplacementTracker,
    BlockReference, ProviderError, ReceiptOutcome, ReplacementReason,
};
#[path = "support/account_replacement.rs"]
mod fixture;
use fixture::{Mock, hash, original, replacement};
fn request() -> AccountReplacementScanRequest {
    AccountReplacementScanRequest {
        from_block: 16,
        to_block: 17,
        max_transactions_per_block: 16,
        max_total_transactions: 32,
        preceding_block: Some(BlockReference {
            number: 15,
            hash: hash(15),
        }),
    }
}
#[tokio::test]
async fn portable_cursor_rechecks_prior_page_even_when_head_retreats_and_errors_do_not_advance() {
    let tx = replacement(ReplacementReason::Repriced);
    let m = Mock::new(tx.clone(), 14);
    let mut tracker = AccountReplacementTracker::new(&original(), hash(1), 16, 1).unwrap();
    assert!(matches!(
        tracker.poll(&m.provider()).await.unwrap(),
        AccountReplacementPoll::Pending {
            more_available: true,
            ..
        }
    ));
    // This endpoint reports a head below the next page and a changed prior anchor.
    let changed = Mock::new(tx, 15);
    assert!(matches!(
        tracker.poll(&changed.provider()).await,
        Err(ProviderError::ObservationChanged)
    ));
    // An unsuccessful poll retains the last successfully checked page boundary.
    let AccountReplacementPoll::Confirmed(candidate) = tracker.poll(&m.provider()).await.unwrap()
    else {
        panic!("next page was skipped")
    };
    assert_eq!(candidate.inclusion.block_number, 272);
}
#[tokio::test]
async fn detects_unregistered_repricing_cancellation_and_changed_recipient() {
    for reason in [
        ReplacementReason::Repriced,
        ReplacementReason::Cancelled,
        ReplacementReason::Replaced,
    ] {
        let tx = replacement(reason);
        let m = Mock::new(tx.clone(), 0);
        let r = m
            .provider()
            .observe_account_replacements(&original(), hash(1), request())
            .await
            .unwrap();
        let c = r.candidate.unwrap();
        assert_eq!(c.reason, Some(reason));
        assert_eq!(
            c.transaction.signed_bytes().unwrap(),
            tx.signed_bytes().unwrap()
        );
        assert_eq!(c.confirmations, 2);
        assert_eq!(c.receipt.unwrap().outcome, ReceiptOutcome::Succeeded);
        assert_eq!(r.scanned_through.unwrap().number, 17);
        assert_eq!(r.missing_block, None);
    }
}
#[tokio::test]
async fn original_failed_and_missing_receipts_remain_distinct_from_missing_blocks() {
    for mode in [0, 11, 12] {
        let m = Mock::new(original(), mode);
        let r = m
            .provider()
            .observe_account_replacements(&original(), hash(1), request())
            .await
            .unwrap();
        let c = r.candidate.unwrap();
        assert_eq!(c.reason, None);
        assert_eq!(
            c.receipt.as_ref().map(|r| r.outcome),
            match mode {
                11 => Some(ReceiptOutcome::Failed),
                12 => None,
                _ => Some(ReceiptOutcome::Succeeded),
            }
        );
    }
    let m = Mock::new(original(), 1);
    let r = m
        .provider()
        .observe_account_replacements(&original(), hash(1), request())
        .await
        .unwrap();
    assert!(r.candidate.is_none());
    assert_eq!(r.missing_block, Some(16));
    assert!(r.scanned_through.is_none());
    let m = Mock::new(original(), 0);
    let mut q = request();
    q.from_block = 17;
    q.preceding_block = Some(BlockReference {
        number: 16,
        hash: hash(16),
    });
    let r = m
        .provider()
        .observe_account_replacements(&original(), hash(1), q)
        .await
        .unwrap();
    assert!(r.candidate.is_none());
    assert_eq!(r.scanned_through.unwrap().number, 17);
    assert!(r.missing_block.is_none());
}
#[tokio::test]
async fn forged_signatures_receipts_reorgs_and_competing_occupants_reject() {
    for mode in [2, 3, 4, 5, 6, 7, 8, 9, 10] {
        let m = Mock::new(replacement(ReplacementReason::Replaced), mode);
        assert!(
            m.provider()
                .observe_account_replacements(&original(), hash(1), request())
                .await
                .is_err(),
            "mode {mode}"
        );
    }
}

#[cfg(feature = "polling")]
#[tokio::test]
async fn waiter_returns_unregistered_failed_winner_and_bounds_missing_or_stalled_reads() {
    use quai_provider::{WaitConfig, WaitError};
    use std::time::Duration;
    let config = WaitConfig::new(2, Duration::from_millis(100), Duration::from_millis(1));
    let m = Mock::new(replacement(ReplacementReason::Cancelled), 11);
    let c = m
        .provider()
        .wait_for_account_transaction(&original(), hash(1), 16, config)
        .await
        .unwrap();
    assert_eq!(c.reason, Some(ReplacementReason::Cancelled));
    assert_eq!(c.receipt.unwrap().outcome, ReceiptOutcome::Failed);
    for mode in [1, 12, 13] {
        let m = Mock::new(original(), mode);
        assert!(matches!(
            m.provider()
                .wait_for_account_transaction(&original(), hash(1), 16, config)
                .await,
            Err(WaitError::Timeout { .. })
        ));
        let count = m.counters.lock().unwrap()[0];
        tokio::time::sleep(Duration::from_millis(3)).await;
        assert_eq!(m.counters.lock().unwrap()[0], count);
    }
    let m = Mock::new(original(), 13);
    let p = m.provider();
    let original = original();
    let mut wait = Box::pin(p.wait_for_account_transaction(&original, hash(1), 16, config));
    tokio::select! {
        _ = &mut wait => panic!("stalled read completed"),
        _ = tokio::time::sleep(Duration::from_millis(3)) => {},
    }
    drop(wait);
    let count = m.counters.lock().unwrap()[0];
    tokio::time::sleep(Duration::from_millis(3)).await;
    assert_eq!(m.counters.lock().unwrap()[0], count);
}
#[tokio::test]
async fn invalid_page_and_scope_reject_before_io_and_budgets_do_not_silently_truncate() {
    for which in 0..6 {
        let mut q = request();
        match which {
            0 => q.from_block = 0,
            1 => q.to_block = 300,
            2 => q.max_transactions_per_block = 0,
            3 => q.max_total_transactions = 0,
            4 => q.preceding_block.as_mut().unwrap().number = 14,
            _ => q.to_block = 15,
        };
        let m = Mock::new(original(), 0);
        assert!(
            m.provider()
                .observe_account_replacements(&original(), hash(1), q)
                .await
                .is_err()
        );
        assert_eq!(m.counters.lock().unwrap()[0], 0);
    }
    let m = Mock::new(original(), 0);
    assert!(
        m.provider()
            .observe_account_replacements(&original(), Hash32::ZERO, request())
            .await
            .is_err()
    );
    assert_eq!(m.counters.lock().unwrap()[0], 0);
    let mut q = request();
    q.max_total_transactions = 1;
    let m = Mock::new(original(), 10);
    assert!(matches!(
        m.provider()
            .observe_account_replacements(&original(), hash(1), q)
            .await,
        Err(ProviderError::InvalidResult(
            "replacement scan budget exceeded"
        ))
    ));
}

#[cfg(feature = "polling")]
#[tokio::test]
async fn waiter_drains_multiple_bounded_pages_without_skipping_the_next_block() {
    let m = Mock::new(replacement(ReplacementReason::Repriced), 14);
    let c = m
        .provider()
        .wait_for_account_transaction(
            &original(),
            hash(1),
            16,
            quai_provider::WaitConfig::new(
                1,
                std::time::Duration::from_secs(5),
                std::time::Duration::from_millis(1),
            ),
        )
        .await
        .unwrap();
    assert_eq!(c.inclusion.block_number, 272);
    assert_eq!(c.reason, Some(ReplacementReason::Repriced));
    assert_eq!(c.confirmations, 1);
}
