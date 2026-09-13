//! Portable fee-only account replacement quotation without custody side effects.
use crate::account_preflight::{
    AccountObservationPolicy, AccountPreflightError, FeePolicy, check_network,
};
use quai_consensus::{QuaiToQiTransaction, QuaiTransaction, SignedQuaiTransaction};
use quai_primitives::{Hash32, Ledger, QuaiAddress};
use quai_provider::{AccessListItem, BlockTag, CallRequest, Provider, RpcData};
use quai_rpc::{Transport, U256};
use quai_wallet::discovery::NetworkScope;
/// Explicit pool bump and maximum debit policy. Nodes may configure different
/// admission rules; a quote does not guarantee replacement preference or acceptance.
#[derive(Clone, Copy, Debug)]
pub struct ReplacementPolicy {
    /// Required price increase relative to the selected parent, 1..=1000 percent.
    pub minimum_price_bump_percent: u16,
    /// Gas/price/debit limits for the reviewed candidate.
    pub fees: FeePolicy,
}
/// Immutable fee-only candidate. The parent may still mine; both retain one nonce.
#[derive(Debug)]
pub struct AccountReplacementQuote {
    transaction: QuaiTransaction,
    parent: Hash32,
    fee: U256,
    digest: Hash32,
}
impl AccountReplacementQuote {
    /// All parent fields are retained except the increased gas price.
    pub fn transaction(&self) -> &QuaiTransaction {
        &self.transaction
    }
    /// Exact selected signed parent's identity.
    pub fn parent_hash(&self) -> Hash32 {
        self.parent
    }
    /// Maximum gas debit in Its.
    pub fn maximum_fee(&self) -> U256 {
        self.fee
    }
    /// Exact fixed signing digest.
    pub fn signing_digest(&self) -> Hash32 {
        self.digest
    }
}
/// Increase only gas price, with explicit bump/debit limits, without allocating
/// a nonce or repopulating calldata/access entries. Reject a confirmed spent nonce;
/// a pending nonce beyond this candidate does not prevent replacing a pooled one.
/// Check the exact ordinary or specialized estimate and all canonical/network
/// anchors. This read-only quote must be bound to durable family custody by a caller.
pub async fn quote_account_replacement<T: Transport>(
    provider: &Provider<T>,
    scope: NetworkScope,
    parent: &SignedQuaiTransaction,
    observation: AccountObservationPolicy,
    policy: ReplacementPolicy,
) -> Result<AccountReplacementQuote, AccountPreflightError> {
    use AccountPreflightError as E;
    let sender = parent.from();
    let mut transaction = parent.transaction().clone();
    if scope.genesis == Hash32::ZERO
        || scope.chain_id == U256::ZERO
        || sender.zone() != scope.zone
        || transaction.chain_id != scope.chain_id
        || !(1..=1000).contains(&policy.minimum_price_bump_percent)
        || policy.fees.gas_margin_bps > 10_000
    {
        return Err(E::Invalid);
    }
    if transaction.gas_limit == 0 || transaction.gas_limit > policy.fees.max_gas {
        return Err(E::FeeLimit);
    }
    check_network(provider, scope).await?;
    let confirmed = provider
        .latest_header(scope.zone)
        .await?
        .ok_or(E::ObservationChanged)?;
    if provider
        .transaction_count(sender, BlockTag::Number(U256::from(confirmed.number)))
        .await?
        > transaction.nonce
    {
        return Err(E::Invalid);
    }
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
    let old = transaction.gas_price;
    let numerator = old
        .checked_mul(U256::from(
            100 + u64::from(policy.minimum_price_bump_percent),
        ))
        .ok_or(E::FeeLimit)?;
    let minimum = (numerator / U256::from(100))
        .checked_add(U256::from(u8::from(
            numerator % U256::from(100) != U256::ZERO,
        )))
        .ok_or(E::FeeLimit)?;
    transaction.gas_price = provider
        .gas_price(scope.zone)
        .await?
        .max(minimum)
        .max(old.checked_add(U256::from(1)).ok_or(E::FeeLimit)?);
    let fee = transaction
        .gas_price
        .checked_mul(U256::from(transaction.gas_limit))
        .ok_or(E::FeeLimit)?;
    if transaction.gas_price > policy.fees.max_gas_price || fee > policy.fees.max_total_fee {
        return Err(E::FeeLimit);
    }
    let estimate = if transaction.to.is_some_and(|a| a.ledger() == Ledger::Qi) {
        let typed = QuaiToQiTransaction::new(transaction.clone()).map_err(|_| E::Invalid)?;
        provider
            .estimate_quai_conversion_gas(sender, &typed, block)
            .await?
    } else {
        let request = CallRequest {
            from: sender,
            to: transaction
                .to
                .map(QuaiAddress::try_from)
                .transpose()
                .map_err(|_| E::Invalid)?,
            gas: Some(transaction.gas_limit),
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
        provider.estimate_gas(&request, block).await?
    };
    let gas =
        (u128::from(estimate) * (10_000 + u128::from(policy.fees.gas_margin_bps))).div_ceil(10_000);
    if gas == 0 || gas > u128::from(transaction.gas_limit) {
        return Err(E::FeeLimit);
    }
    if fee.checked_add(transaction.value).ok_or(E::FeeLimit)?
        > provider.balance(sender, block).await?
    {
        return Err(E::InsufficientBalance);
    }
    if let Some(before) = header
        && provider
            .latest_header(scope.zone)
            .await?
            .is_none_or(|after| before.hash != after.hash || before.number != after.number)
    {
        return Err(E::ObservationChanged);
    }
    if provider
        .header_at(scope.zone, confirmed.number)
        .await?
        .is_none_or(|h| h.hash != confirmed.hash)
    {
        return Err(E::ObservationChanged);
    }
    check_network(provider, scope).await?;
    let digest = transaction.signing_digest().map_err(|_| E::Invalid)?;
    Ok(AccountReplacementQuote {
        transaction,
        parent: parent.hash().map_err(|_| E::Invalid)?,
        fee,
        digest,
    })
}
