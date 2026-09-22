//! Typed wallet queries for the pinned node, with explicit observation limits.
use crate::{AddressOutpoint, BlockTag, Provider, ProviderError, quantity, types};
use futures_util::{StreamExt, stream};
use quai_primitives::{Address, Hash32, QiAddress, QuaiAddress, Zone};
use quai_rpc::RpcError;
use quai_rpc::{Transport, U256};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// One conversion priced two ways at the current head, and the gap between.
///
/// Produced by [`Provider::estimate_conversion`]. The amounts are in the
/// destination ledger's base units: Its for a Qi-to-Quai conversion, Qits for
/// Quai-to-Qi.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ConversionEstimate {
    /// What the node's undiscounted rate alone gives for this amount.
    pub rate_amount: U256,
    /// What the node's single-transaction estimate leaves after its cubic flow
    /// discount, its k-Quai discount and its ten percent floor. Never more than
    /// a tenth below `rate_amount` short of that floor applying.
    pub expected_amount: U256,
    /// `rate_amount` less `expected_amount`, in ten-thousandths of
    /// `rate_amount`, rounded up and bounded by 10,000.
    ///
    /// Compare this against the slippage tolerance a conversion will carry; it
    /// is a lower bound on the realized slippage, not a prediction of it, for
    /// the batching reason given on [`Provider::estimate_conversion`].
    pub implied_slippage_bps: u16,
}

/// Starting addresses per batched outpoint page.
///
/// The transport accepts 128 calls per batch and each page adds one chain-guard
/// call, so this is the full usable headroom. It is a starting point rather
/// than a fixed size: see `outpoints_many` for the downward adaptation.
const MAX_OUTPOINT_PAGE: usize = 127;

/// Each page adds a leading `quai_chainId` guard, so the page plus its guard
/// must fit the transport's batch limit. Asserted at compile time
/// because the limit lives in another crate: without this, lowering it there
/// would break `outpoints_many` on its first page with `InvalidConfig`, which is
/// neither retried nor falls back.
#[cfg(feature = "http")]
const _: () = assert!(MAX_OUTPOINT_PAGE < quai_rpc::MAX_BATCH_CALLS);

/// Most addresses one [`Provider::outpoints_many`] call accepts. It pages them
/// internally, so a caller needs no page size of its own.
pub const MAX_OUTPOINT_ADDRESSES: usize = 1024;

/// Most accounts one [`Provider::account_states`] call accepts.
pub const MAX_ACCOUNT_STATES: usize = 1024;

/// One account's balance and nonce at the block an
/// [`Provider::account_states`] call was pinned to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct AccountState {
    /// Balance in base units.
    pub balance: U256,
    /// Transaction count.
    pub nonce: u64,
}

/// Accounts per batched balance-and-nonce page: two calls each, plus the
/// chain guard, within the transport's batch limit.
const ACCOUNT_STATE_PAGE: usize = (quai_rpc::MAX_BATCH_CALLS - 1) / 2;

/// Exact created/deleted outpoints reported over a block-hash range, inclusive.
/// The connected node must retain spent/trimmed history; this is not a proof.
#[derive(Clone, Debug)]
#[non_exhaustive]
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
    /// Undiscounted quote, discounted estimate, and the slippage between them,
    /// for one conversion of `value` from `from`'s ledger to `to`'s.
    ///
    /// This is the pair of numbers a wallet shows before asking the user to
    /// choose a slippage tolerance: what the rate alone would give, and what the
    /// node's discounts currently leave of it. Both halves are read at the
    /// current head, because `quai_calculateConversionAmount` takes no block
    /// argument and `quai_quaiToQi` resolves its rate from the current head
    /// whatever selector it is given.
    ///
    /// **The two halves do not share a rate basis, and this figure carries that
    /// difference.** The rate RPCs evaluate `QiToQuai`/`QuaiToQi` against the
    /// *zone* header, while `quai_calculateConversionAmount` evaluates them
    /// against the *prime terminus* block. At and after the conversion-lock
    /// fork the rate includes `2 x AvgTxFees`, a per-block field, so the two
    /// denominators differ and `implied_slippage_bps` is the discount plus that
    /// basis gap rather than the discount alone. No RPC exposes the
    /// prime-terminus basis on its own, so the gap cannot be removed here; the
    /// reference wallet computes its displayed slippage the same way, which is
    /// why this matches what a user sees there.
    ///
    /// Note also that the cubic flow discount is at least 20 basis points for
    /// any value, so a real conversion never reads as zero slippage.
    ///
    /// **This is not the slippage the conversion will experience.** The node
    /// prices a whole prime-block batch together, in descending slippage order,
    /// so conversions submitted by other people in the same block change the
    /// realized discount. This estimate prices one transaction alone, exactly as
    /// the node's own RPC does. Size a tolerance against the batch you are
    /// willing to share with, using
    /// [`quai_consensus::conversion_batch_discount_bps`], and treat this figure
    /// as the floor of what you will lose rather than the whole of it.
    pub async fn estimate_conversion(
        &self,
        from: Address,
        to: Address,
        value: U256,
    ) -> Result<ConversionEstimate, ProviderError> {
        // Validates scope, ledgers and amount, so the rate half below can assume
        // a well-formed same-zone cross-ledger conversion.
        let expected_amount = self.calculate_conversion_amount(from, to, value).await?;
        let zone = from
            .zone()
            .map_err(|_| ProviderError::InvalidRequest("conversion origin"))?;
        let rate_amount = match from.ledger() {
            quai_primitives::Ledger::Qi => self.qi_to_quai(zone, value, BlockTag::Latest).await?,
            quai_primitives::Ledger::Quai => self.quai_to_qi(zone, value, BlockTag::Latest).await?,
        }
        .filter(|quote| *quote != U256::ZERO)
        .ok_or(ProviderError::InvalidResult("conversion rate unavailable"))?;
        // The discounts only ever remove value, but the two figures come from
        // separate integer divisions, so a rounding step could leave the
        // estimate marginally above the rate quote. Report no slippage rather
        // than a negative one.
        let removed = rate_amount.saturating_sub(expected_amount);
        let scaled = removed
            .checked_mul(U256::from(10_000))
            .ok_or(ProviderError::InvalidResult("conversion slippage overflow"))?;
        // Round up: a wallet comparing this against a tolerance must not be
        // shown less slippage than the estimate implies.
        let rounded = scaled
            .checked_add(rate_amount - U256::from(1))
            .ok_or(ProviderError::InvalidResult("conversion slippage overflow"))?
            / rate_amount;
        // `removed <= rate_amount` bounds this by 10,000 before the clamp.
        let implied_slippage_bps = rounded.min(U256::from(10_000)).to::<u64>() as u16;
        Ok(ConversionEstimate {
            rate_amount,
            expected_amount,
            implied_slippage_bps,
        })
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
    /// Attempt one batched page, led by the chain check so one backend answers
    /// the guard and the payload.
    ///
    /// `None` means the transport does not batch and **nothing was sent**, which
    /// is what makes the caller's fallback safe. Only this path may be retried
    /// with a smaller page, because only here does page size influence the
    /// response size.
    #[allow(clippy::type_complexity)]
    async fn outpoints_batch(
        &self,
        zone: Zone,
        page: &[QiAddress],
    ) -> Option<Result<Vec<(QiAddress, Vec<AddressOutpoint>)>, ProviderError>> {
        let endpoint = match self.routing.endpoint(zone.into()) {
            Ok(endpoint) => endpoint,
            Err(error) => return Some(Err(error.into())),
        };
        let calls = page
            .iter()
            .map(|address| ("quai_getOutpointsByAddress", json!([address.to_string()])))
            .collect();
        let batch = self.guarded_batch(endpoint, calls).await?;
        Some(batch.and_then(|responses| {
            page.iter()
                .copied()
                .zip(responses)
                .map(|(address, response)| Ok((address, types::parse_outpoints(response?)?)))
                .collect()
        }))
    }

    /// Read a page one address at a time, bounded in flight. Each read carries
    /// its own chain guard.
    ///
    /// Page size has no influence on any individual response here, so an
    /// oversize response on this path means one address's own set exceeds the
    /// cap. Reducing the page cannot fix that, and the caller must not try.
    async fn outpoints_singles(
        &self,
        page: &[QiAddress],
    ) -> Result<Vec<(QiAddress, Vec<AddressOutpoint>)>, ProviderError> {
        let mut pending = stream::iter(page.iter().copied().map(|address| async move {
            self.outpoints(address)
                .await
                .map(|outputs| (address, outputs))
        }))
        .buffered(4);
        let mut outputs = Vec::new();
        while let Some(response) = pending.next().await {
            outputs.push(response?);
        }
        Ok(outputs)
    }

    /// Bounded group of latest-only address queries. Each response is a separate
    /// observation; this convenience method never claims an atomic snapshot.
    /// Uses explicit HTTP batches where supported, with chain checks in each batch.
    /// Otherwise at most four requests are in flight per zone. No failed batch is replayed.
    pub async fn outpoints_many(
        &self,
        addresses: &[QiAddress],
    ) -> Result<BTreeMap<QiAddress, Vec<AddressOutpoint>>, ProviderError> {
        if addresses.is_empty() || addresses.len() > MAX_OUTPOINT_ADDRESSES {
            return Err(ProviderError::InvalidRequest("address query bound"));
        }
        let unique: BTreeSet<_> = addresses.iter().copied().collect();
        if unique.len() != addresses.len() {
            return Err(ProviderError::InvalidRequest("duplicate query address"));
        }
        let mut result = BTreeMap::new();
        let mut count = 0usize;
        let mut page_size = MAX_OUTPOINT_PAGE;
        let zones: BTreeSet<_> = addresses.iter().map(|a| a.zone()).collect();
        for zone in zones {
            let scoped: Vec<_> = addresses
                .iter()
                .copied()
                .filter(|a| a.zone() == zone)
                .collect();
            // Page size adapts downward on an oversize response rather than being
            // fixed conservatively. The binding limit is the response cap (2 MiB
            // by default), not the request, and it cannot be predicted: an
            // AddressOutpoint carries uninterpreted node extensions, so the node
            // decides the per-row size. A page that is too large fails the whole
            // page and cannot fall back, because the batch was already sent.
            //
            // Start at the transport's full headroom and halve on the specific
            // ResponseTooLarge error, which the transport reports distinctly, so
            // no other failure is ever retried. These are reads, so replaying one
            // is idempotent; this is not a pattern that would be safe for
            // submission. The working size is remembered across pages and zones,
            // so a wallet with dense outpoint sets probes once per call rather
            // than once per page.
            let mut offset = 0usize;
            while offset < scoped.len() {
                let page = &scoped[offset..scoped.len().min(offset + page_size)];
                let outputs = match self.outpoints_batch(zone, page).await {
                    Some(Ok(outputs)) => outputs,
                    Some(Err(ProviderError::Rpc(RpcError::ResponseTooLarge))) if page.len() > 1 => {
                        // Retry the same addresses in smaller pages. A single
                        // address that still exceeds the cap is a real error and
                        // propagates below rather than looping.
                        page_size = page.len() / 2;
                        continue;
                    }
                    Some(Err(error)) => return Err(error),
                    // The transport does not batch and sent nothing. Halving is
                    // provably futile here, so it is not attempted: an oversize
                    // response means one address exceeds the cap on its own, and
                    // retrying smaller pages would re-read every prefix address
                    // at every level before surfacing the same error.
                    None => self.outpoints_singles(page).await?,
                };
                offset += page.len();
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
    /// Balance and nonce for several accounts at one block, in input order.
    ///
    /// Uses explicit batches led by a chain guard where the transport
    /// supports them, one per zone run of up to 63 accounts. Otherwise each
    /// account is read through the guarded single-call path, at most four in
    /// flight. No failed batch is replayed. Pin `block` to a number and bracket
    /// the call with a canonical-header check to get a consistent view.
    pub async fn account_states(
        &self,
        accounts: &[QuaiAddress],
        block: BlockTag,
    ) -> Result<Vec<AccountState>, ProviderError> {
        if accounts.is_empty() || accounts.len() > MAX_ACCOUNT_STATES {
            return Err(ProviderError::InvalidRequest("account query bound"));
        }
        let selector = block.rpc_value()?;
        let mut states = Vec::with_capacity(accounts.len());
        let mut rest = accounts;
        while let Some(first) = rest.first() {
            let zone = first.zone();
            let run = rest
                .iter()
                .take(ACCOUNT_STATE_PAGE)
                .take_while(|account| account.zone() == zone)
                .count();
            let (page, tail) = rest.split_at(run);
            rest = tail;
            let endpoint = self.routing.endpoint(zone.into())?;
            let mut calls = Vec::with_capacity(page.len() * 2);
            for account in page {
                let params = json!([account.to_string(), selector]);
                calls.push(("quai_getBalance", params.clone()));
                calls.push(("quai_getTransactionCount", params));
            }
            let Some(batch) = self.guarded_batch(endpoint, calls).await else {
                let mut pending = stream::iter(page.iter().copied().map(|account| async move {
                    Ok::<_, ProviderError>(AccountState {
                        balance: self.balance(account, block).await?,
                        nonce: self.transaction_count(account, block).await?,
                    })
                }))
                .buffered(4);
                while let Some(state) = pending.next().await {
                    states.push(state?);
                }
                continue;
            };
            let mut responses = batch?.into_iter();
            while let (Some(balance), Some(nonce)) = (responses.next(), responses.next()) {
                states.push(AccountState {
                    balance: quantity(balance?)?,
                    nonce: types::uint64(nonce?)?,
                });
            }
        }
        Ok(states)
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
