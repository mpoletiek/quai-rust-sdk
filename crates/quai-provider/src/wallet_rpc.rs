//! Typed wallet queries for the pinned node, with explicit observation limits.
use crate::{AddressOutpoint, BlockTag, Provider, ProviderError, quantity, types};
use futures_util::{StreamExt, stream};
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
    /// The pinned estimator omits origin costs and denomination fragmentation;
    /// see [`Self::estimate_quai_conversion_gas_budget`] for preparation.
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
    /// Conservative empty-access-list Quai-to-Qi gas budget for the pinned
    /// go-quai gas schedule. Unlike the raw estimator, includes origin intrinsic
    /// gas, ETX creation and a denomination-count bound for every amount at or
    /// below the sampled nominal quote. Discounted values can require *more*
    /// outputs than the nominal amount. A later higher quote or changed gas
    /// schedule can still exceed this budget; estimates are not guarantees.
    /// Missing/zero quotes, arithmetic overflow and access lists fail explicitly.
    pub async fn estimate_quai_conversion_gas_budget(
        &self,
        from: QuaiAddress,
        transaction: &quai_consensus::QuaiToQiTransaction,
        block: BlockTag,
    ) -> Result<u64, ProviderError> {
        let tx = transaction.transaction();
        if !tx.access_list.is_empty() {
            return Err(ProviderError::InvalidRequest(
                "conversion gas budget access list",
            ));
        }
        let estimate = self
            .estimate_quai_conversion_gas(from, transaction, block)
            .await?;
        let quote = self
            .quai_to_qi(from.zone(), tx.value, block)
            .await?
            .filter(|v| *v != U256::ZERO)
            .ok_or(ProviderError::InvalidResult("conversion quote unavailable"))?;
        conversion_gas_budget(estimate, quote, &tx.data)
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
    /// Observe backing while preserving the pinned go-quai absence distinction.
    /// Only error -32000 with the exact message `no wrapped Qi balance` and no
    /// extra error data becomes None. Every other failure propagates. A successful
    /// zero quantity remains Some(0). No historical coverage or code is inferred.
    pub async fn wrapped_qi_deposit_optional(
        &self,
        owner: QuaiAddress,
        beneficiary: QuaiAddress,
        block: BlockTag,
    ) -> Result<Option<U256>, ProviderError> {
        match self.wrapped_qi_deposit(owner, beneficiary, block).await {
            Ok(value) => Ok(Some(value)),
            Err(ProviderError::Rpc(quai_rpc::RpcError::Remote(error)))
                if error.code == -32000
                    && error.message == "no wrapped Qi balance"
                    && error.data.is_none() =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }
    /// Bounded group of latest-only address queries. Each response is a separate
    /// observation; this convenience method never claims an atomic snapshot.
    /// Uses explicit HTTP batches where supported, with chain checks in each batch.
    /// Otherwise at most four requests are in flight per zone. No failed batch is replayed.
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
        let zones: BTreeSet<_> = addresses.iter().map(|a| a.zone()).collect();
        for zone in zones {
            let scoped: Vec<_> = addresses
                .iter()
                .copied()
                .filter(|a| a.zone() == zone)
                .collect();
            // 32 rather than the transport's 126-call headroom. The binding limit
            // is the response cap (2 MiB by default), not the request: an oversize
            // batch response fails the whole page and does not fall back, because
            // the batch was sent. At 32 addresses each may return ~64 KiB of
            // outpoints; at 126 that budget falls to ~16 KiB, which a busy Qi
            // address can exceed. Raising this needs outpoint-count data first.
            for page in scoped.chunks(32) {
                let mut requests = vec![("quai_chainId", json!([]))];
                requests.extend(
                    page.iter().map(|address| {
                        ("quai_getOutpointsByAddress", json!([address.to_string()]))
                    }),
                );
                requests.push(("quai_chainId", json!([])));
                let outputs = if let Some(batch) = self
                    .transport
                    .request_batch(self.routing.endpoint(zone.into())?, requests)
                    .await
                {
                    let mut responses = batch?;
                    if responses.len() != page.len() + 2 {
                        return Err(ProviderError::InvalidResult("batch response count"));
                    }
                    for observed in [responses.pop().expect("checked count"), responses.remove(0)] {
                        self.check_chain_id(observed?)?;
                    }
                    page.iter()
                        .copied()
                        .zip(responses)
                        .map(|(address, response)| {
                            Ok((address, types::parse_outpoints(response?)?))
                        })
                        .collect::<Result<Vec<_>, ProviderError>>()?
                } else {
                    let mut pending =
                        stream::iter(page.iter().copied().map(|address| async move {
                            self.outpoints(address)
                                .await
                                .map(|outputs| (address, outputs))
                        }))
                        .buffered(4);
                    let mut outputs = Vec::new();
                    while let Some(response) = pending.next().await {
                        outputs.push(response?);
                    }
                    outputs
                };
                for (address, outputs) in outputs {
                    count = count
                        .checked_add(outputs.len())
                        .ok_or(ProviderError::InvalidResult("output bound"))?;
                    if count > 100_000 {
                        return Err(ProviderError::InvalidResult("output bound"));
                    }
                    result.insert(address, outputs);
                }
            }
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

// Each lower greedy digit is bounded by the next denomination's radix minus
// one, and by the entire quoted value. This bounds every smaller amount, not
// only the nominal quote's (potentially much shorter) greedy decomposition.
fn conversion_output_bound(quote: U256) -> Result<u64, ProviderError> {
    let values = quai_consensus::Denomination::VALUES;
    let mut count = 0u64;
    for (i, value) in values.iter().enumerate() {
        let mut digit = quote / U256::from(*value);
        if let Some(next) = values.get(i + 1) {
            digit = digit.min(U256::from(next / value - 1));
        }
        let digit = u64::try_from(digit)
            .map_err(|_| ProviderError::InvalidResult("conversion output bound overflow"))?;
        count = count
            .checked_add(digit)
            .ok_or(ProviderError::InvalidResult(
                "conversion output bound overflow",
            ))?;
    }
    // Pinned execution refuses outputIndex >= MaxOutputIndex (65535).
    if count > 65_535 {
        return Err(ProviderError::InvalidResult(
            "conversion output bound exceeds node limit",
        ));
    }
    Ok(count)
}
fn conversion_gas_budget(estimate: u64, quote: U256, data: &[u8]) -> Result<u64, ProviderError> {
    let outputs = conversion_output_bound(quote)?;
    let destination = 21_000 + 9_000 * outputs;
    let origin = 42_000
        + data
            .iter()
            .map(|b| if *b == 0 { 4 } else { 16 })
            .sum::<u64>();
    estimate
        .max(destination)
        .checked_add(origin)
        .ok_or(ProviderError::InvalidResult(
            "conversion gas budget overflow",
        ))
}

#[cfg(test)]
mod conversion_budget_tests {
    use super::*;
    #[test]
    fn orchard_discount_fragmentation_and_origin_costs() {
        // Actual Orchard observation: raw estimate 129000, nominal 387060,
        // settled 386286. A 10% raw margin forwarded only 99880 gas and
        // created 385000 Qits before destination failure.
        let budget = conversion_gas_budget(129_000, U256::from(387_060), &[0, 100]).unwrap();
        assert_eq!(budget, 315_020);
        let mut remaining = 386_286;
        let mut outputs = 0;
        for d in quai_consensus::Denomination::VALUES.iter().rev() {
            outputs += remaining / d;
            remaining %= d;
        }
        assert_eq!(outputs, 17);
        assert!(budget - 42_020 >= 21_000 + 9_000 * outputs);
        assert!(141_900 - 42_020 < 21_000 + 9_000 * outputs);
    }
    #[test]
    fn denomination_bound_covers_every_lower_amount() {
        let mut maximum = 0;
        for amount in 1..=1_000_000u64 {
            let mut remaining = amount;
            let mut count = 0;
            for d in quai_consensus::Denomination::VALUES.iter().rev() {
                count += remaining / d;
                remaining %= d;
            }
            maximum = maximum.max(count);
            assert!(conversion_output_bound(U256::from(amount)).unwrap() >= maximum);
        }
        assert!(conversion_output_bound(U256::MAX).is_err());
        assert!(conversion_gas_budget(u64::MAX, U256::from(1), &[0, 100]).is_err());
    }
}
