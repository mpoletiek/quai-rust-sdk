//! Bounded deterministic ordinary-transfer selection over an explicit UTXO snapshot.
use quai_consensus::{Denomination, MAX_TRANSACTION_MESSAGES, OutPoint, U256};
use quai_primitives::{QiAddress, Zone};
use std::collections::BTreeSet;
use thiserror::Error;
mod aggregation;
pub use aggregation::{AggregationPolicy, select_aggregate};

/// Caller-supplied, validated-index UTXO candidate; this type does not prove on-chain ownership.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateCoin {
    /// Exact output reference.
    pub outpoint: OutPoint,
    /// Output owner, used to restrict selection to a zone.
    pub address: QiAddress,
    /// Amount denomination.
    pub denomination: Denomination,
    /// Earliest candidate block at which the node allows spending.
    pub unlock_height: U256,
    /// Exclusive upper spend-height bound if a qualified profile predicts trimming.
    /// This is supplied by the wallet's profile, not inferred by the selector.
    pub expires_at: Option<U256>,
    /// Reserved by another pending payment in the wallet snapshot.
    pub reserved: bool,
}
/// Explicit transfer constraints, including the fee budget approved by the caller.
#[derive(Clone, Debug)]
pub struct SelectionRequest {
    /// Spend from this zone only.
    pub zone: Zone,
    /// Height of the candidate block, not the height of an old wallet checkpoint.
    pub candidate_height: U256,
    /// Recipient amount in Qits, positive.
    pub target: U256,
    /// Starting/exact fee in Qits.
    pub fee: U256,
    /// Maximum fee authorized for any convergence step.
    pub max_fee: U256,
    /// Maximum selected inputs, from 1 through 4096.
    pub max_inputs: usize,
    /// Maximum combined recipient/change outputs, from 1 through 4096.
    pub max_outputs: usize,
}
/// Pure selection result; reserving these inputs and fresh output addresses is a separate atomic step.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoinSelection {
    /// Ordered selected coins; stable reference ordering is retained for equal values.
    pub inputs: Vec<CandidateCoin>,
    /// Recipient denominations. Ordinary selection preserves denomination capacity;
    /// aggregation may increase denominations and appends its fee refund separately.
    pub spend_outputs: Vec<Denomination>,
    /// Fresh-change denominations, largest first.
    pub change_outputs: Vec<Denomination>,
    /// Total selected Qits.
    pub input_value: U256,
    /// Exact fee remaining after all recipient/change outputs.
    pub fee: U256,
}
/// Invalid snapshot, insufficient spendable value, or bounded planning failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum SelectionError {
    /// Invalid target, limits, or fee budget.
    #[error("invalid selection request")]
    InvalidRequest,
    /// Duplicate outpoint or a hash/address location mismatch.
    #[error("invalid coin snapshot")]
    InvalidCoin,
    /// Arithmetic exceeded the supported unsigned amount range.
    #[error("coin selection amount overflow")]
    Overflow,
    /// Spendable, unreserved, unexpired candidates cannot cover target plus fee.
    #[error("insufficient spendable funds")]
    InsufficientFunds,
    /// Required shape exceeds an explicit input/output or allocation bound.
    #[error("coin selection exceeds limits")]
    LimitExceeded,
    /// The estimated fee exceeds the approved budget.
    #[error("estimated fee exceeds approved maximum")]
    FeeBudgetExceeded,
    /// A bounded fee loop did not obtain a covered estimate.
    #[error("coin selection fee did not converge")]
    FeeDidNotConverge,
    /// Caller-provided estimator failed.
    #[error("fee estimator failed")]
    EstimationFailed,
}

/// Output policy for sweeping or threshold aggregation in a bounded snapshot.
#[derive(Clone, Copy, Debug)]
pub enum SweepMode {
    /// Aggregate eligible coins up to an input denomination threshold, using
    /// other coins for the exact fee as needed. May leave larger coins unspent.
    AggregateThreshold(AggregationPolicy),
    /// Preserve input denomination capacity, valid at any Qi position in a block.
    PreserveDenominations,
    /// Combine denominations, requiring the node's first-Qi-transaction block
    /// exception. Mempool acceptance does not guarantee eligible block placement.
    Aggregate {
        /// Largest output denomination permitted by the caller.
        maximum: Denomination,
    },
}

/// Spend eligible, unreserved coins with no change. `request.target` must
/// be zero; output value is total minus the explicit fee. Exceeding an input
/// bound fails instead of silently leaving coins behind. Aggregation must reduce
/// output count and may need a node capable of arranging first-Qi block placement.
/// AggregateThreshold delegates to select_aggregate and has its own explicit
/// threshold/reduction policy; the other modes spend every eligible coin.
pub fn select_sweep(
    coins: &[CandidateCoin],
    request: &SelectionRequest,
    mode: SweepMode,
) -> Result<CoinSelection, SelectionError> {
    if let SweepMode::AggregateThreshold(policy) = mode {
        return select_aggregate(coins, request, policy);
    }
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
    let mut inputs = Vec::new();
    let mut capacity = [0u64; 15];
    let mut total = U256::ZERO;
    for coin in coins {
        let hash = coin.outpoint.transaction_hash.bytes();
        if !seen.insert(coin.outpoint)
            || hash[2] != coin.address.zone().byte()
            || hash[3] & 0x80 == 0
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
        if inputs.len() == request.max_inputs {
            return Err(SelectionError::LimitExceeded);
        }
        capacity[usize::from(coin.denomination.index())] += 1;
        total = total
            .checked_add(U256::from(coin.denomination.value()))
            .ok_or(SelectionError::Overflow)?;
        inputs.push(coin.clone());
    }
    if total <= request.fee {
        return Err(SelectionError::InsufficientFunds);
    }
    let value = total - request.fee;
    let spend_outputs = match mode {
        SweepMode::AggregateThreshold(_) => unreachable!("threshold policy dispatched above"),
        SweepMode::PreserveDenominations => {
            denominate_available(value, &mut capacity, request.max_outputs)?
        }
        SweepMode::Aggregate { maximum } => {
            let mut remaining = value;
            let mut outputs = Vec::new();
            for i in (0..=maximum.index()).rev() {
                let denomination = Denomination::new(i).map_err(|_| SelectionError::InvalidCoin)?;
                let unit = U256::from(denomination.value());
                let count = remaining / unit;
                if count > U256::from(request.max_outputs - outputs.len()) {
                    return Err(SelectionError::LimitExceeded);
                }
                outputs.extend(std::iter::repeat_n(denomination, count.to::<usize>()));
                remaining %= unit;
            }
            if outputs.len() >= inputs.len() {
                return Err(SelectionError::InvalidRequest);
            }
            outputs
        }
    };
    if inputs.len() * 3 + spend_outputs.len() + 3 > MAX_TRANSACTION_MESSAGES {
        return Err(SelectionError::LimitExceeded);
    }
    Ok(CoinSelection {
        inputs,
        spend_outputs,
        change_outputs: vec![],
        input_value: total,
        fee: request.fee,
    })
}

/// Match the reference's fewest-input policy: smallest sufficient single coin,
/// otherwise descending-value greedy inputs. No exponential subset search.
pub fn select_fewest(
    coins: &[CandidateCoin],
    request: &SelectionRequest,
) -> Result<CoinSelection, SelectionError> {
    if request.target == U256::ZERO
        || request.fee > request.max_fee
        || !(1..=4096).contains(&request.max_inputs)
        || !(1..=4096).contains(&request.max_outputs)
    {
        return Err(SelectionError::InvalidRequest);
    }
    if coins.len() > 100000 {
        return Err(SelectionError::LimitExceeded);
    }
    let required = request
        .target
        .checked_add(request.fee)
        .ok_or(SelectionError::Overflow)?;
    let mut seen = BTreeSet::new();
    let mut buckets: [Vec<&CandidateCoin>; 15] = std::array::from_fn(|_| Vec::new());
    for coin in coins {
        let hash = coin.outpoint.transaction_hash.bytes();
        if !seen.insert(coin.outpoint)
            || hash[2] != coin.address.zone().byte()
            || hash[3] & 0x80 == 0
        {
            return Err(SelectionError::InvalidCoin);
        }
        if coin.address.zone() == request.zone
            && !coin.reserved
            && coin.unlock_height <= request.candidate_height
            && coin
                .expires_at
                .is_none_or(|height| request.candidate_height < height)
        {
            buckets[usize::from(coin.denomination.index())].push(coin);
        }
    }
    // Fifteen denomination buckets provide O(n) grouping while matching the
    // reference's stable ascending order and reversed tie order in its fallback.
    let mut chosen = Vec::new();
    let mut total = U256::ZERO;
    if let Some(coin) = buckets
        .iter()
        .flatten()
        .find(|c| U256::from(c.denomination.value()) >= required)
    {
        chosen.push((*coin).clone());
        total = U256::from(coin.denomination.value());
    } else {
        for coin in buckets
            .into_iter()
            .rev()
            .flat_map(|bucket| bucket.into_iter().rev())
        {
            if total >= required {
                break;
            }
            if chosen.len() == request.max_inputs {
                return Err(SelectionError::LimitExceeded);
            }
            total = total
                .checked_add(U256::from(coin.denomination.value()))
                .ok_or(SelectionError::Overflow)?;
            chosen.push(coin.clone());
        }
    }
    if total < required {
        return Err(SelectionError::InsufficientFunds);
    }
    let mut capacity = [0u64; 15];
    for coin in &chosen {
        capacity[usize::from(coin.denomination.index())] += 1;
    }
    let spend_outputs = denominate_available(request.target, &mut capacity, request.max_outputs)?;
    let change_outputs = denominate_available(
        total - required,
        &mut capacity,
        request.max_outputs - spend_outputs.len(),
    )?;
    if chosen.len() * 3 + spend_outputs.len() + change_outputs.len() + 3 > MAX_TRANSACTION_MESSAGES
    {
        return Err(SelectionError::LimitExceeded);
    }
    Ok(CoinSelection {
        inputs: chosen,
        spend_outputs,
        change_outputs,
        input_value: total,
        fee: request.fee,
    })
}

// Ordinary transactions may split denominations, but cannot combine smaller
// inputs into larger outputs. Consume a shared inventory for recipient + change.
// Borrow only the coins needed from larger denominations, preserving large change.
fn denominate_available(
    mut value: U256,
    capacity: &mut [u64; 15],
    max_outputs: usize,
) -> Result<Vec<Denomination>, SelectionError> {
    let mut result = Vec::new();
    for i in (0..15).rev() {
        let denomination = Denomination::new(i as u8).map_err(|_| SelectionError::InvalidCoin)?;
        let unit = U256::from(denomination.value());
        while value >= unit {
            let Some(source) = (i..15).find(|&j| capacity[j] > 0) else {
                break;
            };
            if result.len() == max_outputs {
                return Err(SelectionError::LimitExceeded);
            }
            for j in (i + 1..=source).rev() {
                capacity[j] -= 1;
                let larger = Denomination::new(j as u8)
                    .map_err(|_| SelectionError::InvalidCoin)?
                    .value();
                let smaller = Denomination::new((j - 1) as u8)
                    .map_err(|_| SelectionError::InvalidCoin)?
                    .value();
                capacity[j - 1] = capacity[j - 1]
                    .checked_add(larger / smaller)
                    .ok_or(SelectionError::Overflow)?;
            }
            capacity[i] -= 1;
            value -= unit;
            result.push(denomination);
        }
    }
    if value != U256::ZERO {
        return Err(SelectionError::InsufficientFunds);
    }
    Ok(result)
}

/// Re-select until the exact selected shape's quoted fee is covered, never
/// exceeding the authorized maximum. Fees only increase to prevent oscillation;
/// an overestimate can therefore remain paid and is reported in the result.
/// The estimator must price the final input/output shape, including change.
pub fn select_with_fee(
    coins: &[CandidateCoin],
    request: &SelectionRequest,
    max_rounds: u8,
    mut estimate: impl FnMut(&CoinSelection) -> Result<U256, SelectionError>,
) -> Result<CoinSelection, SelectionError> {
    if !(1..=32).contains(&max_rounds) {
        return Err(SelectionError::InvalidRequest);
    }
    let mut current = request.clone();
    for _ in 0..max_rounds {
        let selection = select_fewest(coins, &current)?;
        let quoted = estimate(&selection)?;
        if quoted <= current.fee {
            return Ok(selection);
        }
        if quoted > request.max_fee {
            return Err(SelectionError::FeeBudgetExceeded);
        }
        current.fee = quoted;
    }
    Err(SelectionError::FeeDidNotConverge)
}

/// Verify that outputs can split the input inventory without combining smaller
/// input denominations into larger outputs. Output ordering does not affect this
/// capacity check; aggregation requires a separate explicit block-position policy.
pub fn preserves_denominations(
    inputs: &[Denomination],
    outputs: &[Denomination],
) -> Result<(), SelectionError> {
    if inputs.is_empty() || inputs.len() > 4096 || outputs.len() > 4096 {
        return Err(SelectionError::InvalidRequest);
    }
    let mut capacity = [0u64; 15];
    for input in inputs {
        capacity[input.index() as usize] += 1;
    }
    let mut ordered = outputs.to_vec();
    ordered.sort_unstable_by_key(|d| std::cmp::Reverse(d.index()));
    for output in ordered {
        if denominate_available(U256::from(output.value()), &mut capacity, 1)? != [output] {
            return Err(SelectionError::InvalidRequest);
        }
    }
    Ok(())
}
