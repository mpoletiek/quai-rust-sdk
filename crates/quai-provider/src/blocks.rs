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

fn block_fields(
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
