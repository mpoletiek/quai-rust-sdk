//! Window/worker code appearance waits without a known deployment transaction.
use crate::receipt_wait::{Timer, now};
use quai_primitives::Hash32;
use quai_provider::{
    CodeWaitConfig, CodeWaitError, ContractCodeObservation, ContractCodeTarget, Provider,
};
use quai_rpc::Transport;
use std::{cell::Cell, future::Future, task::Poll};

/// Wait under an observable monotonic deadline and completed-observation budget.
/// Empty code or changed anchors remain pending; mismatched genesis/runtime or
/// other source errors stop. Suspension can delay notification but cannot permit
/// success after observed expiry. Drop clears timers and active reads. No Send
/// bound, hidden deployment, binding mutation or finality assertion is involved.
pub async fn wait_for_contract_code<T: Transport>(
    provider: &Provider<T>,
    target: ContractCodeTarget,
    config: CodeWaitConfig,
) -> Result<ContractCodeObservation, CodeWaitError> {
    config.validate()?;
    if target.genesis == Hash32::ZERO {
        return Err(CodeWaitError::InvalidConfig);
    }
    let start = now().map_err(|_| CodeWaitError::Runtime)?;
    let timer = Timer::new(config.timeout_ms).map_err(|_| CodeWaitError::Runtime)?;
    let mut deadline = std::pin::pin!(timer.wait());
    let polls = Cell::new(0u32);
    let timeout = || CodeWaitError::Timeout {
        address: target.address,
        polls_completed: polls.get(),
    };
    let expired = || {
        now()
            .map(|n| n - start >= f64::from(config.timeout_ms))
            .map_err(|_| CodeWaitError::Runtime)
    };
    let work = async {
        loop {
            if expired()? {
                return Err(timeout());
            }
            let result = target.observe(provider).await;
            if expired()? {
                return Err(timeout());
            }
            let result = result?;
            polls.set(polls.get() + 1);
            if let Some(code) = result {
                return Ok(code);
            }
            if polls.get() >= config.max_polls {
                return Err(CodeWaitError::PollLimit {
                    address: target.address,
                    polls_completed: polls.get(),
                });
            }
            Timer::new(config.poll_interval_ms)
                .map_err(|_| CodeWaitError::Runtime)?
                .wait()
                .await
                .map_err(|_| CodeWaitError::Runtime)?;
        }
    };
    let mut work = std::pin::pin!(work);
    std::future::poll_fn(|cx| {
        match expired() {
            Ok(true) => return Poll::Ready(Err(timeout())),
            Err(e) => return Poll::Ready(Err(e)),
            _ => {}
        }
        if let Poll::Ready(result) = deadline.as_mut().poll(cx) {
            return Poll::Ready(Err(if result.is_ok() {
                timeout()
            } else {
                CodeWaitError::Runtime
            }));
        }
        work.as_mut().poll(cx)
    })
    .await
}
