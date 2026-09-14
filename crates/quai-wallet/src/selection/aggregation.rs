use super::*;

/// Threshold aggregation policy. Increasing denominations requires the pinned
/// node's first-Qi-transaction block exception; this selector cannot arrange it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AggregationPolicy {
    /// Coins up to this denomination are aggregation candidates.
    pub maximum_input: Denomination,
    /// Largest denomination emitted, including fee refund outputs.
    pub maximum_output: Denomination,
    /// Reject plans that do not reduce the total selected UTXO count. Set false
    /// explicitly to allow the reference's warning-only behavior.
    pub require_reduction: bool,
}
impl Default for AggregationPolicy {
    fn default() -> Self {
        Self {
            maximum_input: Denomination::new(6).expect("valid fixed denomination"),
            maximum_output: Denomination::new(14).expect("valid fixed denomination"),
            require_reduction: true,
        }
    }
}

/// Aggregate an ascending stable prefix of eligible small coins; fund the exact
/// fee from other eligible coins, removing the smallest aggregation coins when
/// needed. Fee inputs precede aggregation inputs; their refund denominations are
/// appended after aggregation outputs, matching the reference's ordering.
///
/// Unlike the reference this never accepts a fee shortfall, ignores no locks or
/// reservations, and enforces explicit resource limits. `target` must be zero.
/// At most 100,000 candidates are examined in linear denomination buckets; only
/// the final selected inputs (at most 4096) are cloned. This does not reserve them.
pub fn select_aggregate(
    coins: &[CandidateCoin],
    request: &SelectionRequest,
    policy: AggregationPolicy,
) -> Result<CoinSelection, SelectionError> {
    if request.target != U256::ZERO
        || request.fee > request.max_fee
        || !(1..=4096).contains(&request.max_inputs)
        || !(1..=4096).contains(&request.max_outputs)
    {
        return Err(SelectionError::InvalidRequest);
    }
    if coins.len() > 100_000 {
        return Err(SelectionError::LimitExceeded);
    }
    let mut seen = BTreeSet::new();
    let mut buckets: [Vec<&CandidateCoin>; 15] = std::array::from_fn(|_| Vec::new());
    let mut total = U256::ZERO;
    let mut small_value = U256::ZERO;
    for coin in coins {
        let hash = coin.outpoint.transaction_hash.bytes();
        if !seen.insert(coin.outpoint) || hash[2] != coin.address.zone().byte() || *hash == [0; 32]
        {
            return Err(SelectionError::InvalidCoin);
        }
        if coin.address.zone() != request.zone
            || coin.reserved
            || coin.unlock_height > request.candidate_height
            || coin
                .expires_at
                .is_some_and(|end| request.candidate_height >= end)
        {
            continue;
        }
        let value = U256::from(coin.denomination.value());
        total = total.checked_add(value).ok_or(SelectionError::Overflow)?;
        if coin.denomination.index() <= policy.maximum_input.index() {
            small_value = small_value
                .checked_add(value)
                .ok_or(SelectionError::Overflow)?;
        }
        buckets[usize::from(coin.denomination.index())].push(coin);
    }
    if total <= request.fee || small_value == U256::ZERO {
        return Err(SelectionError::InsufficientFunds);
    }
    let large_value = total - small_value;
    let wanted = small_value - request.fee.saturating_sub(large_value);
    let ordered: Vec<_> = buckets.into_iter().flatten().collect();
    let mut aggregate_value = U256::ZERO;
    let mut end = 0;
    while aggregate_value < wanted {
        aggregate_value += U256::from(ordered[end].denomination.value());
        end += 1;
    }
    // The aggregation set remains a contiguous interval in the sorted list.
    // Removing its smallest prefix increases outside funding without quadratic
    // rescanning, and corrects the source's acceptance of a 25% fee shortfall.
    let mut start = 0;
    while total - aggregate_value < request.fee {
        aggregate_value -= U256::from(ordered[start].denomination.value());
        start += 1;
    }
    let mut inputs = Vec::new();
    let mut fee_value = U256::ZERO;
    for (i, coin) in ordered.iter().enumerate() {
        if fee_value >= request.fee {
            break;
        }
        if (start..end).contains(&i) {
            continue;
        }
        if inputs.len() == request.max_inputs {
            return Err(SelectionError::LimitExceeded);
        }
        fee_value += U256::from(coin.denomination.value());
        inputs.push((*coin).clone());
    }
    if fee_value < request.fee {
        return Err(SelectionError::InsufficientFunds);
    }
    if end - start > request.max_inputs - inputs.len() {
        return Err(SelectionError::LimitExceeded);
    }
    inputs.extend(ordered[start..end].iter().map(|coin| (*coin).clone()));
    let mut outputs = Vec::new();
    for mut value in [aggregate_value, fee_value - request.fee] {
        for index in (0..=policy.maximum_output.index()).rev() {
            let denomination = Denomination::new(index).map_err(|_| SelectionError::InvalidCoin)?;
            let unit = U256::from(denomination.value());
            let count = value / unit;
            if count > U256::from(request.max_outputs - outputs.len()) {
                return Err(SelectionError::LimitExceeded);
            }
            outputs.extend(std::iter::repeat_n(denomination, count.to::<usize>()));
            value %= unit;
        }
    }
    if outputs.is_empty() || (policy.require_reduction && outputs.len() >= inputs.len()) {
        return Err(SelectionError::InvalidRequest);
    }
    if inputs.len() * 3 + outputs.len() + 3 > MAX_TRANSACTION_MESSAGES {
        return Err(SelectionError::LimitExceeded);
    }
    Ok(CoinSelection {
        inputs,
        spend_outputs: outputs,
        change_outputs: vec![],
        input_value: aggregate_value + fee_value,
        fee: request.fee,
    })
}
