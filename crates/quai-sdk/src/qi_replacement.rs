//! Portable same-input Qi replacement funded only by explicitly selected owned change.
use crate::qi_preflight::{
    QiFeeMode, QiPolicy, QiPreflightError, QiSource, candidate_height, network,
};
use quai_consensus::{
    QiConversionTransaction, QiOutput, QiTransaction, QiWrappingTransaction, SignedQiOperation,
};
use quai_primitives::{Hash32, QiAddress};
use quai_provider::Provider;
use quai_rpc::{Transport, U256};
use quai_wallet::{SelectionError, metadata::PublicAddress};
use std::collections::{BTreeMap, BTreeSet};
/// Explicit parent and owned change to reduce. All nonselected parent outputs are
/// preserved, including payment recipients and conversion/wrapping destinations.
#[derive(Clone, Debug)]
pub struct QiReplacementIntent {
    /// Original or earlier persisted candidate to replace.
    pub parent: Hash32,
    /// Distinct parent output indexes asserted to be local change; ownership is checked.
    pub change_indexes: Vec<u16>,
    /// Replacement owned change outputs; their total must be strictly lower.
    pub change_outputs: Vec<QiOutput>,
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
fn value(outputs: &[QiOutput]) -> Result<U256, QiPreflightError> {
    outputs.iter().try_fold(U256::ZERO, |sum, o| {
        sum.checked_add(U256::from(o.denomination.value()))
            .ok_or(QiPreflightError::Invalid)
    })
}
