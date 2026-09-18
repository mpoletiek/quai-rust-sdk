use crate::{ConfirmedReceipt, Inclusion, Provider, ProviderError};
use quai_primitives::{Hash32, Zone};
use quai_rpc::Transport;
use std::time::Duration;
use thiserror::Error;

/// Explicit limits for native confirmation polling. No retry/submission is performed.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct WaitConfig {
    /// Positive block confirmation count, including the receipt's own block.
    pub confirmations: u64,
    /// Overall elapsed-time limit, including every RPC and polling delay.
    pub timeout: Duration,
    /// Positive delay between incomplete observations, no longer than timeout.
    pub poll_interval: Duration,
}
impl WaitConfig {
    /// Every limit is required: there is deliberately no default.
    pub const fn new(confirmations: u64, timeout: Duration, poll_interval: Duration) -> Self {
        Self {
            confirmations,
            timeout,
            poll_interval,
        }
    }
    /// Replace `confirmations`.
    pub const fn with_confirmations(mut self, confirmations: u64) -> Self {
        self.confirmations = confirmations;
        self
    }
    /// Replace `timeout`.
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
    /// Replace `poll_interval`.
    pub const fn with_poll_interval(mut self, poll_interval: Duration) -> Self {
        self.poll_interval = poll_interval;
        self
    }
}

/// Native confirmation-wait failures never imply a transaction was rejected or cancelled.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum WaitError {
    /// No RPC was made because configuration cannot produce a bounded poll loop.
    #[error("invalid confirmation wait limits")]
    InvalidConfig,
    /// Overall wait expired; receipt/transaction state may change afterwards.
    #[error("timed out waiting for transaction {transaction_hash}")]
    Timeout {
        /// Transaction that remains eligible for later reconciliation.
        transaction_hash: Hash32,
        /// Most recently observed inclusion, which may have been reorged away.
        last_observed_inclusion: Option<Inclusion>,
    },
    /// A read failed. The caller decides whether to start a new wait.
    #[error("transaction wait for {transaction_hash} failed: {source}")]
    Provider {
        /// Identity being watched.
        transaction_hash: Hash32,
        /// Read failure without automatic retry or reinterpretation as rejection.
        #[source]
        source: ProviderError,
    },
}

impl<T: Transport> Provider<T> {
    /// Wait for the original or a mined same-sender/nonce competitor, including
    /// candidates never registered by the wallet. Scans at most 256 blocks and
    /// 65,536 executed transactions per page, with 4,096 per block. Retains the
    /// next page anchor only within this future. Missing history does not advance
    /// coverage; changed anchors return an error requiring explicit restart from
    /// a trusted height. The overall timeout bounds reads and delays. A returned
    /// failed receipt still consumes the nonce. This never adopts a candidate
    /// into custody, releases a claim or submits a transaction.
    pub async fn wait_for_account_transaction(
        &self,
        original: &quai_consensus::SignedQuaiTransaction,
        genesis: Hash32,
        start_block: u64,
        config: WaitConfig,
    ) -> Result<crate::AccountNonceCandidate, WaitError> {
        if start_block == 0
            || start_block > i64::MAX as u64
            || config.confirmations == 0
            || config.timeout.is_zero()
            || config.poll_interval.is_zero()
            || config.poll_interval > config.timeout
            || tokio::time::Instant::now()
                .checked_add(config.timeout)
                .is_none()
        {
            return Err(WaitError::InvalidConfig);
        }
        let transaction_hash = original.hash().map_err(|_| WaitError::InvalidConfig)?;
        if genesis == Hash32::ZERO || original.transaction().chain_id != self.expected_chain_id {
            return Err(WaitError::InvalidConfig);
        }
        let mut tracker = crate::AccountReplacementTracker::new(
            original,
            genesis,
            start_block,
            config.confirmations,
        )
        .map_err(|_| WaitError::InvalidConfig)?;
        let mut last_observed_inclusion = None;
        let work = async {
            loop {
                match tracker.poll(self).await? {
                    crate::AccountReplacementPoll::Confirmed(candidate) => return Ok(*candidate),
                    crate::AccountReplacementPoll::Pending {
                        inclusion,
                        more_available,
                    } => {
                        last_observed_inclusion = inclusion;
                        if more_available {
                            continue;
                        }
                    }
                }
                tokio::time::sleep(config.poll_interval).await;
            }
        };
        match tokio::time::timeout(config.timeout, work).await {
            Ok(result) => result.map_err(|source| WaitError::Provider {
                transaction_hash,
                source,
            }),
            Err(_) => Err(WaitError::Timeout {
                transaction_hash,
                last_observed_inclusion,
            }),
        }
    }

    /// Wait for a receipt and observed confirmation depth, tolerating missing/reorged inclusion.
    ///
    /// Each candidate's block hash is checked against the canonical number lookup;
    /// the receipt and observed head are re-read before success. Separate RPC reads
    /// cannot eliminate time-of-check races or authenticate finality. A node can lie
    /// or reorg immediately afterwards. Failed execution receipts are returned with
    /// their explicit outcome. Dropping the future stops polling; it cannot cancel
    /// an already submitted transaction. RPC errors return immediately, without retry.
    pub async fn wait_for_receipt(
        &self,
        zone: Zone,
        transaction_hash: Hash32,
        config: WaitConfig,
    ) -> Result<ConfirmedReceipt, WaitError> {
        if config.confirmations == 0
            || config.timeout.is_zero()
            || config.poll_interval.is_zero()
            || config.poll_interval > config.timeout
            || tokio::time::Instant::now()
                .checked_add(config.timeout)
                .is_none()
        {
            return Err(WaitError::InvalidConfig);
        }
        let mut last_observed_inclusion = None;
        let work = async {
            loop {
                if let Some(receipt) = self
                    .confirmation_poll(
                        zone,
                        transaction_hash,
                        config.confirmations,
                        &mut last_observed_inclusion,
                    )
                    .await?
                {
                    return Ok(receipt);
                }
                tokio::time::sleep(config.poll_interval).await;
            }
        };
        match tokio::time::timeout(config.timeout, work).await {
            Ok(result) => result.map_err(|source| WaitError::Provider {
                transaction_hash,
                source,
            }),
            Err(_) => Err(WaitError::Timeout {
                transaction_hash,
                last_observed_inclusion,
            }),
        }
    }
    /// Wait for indexed transaction inclusion, including Qi without receipts.
    /// Canonical block position, refreshed transaction fields and head are checked
    /// by the portable observer. Deadline includes stalled RPCs; dropping the
    /// future cancels polling, never the transaction. Errors do not retry.
    pub async fn wait_for_transaction(
        &self,
        zone: Zone,
        transaction_hash: Hash32,
        config: WaitConfig,
    ) -> Result<crate::ConfirmedTransaction, WaitError> {
        if transaction_hash == Hash32::ZERO
            || config.confirmations == 0
            || config.timeout.is_zero()
            || config.poll_interval.is_zero()
            || config.poll_interval > config.timeout
            || tokio::time::Instant::now()
                .checked_add(config.timeout)
                .is_none()
        {
            return Err(WaitError::InvalidConfig);
        }
        let mut last_observed_inclusion = None;
        let work = async {
            loop {
                if let Some(tx) = self
                    .transaction_confirmation_poll(
                        zone,
                        transaction_hash,
                        config.confirmations,
                        &mut last_observed_inclusion,
                    )
                    .await?
                {
                    return Ok(tx);
                }
                tokio::time::sleep(config.poll_interval).await;
            }
        };
        match tokio::time::timeout(config.timeout, work).await {
            Ok(result) => result.map_err(|source| WaitError::Provider {
                transaction_hash,
                source,
            }),
            Err(_) => Err(WaitError::Timeout {
                transaction_hash,
                last_observed_inclusion,
            }),
        }
    }
}
