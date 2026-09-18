//! Bounded source-reported conversion correlation, without finality or maturity claims.
use crate::{
    Provider, ProviderError, Receipt, ReceiptOutcome, RpcData, Transaction, TransactionDetails,
    TransactionKind,
};
use quai_consensus::{QuaiToQiTransaction, SignedQiConversionTransaction, SignedQuaiTransaction};
use quai_primitives::{Address, Hash32, Ledger, QuaiAddress, Zone};
use quai_rpc::{Transport, U256};
use serde_json::{Map, Value, json};

/// A source-reported zone block identity, not a verified chain proof.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockReference {
    /// Canonical-by-number lookup height.
    pub number: u64,
    /// Work-object hash reported at that height.
    pub hash: Hash32,
}
/// Executed block transactions; emitted outbound ETXs are deliberately separate extensions.
#[derive(Clone, Debug, PartialEq)]
pub struct TransactionBlock {
    /// Reported block identity.
    pub block: BlockReference,
    /// Reported immediate parent, used to check contiguous scans.
    pub parent_hash: Hash32,
    /// Validated zone location.
    pub zone: Zone,
    /// Executed transactions in exact block-index order.
    pub transactions: Vec<Transaction>,
    /// Remaining block fields, including outboundEtxs; never interpreted as executions.
    pub extensions: Map<String, Value>,
}
/// Stable ETX correlation identity; prime processing may change the final transaction hash.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EtxCorrelation {
    /// Hash of the originally signed transaction.
    pub originating_tx_hash: Hash32,
    /// Original external-transaction index.
    pub etx_index: u16,
}
/// Explicit inclusive scan bounds. There is no implicit genesis-to-latest lookup.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct EtxScanRequest {
    /// Destination zone to read.
    pub zone: Zone,
    /// First zone block, at least one.
    pub from: u64,
    /// Last zone block, inclusive; at most 256 blocks in one scan.
    pub to: u64,
    /// Maximum executed transaction count per block, 1..=4096.
    pub max_transactions_per_block: usize,
    /// Maximum transactions examined across this scan, 1..=65,536.
    pub max_total_transactions: usize,
    /// Optional prior page anchor. It must immediately precede `from`.
    pub preceding_block: Option<BlockReference>,
}
impl EtxScanRequest {
    /// A page with no preceding anchor; set one with `with_preceding_block`.
    pub const fn new(
        zone: Zone,
        from: u64,
        to: u64,
        max_transactions_per_block: usize,
        max_total_transactions: usize,
    ) -> Self {
        Self {
            zone,
            from,
            to,
            max_transactions_per_block,
            max_total_transactions,
            preceding_block: None,
        }
    }
    /// Replace `zone`.
    pub const fn with_zone(mut self, zone: Zone) -> Self {
        self.zone = zone;
        self
    }
    /// Replace `from`.
    pub const fn with_from(mut self, from: u64) -> Self {
        self.from = from;
        self
    }
    /// Replace `to`.
    pub const fn with_to(mut self, to: u64) -> Self {
        self.to = to;
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
/// Completeness of this requested range only; neither variant proves complete wallet history.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanCoverage {
    /// Every requested block was supplied and the ending anchor was rechecked.
    Complete,
    /// A block was unavailable (future, pruned, unindexed or otherwise missing).
    Unavailable {
        /// First missing block. No later block was examined.
        block_number: u64,
    },
}
/// A final-hash ETX and its optionally available, association-checked receipt.
#[derive(Clone, Debug, PartialEq)]
pub struct EtxExecutionObservation {
    /// Executed external transaction with the correlation key preserved.
    pub transaction: Transaction,
    /// Missing means unknown; status one does not establish unlocked/spendable funds.
    pub receipt: Option<Receipt>,
}
/// A bounded, linked and rechecked source observation.
#[derive(Clone, Debug, PartialEq)]
pub struct EtxScanResult {
    /// Requested range completeness; absence is never reclassified as rejection.
    pub coverage: ScanCoverage,
    /// Last block examined, suitable as an explicit continuation-page anchor.
    pub last_block: Option<BlockReference>,
    /// Total executed transactions examined, including unrelated entries.
    pub transactions_examined: usize,
    /// At most one execution for this stable key. Conflicting matches fail closed.
    pub execution: Option<EtxExecutionObservation>,
}

/// Locally signed conversion identity and intent used to validate origin/final ETX observations.
#[derive(Clone, Debug)]
pub struct ConversionReference {
    chain_id: U256,
    genesis: Hash32,
    zone: Zone,
    correlation: EtxCorrelation,
    origin_kind: TransactionKind,
    destination: Address,
    refund: Address,
    emitted_from: Address,
    input: RpcData,
    value: U256,
}
impl ConversionReference {
    /// Bind a signed Qi conversion to an explicitly trusted genesis identity.
    pub fn from_qi(
        genesis: Hash32,
        signed: &SignedQiConversionTransaction,
    ) -> Result<Self, ProviderError> {
        if genesis == Hash32::ZERO {
            return Err(invalid_request("zero trusted genesis"));
        }
        let transaction = signed.transaction();
        let intent = transaction.intent();
        let zone = transaction.origin_zone();
        let mut zero = [0; 20];
        zero[0] = zone.byte();
        let value = transaction
            .transaction()
            .outputs
            .iter()
            .filter(|output| output.address.ledger() == Ledger::Quai)
            .try_fold(U256::ZERO, |value, output| {
                value.checked_add(U256::from(output.denomination.value()))
            })
            .ok_or(invalid_request("conversion amount overflow"))?;
        Ok(Self {
            chain_id: transaction.chain_id(),
            genesis,
            zone,
            correlation: EtxCorrelation {
                originating_tx_hash: signed
                    .hash()
                    .map_err(|_| invalid_request("invalid signed conversion"))?,
                etx_index: 0,
            },
            origin_kind: TransactionKind::Qi,
            destination: intent.destination.address(),
            refund: intent.refund.address(),
            emitted_from: Address::from_bytes(zero),
            input: RpcData::new(transaction.transaction().data.clone())?,
            value,
        })
    }
    /// Bind a signed direct Quai-to-Qi conversion. Contract-created ETXs require
    /// their own reviewed references and are not inferred by this constructor.
    pub fn from_quai(
        genesis: Hash32,
        signed: &SignedQuaiTransaction,
    ) -> Result<Self, ProviderError> {
        if genesis == Hash32::ZERO {
            return Err(invalid_request("zero trusted genesis"));
        }
        let transaction = QuaiToQiTransaction::new(signed.transaction().clone())
            .map_err(|_| invalid_request("not a direct Quai conversion"))?;
        Ok(Self {
            chain_id: signed.transaction().chain_id,
            genesis,
            zone: signed.from().zone(),
            correlation: EtxCorrelation {
                originating_tx_hash: signed
                    .hash()
                    .map_err(|_| invalid_request("invalid signed conversion"))?,
                etx_index: 0,
            },
            origin_kind: TransactionKind::Quai,
            destination: transaction.destination().address(),
            refund: signed.from().address(),
            emitted_from: signed.from().address(),
            input: RpcData::new(transaction.transaction().data.clone())?,
            value: signed.transaction().value,
        })
    }
    /// Stable key for pagination or a custom observer.
    pub fn correlation(&self) -> EtxCorrelation {
        self.correlation
    }
    /// Destination zone for these same-zone conversion types.
    pub fn zone(&self) -> Zone {
        self.zone
    }
    /// Exact original conversion destination; refunds may use a different beneficiary.
    pub fn destination(&self) -> Address {
        self.destination
    }
    /// Original refund beneficiary from the signed sender or conversion data.
    pub fn refund_destination(&self) -> Address {
        self.refund
    }
    fn check_etx(&self, transaction: &Transaction, origin: bool) -> Result<u64, ProviderError> {
        let TransactionDetails::External(etx) = &transaction.details else {
            return Err(invalid_result("conversion transaction is not external"));
        };
        if etx.originating_tx_hash != self.correlation.originating_tx_hash
            || etx.etx_index != self.correlation.etx_index
            || etx.to != self.destination
            || etx.from != self.emitted_from
            || transaction.input != self.input
            || (origin && etx.value != self.value)
        {
            return Err(invalid_result(
                "external transaction differs from signed conversion intent",
            ));
        }
        Ok(etx.etx_type)
    }
}
/// Current origin-receipt observation; unavailable/noncanonical do not mean rejected.
#[derive(Clone, Debug, PartialEq)]
pub enum ConversionOriginObservation {
    /// The node could not supply an origin receipt.
    Unavailable,
    /// The origin receipt names a block absent from the current canonical-number view.
    Noncanonical {
        /// Orphaned/source-conflicting inclusion.
        reported: BlockReference,
        /// Current canonical identity, if supplied.
        canonical: Option<BlockReference>,
    },
    /// A canonical receipt reports execution failure.
    Failed {
        /// Source-reported canonical origin inclusion.
        block: BlockReference,
    },
    /// A canonical origin receipt contains no ETX for the expected index.
    EmissionNotObserved {
        /// Source-reported canonical origin inclusion.
        block: BlockReference,
    },
    /// An intent-matching origin emission was observed. Its hash may change later.
    Emitted {
        /// Source-reported origin inclusion.
        block: BlockReference,
        /// Full source-reported initial ETX; not a final destination execution.
        transaction: Box<Transaction>,
    },
}
/// Interpretations of reported external subtypes under the pinned static mapping.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConversionEffect {
    /// Subtype 2 reports conversion processing; lock and spendability remain unverified.
    ConversionReported,
    /// Subtype 5 reports refund processing; refund beneficiary comes from original intent.
    RefundReported {
        /// Original Quai sender or signed Qi refund address, not necessarily ETX `to`.
        beneficiary: Address,
    },
    /// Unknown future/profile subtype is preserved without claiming its effect.
    UnknownSubtype(u64),
    /// The block includes the ETX but its execution receipt is unavailable.
    ReceiptUnavailable {
        /// Preserved numeric external subtype.
        etx_type: u64,
    },
    /// A receipt explicitly reports execution failure, not a successful conversion/refund.
    ExecutionFailed {
        /// Preserved numeric external subtype.
        etx_type: u64,
    },
    /// Status two explicitly reports locked value; it does not establish current maturity.
    Locked {
        /// Preserved conversion or refund subtype.
        etx_type: u64,
    },
    /// A legacy post-state root cannot be interpreted as a status by this observer.
    LegacyOutcome {
        /// Preserved numeric external subtype.
        etx_type: u64,
    },
}
/// Receipts cannot qualify whether converted/refunded funds are currently spendable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConversionSpendability {
    /// Lock status is lossy in RPC receipts; fork/maturity/state verification is outstanding.
    Unverified,
}
/// Current lifecycle observations; they are not a persistent monotonic state machine.
#[derive(Clone, Debug, PartialEq)]
pub struct ConversionObservation {
    /// Canonicality-checked origin observation.
    pub origin: ConversionOriginObservation,
    /// Destination range observation, only attempted after a matching origin emission.
    pub scan: Option<EtxScanResult>,
    /// Final subtype interpretation if an intent-matching execution was found.
    pub effect: Option<ConversionEffect>,
    /// Always unverified from this receipt/block-only observation.
    pub spendability: ConversionSpendability,
}
/// Latest-only aggregate locked balance; cannot attribute funds to a conversion or historical block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LockedBalanceObservation {
    /// Queried account.
    pub address: QuaiAddress,
    /// Node-reported aggregate currently locked base units.
    pub balance: U256,
    /// Header observed before the latest-only balance read.
    pub before: BlockReference,
    /// Header observed afterward. Even equality does not create an atomic pinned query.
    pub after: BlockReference,
}

fn invalid_request(message: &'static str) -> ProviderError {
    ProviderError::InvalidRequest(message)
}
fn invalid_result(message: &'static str) -> ProviderError {
    ProviderError::InvalidResult(message)
}
fn block_ref(header: crate::ZoneHeader) -> BlockReference {
    BlockReference {
        number: header.number,
        hash: header.hash,
    }
}
impl EtxScanRequest {
    pub(crate) fn validate(&self) -> Result<(), ProviderError> {
        if self.from == 0
            || self.to < self.from
            || self.to > i64::MAX as u64
            || self.to - self.from >= 256
            || !(1..=4096).contains(&self.max_transactions_per_block)
            || !(1..=65_536).contains(&self.max_total_transactions)
            || self.preceding_block.is_some_and(|block| {
                block.number.checked_add(1) != Some(self.from) || block.hash == Hash32::ZERO
            })
        {
            return Err(invalid_request("invalid bounded ETX scan"));
        }
        Ok(())
    }
}
impl<T: Transport> Provider<T> {
    /// Correlate a stable ETX key across a bounded contiguous canonical-number range.
    /// Full executed transactions are inspected; outboundEtxs are never mistaken
    /// for destination executions. Missing history yields explicit partial coverage.
    pub async fn scan_external_transactions(
        &self,
        key: EtxCorrelation,
        request: EtxScanRequest,
    ) -> Result<EtxScanResult, ProviderError> {
        request.validate()?;
        if key.originating_tx_hash == Hash32::ZERO {
            return Err(invalid_request("zero ETX origin"));
        }
        let mut result = EtxScanResult {
            coverage: ScanCoverage::Complete,
            last_block: None,
            transactions_examined: 0,
            execution: None,
        };
        let mut previous = request.preceding_block;
        for number in request.from..=request.to {
            let remaining = request
                .max_total_transactions
                .saturating_sub(result.transactions_examined);
            if remaining == 0 {
                return Err(invalid_result("ETX scan transaction budget exceeded"));
            }
            let Some(block) = self
                .block_with_transactions(
                    request.zone,
                    number,
                    request.max_transactions_per_block.min(remaining),
                )
                .await?
            else {
                result.coverage = ScanCoverage::Unavailable {
                    block_number: number,
                };
                break;
            };
            if previous.is_some_and(|previous| block.parent_hash != previous.hash) {
                return Err(ProviderError::ObservationChanged);
            }
            result.transactions_examined = result
                .transactions_examined
                .checked_add(block.transactions.len())
                .filter(|count| *count <= request.max_total_transactions)
                .ok_or(invalid_result("ETX scan transaction budget exceeded"))?;
            for transaction in block.transactions {
                let TransactionDetails::External(etx) = &transaction.details else {
                    continue;
                };
                if etx.originating_tx_hash != key.originating_tx_hash
                    || etx.etx_index != key.etx_index
                {
                    continue;
                }
                if result.execution.is_some() {
                    return Err(invalid_result(
                        "conflicting executions for one ETX correlation key",
                    ));
                }
                let receipt = self.receipt(request.zone, transaction.hash).await?;
                if let Some(receipt) = &receipt
                    && (receipt.kind != TransactionKind::External
                        || receipt.originating_tx_hash != Some(key.originating_tx_hash)
                        || receipt.etx_type != Some(etx.etx_type)
                        || Some(receipt.inclusion) != transaction.inclusion
                        || receipt.from.is_some_and(|from| from != etx.from)
                        || receipt.to.is_some_and(|to| to != etx.to))
                {
                    return Err(invalid_result("ETX receipt association mismatch"));
                }
                result.execution = Some(EtxExecutionObservation {
                    transaction,
                    receipt,
                });
            }
            previous = Some(block.block);
            result.last_block = Some(block.block);
        }
        if let Some(last) = result.last_block.or(request.preceding_block) {
            let canonical = if last.number == 0 {
                Some(BlockReference {
                    number: 0,
                    hash: self.genesis_hash(request.zone).await?,
                })
            } else {
                self.header_at(request.zone, last.number)
                    .await?
                    .map(block_ref)
            };
            if canonical != Some(last) {
                return Err(ProviderError::ObservationChanged);
            }
        }
        Ok(result)
    }
    /// Observe a signed conversion's canonical origin and matching destination ETX.
    /// Final hashes, converted amounts and subtypes can change at prime. Matching
    /// requires the stable key and unchanged original destination/sender/data.
    /// Status one does not distinguish a persisted locked receipt from spendable funds.
    pub async fn observe_conversion(
        &self,
        reference: &ConversionReference,
        request: EtxScanRequest,
    ) -> Result<ConversionObservation, ProviderError> {
        request.validate()?;
        if request.zone != reference.zone {
            return Err(invalid_request("conversion scan zone mismatch"));
        }
        if reference.chain_id != self.expected_chain_id {
            return Err(ProviderError::ChainMismatch {
                expected: self.expected_chain_id,
                actual: reference.chain_id,
            });
        }
        if self.genesis_hash(reference.zone).await? != reference.genesis {
            return Err(ProviderError::GenesisMismatch);
        }
        let mut result = ConversionObservation {
            origin: ConversionOriginObservation::Unavailable,
            scan: None,
            effect: None,
            spendability: ConversionSpendability::Unverified,
        };
        let Some(receipt) = self
            .receipt(reference.zone, reference.correlation.originating_tx_hash)
            .await?
        else {
            return Ok(result);
        };
        if receipt.kind != reference.origin_kind {
            return Err(invalid_result("conversion origin receipt kind mismatch"));
        }
        let origin_block = BlockReference {
            number: receipt.inclusion.block_number,
            hash: receipt.inclusion.block_hash,
        };
        let canonical = self
            .header_at(reference.zone, origin_block.number)
            .await?
            .map(block_ref);
        if canonical != Some(origin_block) {
            result.origin = ConversionOriginObservation::Noncanonical {
                reported: origin_block,
                canonical,
            };
            return Ok(result);
        }
        if receipt.from.is_some_and(|from| {
            reference.origin_kind == TransactionKind::Quai && from != reference.refund
        }) || receipt.to.is_some_and(|to| to != reference.destination)
        {
            return Err(invalid_result("origin receipt differs from signed intent"));
        }
        if receipt.outcome == ReceiptOutcome::Failed {
            result.origin = ConversionOriginObservation::Failed {
                block: origin_block,
            };
            return Ok(result);
        }
        let mut emitted = None;
        for transaction in receipt.outbound_etxs {
            let TransactionDetails::External(etx) = &transaction.details else {
                continue;
            };
            if etx.originating_tx_hash != reference.correlation.originating_tx_hash
                || etx.etx_index != reference.correlation.etx_index
            {
                continue;
            }
            if emitted.is_some() {
                return Err(invalid_result("duplicate origin ETX index"));
            }
            if transaction.inclusion != Some(receipt.inclusion) {
                return Err(invalid_result("origin emission inclusion mismatch"));
            }
            reference.check_etx(&transaction, true)?;
            emitted = Some(transaction);
        }
        let Some(emitted) = emitted else {
            result.origin = ConversionOriginObservation::EmissionNotObserved {
                block: origin_block,
            };
            return Ok(result);
        };
        let origin_subtype = reference.check_etx(&emitted, true)?;
        let scan = self
            .scan_external_transactions(reference.correlation, request)
            .await?;
        if let Some(execution) = &scan.execution {
            let subtype = reference.check_etx(&execution.transaction, false)?;
            if origin_subtype == 2
                && subtype == 5
                && let TransactionDetails::External(etx) = &execution.transaction.details
                && etx.value != reference.value
            {
                return Err(invalid_result(
                    "refund value differs from signed conversion amount",
                ));
            }
            if execution
                .transaction
                .inclusion
                .is_none_or(|inclusion| inclusion.block_number < origin_block.number)
            {
                return Err(invalid_result("destination execution predates origin"));
            }
            result.effect = Some(if origin_subtype != 2 {
                ConversionEffect::UnknownSubtype(origin_subtype)
            } else if subtype != 2 && subtype != 5 {
                ConversionEffect::UnknownSubtype(subtype)
            } else {
                match execution.receipt.as_ref().map(|receipt| receipt.outcome) {
                    None => ConversionEffect::ReceiptUnavailable { etx_type: subtype },
                    Some(ReceiptOutcome::Failed) => {
                        ConversionEffect::ExecutionFailed { etx_type: subtype }
                    }
                    Some(ReceiptOutcome::Locked) => ConversionEffect::Locked { etx_type: subtype },
                    Some(ReceiptOutcome::PostState(_)) => {
                        ConversionEffect::LegacyOutcome { etx_type: subtype }
                    }
                    Some(ReceiptOutcome::Succeeded) if subtype == 2 => {
                        ConversionEffect::ConversionReported
                    }
                    Some(ReceiptOutcome::Succeeded) => ConversionEffect::RefundReported {
                        beneficiary: reference.refund,
                    },
                }
            });
        }
        if self
            .header_at(reference.zone, origin_block.number)
            .await?
            .map(block_ref)
            != Some(origin_block)
        {
            return Err(ProviderError::ObservationChanged);
        }
        result.origin = ConversionOriginObservation::Emitted {
            block: origin_block,
            transaction: Box::new(emitted),
        };
        result.scan = Some(scan);
        Ok(result)
    }
    /// Observe current aggregate Quai locked balance with surrounding headers.
    /// The RPC has no historical block selector. Even equal headers do not prove
    /// an atomic observation, per-conversion attribution, maturity or spendability.
    pub async fn locked_quai_balance(
        &self,
        address: QuaiAddress,
    ) -> Result<LockedBalanceObservation, ProviderError> {
        let zone = address.zone();
        let before = self
            .latest_header(zone)
            .await?
            .map(block_ref)
            .ok_or(invalid_result("latest header unavailable"))?;
        let balance = crate::quantity(
            self.read(
                zone.into(),
                "quai_getLockedBalance",
                json!([address.to_string()]),
            )
            .await?,
        )?;
        let after = self
            .latest_header(zone)
            .await?
            .map(block_ref)
            .ok_or(invalid_result("latest header unavailable"))?;
        Ok(LockedBalanceObservation {
            address,
            balance,
            before,
            after,
        })
    }
}
