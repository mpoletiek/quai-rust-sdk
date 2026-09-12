//! Typed wallet queries for the pinned node, with explicit observation limits.
use crate::{AddressOutpoint, BlockTag, Provider, ProviderError, quantity, types};
use quai_primitives::{Address, Hash32, QiAddress, QuaiAddress, Zone};
use quai_rpc::{Transport, U256};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Exact created/deleted outpoints reported over a block-hash range, inclusive.
/// The connected node must retain spent/trimmed history; this is not a proof.
#[derive(Clone, Debug)]
pub struct OutpointDeltas {
    /// Created outputs, including outputs subsequently deleted within the range.
    pub created: Vec<AddressOutpoint>,
    /// Spent or trimmed outputs. Deletions take precedence within a range.
    pub deleted: Vec<AddressOutpoint>,
}

impl<T: Transport> Provider<T> {
    /// Estimate an explicit type-0 Quai-to-Qi conversion with the exact data,
    /// value, nonce and gas price. Uses a dedicated typed request, not account
    /// recipient coercion. The node may still override simulation nonce.
    pub async fn estimate_quai_conversion_gas(
        &self,
        from: QuaiAddress,
        transaction: &quai_consensus::QuaiToQiTransaction,
        block: BlockTag,
    ) -> Result<u64, ProviderError> {
        let tx = transaction.transaction();
        if tx.chain_id != self.expected_chain_id || from.zone() != transaction.destination().zone()
        {
            return Err(ProviderError::InvalidRequest(
                "conversion simulation identity",
            ));
        }
        let data = crate::RpcData::new(tx.data.clone())?;
        types::uint64(self.read(from.zone().into(), "quai_estimateGas", json!([{
            "from": from.to_string(), "to": transaction.destination().to_string(), "txType": 0,
            "value": format!("{:#x}", tx.value), "input": data.to_hex(), "nonce": format!("{:#x}", tx.nonce),
            "gas": format!("{:#x}", tx.gas_limit), "gasPrice": format!("{:#x}", tx.gas_price),
            "accessList": tx.access_list.iter().map(|a| json!({"address":a.address.to_string(),"storageKeys":a.storage_keys.iter().map(ToString::to_string).collect::<Vec<_>>()})).collect::<Vec<_>>()
        }, block.rpc_value()?])).await?)
    }
    /// Node-reported Qi-to-Quai amount at a selector, in Its for input Qits.
    /// Null indicates missing header/prime-terminus data, not a zero quote.
    pub async fn qi_to_quai(
        &self,
        zone: Zone,
        qits: U256,
        block: BlockTag,
    ) -> Result<Option<U256>, ProviderError> {
        let value = self
            .read(
                zone.into(),
                "quai_qiToQuai",
                json!([format!("{qits:#x}"), block.rpc_value()?]),
            )
            .await?;
        if value.is_null() {
            Ok(None)
        } else {
            quantity(value).map(Some)
        }
    }
    /// Node-reported Quai-to-Qi amount, in Qits for input Its. The pinned node
    /// uses the current prime terminus even for a historical selector, so this
    /// method deliberately does not promise a historical exchange-rate quote.
    pub async fn quai_to_qi(
        &self,
        zone: Zone,
        its: U256,
        block: BlockTag,
    ) -> Result<Option<U256>, ProviderError> {
        let value = self
            .read(
                zone.into(),
                "quai_quaiToQi",
                json!([format!("{its:#x}"), block.rpc_value()?]),
            )
            .await?;
        if value.is_null() {
            Ok(None)
        } else {
            quantity(value).map(Some)
        }
    }
    /// Current controller-discounted conversion estimate. Not a guaranteed
    /// settlement amount; slippage and eventual prime processing remain separate.
    pub async fn calculate_conversion_amount(
        &self,
        from: Address,
        to: Address,
        value: U256,
    ) -> Result<U256, ProviderError> {
        let zone = from
            .zone()
            .map_err(|_| ProviderError::InvalidRequest("conversion origin"))?;
        if value == U256::ZERO || from.ledger() == to.ledger() || to.zone().ok() != Some(zone) {
            return Err(ProviderError::InvalidRequest("conversion scope or amount"));
        }
        quantity(
            self.read(
                zone.into(),
                "quai_calculateConversionAmount",
                json!([{
                    "from": from.to_string(), "to": to.to_string(), "value": format!("{value:#x}")
                }]),
            )
            .await?,
        )
    }
    /// Unclaimed protocol backing in Qits for an owner contract and beneficiary,
    /// at the requested account-state selector. This is not ERC-20 balanceOf.
    pub async fn wrapped_qi_deposit(
        &self,
        owner: QuaiAddress,
        beneficiary: QuaiAddress,
        block: BlockTag,
    ) -> Result<U256, ProviderError> {
        if owner.zone() != beneficiary.zone() {
            return Err(ProviderError::InvalidRequest("wrapped deposit zone"));
        }
        quantity(
            self.read(
                owner.zone().into(),
                "quai_getWrappedQiDeposit",
                json!([
                    owner.to_string(),
                    beneficiary.to_string(),
                    block.rpc_value()?
                ]),
            )
            .await?,
        )
    }
    /// Bounded group of latest-only address queries. Each response is a separate
    /// observation; this convenience method never claims an atomic snapshot.
    pub async fn outpoints_many(
        &self,
        addresses: &[QiAddress],
    ) -> Result<BTreeMap<QiAddress, Vec<AddressOutpoint>>, ProviderError> {
        if addresses.is_empty() || addresses.len() > 1024 {
            return Err(ProviderError::InvalidRequest("address query bound"));
        }
        let unique: BTreeSet<_> = addresses.iter().copied().collect();
        if unique.len() != addresses.len() {
            return Err(ProviderError::InvalidRequest("duplicate query address"));
        }
        let mut result = BTreeMap::new();
        let mut count = 0usize;
        for address in addresses {
            let outputs = self.outpoints(*address).await?;
            count = count
                .checked_add(outputs.len())
                .ok_or(ProviderError::InvalidResult("output bound"))?;
            if count > 100_000 {
                return Err(ProviderError::InvalidResult("output bound"));
            }
            result.insert(*address, outputs);
        }
        Ok(result)
    }
    /// Read node-indexed deltas for explicit inclusive block hashes. Callers must
    /// verify canonicality, continuity, range limits and retained history. The
    /// node's noncanonical-range semantics are unsuitable for undo/reorg replay.
    pub async fn outpoint_deltas(
        &self,
        zone: Zone,
        addresses: &[QiAddress],
        from: Hash32,
        to: Hash32,
    ) -> Result<BTreeMap<QiAddress, OutpointDeltas>, ProviderError> {
        if addresses.is_empty()
            || addresses.len() > 1024
            || addresses.iter().any(|a| a.zone() != zone)
        {
            return Err(ProviderError::InvalidRequest(
                "delta address scope or bound",
            ));
        }
        let wanted: BTreeSet<_> = addresses.iter().copied().collect();
        if wanted.len() != addresses.len() {
            return Err(ProviderError::InvalidRequest("duplicate delta address"));
        }
        let value = self
            .read(
                zone.into(),
                "quai_getOutpointDeltasForAddressesInRange",
                json!([
                    addresses
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>(),
                    from.to_string(),
                    to.to_string()
                ]),
            )
            .await?;
        let object = value
            .as_object()
            .filter(|o| o.len() == addresses.len())
            .ok_or(ProviderError::InvalidResult("delta address coverage"))?;
        let mut result = BTreeMap::new();
        let mut budget = 100_000usize;
        for (address, changes) in object {
            let address: QiAddress = address
                .parse()
                .map_err(|_| ProviderError::InvalidResult("delta address"))?;
            if !wanted.contains(&address) || result.contains_key(&address) {
                return Err(ProviderError::InvalidResult("delta address identity"));
            }
            let changes = changes
                .as_object()
                .filter(|o| o.len() == 2)
                .ok_or(ProviderError::InvalidResult("delta shape"))?;
            let mut parse = |name| -> Result<Vec<AddressOutpoint>, ProviderError> {
                let groups = changes
                    .get(name)
                    .and_then(Value::as_object)
                    .ok_or(ProviderError::InvalidResult("delta group"))?;
                let mut output = Vec::new();
                let mut seen = BTreeSet::new();
                for (hash, entries) in groups {
                    let hash: Hash32 = hash
                        .parse()
                        .map_err(|_| ProviderError::InvalidResult("delta hash"))?;
                    let entries = entries
                        .as_array()
                        .ok_or(ProviderError::InvalidResult("delta outputs"))?;
                    if entries.len() > budget {
                        return Err(ProviderError::InvalidResult("delta output bound"));
                    }
                    budget -= entries.len();
                    for entry in entries {
                        let mut entry = entry
                            .as_object()
                            .filter(|o| !o.contains_key("txHash"))
                            .ok_or(ProviderError::InvalidResult("delta output fields"))?
                            .clone();
                        entry.insert("txHash".into(), json!(hash.to_string()));
                        let mut parsed = types::parse_outpoints(json!([entry]))?;
                        let coin = parsed
                            .pop()
                            .ok_or(ProviderError::InvalidResult("delta output"))?;
                        if !seen.insert((coin.outpoint.tx_hash, coin.outpoint.index)) {
                            return Err(ProviderError::InvalidResult("duplicate delta output"));
                        }
                        output.push(coin);
                    }
                }
                Ok(output)
            };
            result.insert(
                address,
                OutpointDeltas {
                    created: parse("created")?,
                    deleted: parse("deleted")?,
                },
            );
        }
        Ok(result)
    }
}
