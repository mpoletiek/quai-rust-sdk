//! Portable same-input Qi replacement funded only by explicitly selected owned change.
use crate::qi_preflight::{
    QiFeeMode, QiPolicy, QiPreflightError, QiSource, candidate_height, network,
};
use quai_consensus::{
    QiConversionTransaction, QiOutput, QiTransaction, QiWrappingTransaction, SignedQiOperation,
};
use quai_primitives::{Hash32, QiAddress};
use quai_provider::{Provider, ProviderError};
use quai_rpc::{Transport, U256};
use quai_wallet::{SelectionError, metadata::PublicAddress};
use std::collections::{BTreeMap, BTreeSet};
/// Explicit parent and owned change to reduce. All nonselected parent outputs are
/// preserved, including payment recipients and, unless `aggregate_destination`
/// asks otherwise, conversion/wrapping destinations.
#[derive(Clone, Debug)]
pub struct QiReplacementIntent {
    /// Original or earlier persisted candidate to replace.
    pub parent: Hash32,
    /// Distinct parent output indexes asserted to be local change; ownership is checked.
    pub change_indexes: Vec<u16>,
    /// Replacement owned change outputs; their total must be strictly lower.
    pub change_outputs: Vec<QiOutput>,
    /// Re-decompose a conversion's or wrap's Quai-ledger destination outputs
    /// largest-first, keeping their address and total value.
    ///
    /// The node credits that destination with one aggregated value, so the shape
    /// is free, and it charges `ETXGas` per destination output when deciding
    /// whether to include the transaction. Collapsing twelve outputs into two
    /// therefore cuts the gas the fee has to cover by more than half, which is
    /// how a parent that the miner keeps skipping becomes includable. Rejected
    /// for an ordinary transfer, whose recipient outputs are bound to the input
    /// denominations.
    pub aggregate_destination: bool,
}
/// Immutable same-input candidate; every unselected output and all data are fixed.
#[derive(Debug)]
pub struct QiReplacementQuote {
    transaction: QiTransaction,
    parent: Hash32,
    fee: U256,
    digest: Hash32,
    owners: Vec<PublicAddress>,
}
impl QiReplacementQuote {
    /// Exact fields to review; no signing or provider submission occurred.
    pub fn transaction(&self) -> &QiTransaction {
        &self.transaction
    }
    /// Explicit parent whose original bytes and claims must remain retained.
    pub fn parent_hash(&self) -> Hash32 {
        self.parent
    }
    /// Source-reported Qit fee after reducing selected change.
    pub fn fee(&self) -> U256 {
        self.fee
    }
    /// Exact operation-specific signing digest.
    pub fn signing_digest(&self) -> Hash32 {
        self.digest
    }
    /// Input and old/new change owners; a wallet must prove ownership before
    /// authorizing the candidate. Public metadata alone grants no spending right.
    pub fn required_owners(&self) -> &[PublicAddress] {
        &self.owners
    }
}
/// Quote a fixed higher-fee candidate with every original input and every
/// unselected output retained in order. Selected old/new change must have supplied
/// same-zone Qi ownership metadata, which the signing wallet must independently
/// verify. Recheck current input values/locks/expiry and canonical anchors. There
/// is one exact estimate, no automatic output rewriting, claim release or send.
pub async fn quote_qi_replacement<T: Transport>(
    provider: &Provider<T>,
    source: &QiSource,
    parent: &SignedQiOperation,
    intent: QiReplacementIntent,
    policy: QiPolicy,
    fees: QiFeeMode,
) -> Result<QiReplacementQuote, QiPreflightError> {
    use QiPreflightError as E;
    if parent.hash()? != intent.parent
        || parent.transaction().chain_id != source.scope.chain_id
        || parent.origin_zone()? != source.scope.zone
        || source.owners.len() > 100_000
        || source.coins.len() > 100_000
        || intent.change_indexes.is_empty()
        || intent.change_indexes.len() > 1024
        || intent.change_outputs.len() > 1024
        || !(1..=1024).contains(&policy.max_inputs)
        || !(1..=1024).contains(&policy.max_outputs)
    {
        return Err(E::Invalid);
    }
    let special = !parent.transaction().data.is_empty();
    if special && matches!(fees, QiFeeMode::Node)
        || !special && matches!(fees, QiFeeMode::Profile(_))
    {
        return Err(E::Invalid);
    }
    let metadata: BTreeMap<_, _> = source.owners.iter().map(|o| (o.address(), o)).collect();
    let coins: BTreeMap<_, _> = source.coins.iter().map(|c| (c.outpoint, c)).collect();
    if metadata.len() != source.owners.len() || coins.len() != source.coins.len() {
        return Err(E::Invalid);
    }
    let selected: BTreeSet<_> = intent.change_indexes.iter().copied().collect();
    if selected.len() != intent.change_indexes.len() {
        return Err(E::Invalid);
    }
    let mut owners = BTreeMap::new();
    let mut old_change = Vec::new();
    for index in &selected {
        old_change.push(
            parent
                .transaction()
                .outputs
                .get(*index as usize)
                .ok_or(E::Invalid)?
                .clone(),
        );
    }
    for output in old_change.iter().chain(&intent.change_outputs) {
        if QiAddress::try_from(output.address)
            .map_err(|_| E::Invalid)?
            .zone()
            != source.scope.zone
        {
            return Err(E::Invalid);
        }
        let owner = metadata.get(&output.address).ok_or(E::Invalid)?;
        owners.insert(output.address, (*owner).clone());
    }
    if value(&intent.change_outputs)? >= value(&old_change)? {
        return Err(E::Invalid);
    }
    let mut transaction = parent.transaction().clone();
    transaction.outputs = transaction
        .outputs
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !selected.contains(&(*i as u16)))
        .map(|(_, o)| o)
        .chain(intent.change_outputs)
        .collect();
    if intent.aggregate_destination {
        aggregate_destination_outputs(&mut transaction, policy.max_outputs)?;
    }
    if transaction.inputs.len() > policy.max_inputs
        || transaction.outputs.len() > policy.max_outputs
    {
        return Err(E::Invalid);
    }
    let digest = match transaction.data.len() {
        0 => transaction.signing_digest()?,
        20 => QiWrappingTransaction::from_transaction(transaction.clone())?.signing_digest()?,
        22 => QiConversionTransaction::from_transaction(transaction.clone())?.signing_digest()?,
        _ => return Err(E::Invalid),
    };
    network(provider, source.scope).await?;
    let height = candidate_height(provider, source, policy.max_snapshot_age).await?;
    let mut input_value = U256::ZERO;
    let mut denominations = Vec::new();
    for input in &transaction.inputs {
        let coin = coins.get(&input.previous_output).ok_or(E::Stale)?;
        if coin.address.address() != input.public_key.address()
            || coin.unlock_height > height
            || coin.expires_at.is_some_and(|end| height >= end)
        {
            return Err(E::Stale);
        }
        let owner = metadata
            .get(&input.public_key.address())
            .ok_or(E::Invalid)?;
        if *owner.public_key() != input.public_key.to_compressed() {
            return Err(E::Invalid);
        }
        owners.insert(owner.address(), (*owner).clone());
        input_value = input_value
            .checked_add(U256::from(coin.denomination.value()))
            .ok_or(E::Invalid)?;
        denominations.push(coin.denomination);
    }
    // Only outputs that stay in the Qi ledger are checked: go-quai removes
    // a conversion or wrapping destination from the tally before
    // `CheckDenominations`, because it credits their total as one value.
    quai_wallet::preserves_denominations(
        &denominations,
        &transaction
            .outputs
            .iter()
            .filter(|o| o.address.ledger() != quai_primitives::Ledger::Quai)
            .map(|o| o.denomination)
            .collect::<Vec<_>>(),
    )?;
    let fee = input_value
        .checked_sub(value(&transaction.outputs)?)
        .ok_or(E::Invalid)?;
    if input_value < value(&parent.transaction().outputs)? || fee > policy.max_fee {
        return Err(SelectionError::FeeBudgetExceeded.into());
    }
    // A profiled quote already carries the floor plus its margin; every other
    // mode is checked against the floor directly.
    if !matches!(fees, QiFeeMode::Profile(_))
        && inclusion_floor(provider, &transaction)
            .await?
            .is_some_and(|floor| fee < floor)
    {
        return Err(E::FeeBelowInclusionFloor);
    }
    let required = match fees {
        QiFeeMode::Node => provider.estimate_qi_fee(&transaction).await?,
        QiFeeMode::Profile(profile) => {
            provider
                .estimate_qi_special_fee(&transaction, profile)
                .await?
                .qits
        }
        QiFeeMode::Explicit(exact) => {
            if fee != exact {
                return Err(E::Invalid);
            }
            fee
        }
    };
    if required > fee {
        return Err(SelectionError::FeeBudgetExceeded.into());
    }
    let final_height = candidate_height(provider, source, policy.max_snapshot_age).await?;
    if transaction.inputs.iter().any(|i| {
        coins[&i.previous_output].unlock_height > final_height
            || coins[&i.previous_output]
                .expires_at
                .is_some_and(|end| final_height >= end)
    }) {
        return Err(E::Stale);
    }
    network(provider, source.scope).await?;
    Ok(QiReplacementQuote {
        transaction,
        parent: intent.parent,
        fee,
        digest,
        owners: owners.into_values().collect(),
    })
}
/// The smallest fee a conversion or wrap of this shape needs before the node's
/// inclusion filter will consider it, or `None` for an ordinary transfer or a
/// chain state the profile does not cover.
///
/// A replacement is the remedy for a parent the miner keeps skipping, so it must
/// not be another transaction below the same floor.
pub(crate) async fn inclusion_floor<T: Transport>(
    provider: &Provider<T>,
    transaction: &QiTransaction,
) -> Result<Option<U256>, ProviderError> {
    if transaction.data.is_empty() {
        return Ok(None);
    }
    match provider
        .estimate_qi_special_fee(transaction, quai_provider::QiFeeProfile::V056ShaAnchored)
        .await
    {
        Ok(quote) => Ok(Some(quote.floor_qits)),
        Err(ProviderError::ConversionFeeEstimationUnavailable) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Re-decompose a conversion's or wrap's Quai-ledger destination largest-first,
/// keeping its address and total value, in place of the parent's shape. The node
/// aggregates that destination into one credit and charges `ETXGas` for each of
/// those outputs when deciding whether to include the transaction, so a smaller
/// shape needs a much smaller fee to clear the same floor.
pub(crate) fn aggregate_destination_outputs(
    transaction: &mut QiTransaction,
    max_outputs: usize,
) -> Result<(), SelectionError> {
    let mut total = U256::ZERO;
    let mut destination = None;
    let mut first = None;
    for (index, output) in transaction.outputs.iter().enumerate() {
        if output.address.ledger() != quai_primitives::Ledger::Quai {
            continue;
        }
        if destination.is_some_and(|address| address != output.address) {
            // Consensus rejects a conversion with two destinations anyway.
            return Err(SelectionError::InvalidRequest);
        }
        destination = Some(output.address);
        first.get_or_insert(index);
        total = total
            .checked_add(U256::from(output.denomination.value()))
            .ok_or(SelectionError::Overflow)?;
    }
    // Only a conversion or a wrap has one. An ordinary transfer's recipient
    // outputs stay bound to the input denominations.
    let (destination, first) = destination
        .zip(first)
        .ok_or(SelectionError::InvalidRequest)?;
    let kept = transaction
        .outputs
        .iter()
        .filter(|output| output.address.ledger() != quai_primitives::Ledger::Quai)
        .count();
    let denominations = quai_wallet::denominate_largest(
        total,
        max_outputs
            .checked_sub(kept)
            .ok_or(SelectionError::LimitExceeded)?,
    )?;
    let mut outputs = Vec::with_capacity(kept + denominations.len());
    for (index, output) in std::mem::take(&mut transaction.outputs)
        .into_iter()
        .enumerate()
    {
        if index == first {
            outputs.extend(denominations.iter().map(|denomination| QiOutput {
                address: destination,
                denomination: *denomination,
            }));
        }
        if output.address.ledger() != quai_primitives::Ledger::Quai {
            outputs.push(output);
        }
    }
    transaction.outputs = outputs;
    Ok(())
}

fn value(outputs: &[QiOutput]) -> Result<U256, QiPreflightError> {
    outputs.iter().try_fold(U256::ZERO, |sum, o| {
        sum.checked_add(U256::from(o.denomination.value()))
            .ok_or(QiPreflightError::Invalid)
    })
}
