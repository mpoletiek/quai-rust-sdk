//! Portable, exact-nonce account preparation without storage or signing side effects.
use quai_consensus::{AccessTuple, ConversionSlippage, QuaiToQiTransaction, QuaiTransaction};
use quai_primitives::{Hash32, QiAddress, QuaiAddress, Zone};
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

impl FeePolicy {
    /// Whether the policy can bound anything: a nonzero gas cap and a margin of
    /// at most 100%.
    pub(crate) fn is_usable(&self) -> bool {
        self.max_gas != 0 && self.gas_margin_bps <= 10_000
    }
    /// The estimate plus the margin, rounded up. None when it is zero or does
    /// not fit a gas limit.
    pub(crate) fn margin_gas(&self, estimate: u64) -> Option<u64> {
        let gas =
            (u128::from(estimate) * (10_000 + u128::from(self.gas_margin_bps))).div_ceil(10_000);
        u64::try_from(gas).ok().filter(|gas| *gas != 0)
    }
    /// Gas limit, maximum fee and maximum debit a quote may authorize, or None
    /// when the policy's gas or fee cap refuses it. Every native and portable
    /// quote goes through here, so a new cap applies to all of them; the
    /// balance check stays with the caller, which owns the read.
    pub(crate) fn bound(&self, estimate: u64, gas_price: U256, value: U256) -> Option<FeeBound> {
        let gas = self
            .margin_gas(estimate)
            .filter(|gas| *gas <= self.max_gas)?;
        let fee = gas_price
            .checked_mul(U256::from(gas))
            .filter(|fee| *fee <= self.max_total_fee)?;
        Some(FeeBound {
            gas,
            fee,
            debit: fee.checked_add(value)?,
        })
    }
}

/// What a [`FeePolicy`] allows one quote to authorize.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FeeBound {
    pub(crate) gas: u64,
    pub(crate) fee: U256,
    pub(crate) debit: U256,
}

/// Explicit access-list behavior for ordinary calls and deployment preparation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AccountAccessListPolicy {
    /// Retain the exact application-supplied ordered entries.
    #[default]
    Preserve,
    /// Discover at the final nonce and state selector, requiring coverage of every
    /// supplied address and storage key. Review the resulting list before signing.
    Discover,
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

/// Explicit same-zone account-ledger conversion into Qi.
#[derive(Clone, Debug)]
pub struct QuaiConversionIntent {
    /// Same-zone Qi recipient.
    pub destination: QiAddress,
    /// Native Its amount; the typed conversion minimum applies.
    pub value: U256,
    /// Exact controller slippage encoded in the signed two-byte payload.
    pub slippage: ConversionSlippage,
}
pub(crate) enum AccountOperationIntent {
    Call(AccountIntent),
    Conversion(QuaiConversionIntent),
    #[cfg(feature = "abi")]
    Deployment(crate::contracts::PreparedDeployment),
}
impl AccountOperationIntent {
    pub(crate) fn zone(&self) -> Zone {
        match self {
            Self::Call(v) => v.to.zone(),
            Self::Conversion(v) => v.destination.zone(),
            #[cfg(feature = "abi")]
            Self::Deployment(v) => v.address().zone(),
        }
    }
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
#[non_exhaustive]
pub enum AccountPreflightError {
    /// The endpoint is on a different chain or genesis than the scope.
    #[error("endpoint is on another network")]
    NetworkMismatch,
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

impl AccountPreflightError {
    /// How to react to this failure; see [`quai_primitives::ErrorClass`].
    /// Matched exhaustively so a new variant must choose a class.
    pub fn class(&self) -> quai_primitives::ErrorClass {
        use quai_primitives::ErrorClass;
        match self {
            Self::NetworkMismatch => ErrorClass::NetworkMismatch,
            Self::Provider(error) => error.class(),
            Self::ObservationChanged => ErrorClass::Stale,
            Self::Invalid | Self::FeeLimit | Self::InsufficientBalance => ErrorClass::Invalid,
        }
    }
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
    quote_operation(
        provider,
        scope,
        sender,
        AccountOperationIntent::Call(intent),
        nonce,
        observation,
        policy,
        AccountAccessListPolicy::Preserve,
    )
    .await
}
/// Ordinary quotation with explicit access-list discovery. Required entries must
/// remain covered; no fallback to the supplied list occurs on node failure.
#[allow(clippy::too_many_arguments)]
pub async fn quote_account_with_access<T: Transport>(
    provider: &Provider<T>,
    scope: NetworkScope,
    sender: QuaiAddress,
    intent: AccountIntent,
    nonce: AccountNonce,
    observation: AccountObservationPolicy,
    policy: FeePolicy,
    access: AccountAccessListPolicy,
) -> Result<AccountQuote, AccountPreflightError> {
    quote_operation(
        provider,
        scope,
        sender,
        AccountOperationIntent::Call(intent),
        nonce,
        observation,
        policy,
        access,
    )
    .await
}
/// Estimate a typed Quai-to-Qi conversion without substituting an account
/// destination or dropping its slippage payload. Uses the same exact nonce,
/// fee bounds and network/head checks as ordinary account quotation. Quotes
/// reserve nothing and do not establish destination maturity or final value.
pub async fn quote_quai_conversion<T: Transport>(
    provider: &Provider<T>,
    scope: NetworkScope,
    sender: QuaiAddress,
    intent: QuaiConversionIntent,
    nonce: AccountNonce,
    observation: AccountObservationPolicy,
    policy: FeePolicy,
) -> Result<AccountQuote, AccountPreflightError> {
    quote_operation(
        provider,
        scope,
        sender,
        AccountOperationIntent::Conversion(intent),
        nonce,
        observation,
        policy,
        AccountAccessListPolicy::Preserve,
    )
    .await
}
/// Quote frozen deployment init code at its exact nonce, validating the predicted
/// address against the sender. The caller must reserve that nonce before grinding;
/// this read-only operation does not create or repair a reservation.
#[cfg(feature = "abi")]
pub async fn quote_deployment<T: Transport>(
    provider: &Provider<T>,
    scope: NetworkScope,
    sender: QuaiAddress,
    deployment: crate::contracts::PreparedDeployment,
    observation: AccountObservationPolicy,
    policy: FeePolicy,
) -> Result<AccountQuote, AccountPreflightError> {
    let nonce = AccountNonce::Exact(deployment.nonce());
    quote_operation(
        provider,
        scope,
        sender,
        AccountOperationIntent::Deployment(deployment),
        nonce,
        observation,
        policy,
        AccountAccessListPolicy::Preserve,
    )
    .await
}
#[allow(clippy::too_many_arguments)]
pub(crate) async fn quote_operation<T: Transport>(
    provider: &Provider<T>,
    scope: NetworkScope,
    sender: QuaiAddress,
    intent: AccountOperationIntent,
    nonce: AccountNonce,
    observation: AccountObservationPolicy,
    policy: FeePolicy,
    access: AccountAccessListPolicy,
) -> Result<AccountQuote, AccountPreflightError> {
    use AccountPreflightError as E;
    if scope.chain_id == U256::ZERO || scope.genesis == Hash32::ZERO || sender.zone() != scope.zone
    {
        return Err(E::Invalid);
    }
    if !policy.is_usable() {
        return Err(E::Invalid);
    }
    let conversion = matches!(&intent, AccountOperationIntent::Conversion(_));
    if conversion && intent.zone() != scope.zone {
        return Err(E::Invalid);
    }
    let mut transaction = match intent {
        AccountOperationIntent::Call(v) => QuaiTransaction {
            chain_id: scope.chain_id,
            nonce: u64::MAX,
            to: Some(v.to.address()),
            value: v.value,
            gas_limit: policy.max_gas,
            gas_price: U256::MAX,
            data: v.data.bytes().to_vec(),
            access_list: v.access_list,
        },
        AccountOperationIntent::Conversion(v) => QuaiTransaction {
            chain_id: scope.chain_id,
            nonce: u64::MAX,
            to: Some(v.destination.address()),
            value: v.value,
            gas_limit: policy.max_gas,
            gas_price: U256::MAX,
            data: v.slippage.to_be_bytes().to_vec(),
            access_list: vec![],
        },
        #[cfg(feature = "abi")]
        AccountOperationIntent::Deployment(v) => {
            let predicted = v.address();
            let tx = v
                .into_transaction(policy.max_gas, U256::MAX)
                .map_err(|_| E::Invalid)?;
            if tx.chain_id != scope.chain_id
                || predicted.zone() != scope.zone
                || !matches!(nonce, AccountNonce::Exact(n) if n == tx.nonce)
                || quai_primitives::contract_address(sender.address(), tx.nonce, &tx.data)
                    != predicted.address()
            {
                return Err(E::Invalid);
            }
            tx
        }
    };
    if conversion {
        QuaiToQiTransaction::new(transaction.clone()).map_err(|_| E::Invalid)?;
    }
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
    let estimate = if conversion {
        let typed = QuaiToQiTransaction::new(transaction.clone()).map_err(|_| E::Invalid)?;
        provider
            .estimate_quai_conversion_gas_budget(sender, &typed, block)
            .await?
    } else {
        let mut request = CallRequest {
            from: sender,
            to: transaction
                .to
                .map(QuaiAddress::try_from)
                .transpose()
                .map_err(|_| E::Invalid)?,
            gas: Some(policy.max_gas),
            gas_price: Some(transaction.gas_price),
            value: Some(transaction.value),
            nonce: Some(transaction.nonce),
            input: RpcData::new(transaction.data.clone())?,
            access_list: transaction
                .access_list
                .iter()
                .map(|a| AccessListItem {
                    address: a.address,
                    storage_keys: a.storage_keys.clone(),
                })
                .collect(),
        };
        populate_access(provider, access, &mut request, &mut transaction, block).await?;
        provider.estimate_gas(&request, block).await?
    };
    let FeeBound { gas, fee, debit } = policy
        .bound(estimate, transaction.gas_price, transaction.value)
        .ok_or(E::FeeLimit)?;
    transaction.gas_limit = gas;
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
    if !crate::network::on_network(provider, scope, scope.zone).await? {
        return Err(AccountPreflightError::NetworkMismatch);
    }
    Ok(())
}

/// Shared access population for native and browser preparation. Replacement
/// candidates must never call this helper since their access lists are fixed.
pub(crate) async fn populate_access<T: Transport>(
    provider: &Provider<T>,
    policy: AccountAccessListPolicy,
    request: &mut CallRequest,
    transaction: &mut QuaiTransaction,
    block: BlockTag,
) -> Result<(), AccountPreflightError> {
    if policy == AccountAccessListPolicy::Preserve {
        return Ok(());
    }
    let generated = provider.create_access_list(request, block).await?;
    let mut coverage = std::collections::BTreeMap::<_, std::collections::BTreeSet<_>>::new();
    for entry in &generated.access_list {
        coverage
            .entry(entry.address)
            .or_default()
            .extend(entry.storage_keys.iter().copied());
    }
    for required in &request.access_list {
        let Some(keys) = coverage.get(&required.address) else {
            return Err(AccountPreflightError::Invalid);
        };
        if required.storage_keys.iter().any(|key| !keys.contains(key)) {
            return Err(AccountPreflightError::Invalid);
        }
    }
    transaction.access_list = generated
        .access_list
        .iter()
        .map(|entry| AccessTuple {
            address: entry.address,
            storage_keys: entry.storage_keys.clone(),
        })
        .collect();
    transaction
        .unsigned_bytes()
        .map_err(|_| AccountPreflightError::Invalid)?;
    request.access_list = generated.access_list;
    Ok(())
}

#[cfg(test)]
mod fee_bound_tests {
    use super::*;

    #[test]
    fn bound_matches_the_plain_arithmetic_at_every_edge() {
        // Independent restatement of the rule every quote path now shares.
        let oracle = |p: FeePolicy, estimate: u64, price: U256, value: U256| {
            let gas =
                (u128::from(estimate) * (10_000 + u128::from(p.gas_margin_bps))).div_ceil(10_000);
            if gas == 0 || gas > u128::from(p.max_gas) {
                return None;
            }
            let fee = price.checked_mul(U256::from(gas as u64))?;
            if fee > p.max_total_fee {
                return None;
            }
            Some((gas as u64, fee, fee.checked_add(value)?))
        };
        for max_gas in [1u64, 21_000, 30_000, u64::MAX] {
            for bps in [0u16, 1, 2_000, 10_000] {
                for max_total in [U256::ZERO, U256::from(21_000u64 * 3), U256::MAX] {
                    let policy = FeePolicy {
                        max_gas,
                        max_gas_price: U256::MAX,
                        max_total_fee: max_total,
                        gas_margin_bps: bps,
                    };
                    for estimate in [0u64, 1, 21_000, 25_000, u64::MAX] {
                        for price in [U256::ZERO, U256::from(3), U256::MAX] {
                            for value in [U256::ZERO, U256::MAX] {
                                let got = policy
                                    .bound(estimate, price, value)
                                    .map(|b| (b.gas, b.fee, b.debit));
                                assert_eq!(got, oracle(policy, estimate, price, value));
                            }
                        }
                    }
                }
            }
        }
    }
}
