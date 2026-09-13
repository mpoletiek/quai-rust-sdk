//! Portable, exact-nonce account preparation without storage or signing side effects.
use quai_consensus::{AccessTuple, QuaiTransaction};
use quai_primitives::{Hash32, QuaiAddress};
use quai_provider::{AccessListItem, BlockTag, CallRequest, Provider, ProviderError, RpcData};
use quai_rpc::{Transport, U256};
use quai_wallet::discovery::NetworkScope;

/// Explicit state source for account preflight. No automatic fallback occurs.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AccountObservationPolicy {
    /// Observe the node's pending state; propagate unsupported-RPC errors.
    #[default]
    Pending,
    /// Pin all nonce, balance and simulation reads to a sampled latest height and
    /// recheck its hash before returning. This excludes transactions in the pool;
    /// durable local nonce claims still apply. Estimates remain advisory.
    PinnedLatest,
}

/// Application-specified limits; there is no implicit unlimited-fee default.
#[derive(Clone, Copy, Debug)]
pub struct FeePolicy {
    /// Maximum final gas limit, including the optional safety margin.
    pub max_gas: u64,
    /// Maximum accepted base units per gas.
    pub max_gas_price: U256,
    /// Maximum gas_limit * gas_price authorized in base units.
    pub max_total_fee: U256,
    /// Additional gas above the estimate, in basis points (0..=10,000).
    pub gas_margin_bps: u16,
}

/// An ordinary same-zone account transfer or contract call.
#[derive(Clone, Debug)]
pub struct AccountIntent {
    /// Exact destination. Deployment, cross-zone and conversion have separate workflows.
    pub to: QuaiAddress,
    /// Exact base-unit amount, never floating point.
    pub value: U256,
    /// Exact call bytes; retained in the reviewed/signed payload.
    pub data: RpcData,
    /// Ordered access declaration, retained without normalization.
    pub access_list: Vec<AccessTuple>,
}

/// Source of the final nonce used for every simulation request.
#[derive(Clone, Copy, Debug)]
pub enum AccountNonce {
    /// Use the larger of the durable allocation floor and node observation.
    AtLeast(u64),
    /// Reprepare a retained unsigned nonce; reject if the node has advanced past it.
    Exact(u64),
}
/// Preparation errors never mutate a wallet or trigger submission.
#[derive(Debug, thiserror::Error)]
pub enum AccountPreflightError {
    /// Node read or simulation failed.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// Invalid scope, payload, nonce or configuration.
    #[error("invalid account preflight")]
    Invalid,
    /// Explicit gas/price/fee limits or arithmetic bounds exceeded.
    #[error("account preflight exceeds fee limits")]
    FeeLimit,
    /// Observed balance is below value plus the maximum fee.
    #[error("insufficient observed account balance")]
    InsufficientBalance,
    /// Network identity or sampled canonical head changed.
    #[error("account preflight network observation changed")]
    ObservationChanged,
}
/// Immutable advisory result. A caller must durably reserve its nonce before
/// presenting it as a wallet operation; a quote alone reserves no funds or nonce.
#[derive(Debug)]
pub struct AccountQuote {
    transaction: QuaiTransaction,
    maximum_fee: U256,
    digest: Hash32,
}
impl AccountQuote {
    /// Final transaction, including the exact simulated nonce and ordered access list.
    pub fn transaction(&self) -> &QuaiTransaction {
        &self.transaction
    }
    /// Maximum gas debit in Its.
    pub fn maximum_fee(&self) -> U256 {
        self.maximum_fee
    }
    /// Digest for the fixed transaction.
    pub fn signing_digest(&self) -> Hash32 {
        self.digest
    }
}
/// Validate a complete ordinary account transfer/call and estimate its final
/// nonce. Cross-zone Quai recipients are permitted; destination settlement is
/// separate. Access lists are preserved. Pending reads never silently fall back.
/// PinnedLatest observes a numeric block and rejects any sampled head change.
/// Chain/genesis checks surround the observations. No storage, signature, send,
/// automatic retry or claim of atomic pending state is involved.
pub async fn quote_account<T: Transport>(
    provider: &Provider<T>,
    scope: NetworkScope,
    sender: QuaiAddress,
    intent: AccountIntent,
    nonce: AccountNonce,
    observation: AccountObservationPolicy,
    policy: FeePolicy,
) -> Result<AccountQuote, AccountPreflightError> {
    use AccountPreflightError as E;
    if scope.chain_id == U256::ZERO || scope.genesis == Hash32::ZERO || sender.zone() != scope.zone
    {
        return Err(E::Invalid);
    }
    if policy.max_gas == 0 || policy.gas_margin_bps > 10_000 {
        return Err(E::FeeLimit);
    }
    let mut transaction = QuaiTransaction {
        chain_id: scope.chain_id,
        nonce: u64::MAX,
        to: Some(intent.to.address()),
        value: intent.value,
        gas_limit: policy.max_gas,
        gas_price: U256::MAX,
        data: intent.data.bytes().to_vec(),
        access_list: intent.access_list,
    };
    transaction.unsigned_bytes().map_err(|_| E::Invalid)?;
    check_network(provider, scope).await?;
    let header = match observation {
        AccountObservationPolicy::Pending => None,
        AccountObservationPolicy::PinnedLatest => Some(
            provider
                .latest_header(scope.zone)
                .await?
                .ok_or(E::ObservationChanged)?,
        ),
    };
    let block = header.as_ref().map_or(BlockTag::Pending, |h| {
        BlockTag::Number(U256::from(h.number))
    });
    let remote = provider.transaction_count(sender, block).await?;
    transaction.nonce = match nonce {
        AccountNonce::AtLeast(floor) => floor.max(remote),
        AccountNonce::Exact(n) if n >= remote => n,
        AccountNonce::Exact(_) => return Err(E::Invalid),
    };
    transaction.gas_price = provider.gas_price(scope.zone).await?;
    if transaction.gas_price > policy.max_gas_price {
        return Err(E::FeeLimit);
    }
    let request = CallRequest {
        from: sender,
        to: Some(intent.to),
        gas: Some(policy.max_gas),
        gas_price: Some(transaction.gas_price),
        value: Some(intent.value),
        nonce: Some(transaction.nonce),
        input: intent.data,
        access_list: transaction
            .access_list
            .iter()
            .map(|a| AccessListItem {
                address: a.address,
                storage_keys: a.storage_keys.clone(),
            })
            .collect(),
    };
    let estimate = provider.estimate_gas(&request, block).await?;
    let gas =
        (u128::from(estimate) * (10_000 + u128::from(policy.gas_margin_bps))).div_ceil(10_000);
    if gas == 0 || gas > u128::from(policy.max_gas) {
        return Err(E::FeeLimit);
    }
    transaction.gas_limit = u64::try_from(gas).map_err(|_| E::FeeLimit)?;
    let fee = transaction
        .gas_price
        .checked_mul(U256::from(transaction.gas_limit))
        .ok_or(E::FeeLimit)?;
    if fee > policy.max_total_fee {
        return Err(E::FeeLimit);
    }
    let debit = fee.checked_add(transaction.value).ok_or(E::FeeLimit)?;
    if debit > provider.balance(sender, block).await? {
        return Err(E::InsufficientBalance);
    }
    if let Some(before) = header {
        let after = provider
            .latest_header(scope.zone)
            .await?
            .ok_or(E::ObservationChanged)?;
        if before.hash != after.hash || before.number != after.number {
            return Err(E::ObservationChanged);
        }
    }
    check_network(provider, scope).await?;
    let digest = transaction.signing_digest().map_err(|_| E::Invalid)?;
    Ok(AccountQuote {
        transaction,
        maximum_fee: fee,
        digest,
    })
}
pub(crate) async fn check_network<T: Transport>(
    provider: &Provider<T>,
    scope: NetworkScope,
) -> Result<(), AccountPreflightError> {
    if provider.chain_id(scope.zone.into()).await? != scope.chain_id
        || provider.genesis_hash(scope.zone).await? != scope.genesis
    {
        return Err(AccountPreflightError::ObservationChanged);
    }
    Ok(())
}
