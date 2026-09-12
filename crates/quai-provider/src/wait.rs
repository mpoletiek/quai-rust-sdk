use crate::{Inclusion, Provider, ProviderError, Receipt, ZoneHeader};
use quai_primitives::{Hash32, Zone};
use quai_rpc::Transport;
use std::time::Duration;
use thiserror::Error;

/// Explicit limits for native receipt polling. No retry/submission is performed.
#[derive(Clone, Copy, Debug)]
pub struct WaitConfig {
    /// Positive block confirmation count, including the receipt's own block.
    pub confirmations: u64,
    /// Overall elapsed-time limit, including every RPC and polling delay.
    pub timeout: Duration,
    /// Positive delay between incomplete observations, no longer than timeout.
    pub poll_interval: Duration,
}

/// A receipt that passed the requested observed confirmations and consistency checks.
#[derive(Clone, Debug)]
pub struct ConfirmedReceipt {
    /// Receipt re-read after validating its block association.
    pub receipt: Receipt,
    /// Observed confirmations, including the containing block.
    pub confirmations: u64,
    /// Observed head used for the count and subsequently checked by number/hash.
    pub observed_head: ZoneHeader,
}

/// Native receipt-wait failures never imply a transaction was rejected or cancelled.
#[derive(Debug, Error)]
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
    #[error("receipt wait for transaction {transaction_hash} failed: {source}")]
    Provider {
        /// Identity being watched.
        transaction_hash: Hash32,
        /// Read failure without automatic retry or reinterpretation as rejection.
        #[source]
        source: ProviderError,
    },
}

impl<T: Transport> Provider<T> {
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
                let candidate = self.receipt(zone, transaction_hash).await?;
                if let Some(receipt) = candidate {
                    last_observed_inclusion = Some(receipt.inclusion);
                    let observed_head = self.latest_header(zone).await?;
                    if let Some(head) = observed_head
                        && head.number >= receipt.inclusion.block_number
                    {
                        let confirmations = (head.number - receipt.inclusion.block_number)
                            .checked_add(1)
                            .ok_or(ProviderError::InvalidResult("confirmation count overflow"))?;
                        if confirmations >= config.confirmations {
                            let canonical =
                                self.header_at(zone, receipt.inclusion.block_number).await?;
                            if canonical
                                .is_some_and(|header| header.hash == receipt.inclusion.block_hash)
                            {
                                let latest_receipt = self.receipt(zone, transaction_hash).await?;
                                if let Some(latest_receipt) = latest_receipt
                                    && latest_receipt.inclusion == receipt.inclusion
                                {
                                    let canonical_head = self.header_at(zone, head.number).await?;
                                    if canonical_head.is_some_and(|header| header.hash == head.hash)
                                    {
                                        return Ok(ConfirmedReceipt {
                                            receipt: latest_receipt,
                                            confirmations,
                                            observed_head: head,
                                        });
                                    }
                                }
                            }
                        }
                    }
                } else {
                    last_observed_inclusion = None;
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
