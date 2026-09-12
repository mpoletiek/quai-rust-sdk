//! Bounded deterministic ordinary-transfer selection over an explicit UTXO snapshot.
use quai_consensus::{Denomination, MAX_TRANSACTION_MESSAGES, OutPoint, U256};
use quai_primitives::{QiAddress, Zone};
use std::collections::BTreeSet;
use thiserror::Error;

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
    /// Recipient denominations, largest first and no larger than the largest input.
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
    let maximum = chosen
        .iter()
        .map(|c| c.denomination)
        .max()
        .ok_or(SelectionError::InsufficientFunds)?;
    let spend_outputs = denominate(request.target, maximum, request.max_outputs)?;
    let change_outputs = denominate(
        total - required,
        maximum,
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

fn denominate(
    mut value: U256,
    maximum: Denomination,
    max_outputs: usize,
) -> Result<Vec<Denomination>, SelectionError> {
    let mut result = Vec::new();
    for i in (0..=maximum.index()).rev() {
        let d = Denomination::new(i).map_err(|_| SelectionError::InvalidCoin)?;
        let unit = U256::from(d.value());
        let count = value / unit;
        if count > U256::from(max_outputs - result.len()) {
            return Err(SelectionError::LimitExceeded);
        }
        let count = count.to::<usize>();
        result.extend(std::iter::repeat_n(d, count));
        value %= unit;
    }
    if value != U256::ZERO {
        return Err(SelectionError::InvalidCoin);
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
