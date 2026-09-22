//! Explicit bounded mined-block lookup; hash lookup does not assert canonicality.
use crate::{
    BlockReference, Provider, ProviderError, Transaction, TransactionBlock, TransactionDetails,
    ZoneHeader, types,
};
use quai_primitives::{Hash32, Zone};
use quai_rpc::Transport;
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
/// A mined block selector. Hash lookup may return an orphan; latest is sampled once.
/// Genesis has a root location and uses the separate genesis identity API.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MinedBlock {
    /// Current canonical head reported by the source.
    Latest,
    /// Positive canonical zone height in the node's signed 64-bit range.
    Number(u64),
    /// Exact nonzero block identity; canonicality is not implied.
    Hash(Hash32),
}
impl MinedBlock {
    fn number(self) -> Option<u64> {
        if let Self::Number(n) = self {
            Some(n)
        } else {
            None
        }
    }
    fn hash(self) -> Option<Hash32> {
        if let Self::Hash(h) = self {
            Some(h)
        } else {
            None
        }
    }
    fn rpc(self) -> Result<(&'static str, String), ProviderError> {
        match self {
            Self::Latest => Ok(("quai_getBlockByNumber", "latest".into())),
            Self::Number(n) if n > 0 && n <= i64::MAX as u64 => {
                Ok(("quai_getBlockByNumber", format!("{n:#x}")))
            }
            Self::Hash(h) if h != Hash32::ZERO => Ok(("quai_getBlockByHash", h.to_string())),
            _ => Err(ProviderError::InvalidRequest(
                "invalid mined block selector",
            )),
        }
    }
}
fn invalid_result(message: &'static str) -> ProviderError {
    ProviderError::InvalidResult(message)
}
fn take(object: &mut Map<String, Value>, field: &'static str) -> Result<Value, ProviderError> {
    object
        .remove(field)
        .ok_or(invalid_result("missing block field"))
}
fn object(value: Value) -> Result<Map<String, Value>, ProviderError> {
    if let Value::Object(o) = value {
        Ok(o)
    } else {
        Err(invalid_result("expected block object"))
    }
}

/// A mined block containing executed transaction identities only. This saves
/// transport/decoding work but cannot validate each transaction's inclusion fields.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct BlockHashes {
    /// Reported block identity; hash lookup does not imply canonicality.
    pub block: BlockReference,
    /// Reported immediate parent.
    pub parent_hash: Hash32,
    /// Validated block location.
    pub zone: Zone,
    /// Unique nonzero executed transaction hashes in block order.
    pub transactions: Vec<Hash32>,
    /// Remaining block fields, including outbound ETXs, not treated as executions.
    pub extensions: Map<String, Value>,
}
impl<T: Transport> Provider<T> {
    /// Read executed transaction hashes for a mined block with a 1..4096 item budget.
    /// Null means unavailable; RPC errors, including the pinned node's unknown-hash
    /// error, propagate without being reclassified as missing or dropped.
    pub async fn block_hashes(
        &self,
        zone: Zone,
        selector: MinedBlock,
        max_transactions: usize,
    ) -> Result<Option<BlockHashes>, ProviderError> {
        let (method, parameter) = selector.rpc()?;
        if !(1..=4096).contains(&max_transactions) {
            return Err(ProviderError::InvalidRequest(
                "invalid block transaction budget",
            ));
        }
        let value = self
            .read(zone.into(), method, json!([parameter, false]))
            .await?;
        if value.is_null() {
            return Ok(None);
        }
        let (block, parent_hash, mut fields) = block_fields(value, zone, selector)?;
        let Value::Array(values) = take(&mut fields, "transactions")? else {
            return Err(invalid_result("expected block transaction hashes"));
        };
        if values.len() > max_transactions {
            return Err(invalid_result("block transaction budget exceeded"));
        }
        let mut transactions = Vec::with_capacity(values.len());
        let mut seen = BTreeSet::new();
        for value in values {
            let hash = types::hash(value)?;
            if hash == Hash32::ZERO || !seen.insert(hash) {
                return Err(invalid_result("invalid or duplicate transaction hash"));
            }
            transactions.push(hash);
        }
        Ok(Some(BlockHashes {
            block,
            parent_hash,
            zone,
            transactions,
            extensions: fields,
        }))
    }

    /// Read an exact zone header by hash. This can report an orphaned header;
    /// compare with header_at separately when canonicality matters.
    pub async fn header_by_hash(
        &self,
        zone: Zone,
        hash: Hash32,
    ) -> Result<Option<ZoneHeader>, ProviderError> {
        if hash == Hash32::ZERO {
            return Err(ProviderError::InvalidRequest("zero header hash"));
        }
        let value = self
            .read(
                zone.into(),
                "quai_getHeaderByHash",
                json!([hash.to_string()]),
            )
            .await?;
        if value.is_null() {
            return Ok(None);
        }
        let header = ZoneHeader::try_from(value)?;
        if header.hash != hash
            || header.zone != zone
            || header.number == 0
            || header.number > i64::MAX as u64
        {
            return Err(invalid_result("header hash or zone mismatch"));
        }
        Ok(Some(header))
    }
    /// Read executed transactions at one explicit canonical-by-number zone height.
    /// Every inclusion/index, chain ID and block hash is validated.
    pub async fn block_with_transactions(
        &self,
        zone: Zone,
        number: u64,
        max_transactions: usize,
    ) -> Result<Option<TransactionBlock>, ProviderError> {
        self.mined_block(zone, MinedBlock::Number(number), max_transactions)
            .await
    }
    /// Read a full mined zone block by latest, number or hash with a caller budget
    /// of 1..4096 transactions. Outbound ETXs remain separate extensions.
    /// A missing response is unknown, and node errors are propagated unchanged.
    pub async fn mined_block(
        &self,
        zone: Zone,
        selector: MinedBlock,
        max_transactions: usize,
    ) -> Result<Option<TransactionBlock>, ProviderError> {
        let (method, parameter) = selector.rpc()?;
        if !(1..=4096).contains(&max_transactions) {
            return Err(ProviderError::InvalidRequest(
                "invalid block transaction budget",
            ));
        }
        let value = self
            .read(zone.into(), method, json!([parameter, true]))
            .await?;
        if value.is_null() {
            return Ok(None);
        }
        let (block, parent_hash, mut fields) = block_fields(value, zone, selector)?;
        let BlockReference { number, hash } = block;
        let Value::Array(values) = take(&mut fields, "transactions")? else {
            return Err(invalid_result("expected full block transactions"));
        };
        if values.len() > max_transactions {
            return Err(invalid_result("block transaction budget exceeded"));
        }
        let mut transactions = Vec::with_capacity(values.len());
        let mut hashes = BTreeSet::new();
        for (index, value) in values.into_iter().enumerate() {
            let transaction = Transaction::try_from(value)?;
            let expected = crate::Inclusion {
                block_hash: hash,
                block_number: number,
                transaction_index: index as u64,
            };
            if transaction.inclusion != Some(expected)
                || !hashes.insert(transaction.hash)
                || transaction.hash == Hash32::ZERO
            {
                return Err(invalid_result("block transaction inclusion mismatch"));
            }
            let chain = match &transaction.details {
                TransactionDetails::Quai(tx) => Some(tx.chain_id),
                TransactionDetails::Qi(tx) => Some(tx.chain_id),
                TransactionDetails::External(_) => None,
            };
            if let Some(actual) = chain
                && actual != self.expected_chain_id
            {
                return Err(ProviderError::ChainMismatch {
                    expected: self.expected_chain_id,
                    actual,
                });
            }
            transactions.push(transaction);
        }
        Ok(Some(TransactionBlock {
            block: BlockReference { number, hash },
            parent_hash,
            zone,
            transactions,
            extensions: fields,
        }))
    }
}

pub(crate) fn block_fields(
    value: Value,
    zone: Zone,
    selector: MinedBlock,
) -> Result<(BlockReference, Hash32, Map<String, Value>), ProviderError> {
    let mut fields = object(value)?;
    let hash = types::hash(take(&mut fields, "hash")?)?;
    let work = object(take(&mut fields, "woHeader")?)?;
    let reported_hash = types::hash(
        work.get("hash")
            .cloned()
            .ok_or(invalid_result("missing work hash"))?,
    )?;
    let reported_number = types::uint64(
        work.get("number")
            .cloned()
            .ok_or(invalid_result("missing work number"))?,
    )?;
    let parent_hash = types::hash(
        work.get("parentHash")
            .cloned()
            .ok_or(invalid_result("missing parent hash"))?,
    )?;
    let location = types::data(
        work.get("location")
            .cloned()
            .ok_or(invalid_result("missing work location"))?,
    )?;
    let expected_location = [zone.byte() >> 4, zone.byte() & 15];
    if reported_number == 0
        || reported_number > i64::MAX as u64
        || hash != reported_hash
        || hash == Hash32::ZERO
        || selector
            .number()
            .is_some_and(|number| number != reported_number)
        || selector.hash().is_some_and(|expected| expected != hash)
        || location.bytes() != expected_location
    {
        return Err(invalid_result("block identity or location mismatch"));
    }
    fields.insert("woHeader".into(), Value::Object(work));
    Ok((
        BlockReference {
            number: reported_number,
            hash,
        },
        parent_hash,
        fields,
    ))
}

/// Exact transaction lookup without numeric/string coercion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockTransactionId {
    /// Execution or outbound-array position.
    Index(usize),
    /// Exact transaction hash.
    Hash(Hash32),
}
/// An outbound item is distinct from an executed transaction in this block.
#[derive(Clone, Debug, PartialEq)]
pub enum OutboundBlockTransaction {
    /// Not prefetched; no execution or destination inclusion can be inferred.
    Hash(Hash32),
    /// Parsed external transaction. Its inclusion fields remain source claims.
    Prefetched(Box<Transaction>),
}
impl OutboundBlockTransaction {
    /// Source-reported identity, not cryptographically authenticated ETX evidence.
    pub fn hash(&self) -> Hash32 {
        match self {
            Self::Hash(h) => *h,
            Self::Prefetched(t) => t.hash,
        }
    }
}
/// Borrowed block metadata; field parsing is explicit and performs no RPC.
#[derive(Clone, Copy)]
pub struct BlockMetadata<'a> {
    fields: &'a Map<String, Value>,
}
impl std::fmt::Debug for BlockMetadata<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlockMetadata")
            .field("field_count", &self.fields.len())
            .finish_non_exhaustive()
    }
}
impl<'a> BlockMetadata<'a> {
    /// Inspect all untrusted metadata, including future node fields.
    pub fn fields(&self) -> &'a Map<String, Value> {
        self.fields
    }
    /// Work object header. Missing and malformed are distinct.
    pub fn work_header(&self) -> Result<Option<&'a Map<String, Value>>, ProviderError> {
        self.object_field("woHeader")
    }
    /// Consensus header fields; their presence is not a consensus proof.
    pub fn header(&self) -> Result<Option<&'a Map<String, Value>>, ProviderError> {
        self.object_field("header")
    }
    fn object_field(&self, name: &str) -> Result<Option<&'a Map<String, Value>>, ProviderError> {
        self.fields
            .get(name)
            .map(|v| {
                v.as_object()
                    .ok_or(invalid_result("invalid block metadata object"))
            })
            .transpose()
    }
    /// Exact Unix timestamp in seconds; no floating point or date-range narrowing.
    pub fn timestamp_seconds(&self) -> Result<Option<quai_rpc::U256>, ProviderError> {
        self.work_header()?
            .and_then(|w| w.get("timestamp"))
            .cloned()
            .map(crate::quantity)
            .transpose()
    }
    /// Source-reported encoded size; no claim that this equals a local serialization.
    pub fn size(&self) -> Result<Option<quai_rpc::U256>, ProviderError> {
        self.fields
            .get("size")
            .cloned()
            .map(crate::quantity)
            .transpose()
    }
    /// Source-reported total entropy, preserving all 256 quantity bits.
    pub fn total_entropy(&self) -> Result<Option<quai_rpc::U256>, ProviderError> {
        self.fields
            .get("totalEntropy")
            .cloned()
            .map(crate::quantity)
            .transpose()
    }
    /// Validated bounded interlink hashes, preserving order and duplicates.
    pub fn interlink_hashes(&self) -> Result<Option<Vec<Hash32>>, ProviderError> {
        self.hashes("interlinkHashes")
    }
    /// Validated bounded subordinate manifest hashes.
    pub fn sub_manifest(&self) -> Result<Option<Vec<Hash32>>, ProviderError> {
        self.hashes("subManifest")
    }
    fn hashes(&self, name: &str) -> Result<Option<Vec<Hash32>>, ProviderError> {
        self.list(name)?
            .map(|a| a.iter().cloned().map(types::hash).collect())
            .transpose()
    }
    fn list(&self, name: &str) -> Result<Option<&'a [Value]>, ProviderError> {
        self.fields
            .get(name)
            .map(|v| {
                v.as_array()
                    .filter(|a| a.len() <= 8192)
                    .map(Vec::as_slice)
                    .ok_or(invalid_result("invalid block metadata list"))
            })
            .transpose()
    }
    /// Bounded uncles as raw hash/header values; no hidden network fetch.
    pub fn uncles(&self) -> Result<Option<&'a [Value]>, ProviderError> {
        self.list("uncles")
    }
    /// Bounded work shares. Accepts node `workshares` and JS `workShares`, rejecting
    /// conflicting values when both spellings are present.
    pub fn work_shares(&self) -> Result<Option<&'a [Value]>, ProviderError> {
        if let (Some(a), Some(b)) = (self.fields.get("workshares"), self.fields.get("workShares"))
            && a != b
        {
            return Err(invalid_result("conflicting work share fields"));
        }
        if self.fields.contains_key("workshares") {
            self.list("workshares")
        } else {
            self.list("workShares")
        }
    }
    /// Decode a bounded outbound list (1..4096 caller budget). Unknown hashes stay
    /// separate from prefetched external transactions and never become executions.
    pub fn outbound_etxs(
        &self,
        max_items: usize,
    ) -> Result<Option<Vec<OutboundBlockTransaction>>, ProviderError> {
        if !(1..=4096).contains(&max_items) {
            return Err(ProviderError::InvalidRequest("invalid outbound budget"));
        }
        let Some(items) = self.list("outboundEtxs")? else {
            return Ok(None);
        };
        if items.len() > max_items {
            return Err(invalid_result("outbound budget exceeded"));
        }
        let mut seen = BTreeSet::new();
        let mut out = Vec::with_capacity(items.len());
        for value in items {
            let item = if value.is_string() {
                OutboundBlockTransaction::Hash(types::hash(value.clone())?)
            } else {
                let transaction = Transaction::try_from(value.clone())?;
                if !matches!(transaction.details, TransactionDetails::External(_)) {
                    return Err(invalid_result("outbound item is not external"));
                }
                OutboundBlockTransaction::Prefetched(Box::new(transaction))
            };
            if item.hash() == Hash32::ZERO || !seen.insert(item.hash()) {
                return Err(invalid_result("zero or duplicate outbound identity"));
            }
            out.push(item);
        }
        Ok(Some(out))
    }
}
fn selected<T>(items: &[T], id: BlockTransactionId, hash: impl Fn(&T) -> Hash32) -> Option<&T> {
    match id {
        BlockTransactionId::Index(i) => items.get(i),
        BlockTransactionId::Hash(h) => items.iter().find(|t| hash(t) == h),
    }
}
impl TransactionBlock {
    /// Borrow metadata from this executed-transaction block.
    pub fn metadata(&self) -> BlockMetadata<'_> {
        BlockMetadata {
            fields: &self.extensions,
        }
    }
    /// Exact prefetched executed transaction lookup; absent never returns a neighbor.
    pub fn transaction(&self, id: BlockTransactionId) -> Option<&Transaction> {
        selected(&self.transactions, id, |t| t.hash)
    }
    /// Export normalized full node block JSON. Revalidates block and execution
    /// associations; retained metadata is not authenticated or forced to a JS shape.
    pub fn to_rpc_json(&self) -> Result<Value, ProviderError> {
        if self.transactions.len() > 4096 {
            return Err(invalid_result("block transaction budget exceeded"));
        }
        let mut seen = BTreeSet::new();
        for (i, t) in self.transactions.iter().enumerate() {
            if t.hash == Hash32::ZERO
                || !seen.insert(t.hash)
                || t.inclusion
                    != Some(crate::Inclusion {
                        block_hash: self.block.hash,
                        block_number: self.block.number,
                        transaction_index: i as u64,
                    })
            {
                return Err(invalid_result("block transaction inclusion mismatch"));
            }
        }
        let v = crate::response_json::block_json(
            self.block.hash,
            &self.extensions,
            self.transactions
                .iter()
                .map(Transaction::to_rpc_json)
                .collect::<Result<_, _>>()?,
        );
        validate_export(&v, self.block, self.parent_hash, self.zone)?;
        Ok(v)
    }
}
impl BlockHashes {
    /// Borrow metadata from this hash-only block.
    pub fn metadata(&self) -> BlockMetadata<'_> {
        BlockMetadata {
            fields: &self.extensions,
        }
    }
    /// Exact executed hash lookup, with no implicit transaction RPC.
    pub fn transaction_hash(&self, id: BlockTransactionId) -> Option<Hash32> {
        selected(&self.transactions, id, |h| *h).copied()
    }
    /// One explicit lookup of a contained hash. Verifies its block/index association
    /// and the provider's chain checks, but does not assert that this block is canonical.
    pub async fn transaction<T: Transport>(
        &self,
        provider: &Provider<T>,
        id: BlockTransactionId,
    ) -> Result<Option<Transaction>, ProviderError> {
        let Some(hash) = self.transaction_hash(id) else {
            return Ok(None);
        };
        let index = self
            .transactions
            .iter()
            .position(|h| *h == hash)
            .expect("hash selected above");
        let transaction = provider.transaction(self.zone, hash).await?;
        if let Some(t) = &transaction
            && t.inclusion
                != Some(crate::Inclusion {
                    block_hash: self.block.hash,
                    block_number: self.block.number,
                    transaction_index: index as u64,
                })
        {
            return Err(invalid_result("block transaction inclusion mismatch"));
        }
        Ok(transaction)
    }
    /// Export normalized hash-only node block JSON and revalidate public identities.
    pub fn to_rpc_json(&self) -> Result<Value, ProviderError> {
        if self.transactions.len() > 4096
            || self.transactions.contains(&Hash32::ZERO)
            || self.transactions.iter().collect::<BTreeSet<_>>().len() != self.transactions.len()
        {
            return Err(invalid_result("invalid block transaction identities"));
        }
        let v = crate::response_json::block_json(
            self.block.hash,
            &self.extensions,
            self.transactions
                .iter()
                .map(|h| json!(h.to_string()))
                .collect(),
        );
        validate_export(&v, self.block, self.parent_hash, self.zone)?;
        Ok(v)
    }
}
fn validate_export(
    value: &Value,
    block: BlockReference,
    parent: Hash32,
    zone: Zone,
) -> Result<(), ProviderError> {
    let (observed, observed_parent, _) =
        block_fields(value.clone(), zone, MinedBlock::Hash(block.hash))?;
    if observed != block || observed_parent != parent {
        return Err(invalid_result("block metadata identity mismatch"));
    }
    Ok(())
}
