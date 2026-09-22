//! Bounded runtime-code appearance checks when a deployment transaction is unknown.
use crate::{BlockTag, ContractCodeObservation, Provider, ProviderError};
use quai_primitives::{Hash32, QuaiAddress};
use quai_rpc::Transport;

/// Explicit account and trusted network/runtime constraints, with no deployment hash.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct ContractCodeTarget {
    /// Account whose code should appear.
    pub address: QuaiAddress,
    /// Required nonzero genesis identity, rechecked in every observation.
    pub genesis: Hash32,
    /// Optional expected runtime Keccak; mismatched nonempty code fails immediately.
    pub expected_runtime: Option<Hash32>,
}
impl ContractCodeTarget {
    /// Code expected at `address` on the `genesis` network, optionally exact.
    pub const fn new(
        address: QuaiAddress,
        genesis: Hash32,
        expected_runtime: Option<Hash32>,
    ) -> Self {
        Self {
            address,
            genesis,
            expected_runtime,
        }
    }
}
impl ContractCodeTarget {
    /// One bounded code observation. Empty code or changed canonical anchors
    /// return None; other failures stop. Nonempty code must satisfy the target.
    /// Appearance is not authenticated finality, ABI/proxy semantics or execution proof.
    pub async fn observe<T: Transport>(
        &self,
        provider: &Provider<T>,
    ) -> Result<Option<ContractCodeObservation>, ProviderError> {
        if self.genesis == Hash32::ZERO {
            return Err(ProviderError::InvalidRequest("zero code target genesis"));
        }
        let observation = match provider
            .observe_contract_code(self.address, BlockTag::Latest, self.expected_runtime)
            .await
        {
            Err(ProviderError::ObservationChanged) => return Ok(None),
            other => other?,
        };
        if observation.genesis != self.genesis {
            return Err(ProviderError::GenesisMismatch);
        }
        if observation.code.bytes.bytes().is_empty() {
            return Ok(None);
        }
        if observation.code.matches_expected == Some(false) {
            return Err(ProviderError::InvalidResult(
                "contract runtime code mismatch",
            ));
        }
        Ok(Some(observation))
    }
}
/// Portable millisecond limits for code-appearance waiting, without a misleading
/// confirmation-depth setting. At most 100,000 completed observations.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct CodeWaitConfig {
    /// Overall deadline, 1..=i32::MAX milliseconds, including RPCs and delays.
    pub timeout_ms: u32,
    /// Positive interval, no longer than the timeout.
    pub poll_interval_ms: u32,
    /// Independent limit of 1..100000 completed observations.
    pub max_polls: u32,
}
impl CodeWaitConfig {
    /// Every limit is required: there is deliberately no default.
    pub const fn new(timeout_ms: u32, poll_interval_ms: u32, max_polls: u32) -> Self {
        Self {
            timeout_ms,
            poll_interval_ms,
            max_polls,
        }
    }
    /// Replace `timeout_ms`.
    pub const fn with_timeout_ms(mut self, timeout_ms: u32) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }
    /// Replace `poll_interval_ms`.
    pub const fn with_poll_interval_ms(mut self, poll_interval_ms: u32) -> Self {
        self.poll_interval_ms = poll_interval_ms;
        self
    }
    /// Replace `max_polls`.
    pub const fn with_max_polls(mut self, max_polls: u32) -> Self {
        self.max_polls = max_polls;
        self
    }
    /// Validate before timers or I/O on both native and browser runtimes.
    pub fn validate(self) -> Result<(), CodeWaitError> {
        if self.timeout_ms == 0
            || self.timeout_ms > i32::MAX as u32
            || self.poll_interval_ms == 0
            || self.poll_interval_ms > self.timeout_ms
            || !(1..=100_000).contains(&self.max_polls)
        {
            return Err(CodeWaitError::InvalidConfig);
        }
        Ok(())
    }
}
/// Waiting errors do not undo deployments, change bindings or authorize transactions.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CodeWaitError {
    /// Invalid limits or zero expected genesis; no I/O occurs.
    #[error("invalid contract code wait configuration")]
    InvalidConfig,
    /// Observable deadline expired, including time in a stalled read.
    #[error("timed out waiting for contract code at {address}")]
    Timeout {
        /// Watched account.
        address: QuaiAddress,
        /// Fully completed observations; a cancelled partial poll does not count.
        polls_completed: u32,
    },
    /// Configured observation count exhausted; absence is not a terminal conclusion.
    #[error("contract code observation budget exhausted at {address}")]
    PollLimit {
        /// Watched account.
        address: QuaiAddress,
        /// Fully completed observations.
        polls_completed: u32,
    },
    /// Source error or mismatched identity/code, with no automatic retry.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// Browser monotonic timer infrastructure failed.
    #[error("contract code wait timer failed")]
    Runtime,
}
#[cfg(all(feature = "polling", not(target_arch = "wasm32")))]
impl<T: Transport> Provider<T> {
    /// Wait for nonempty, rechecked code without knowing a deployment transaction.
    /// Overall time and completed polls are bounded. Drop cancels active reads;
    /// source errors stop, and no transaction is submitted or considered final.
    pub async fn wait_for_contract_code(
        &self,
        target: ContractCodeTarget,
        config: CodeWaitConfig,
    ) -> Result<ContractCodeObservation, CodeWaitError> {
        use std::time::Duration;
        config.validate()?;
        if target.genesis == Hash32::ZERO {
            return Err(CodeWaitError::InvalidConfig);
        }
        let deadline = tokio::time::Instant::now()
            .checked_add(Duration::from_millis(u64::from(config.timeout_ms)))
            .ok_or(CodeWaitError::InvalidConfig)?;
        let mut polls = 0;
        let work = async {
            loop {
                let result = target.observe(self).await;
                if tokio::time::Instant::now() >= deadline {
                    return Err(CodeWaitError::Timeout {
                        address: target.address,
                        polls_completed: polls,
                    });
                }
                let result = result?;
                polls += 1;
                if let Some(code) = result {
                    return Ok(code);
                }
                if polls >= config.max_polls {
                    return Err(CodeWaitError::PollLimit {
                        address: target.address,
                        polls_completed: polls,
                    });
                }
                tokio::time::sleep(Duration::from_millis(u64::from(config.poll_interval_ms))).await;
            }
        };
        match tokio::time::timeout_at(deadline, work).await {
            Ok(result) => result,
            Err(_) => Err(CodeWaitError::Timeout {
                address: target.address,
                polls_completed: polls,
            }),
        }
    }
}
