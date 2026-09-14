//! Portable transaction inclusion observations, including Qi without receipts.
use crate::{BlockHashes, Inclusion, MinedBlock, Provider, ProviderError, Transaction, ZoneHeader};
use quai_primitives::{Hash32, Zone};
use quai_rpc::Transport;

/// Rechecked indexed transaction inclusion; signature verification is separate.
#[derive(Clone, Debug)]
pub struct ConfirmedTransaction {
    /// Fresh transaction response. Inclusion and other metadata remain node claims.
    pub transaction: Transaction,
    /// Observed depth including the containing block.
    pub confirmations: u64,
    /// Head rechecked by number/hash before success.
    pub observed_head: ZoneHeader,
}
/// One bounded transaction observation without timers, retry or submission.
#[derive(Clone, Debug)]
pub enum TransactionConfirmation {
    /// Missing, insufficient depth, changed metadata or noncanonical inclusion.
    Pending {
        /// Last reported inclusion, which is not accepted as canonical.
        last_observed_inclusion: Option<Inclusion>,
    },
    /// Block position, transaction association and observed head were rechecked.
    Confirmed(Box<ConfirmedTransaction>),
}
impl<T: Transport> Provider<T> {
    /// Fetch this reported inclusion's block and require its exact hash and
    /// transaction position. At most 4,096 executed hashes are accepted. None
    /// means pending/unavailable/changed association, not rejection or cancellation.
    /// Does not verify transaction signatures or authenticate the node's chain.
    pub async fn transaction_block(
        &self,
        zone: Zone,
        transaction: &Transaction,
    ) -> Result<Option<BlockHashes>, ProviderError> {
        let Some(inclusion) = transaction.inclusion else {
            return Ok(None);
        };
        let Some(block) = self
            .block_hashes(zone, MinedBlock::Number(inclusion.block_number), 4096)
            .await?
        else {
            return Ok(None);
        };
        if block.block.hash != inclusion.block_hash
            || usize::try_from(inclusion.transaction_index)
                .ok()
                .and_then(|i| block.transactions.get(i))
                != Some(&transaction.hash)
        {
            return Ok(None);
        }
        Ok(Some(block))
    }
    /// Poll once for positive transaction depth without requiring a receipt.
    /// Works for Qi, Quai and indexed ETXs. Each typed read checks chain identity;
    /// at most five reads check transaction, head, inclusion block membership,
    /// transaction again and head again. Separate reads do not establish finality.
    /// Mutated transaction payloads or inclusion return pending; malformed source
    /// data/RPC failures return errors. No retry or custody changes occur.
    pub async fn observe_transaction_confirmation(
        &self,
        zone: Zone,
        transaction_hash: Hash32,
        required_confirmations: u64,
    ) -> Result<TransactionConfirmation, ProviderError> {
        let mut last = None;
        Ok(
            match self
                .transaction_confirmation_poll(
                    zone,
                    transaction_hash,
                    required_confirmations,
                    &mut last,
                )
                .await?
            {
                Some(tx) => TransactionConfirmation::Confirmed(Box::new(tx)),
                None => TransactionConfirmation::Pending {
                    last_observed_inclusion: last,
                },
            },
        )
    }
    pub(crate) async fn transaction_confirmation_poll(
        &self,
        zone: Zone,
        hash: Hash32,
        required: u64,
        last: &mut Option<Inclusion>,
    ) -> Result<Option<ConfirmedTransaction>, ProviderError> {
        if required == 0 || hash == Hash32::ZERO {
            return Err(ProviderError::InvalidRequest(
                "invalid transaction confirmation request",
            ));
        }
        let Some(transaction) = self.transaction(zone, hash).await? else {
            *last = None;
            return Ok(None);
        };
        *last = transaction.inclusion;
        let Some(inclusion) = transaction.inclusion else {
            return Ok(None);
        };
        let Some(head) = self.latest_header(zone).await? else {
            return Ok(None);
        };
        let Some(confirmations) = head
            .number
            .checked_sub(inclusion.block_number)
            .and_then(|n| n.checked_add(1))
        else {
            return Ok(None);
        };
        if confirmations < required || self.transaction_block(zone, &transaction).await?.is_none() {
            return Ok(None);
        }
        let Some(refreshed) = self.transaction(zone, hash).await? else {
            return Ok(None);
        };
        // Unknown extension metadata can change without changing signed contents.
        if refreshed.inclusion != transaction.inclusion
            || refreshed.input != transaction.input
            || refreshed.details != transaction.details
        {
            return Ok(None);
        }
        if !self
            .header_at(zone, head.number)
            .await?
            .is_some_and(|h| h.hash == head.hash)
        {
            return Ok(None);
        }
        Ok(Some(ConfirmedTransaction {
            transaction: refreshed,
            confirmations,
            observed_head: head,
        }))
    }
}
