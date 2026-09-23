//! Signed-intent-bound wrapping, redemption and cross-zone ETX observations.
use crate::{
    BlockReference, ConversionOriginObservation, EtxCorrelation, EtxScanRequest, EtxScanResult,
    Provider, ProviderError, ReceiptOutcome, RpcData, Transaction, TransactionDetails,
    TransactionKind,
};
use quai_consensus::{SignedQiTransaction, SignedQiWrappingTransaction, SignedQuaiTransaction};
use quai_primitives::{Address, Hash32, Ledger, QiAddress, QuaiAddress, Zone};
use quai_rpc::{Transport, U256};

/// Exact nonconversion ETX expectation reconstructed from locally signed bytes.
/// Persist those bytes to reconstruct this identity after a restart.
#[derive(Clone, Debug)]
pub struct ExternalReference {
    chain_id: U256,
    genesis: Hash32,
    origin_zone: Zone,
    origin_kind: TransactionKind,
    origin_from: Option<Address>,
    origin_to: Option<Address>,
    key: EtxCorrelation,
    from: Address,
    to: Address,
    value: U256,
    data: RpcData,
    subtype: u64,
    gas: Option<u64>,
}
fn invalid(message: &'static str) -> ProviderError {
    ProviderError::InvalidRequest(message)
}
fn malformed(message: &'static str) -> ProviderError {
    ProviderError::InvalidResult(message)
}
impl ExternalReference {
    /// Native subtype-4 wrapping, distinct from subsequent claimDeposit token minting.
    pub fn from_qi_wrapping(
        genesis: Hash32,
        signed: &SignedQiWrappingTransaction,
    ) -> Result<Self, ProviderError> {
        let tx = signed.transaction();
        let zone = tx.origin_zone();
        let mut sender = [0; 20];
        sender[0] = zone.byte();
        let value = tx
            .transaction()
            .outputs
            .iter()
            .filter(|o| o.address.ledger() == Ledger::Quai)
            .try_fold(U256::ZERO, |v, o| {
                v.checked_add(U256::from(o.denomination.value()))
            })
            .ok_or(invalid("wrapping amount overflow"))?;
        let result = Self {
            chain_id: tx.chain_id(),
            genesis,
            origin_zone: zone,
            origin_kind: TransactionKind::Qi,
            origin_from: None,
            origin_to: Some(tx.intent().destination.address()),
            key: EtxCorrelation {
                originating_tx_hash: signed
                    .hash()
                    .map_err(|_| invalid("invalid wrapping signature"))?,
                etx_index: 0,
            },
            from: Address::from_bytes(sender),
            to: tx.intent().destination.address(),
            value,
            data: RpcData::new(tx.transaction().data.clone())?,
            subtype: 4,
            gas: None,
        };
        result.validate()?;
        Ok(result)
    }
    /// One cross-zone Qi output. Pinned go-quai carries a denomination index in
    /// the ETX value and retains the original Qi hash/output index for its UTXO.
    pub fn from_cross_zone_qi(
        genesis: Hash32,
        signed: &SignedQiTransaction,
        output_index: u16,
    ) -> Result<Self, ProviderError> {
        let tx = signed.transaction();
        let zone = tx.origin_zone().map_err(|_| invalid("invalid Qi origin"))?;
        let output = tx
            .outputs
            .get(output_index as usize)
            .ok_or(invalid("invalid Qi output index"))?;
        let to =
            QiAddress::try_from(output.address).map_err(|_| invalid("expected Qi destination"))?;
        if to.zone() == zone || !tx.data.is_empty() {
            return Err(invalid("expected ordinary cross-zone Qi output"));
        }
        let mut sender = [0; 20];
        sender[0] = zone.byte();
        let result = Self {
            chain_id: tx.chain_id,
            genesis,
            origin_zone: zone,
            origin_kind: TransactionKind::Qi,
            origin_from: None,
            origin_to: None,
            key: EtxCorrelation {
                originating_tx_hash: signed.hash().map_err(|_| invalid("invalid Qi signature"))?,
                etx_index: output_index,
            },
            from: Address::from_bytes(sender),
            to: to.address(),
            value: U256::from(output.denomination.index()),
            data: RpcData::default(),
            subtype: 0,
            gas: Some(21_000),
        };
        result.validate()?;
        Ok(result)
    }
    /// Direct cross-zone Quai transfer/call. Its origin nonce and asynchronous
    /// destination execution are separate; emitted index zero is bound to the intent.
    pub fn from_cross_zone_quai(
        genesis: Hash32,
        signed: &SignedQuaiTransaction,
    ) -> Result<Self, ProviderError> {
        let tx = signed.transaction();
        let to = tx
            .to
            .ok_or(invalid("cross-zone creation is not a direct transfer"))?;
        let destination =
            QuaiAddress::try_from(to).map_err(|_| invalid("expected Quai destination"))?;
        if destination.zone() == signed.from().zone() {
            return Err(invalid("expected another destination zone"));
        }
        let result = Self {
            chain_id: tx.chain_id,
            genesis,
            origin_zone: signed.from().zone(),
            origin_kind: TransactionKind::Quai,
            origin_from: Some(signed.from().address()),
            origin_to: Some(to),
            key: EtxCorrelation {
                originating_tx_hash: signed
                    .hash()
                    .map_err(|_| invalid("invalid account signature"))?,
                etx_index: 0,
            },
            from: signed.from().address(),
            to,
            value: tx.value,
            data: RpcData::new(tx.data.clone())?,
            subtype: 0,
            gas: None,
        };
        result.validate()?;
        Ok(result)
    }
    /// Bind the exact ABI `unwrapQi(address,uint256,uint64)` call to an explicit
    /// WQI contract. Amount is decoded from token atoms to exact Qits. The caller
    /// selects the ETX index (normally zero); contract behavior is still checked
    /// against the observed emission. This does not attest deployed bytecode.
    pub fn from_wqi_unwrap(
        genesis: Hash32,
        signed: &SignedQuaiTransaction,
        contract: QuaiAddress,
        etx_index: u16,
    ) -> Result<Self, ProviderError> {
        let tx = signed.transaction();
        let bytes = &tx.data;
        if tx.to != Some(contract.address())
            || signed.from().zone() != contract.zone()
            || tx.value != U256::ZERO
            || bytes.len() != 100
            || bytes[..4] != [0xbc, 0x35, 0xba, 0xfe]
            || bytes[4..16] != [0; 12]
            || bytes[68..92] != [0; 24]
        {
            return Err(invalid("not the exact reviewed WQI unwrap call"));
        }
        let destination = QiAddress::try_from(
            Address::try_from(&bytes[16..36]).map_err(|_| invalid("invalid redemption address"))?,
        )
        .map_err(|_| invalid("expected Qi redemption address"))?;
        let atoms = U256::from_be_slice(&bytes[36..68]);
        let scale = U256::from(1_000_000_000_000_000u64);
        let gas = u64::from_be_bytes(
            bytes[92..100]
                .try_into()
                .map_err(|_| invalid("invalid redemption gas"))?,
        );
        if atoms == U256::ZERO
            || atoms % scale != U256::ZERO
            || gas == 0
            || destination.zone() != contract.zone()
        {
            return Err(invalid("invalid exact redemption amount, gas or zone"));
        }
        let result = Self {
            chain_id: tx.chain_id,
            genesis,
            origin_zone: signed.from().zone(),
            origin_kind: TransactionKind::Quai,
            origin_from: Some(signed.from().address()),
            origin_to: Some(contract.address()),
            key: EtxCorrelation {
                originating_tx_hash: signed
                    .hash()
                    .map_err(|_| invalid("invalid account signature"))?,
                etx_index,
            },
            from: contract.address(),
            to: destination.address(),
            value: atoms / scale,
            data: RpcData::default(),
            subtype: 6,
            gas: Some(gas),
        };
        result.validate()?;
        Ok(result)
    }
    fn validate(&self) -> Result<(), ProviderError> {
        if self.genesis == Hash32::ZERO
            || self.chain_id == U256::ZERO
            || self.key.originating_tx_hash == Hash32::ZERO
        {
            return Err(invalid("invalid external network identity"));
        }
        Ok(())
    }
    /// Stable original hash and ETX index, independent of the final external hash.
    pub fn correlation(&self) -> EtxCorrelation {
        self.key
    }
    /// Destination zone for bounded execution scans.
    pub fn destination_zone(&self) -> Zone {
        self.to.zone().expect("constructor validates destination")
    }
    /// Origin zone whose canonical emission must be rechecked.
    pub fn origin_zone(&self) -> Zone {
        self.origin_zone
    }
    /// Exact destination beneficiary.
    pub fn destination(&self) -> Address {
        self.to
    }
    /// Expected wire value: Qits for wrapping/redemption, Its for account transfers,
    /// and a denomination index for a cross-zone Qi output.
    pub fn value(&self) -> U256 {
        self.value
    }
    fn check(&self, transaction: &Transaction) -> Result<(), ProviderError> {
        let TransactionDetails::External(etx) = &transaction.details else {
            return Err(malformed("expected external transaction"));
        };
        if etx.originating_tx_hash != self.key.originating_tx_hash
            || etx.etx_index != self.key.etx_index
            || etx.from != self.from
            || etx.to != self.to
            || etx.value != self.value
            || etx.etx_type != self.subtype
            || transaction.input != self.data
            || self.gas.is_some_and(|gas| gas != etx.gas)
        {
            return Err(malformed("external transaction differs from signed intent"));
        }
        Ok(())
    }
}
/// Source-reported execution outcome, separate from token minting or Qi maturity.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ExternalObservation {
    /// Canonical origin emission observation. An outbound ETX is not an execution.
    pub origin: ConversionOriginObservation,
    /// Explicit bounded destination scan, absent until an origin emission is observed.
    pub scan: Option<EtxScanResult>,
    /// Receipt outcome of the intent-matching executed ETX, if supplied.
    pub outcome: Option<ReceiptOutcome>,
}
#[cfg(feature = "test-fixtures")]
impl ExternalObservation {
    /// Test fixture taking every field; the outcome is not checked against the
    /// scan. Enabled by `test-fixtures`.
    pub fn new(
        origin: ConversionOriginObservation,
        scan: Option<EtxScanResult>,
        outcome: Option<ReceiptOutcome>,
    ) -> Self {
        Self {
            origin,
            scan,
            outcome,
        }
    }
}
impl<T: Transport> Provider<T> {
    /// Check signed wrapping/redemption/cross-zone intent against both origin
    /// emission and canonical destination execution. Does not infer finality,
    /// unlocked funds, a WQI claim or token balance from a successful receipt.
    pub async fn observe_external(
        &self,
        reference: &ExternalReference,
        request: EtxScanRequest,
    ) -> Result<ExternalObservation, ProviderError> {
        request.validate()?;
        if request.zone != reference.destination_zone() {
            return Err(invalid("external destination scan zone mismatch"));
        }
        for zone in [reference.origin_zone, request.zone] {
            if self.chain_id(zone.into()).await? != reference.chain_id
                || reference.chain_id != self.expected_chain_id
                || self.genesis_hash(zone).await? != reference.genesis
            {
                return Err(malformed("external network identity mismatch"));
            }
        }
        let mut result = ExternalObservation {
            origin: ConversionOriginObservation::Unavailable,
            scan: None,
            outcome: None,
        };
        let Some(receipt) = self
            .receipt(reference.origin_zone, reference.key.originating_tx_hash)
            .await?
        else {
            return Ok(result);
        };
        if receipt.kind != reference.origin_kind
            || receipt.from.is_some_and(|from| {
                reference
                    .origin_from
                    .is_some_and(|expected| expected != from)
            })
            || receipt
                .to
                .is_some_and(|to| reference.origin_to.is_some_and(|expected| expected != to))
        {
            return Err(malformed(
                "external origin receipt differs from signed intent",
            ));
        }
        let block = BlockReference {
            number: receipt.inclusion.block_number,
            hash: receipt.inclusion.block_hash,
        };
        let canonical = self
            .header_at(reference.origin_zone, block.number)
            .await?
            .map(|h| BlockReference {
                number: h.number,
                hash: h.hash,
            });
        if canonical != Some(block) {
            result.origin = ConversionOriginObservation::Noncanonical {
                reported: block,
                canonical,
            };
            return Ok(result);
        }
        if receipt.outcome == ReceiptOutcome::Failed {
            result.origin = ConversionOriginObservation::Failed { block };
            return Ok(result);
        }
        let mut emitted = None;
        for tx in receipt.outbound_etxs {
            if let TransactionDetails::External(etx) = &tx.details
                && etx.originating_tx_hash == reference.key.originating_tx_hash
                && etx.etx_index == reference.key.etx_index
            {
                if emitted.is_some() || tx.inclusion != Some(receipt.inclusion) {
                    return Err(malformed("duplicate or conflicting external emission"));
                }
                reference.check(&tx)?;
                emitted = Some(tx);
            }
        }
        let Some(emitted) = emitted else {
            result.origin = ConversionOriginObservation::EmissionNotObserved { block };
            return Ok(result);
        };
        let scan = self
            .scan_external_transactions(reference.key, request)
            .await?;
        if let Some(execution) = &scan.execution {
            reference.check(&execution.transaction)?;
            if reference.origin_zone == request.zone
                && execution
                    .transaction
                    .inclusion
                    .is_none_or(|i| i.block_number < block.number)
            {
                return Err(malformed("external execution predates origin"));
            }
            result.outcome = execution.receipt.as_ref().map(|r| r.outcome);
        }
        if self
            .header_at(reference.origin_zone, block.number)
            .await?
            .is_none_or(|h| h.hash != block.hash)
        {
            return Err(ProviderError::ObservationChanged);
        }
        result.origin = ConversionOriginObservation::Emitted {
            block,
            transaction: Box::new(emitted),
        };
        result.scan = Some(scan);
        Ok(result)
    }
}
