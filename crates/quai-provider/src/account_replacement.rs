//! Bounded discovery of mined account nonce competitors, independent of custody.
use crate::{
    BlockReference, Inclusion, Provider, ProviderError, Receipt, TransactionDetails,
    TransactionKind, ZoneHeader,
};
use quai_consensus::SignedQuaiTransaction;
use quai_primitives::Hash32;
use quai_rpc::Transport;

/// Classification used by the pinned quais.js account response waiter. This is
/// descriptive, never authorization to adopt a transaction or release a claim.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ReplacementReason {
    /// Same recipient, value and data; gas/access-list changes are not compared.
    Repriced,
    /// Empty-data zero-value transfer to the sender, after the repricing check.
    Cancelled,
    /// Other same-sender, same-nonce transaction, including different recipients.
    Replaced,
}
/// One explicit page of canonical-by-number account transaction discovery.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct AccountReplacementScanRequest {
    /// Positive inclusive start; never silently advances across missing history.
    pub from_block: u64,
    /// Inclusive end, at most 256 blocks from start and not beyond sampled head.
    pub to_block: u64,
    /// Per-block executed transaction budget, 1..=4096.
    pub max_transactions_per_block: usize,
    /// Total transaction budget across this page, 1..=65536.
    pub max_total_transactions: usize,
    /// Optional previously checked page end; must directly precede this page.
    pub preceding_block: Option<BlockReference>,
}
impl AccountReplacementScanRequest {
    /// A page with no preceding anchor; set one with `with_preceding_block`.
    pub const fn new(
        from_block: u64,
        to_block: u64,
        max_transactions_per_block: usize,
        max_total_transactions: usize,
    ) -> Self {
        Self {
            from_block,
            to_block,
            max_transactions_per_block,
            max_total_transactions,
            preceding_block: None,
        }
    }
    /// Replace `from_block`.
    pub const fn with_from_block(mut self, from_block: u64) -> Self {
        self.from_block = from_block;
        self
    }
    /// Replace `to_block`.
    pub const fn with_to_block(mut self, to_block: u64) -> Self {
        self.to_block = to_block;
        self
    }
    /// Replace `max_transactions_per_block`.
    pub const fn with_max_transactions_per_block(
        mut self,
        max_transactions_per_block: usize,
    ) -> Self {
        self.max_transactions_per_block = max_transactions_per_block;
        self
    }
    /// Replace `max_total_transactions`.
    pub const fn with_max_total_transactions(mut self, max_total_transactions: usize) -> Self {
        self.max_total_transactions = max_total_transactions;
        self
    }
    /// Replace `preceding_block`.
    pub const fn with_preceding_block(mut self, preceding_block: Option<BlockReference>) -> Self {
        self.preceding_block = preceding_block;
        self
    }
}
/// A cryptographically verified transaction occupying the watched sender/nonce.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct AccountNonceCandidate {
    /// Exact verified signed transaction, possibly not known to the wallet.
    pub transaction: SignedQuaiTransaction,
    /// None for the original identity; otherwise descriptive replacement class.
    pub reason: Option<ReplacementReason>,
    /// Canonical-by-number inclusion, rechecked before returning.
    pub inclusion: Inclusion,
    /// Re-read receipt, when indexed. Failed execution still consumes the nonce.
    pub receipt: Option<Receipt>,
    /// Observed depth including this block; not a finality guarantee.
    pub confirmations: u64,
}
impl AccountNonceCandidate {
    /// An included candidate with no reason, receipt or confirmations recorded.
    pub fn new(transaction: SignedQuaiTransaction, inclusion: Inclusion) -> Self {
        Self {
            transaction,
            reason: None,
            inclusion,
            receipt: None,
            confirmations: 0,
        }
    }
}
/// Advisory bounded scan result. Absence only covers the reported page prefix.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct AccountReplacementScan {
    /// At most one verified occupant; competing canonical occupants reject.
    pub candidate: Option<AccountNonceCandidate>,
    /// Last contiguous block examined; supply as next page's preceding anchor.
    pub scanned_through: Option<BlockReference>,
    /// First unavailable block, if the page stopped before its requested end.
    pub missing_block: Option<u64>,
    /// Rechecked head used for confirmation counts.
    pub observed_head: ZoneHeader,
}

/// Result of one portable replacement-tracker poll; no timer or custody change.
#[derive(Clone, Debug)]
pub enum AccountReplacementPoll {
    /// No indexed occupant has the requested depth yet.
    Pending {
        /// Latest completed page's candidate inclusion, if any.
        inclusion: Option<Inclusion>,
        /// Another bounded page is already available at the sampled head.
        more_available: bool,
    },
    /// Verified original or competitor with an indexed, rechecked receipt.
    Confirmed(Box<AccountNonceCandidate>),
}
/// In-memory, timer-independent cursor for successive canonical replacement pages.
/// A failed or cancelled poll never advances the cursor. Reorgs fail explicitly;
/// reconstruct from a trusted start height rather than trusting a stale cursor.
pub struct AccountReplacementTracker {
    original: SignedQuaiTransaction,
    genesis: Hash32,
    from_block: u64,
    preceding_block: Option<BlockReference>,
    confirmations: u64,
}
impl AccountReplacementTracker {
    /// Validate an immutable original, nonzero trusted genesis, positive start and
    /// required depth before I/O. Provider chain binding is checked on every poll.
    pub fn new(
        original: &SignedQuaiTransaction,
        genesis: Hash32,
        start_block: u64,
        confirmations: u64,
    ) -> Result<Self, ProviderError> {
        if genesis == Hash32::ZERO
            || original.transaction().chain_id == quai_rpc::U256::ZERO
            || start_block == 0
            || start_block > i64::MAX as u64
            || confirmations == 0
            || original.hash().is_err()
        {
            return Err(ProviderError::InvalidRequest(
                "invalid account replacement tracker",
            ));
        }
        Ok(Self {
            original: original.clone(),
            genesis,
            from_block: start_block,
            preceding_block: None,
            confirmations,
        })
    }
    /// Observe at most one 256-block page under the observer's transaction bounds.
    /// Missing history stops advancement, and insufficient-depth candidates keep
    /// their page under observation. More-available is only a scheduling hint.
    pub async fn poll<T: Transport>(
        &mut self,
        provider: &Provider<T>,
    ) -> Result<AccountReplacementPoll, ProviderError> {
        if self.original.transaction().chain_id != provider.expected_chain_id {
            return Err(ProviderError::InvalidRequest(
                "replacement tracker chain mismatch",
            ));
        }
        let zone = self.original.from().zone();
        if provider.genesis_hash(zone).await? != self.genesis {
            return Err(ProviderError::ObservationChanged);
        }
        if let Some(prior) = self.preceding_block
            && provider
                .header_at(zone, prior.number)
                .await?
                .is_none_or(|h| h.hash != prior.hash)
        {
            return Err(ProviderError::ObservationChanged);
        }
        let Some(head) = provider.latest_header(self.original.from().zone()).await? else {
            return Ok(AccountReplacementPoll::Pending {
                inclusion: None,
                more_available: false,
            });
        };
        if head.number < self.from_block {
            return Ok(AccountReplacementPoll::Pending {
                inclusion: None,
                more_available: false,
            });
        }
        let page = provider
            .observe_account_replacements(
                &self.original,
                self.genesis,
                AccountReplacementScanRequest {
                    from_block: self.from_block,
                    to_block: head.number.min(self.from_block.saturating_add(255)),
                    max_transactions_per_block: 4096,
                    max_total_transactions: 65536,
                    preceding_block: self.preceding_block,
                },
            )
            .await?;
        if let Some(candidate) = page.candidate {
            if candidate.receipt.is_some() && candidate.confirmations >= self.confirmations {
                return Ok(AccountReplacementPoll::Confirmed(Box::new(candidate)));
            }
            return Ok(AccountReplacementPoll::Pending {
                inclusion: Some(candidate.inclusion),
                more_available: false,
            });
        }
        if page.missing_block.is_none()
            && let Some(end) = page.scanned_through
        {
            let next = end
                .number
                .checked_add(1)
                .ok_or(ProviderError::ObservationChanged)?;
            self.preceding_block = Some(end);
            self.from_block = next;
            return Ok(AccountReplacementPoll::Pending {
                inclusion: None,
                more_available: next <= head.number,
            });
        }
        Ok(AccountReplacementPoll::Pending {
            inclusion: None,
            more_available: false,
        })
    }
}
impl AccountReplacementScanRequest {
    fn validate(self) -> Result<(), ProviderError> {
        if self.from_block == 0
            || self.to_block < self.from_block
            || self.to_block > i64::MAX as u64
            || self.to_block - self.from_block >= 256
            || !(1..=4096).contains(&self.max_transactions_per_block)
            || !(1..=65536).contains(&self.max_total_transactions)
            || self.preceding_block.is_some_and(|b| {
                b.number.checked_add(1) != Some(self.from_block) || b.hash == Hash32::ZERO
            })
        {
            return Err(ProviderError::InvalidRequest(
                "invalid account replacement scan limits",
            ));
        }
        Ok(())
    }
}
impl<T: Transport> Provider<T> {
    /// Discover mined replacements without requiring prior candidate registration.
    /// Every matching transaction is signature/hash verified, including an original
    /// match. The scan checks parent links, receipts, head and trusted genesis.
    /// Missing blocks stop coverage; changed anchors fail. No custody write, claim
    /// release, send, sleep or unbounded scan occurs. The caller retains the start
    /// block and explicitly repeats/revalidates pages after reorgs or restart.
    /// A malicious source can still omit transactions or lie about canonicality.
    pub async fn observe_account_replacements(
        &self,
        original: &SignedQuaiTransaction,
        genesis: Hash32,
        request: AccountReplacementScanRequest,
    ) -> Result<AccountReplacementScan, ProviderError> {
        request.validate()?;
        let tx = original.transaction();
        let zone = original.from().zone();
        if genesis == Hash32::ZERO || tx.chain_id != self.expected_chain_id {
            return Err(ProviderError::InvalidRequest(
                "invalid signed replacement scope",
            ));
        }
        let original_hash = original
            .hash()
            .map_err(|_| ProviderError::InvalidRequest("invalid signed original"))?;
        if self.genesis_hash(zone).await? != genesis {
            return Err(ProviderError::ObservationChanged);
        }
        let head = self
            .latest_header(zone)
            .await?
            .ok_or(ProviderError::ObservationChanged)?;
        if request.to_block > head.number {
            return Err(ProviderError::InvalidRequest(
                "replacement scan exceeds observed head",
            ));
        }
        if let Some(before) = request.preceding_block {
            if before.number == 0 {
                if before.hash != genesis {
                    return Err(ProviderError::ObservationChanged);
                }
            } else if self
                .header_at(zone, before.number)
                .await?
                .is_none_or(|h| h.hash != before.hash)
            {
                return Err(ProviderError::ObservationChanged);
            }
        }
        let mut anchors = Vec::new();
        let mut prior = request.preceding_block;
        let mut missing_block = None;
        let mut candidate: Option<AccountNonceCandidate> = None;
        let mut total = 0usize;
        for number in request.from_block..=request.to_block {
            let Some(block) = self
                .block_with_transactions(zone, number, request.max_transactions_per_block)
                .await?
            else {
                missing_block = Some(number);
                break;
            };
            total =
                total
                    .checked_add(block.transactions.len())
                    .ok_or(ProviderError::InvalidResult(
                        "replacement scan budget overflow",
                    ))?;
            if total > request.max_total_transactions {
                return Err(ProviderError::InvalidResult(
                    "replacement scan budget exceeded",
                ));
            }
            if prior.is_some_and(|p| block.parent_hash != p.hash) {
                return Err(ProviderError::ObservationChanged);
            }
            prior = Some(block.block);
            anchors.push(block.block);
            for found in block.transactions {
                let TransactionDetails::Quai(fields) = &found.details else {
                    continue;
                };
                if fields.from != original.from() || fields.nonce != tx.nonce {
                    continue;
                }
                if candidate.is_some() {
                    return Err(ProviderError::InvalidResult(
                        "competing canonical account nonce occupants",
                    ));
                }
                let signed = found.verified_quai()?;
                let reason = if found.hash == original_hash {
                    None
                } else if fields.to == tx.to
                    && fields.value == tx.value
                    && found.input.bytes() == tx.data
                {
                    Some(ReplacementReason::Repriced)
                } else if fields.to == Some(fields.from.address())
                    && fields.value == quai_rpc::U256::ZERO
                    && found.input.bytes().is_empty()
                {
                    Some(ReplacementReason::Cancelled)
                } else {
                    Some(ReplacementReason::Replaced)
                };
                let inclusion = found.inclusion.ok_or(ProviderError::InvalidResult(
                    "mined candidate lacks inclusion",
                ))?;
                candidate = Some(AccountNonceCandidate {
                    transaction: signed,
                    reason,
                    inclusion,
                    receipt: None,
                    confirmations: head.number - number + 1,
                });
            }
        }
        if let Some(c) = &mut candidate {
            let hash = c
                .transaction
                .hash()
                .map_err(|_| ProviderError::InvalidResult("invalid candidate identity"))?;
            let first = self.receipt(zone, hash).await?;
            let second = self.receipt(zone, hash).await?;
            if first != second {
                return Err(ProviderError::ObservationChanged);
            }
            if let Some(receipt) = second {
                if receipt.kind != TransactionKind::Quai
                    || receipt.inclusion != c.inclusion
                    || receipt.from != Some(original.from().address())
                    || receipt.to != c.transaction.transaction().to
                {
                    return Err(ProviderError::InvalidResult(
                        "replacement receipt association mismatch",
                    ));
                }
                c.receipt = Some(receipt);
            }
        }
        // Recheck every scanned anchor, not just a candidate or the last block.
        for block in request
            .preceding_block
            .iter()
            .chain(anchors.iter())
            .filter(|b| b.number > 0)
        {
            if self
                .header_at(zone, block.number)
                .await?
                .is_none_or(|h| h.hash != block.hash)
            {
                return Err(ProviderError::ObservationChanged);
            }
        }
        if self
            .header_at(zone, head.number)
            .await?
            .is_none_or(|h| h.hash != head.hash)
            || self.genesis_hash(zone).await? != genesis
        {
            return Err(ProviderError::ObservationChanged);
        }
        Ok(AccountReplacementScan {
            candidate,
            scanned_through: anchors.last().copied(),
            missing_block,
            observed_head: head,
        })
    }
}
