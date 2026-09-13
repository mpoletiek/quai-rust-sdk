//! Browser/worker receipt polling with owned timers and explicit attempt bounds.
use crate::{BrowserError, BrowserWaitConfig};
use quai_primitives::{Hash32, Zone};
use quai_provider::{ConfirmedReceipt, Inclusion, Provider, ProviderError, ReceiptConfirmation};
use quai_rpc::Transport;
use std::{cell::Cell, future::Future, task::Poll};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/src/timer.js")]
extern "C" {
    #[wasm_bindgen(catch, js_name = monotonicNow)]
    fn now() -> Result<f64, JsValue>;
    #[wasm_bindgen(catch, js_name = newTimer)]
    fn new_timer(milliseconds: u32) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch, js_name = waitTimer)]
    async fn wait_timer(handle: &JsValue) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_name = closeTimer)]
    fn close_timer(handle: &JsValue);
}
struct Timer(JsValue);
impl Timer {
    fn new(ms: u32) -> Result<Self, BrowserError> {
        new_timer(ms)
            .map(Self)
            .map_err(|_| BrowserError::InvalidConfig)
    }
    async fn wait(&self) -> Result<(), BrowserError> {
        let elapsed = wait_timer(&self.0)
            .await
            .map_err(|_| BrowserError::InvalidResult)?;
        if elapsed.as_bool() != Some(true) {
            return Err(BrowserError::InvalidResult);
        }
        Ok(())
    }
}
impl Drop for Timer {
    fn drop(&mut self) {
        close_timer(&self.0);
    }
}
/// Wait failure never cancels a transaction or authorizes releasing its claims.
#[derive(Debug, thiserror::Error)]
pub enum BrowserReceiptWaitError {
    /// Invalid limits are rejected before timers and provider I/O.
    #[error("invalid browser receipt wait limits")]
    InvalidConfig,
    /// Deadline expired, including time spent awaiting RPCs. Information below is
    /// from completed polls only; a cancelled partial poll contributes no inclusion.
    #[error("browser receipt wait timed out for {transaction_hash}")]
    Timeout {
        /// Transaction still eligible for later observation.
        transaction_hash: Hash32,
        /// Most recent completed poll's inclusion, possibly reorged away.
        last_completed_inclusion: Option<Inclusion>,
        /// Number of fully completed observations.
        polls_completed: u32,
    },
    /// Explicit observation budget exhausted, with no rejection inference.
    #[error("browser receipt observation budget exhausted for {transaction_hash}")]
    PollLimit {
        /// Watched transaction.
        transaction_hash: Hash32,
        /// Most recent completed poll's inclusion, possibly reorged away.
        last_completed_inclusion: Option<Inclusion>,
        /// Number of fully completed observations.
        polls_completed: u32,
    },
    /// A typed provider read failed; no implicit retry or submission is performed.
    #[error("browser receipt read failed for {transaction_hash}: {source}")]
    Provider {
        /// Watched transaction.
        transaction_hash: Hash32,
        /// Read error, without raw browser/provider objects.
        #[source]
        source: ProviderError,
    },
    /// Required worker/window clock or timer failed. No wall-clock fallback.
    #[error("browser receipt wait runtime unavailable")]
    Runtime,
}
/// Wait using portable confirmation checks and window/worker monotonic timers.
/// At most two timers are held; drop clears them and drops the active read future.
/// Non-Send transports are supported. The deadline covers RPCs and polling delays;
/// delayed background callbacks cannot return success after observed clock expiry.
/// Browser scheduling is not real-time and confirmations do not prove finality.
/// Execution-failed receipts remain explicit successful *observations*.
pub async fn wait_for_receipt<T: Transport>(
    provider: &Provider<T>,
    zone: Zone,
    transaction_hash: Hash32,
    config: BrowserWaitConfig,
) -> Result<ConfirmedReceipt, BrowserReceiptWaitError> {
    config
        .validate()
        .map_err(|_| BrowserReceiptWaitError::InvalidConfig)?;
    let started = now().map_err(|_| BrowserReceiptWaitError::Runtime)?;
    let timer = Timer::new(config.timeout_ms).map_err(|_| BrowserReceiptWaitError::Runtime)?;
    let mut deadline = std::pin::pin!(timer.wait());
    let last = Cell::new(None);
    let polls = Cell::new(0u32);
    let timeout = || BrowserReceiptWaitError::Timeout {
        transaction_hash,
        last_completed_inclusion: last.get(),
        polls_completed: polls.get(),
    };
    let expired = || {
        now()
            .map(|value| value - started >= f64::from(config.timeout_ms))
            .map_err(|_| BrowserReceiptWaitError::Runtime)
    };
    let work = async {
        loop {
            if expired()? {
                return Err(timeout());
            }
            let observation = provider
                .observe_receipt_confirmation(zone, transaction_hash, config.confirmations)
                .await;
            if expired()? {
                return Err(timeout());
            }
            let observation = observation.map_err(|source| BrowserReceiptWaitError::Provider {
                transaction_hash,
                source,
            })?;
            polls.set(polls.get() + 1);
            match observation {
                ReceiptConfirmation::Confirmed(receipt) => return Ok(*receipt),
                ReceiptConfirmation::Pending {
                    last_observed_inclusion,
                } => last.set(last_observed_inclusion),
            }
            if polls.get() >= config.max_polls {
                return Err(BrowserReceiptWaitError::PollLimit {
                    transaction_hash,
                    last_completed_inclusion: last.get(),
                    polls_completed: polls.get(),
                });
            }
            Timer::new(config.poll_interval_ms)
                .map_err(|_| BrowserReceiptWaitError::Runtime)?
                .wait()
                .await
                .map_err(|_| BrowserReceiptWaitError::Runtime)?;
        }
    };
    let mut work = std::pin::pin!(work);
    std::future::poll_fn(|cx| {
        match expired() {
            Ok(true) => return Poll::Ready(Err(timeout())),
            Err(e) => return Poll::Ready(Err(e)),
            Ok(false) => {}
        }
        if let Poll::Ready(result) = deadline.as_mut().poll(cx) {
            return Poll::Ready(Err(if result.is_ok() {
                timeout()
            } else {
                BrowserReceiptWaitError::Runtime
            }));
        }
        work.as_mut().poll(cx)
    })
    .await
}
