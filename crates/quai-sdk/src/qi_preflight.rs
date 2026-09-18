//! Portable exact-denomination Qi planning over explicit current observations.
use crate::discovery::CurrentQiDiscovery;
use quai_consensus::{
    QiConversionIntent, QiConversionTransaction, QiInput, QiOutput, QiTransaction,
    QiWrappingIntent, QiWrappingTransaction, TransactionError,
};
use quai_crypto::PublicKey;
use quai_primitives::{Hash32, QiAddress};
use quai_provider::{Provider, ProviderError, QiFeeProfile, QiFeeQuote};
use quai_rpc::{Transport, U256};
use quai_wallet::discovery::{CanonicalStatus, Checkpoint, NetworkScope};
use quai_wallet::metadata::{PublicAddress, StorageError};
use quai_wallet::{
    AccountPublic, CandidateCoin, CoinType, SelectionError, SelectionRequest, SweepMode,
    select_fewest, select_sweep,
};
use std::collections::{BTreeMap, BTreeSet};
/// Explicit limits for bounded selection and network fee convergence.
#[derive(Clone, Copy, Debug)]
pub struct QiPolicy {
    /// Starting fee in Qits. Zero is valid; estimates only increase it.
    pub initial_fee: U256,
    /// Maximum planned fee in Qits, including convergence overpayment.
    /// Input values are source claims; this is not a cryptographic on-chain fee cap.
    pub max_fee: U256,
    /// Maximum aggregate signing inputs, 1..=1024.
    pub max_inputs: usize,
    /// Maximum combined recipient and change outputs, 1..=1024.
    pub max_outputs: usize,
    /// Maximum exact-payload fee estimates, 1..=32.
    pub max_fee_rounds: u8,
    /// Maximum observed tip distance from the stored checkpoint, in zone blocks.
    pub max_snapshot_age: u64,
}

/// An exact Qit amount and ordered, distinct recipient address capacity.
///
/// The selector decomposes the amount into denominations and uses that many
/// addresses from the beginning of `destinations`, largest denomination first.
/// Every output requires its own address. Inspect the prepared outputs before signing.
#[derive(Clone, Debug)]
pub struct QiIntent {
    /// Positive exact recipient amount in Qits.
    pub amount: U256,
    /// Up to 1024 distinct Qi recipient addresses; cross-zone preparation is explicit.
    pub destinations: Vec<QiAddress>,
}

/// Explicit source observations, including imported and payment-code owners.
/// Metadata and checkpoints are not cryptographic proof of unspentness or ownership.
#[derive(Clone, Debug)]
pub struct QiSource {
    /// Independently configured network identity.
    pub scope: NetworkScope,
    /// Sampled source header, rechecked before and after estimation.
    pub checkpoint: Checkpoint,
    /// Bounded current candidates; local claims override source reservation flags.
    pub coins: Vec<CandidateCoin>,
    /// Exact public origins used to reconstruct selected input keys.
    pub owners: Vec<PublicAddress>,
}
impl QiSource {
    /// Convert a successful current HD scan, re-deriving every funded address
    /// against an independently trusted xpub. Latest-only queries remain separate
    /// observations. Use explicit sources for imported/payment-address observations.
    pub fn from_discovery(
        report: &CurrentQiDiscovery,
        account: &AccountPublic,
    ) -> Result<Self, QiPreflightError> {
        if report.canonical != CanonicalStatus::Matches
            || account.coin_type() != CoinType::Qi
            || report.addresses.len() > 100_000
        {
            return Err(QiPreflightError::Invalid);
        }
        let mut source = Self {
            scope: report.scope,
            checkpoint: report.checkpoint,
            coins: vec![],
            owners: vec![],
        };
        for address in &report.addresses {
            if address.outputs.is_empty() {
                continue;
            }
            if source
                .coins
                .len()
                .checked_add(address.outputs.len())
                .is_none_or(|n| n > 100_000)
            {
                return Err(QiPreflightError::Invalid);
            }
            let d = &address.derived;
            let owner = PublicAddress::derive(account, d.change, d.index)?;
            if d.coin != CoinType::Qi
                || d.account != account.account_index()
                || d.zone != report.scope.zone
                || owner.address() != d.address
                || *owner.public_key() != d.public_key
            {
                return Err(QiPreflightError::Invalid);
            }
            let qi = QiAddress::try_from(owner.address()).map_err(|_| QiPreflightError::Invalid)?;
            source
                .coins
                .extend(address.outputs.iter().map(|o| CandidateCoin {
                    outpoint: o.outpoint,
                    address: qi,
                    denomination: o.denomination,
                    unlock_height: o.unlock_height,
                    expires_at: None,
                    reserved: false,
                }));
            source.owners.push(owner);
        }
        Ok(source)
    }
}
/// Exact operation shape. Specialized data is never passed to an ordinary estimator.
#[derive(Clone, Debug)]
pub enum QiOperationIntent {
    /// Same-zone transfer, one fresh address for each recipient denomination.
    Transfer(QiIntent),
    /// Transfer to one explicitly different destination zone.
    CrossZone(QiIntent),
    /// Same-zone conversion, with the typed refund/slippage data.
    Conversion {
        /// Positive Qit amount to convert.
        amount: U256,
        /// Exact destination/refund/slippage.
        intent: QiConversionIntent,
    },
    /// Same-zone native backing deposit for a Quai beneficiary.
    Wrapping {
        /// Positive Qit amount to wrap.
        amount: U256,
        /// Exact owner contract and beneficiary.
        intent: QiWrappingIntent,
    },
    /// Spend every eligible input with no change, under an explicit denomination
    /// policy. Aggregation requires independently qualified block placement.
    Sweep {
        /// One fresh same-zone address per resulting output.
        destinations: Vec<QiAddress>,
        /// Preserve denominations or explicit aggregation.
        mode: SweepMode,
    },
}
/// Caller-selected fee source. Explicit fees are not presented as node estimates.
#[derive(Clone, Copy, Debug)]
pub enum QiFeeMode {
    /// Exact ordinary-payload node estimate with bounded monotonic convergence.
    Node,
    /// Caller-authorized Qits; insufficient fees may be rejected by the node.
    Explicit(U256),
    /// Specialized estimator under an independently selected compatible profile.
    Profile(QiFeeProfile),
}
/// Validation failures never sign, reserve inputs or send transactions.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum QiPreflightError {
    /// Invalid source identity, metadata, address capacity or policy.
    #[error("invalid Qi preflight")]
    Invalid,
    /// Source checkpoint is noncanonical, too old, or lock/expiry checks changed.
    #[error("stale Qi preflight observations")]
    Stale,
    /// Source or estimator failed.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// Exact denomination selection, resource or fee limit failed.
    #[error(transparent)]
    Selection(#[from] SelectionError),
    /// Public metadata failed validation.
    #[error(transparent)]
    Metadata(#[from] StorageError),
    /// Exact consensus shape failed validation.
    #[error(transparent)]
    Transaction(#[from] TransactionError),
}
/// Read-only preparation inputs. Fresh change allocation and custody are separate.
pub struct QiQuoteRequest<'a> {
    /// Explicit bounded current observations.
    pub source: &'a QiSource,
    /// Exact amount, destination capacity and operation data.
    pub intent: QiOperationIntent,
    /// Explicit Qit fee/resource/age bounds.
    pub policy: QiPolicy,
    /// Node, caller-specified or profiled specialized fee.
    pub fees: QiFeeMode,
    /// Already allocated fresh same-zone change addresses, in output order.
    pub change: &'a [PublicAddress],
}
/// Immutable advisory transaction. A quote alone neither burns change nor claims inputs.
#[derive(Debug)]
pub struct QiQuote {
    transaction: QiTransaction,
    fee: U256,
    digest: Hash32,
    recipient_outputs: usize,
    fee_quote: Option<QiFeeQuote>,
    selected: Vec<CandidateCoin>,
    owners: Vec<PublicAddress>,
    candidate_height: U256,
}
impl QiQuote {
    /// Exact ordered inputs, outputs and operation data for review.
    pub fn transaction(&self) -> &QiTransaction {
        &self.transaction
    }
    /// Source-reported input value minus outputs, in Qits; not proof of on-chain debit.
    pub fn fee(&self) -> U256 {
        self.fee
    }
    /// Exact operation-specific signing digest.
    pub fn signing_digest(&self) -> Hash32 {
        self.digest
    }
    /// Recipient denominations preceding all change outputs.
    pub fn recipient_outputs(&self) -> usize {
        self.recipient_outputs
    }
    /// Last specialized quote, when explicitly using a profile.
    pub fn fee_quote(&self) -> Option<QiFeeQuote> {
        self.fee_quote
    }
    /// Public owners of the selected inputs, deduplicated by address.
    pub fn selected_owners(&self) -> &[PublicAddress] {
        &self.owners
    }
    /// Candidate height used in the final lock and expiry checks.
    pub fn candidate_height(&self) -> U256 {
        self.candidate_height
    }
    /// Selected current observations; signing must verify key ownership independently.
    pub fn selected_inputs(&self) -> &[CandidateCoin] {
        &self.selected
    }
}
/// Validate scope, current metadata and fresh address shape, select exact
/// denominations, converge the fee and recheck canonicality/locks/expiry. The
/// caller must overlay durable claims before calling, then reserve the returned
/// inputs under a revision captured before source reads. No retries or side effects.
pub async fn quote_qi<T: Transport>(
    provider: &Provider<T>,
    request: QiQuoteRequest<'_>,
) -> Result<QiQuote, QiPreflightError> {
    use QiPreflightError as E;
    let QiQuoteRequest {
        source,
        intent,
        policy,
        fees,
        change,
    } = request;
    if !(1..=1024).contains(&policy.max_inputs)
        || !(1..=1024).contains(&policy.max_outputs)
        || !(1..=32).contains(&policy.max_fee_rounds)
        || policy.initial_fee > policy.max_fee
        || source.scope.chain_id == U256::ZERO
        || source.scope.genesis == Hash32::ZERO
        || source.checkpoint.hash == Hash32::ZERO
        || source.coins.len() > 100_000
        || source.owners.len() > 100_000
        || change.len() > 1024
    {
        return Err(E::Invalid);
    }
    let special = matches!(
        intent,
        QiOperationIntent::Conversion { .. } | QiOperationIntent::Wrapping { .. }
    );
    if matches!(fees, QiFeeMode::Node) && special
        || matches!(fees, QiFeeMode::Profile(_)) && !special
    {
        return Err(E::Invalid);
    }
    let scope = source.scope;
    let mut metadata = BTreeMap::new();
    for owner in &source.owners {
        if owner.address().zone().ok() != Some(scope.zone)
            || QiAddress::try_from(owner.address()).is_err()
            || metadata.insert(owner.address(), owner).is_some()
        {
            return Err(E::Invalid);
        }
    }
    let mut points = BTreeSet::new();
    for coin in &source.coins {
        if coin.address.zone() != scope.zone
            || !metadata.contains_key(&coin.address.address())
            || !points.insert(coin.outpoint)
        {
            return Err(E::Invalid);
        }
    }
    let (amount, destinations, sweep) = match &intent {
        QiOperationIntent::Transfer(v) | QiOperationIntent::CrossZone(v) => {
            (v.amount, v.destinations.as_slice(), None)
        }
        QiOperationIntent::Conversion { amount, .. }
        | QiOperationIntent::Wrapping { amount, .. } => (*amount, &[][..], None),
        QiOperationIntent::Sweep { destinations, mode } => {
            (U256::ZERO, destinations.as_slice(), Some(*mode))
        }
    };
    if sweep.is_none() && amount == U256::ZERO {
        return Err(E::Invalid);
    }
    if !special && (destinations.is_empty() || destinations.len() > 1024) {
        return Err(E::Invalid);
    }
    let destination_zone = destinations.first().map(|a| a.zone()).unwrap_or(scope.zone);
    let cross = matches!(intent, QiOperationIntent::CrossZone(_));
    if !special
        && ((destination_zone != scope.zone) != cross
            || destinations.iter().any(|a| a.zone() != destination_zone))
    {
        return Err(E::Invalid);
    }
    let mut addresses = BTreeSet::new();
    for address in destinations {
        if !addresses.insert(address.address()) {
            return Err(E::Invalid);
        }
    }
    for address in change {
        if address.address().zone().ok() != Some(scope.zone)
            || QiAddress::try_from(address.address()).is_err()
            || !addresses.insert(address.address())
        {
            return Err(E::Invalid);
        }
    }
    if sweep.is_some() && !change.is_empty() {
        return Err(E::Invalid);
    }
    network(provider, scope).await?;
    let height = candidate_height(provider, source, policy.max_snapshot_age).await?;
    let (mut fee, rounds) = match fees {
        QiFeeMode::Explicit(f) => (f, 1),
        _ => (policy.initial_fee, policy.max_fee_rounds),
    };
    if fee > policy.max_fee {
        return Err(SelectionError::FeeBudgetExceeded.into());
    }
    for _ in 0..rounds {
        let selection_request = SelectionRequest {
            zone: scope.zone,
            candidate_height: height,
            target: amount,
            fee,
            max_fee: policy.max_fee,
            max_inputs: policy.max_inputs,
            max_outputs: policy.max_outputs,
        };
        let selection = if let Some(mode) = sweep {
            select_sweep(&source.coins, &selection_request, mode)?
        } else {
            select_fewest(&source.coins, &selection_request)?
        };
        if (!special && selection.spend_outputs.len() > destinations.len())
            || selection.change_outputs.len() > change.len()
        {
            return Err(E::Invalid);
        }
        let inputs = selection
            .inputs
            .iter()
            .map(|coin| {
                let owner = metadata.get(&coin.address.address()).ok_or(E::Invalid)?;
                Ok(QiInput {
                    previous_output: coin.outpoint,
                    public_key: PublicKey::from_sec1_bytes(owner.public_key())
                        .map_err(|_| E::Invalid)?,
                })
            })
            .collect::<Result<Vec<_>, E>>()?;
        let recipient_outputs = selection.spend_outputs.len();
        let change_outputs: Vec<_> = selection
            .change_outputs
            .iter()
            .zip(change)
            .map(|(d, a)| QiOutput {
                address: a.address(),
                denomination: *d,
            })
            .collect();
        let (transaction, digest) = match &intent {
            QiOperationIntent::Conversion { intent, .. } => {
                let typed = QiConversionTransaction::new(
                    scope.chain_id,
                    inputs,
                    selection.spend_outputs,
                    change_outputs,
                    *intent,
                )?;
                let digest = typed.signing_digest()?;
                (typed.transaction().clone(), digest)
            }
            QiOperationIntent::Wrapping { intent, .. } => {
                let typed = QiWrappingTransaction::new(
                    scope.chain_id,
                    inputs,
                    selection.spend_outputs,
                    change_outputs,
                    *intent,
                )?;
                let digest = typed.signing_digest()?;
                (typed.transaction().clone(), digest)
            }
            _ => {
                let tx = QiTransaction {
                    chain_id: scope.chain_id,
                    inputs,
                    outputs: selection
                        .spend_outputs
                        .iter()
                        .zip(destinations)
                        .map(|(d, a)| QiOutput {
                            address: a.address(),
                            denomination: *d,
                        })
                        .chain(change_outputs)
                        .collect(),
                    data: vec![],
                };
                let digest = tx.signing_digest()?;
                (tx, digest)
            }
        };
        let fee_quote = match fees {
            QiFeeMode::Explicit(_) => None,
            QiFeeMode::Node => {
                let estimated = provider.estimate_qi_fee(&transaction).await?;
                if estimated > policy.max_fee {
                    return Err(SelectionError::FeeBudgetExceeded.into());
                }
                if estimated > fee {
                    fee = estimated;
                    continue;
                }
                None
            }
            QiFeeMode::Profile(profile) => {
                let q = provider
                    .estimate_qi_special_fee(&transaction, profile)
                    .await?;
                if q.qits > policy.max_fee {
                    return Err(SelectionError::FeeBudgetExceeded.into());
                }
                if q.qits > fee {
                    fee = q.qits;
                    continue;
                }
                Some(q)
            }
        };
        let candidate_height = candidate_height(provider, source, policy.max_snapshot_age).await?;
        if selection.inputs.iter().any(|coin| {
            coin.unlock_height > candidate_height
                || coin.expires_at.is_some_and(|h| candidate_height >= h)
        }) {
            return Err(E::Stale);
        }
        network(provider, scope).await?;
        let selected_owners: BTreeMap<_, _> = selection
            .inputs
            .iter()
            .map(|coin| {
                let owner = metadata[&coin.address.address()];
                (owner.address(), owner.clone())
            })
            .collect();
        return Ok(QiQuote {
            transaction,
            fee: selection.fee,
            digest,
            recipient_outputs,
            fee_quote,
            selected: selection.inputs,
            owners: selected_owners.into_values().collect(),
            candidate_height,
        });
    }
    Err(SelectionError::FeeDidNotConverge.into())
}
pub(crate) async fn network<T: Transport>(
    provider: &Provider<T>,
    scope: NetworkScope,
) -> Result<(), QiPreflightError> {
    if !crate::network::on_network(provider, scope, scope.zone).await? {
        return Err(QiPreflightError::Stale);
    }
    Ok(())
}
pub(crate) async fn candidate_height<T: Transport>(
    provider: &Provider<T>,
    source: &QiSource,
    max_age: u64,
) -> Result<U256, QiPreflightError> {
    let n = u64::try_from(source.checkpoint.height).map_err(|_| QiPreflightError::Stale)?;
    if provider
        .header_at(source.scope.zone, n)
        .await?
        .is_none_or(|h| h.hash != source.checkpoint.hash)
    {
        return Err(QiPreflightError::Stale);
    }
    let head = provider
        .latest_header(source.scope.zone)
        .await?
        .ok_or(QiPreflightError::Stale)?;
    let height = U256::from(head.number);
    if height
        .checked_sub(source.checkpoint.height)
        .is_none_or(|age| age > U256::from(max_age))
    {
        return Err(QiPreflightError::Stale);
    }
    height
        .checked_add(U256::from(1))
        .ok_or(QiPreflightError::Stale)
}

/// Include persisted HD/imported/payment owners beyond a bounded gap scan.
/// Already-present owners retain the original source observations; missing owners
/// are queried at latest in batches. The complete extension is applied only after
/// network/checkpoint rechecks succeed. It remains an advisory current view, with
/// no historical/atomic guarantee and no trimming profile inferred from absence.
pub async fn include_known_qi_addresses<T: Transport>(
    provider: &Provider<T>,
    source: &mut QiSource,
    owners: &[PublicAddress],
    max_age: u64,
) -> Result<(), QiPreflightError> {
    if owners.len() > 100_000 || source.owners.len() > 100_000 || source.coins.len() > 100_000 {
        return Err(QiPreflightError::Invalid);
    }
    let mut known: BTreeMap<_, _> = source
        .owners
        .iter()
        .map(|o| (o.address(), o.clone()))
        .collect();
    if known.len() != source.owners.len() {
        return Err(QiPreflightError::Invalid);
    }
    let mut missing = BTreeMap::new();
    for owner in owners {
        if owner.address().zone().ok() != Some(source.scope.zone)
            || QiAddress::try_from(owner.address()).is_err()
        {
            return Err(QiPreflightError::Invalid);
        }
        if let Some(old) = known.get(&owner.address()) {
            if old != owner {
                return Err(QiPreflightError::Invalid);
            }
        } else {
            if missing
                .insert(owner.address(), owner.clone())
                .is_some_and(|old| old != *owner)
            {
                return Err(QiPreflightError::Invalid);
            }
        }
    }
    if known.len() + missing.len() > 100_000 {
        return Err(QiPreflightError::Invalid);
    }
    network(provider, source.scope).await?;
    candidate_height(provider, source, max_age).await?;
    let mut points: BTreeSet<_> = source.coins.iter().map(|c| c.outpoint).collect();
    if points.len() != source.coins.len() {
        return Err(QiPreflightError::Invalid);
    }
    let mut added = Vec::new();
    let addresses = missing
        .keys()
        .map(|&address| QiAddress::try_from(address).map_err(|_| QiPreflightError::Invalid))
        .collect::<Result<Vec<_>, _>>()?;
    for page in addresses.chunks(quai_provider::MAX_OUTPOINT_ADDRESSES) {
        let mut observed = provider.outpoints_many(page).await?;
        for &address in page {
            // A missing row is a failure, never an owner without coins.
            let outputs = observed.remove(&address).ok_or(QiPreflightError::Invalid)?;
            for output in outputs {
                if points.len() >= 100_000 {
                    return Err(QiPreflightError::Invalid);
                }
                let outpoint = quai_consensus::OutPoint {
                    transaction_hash: output.outpoint.tx_hash,
                    index: output.outpoint.index,
                };
                if !points.insert(outpoint) {
                    return Err(QiPreflightError::Invalid);
                }
                added.push(CandidateCoin {
                    outpoint,
                    address,
                    denomination: quai_consensus::Denomination::new(output.denomination)?,
                    unlock_height: output.lock,
                    expires_at: None,
                    reserved: false,
                });
            }
        }
    }
    candidate_height(provider, source, max_age).await?;
    network(provider, source.scope).await?;
    source.coins.extend(added);
    known.extend(missing);
    source.owners = known.into_values().collect();
    Ok(())
}
