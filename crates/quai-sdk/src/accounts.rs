//! Native account transaction preparation with durable nonce and signed-byte custody.
//!
//! Preparation, authorization and submission are separate steps. A caller must
//! review the immutable prepared payload before calling `sign`. Network reads
//! are observations, not reservations of account balance or guarantees of mining.
mod access;
pub use access::AccountAccessListPolicy;
use quai_consensus::{QuaiTransaction, SignedQuaiTransaction};
use quai_primitives::{Hash32, QuaiAddress};
use quai_provider::{
    AccessListItem, BlockTag, BroadcastError, BroadcastResult, CallRequest, Provider, ProviderError,
};
// Only `prepare_deployment` builds an init-code call request.
#[cfg(feature = "abi")]
use quai_provider::RpcData;
use quai_rpc::{Transport, U256};
use quai_signer::{Signer, SignerError};
use quai_wallet::storage::{
    ReservationId, ReservationState, SqliteStore, StorageError, StoreInstance,
};
use thiserror::Error;

pub use crate::account_preflight::{AccountIntent, AccountObservationPolicy, FeePolicy};

/// Errors never automatically release or replace signed reservations.
#[derive(Debug, Error)]
pub enum AccountError {
    /// A network observation or simulation failed.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// Atomic durable state operation failed.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// Signer refused the request.
    #[error(transparent)]
    Signer(#[from] SignerError),
    /// Submission failed; inspect the contained ambiguity/expected transaction ID.
    #[error(transparent)]
    Broadcast(#[from] BroadcastError),
    /// Signer, database handle, node, or prepared operation belongs to another scope.
    #[error("wallet storage handle, chain, genesis, zone or signer identity mismatch")]
    IdentityMismatch,
    /// Unsupported intent, malformed payload or inconsistent reservation.
    #[error("invalid account operation")]
    InvalidOperation,
    /// Checked arithmetic overflow or a caller fee limit was exceeded.
    #[error("account transaction exceeds the explicit fee policy")]
    FeeLimit,
    /// Observed balance does not cover value and maximum fee.
    #[error("observed balance does not cover value and maximum fee")]
    InsufficientBalance,
    /// The sampled confirmed head changed during preparation.
    #[error("account observation head changed during preparation")]
    ObservationChanged,
    /// The signer returned bytes differing from the exact reviewed payload.
    #[error("signer changed the authorized transaction")]
    PayloadMismatch,
    /// No validated signed bytes were saved for this operation.
    #[error("no recoverable signed transaction for reservation")]
    MissingSignedPayload,
    /// Contract creation parameters or address grinding failed.
    #[cfg(feature = "abi")]
    #[error(transparent)]
    Contract(#[from] crate::contracts::ContractError),
}

/// Exact frozen payload associated with a durable unsigned nonce reservation.
///
/// Bound to the exact opened storage handle: moving to another or reopened handle
/// is rejected even if public metadata/reservations match. Restart broadcast uses
/// durable signed bytes separately. Dropping this value leaves the reservation intact. Explicit unsigned release
/// never rewinds the monotonic nonce cursor; an abandoned nonce may need a
/// separately authorized gap-filling transaction before later nonces can mine.
#[derive(Debug)]
pub struct PreparedAccountTransaction {
    instance: StoreInstance,
    id: ReservationId,
    sender: QuaiAddress,
    genesis: Hash32,
    transaction: QuaiTransaction,
    maximum_fee: U256,
    digest: Hash32,
}
impl PreparedAccountTransaction {
    /// Predicted CREATE address for a prepared deployment; transfers return None.
    pub fn created_address(&self) -> Option<QuaiAddress> {
        if self.transaction.to.is_some() {
            return None;
        }
        QuaiAddress::try_from(quai_primitives::contract_address(
            self.sender.address(),
            self.transaction.nonce,
            &self.transaction.data,
        ))
        .ok()
    }
    /// Durable operation identifier for restart/reconciliation.
    pub fn reservation_id(&self) -> ReservationId {
        self.id
    }
    /// Exact unsigned payload to display and authorize; no mutation is exposed.
    pub fn transaction(&self) -> &QuaiTransaction {
        &self.transaction
    }
    /// Maximum gas debit implied by the exact signed gas parameters.
    pub fn maximum_fee(&self) -> U256 {
        self.maximum_fee
    }
    /// Exact signing digest, useful for an external review display.
    pub fn signing_digest(&self) -> Hash32 {
        self.digest
    }
}

/// A scoped native account workflow using an application-owned provider and store.
pub struct AccountSession<'a, T, S> {
    provider: &'a Provider<T>,
    signer: &'a S,
    store: &'a mut SqliteStore,
    observation_policy: AccountObservationPolicy,
    access_list_policy: AccountAccessListPolicy,
}
impl<'a, T: Transport, S: Signer> AccountSession<'a, T, S> {
    /// Prepare a same-zone native Quai-to-Qi conversion with exact slippage.
    /// Budget origin costs and denomination fragmentation below the current quote
    /// before applying the caller's margin/caps. Later rates remain uncertain.
    /// Estimation failure after reservation retains an unsigned nonce for recovery.
    /// Existing `sign` and `broadcast` persist and submit the frozen conversion.
    pub async fn prepare_conversion(
        &mut self,
        id: ReservationId,
        destination: quai_primitives::QiAddress,
        value: U256,
        slippage: quai_consensus::ConversionSlippage,
        policy: FeePolicy,
    ) -> Result<PreparedAccountTransaction, AccountError> {
        let sender = QuaiAddress::try_from(self.signer.address())
            .map_err(|_| AccountError::IdentityMismatch)?;
        if destination.zone() != sender.zone()
            || policy.max_gas == 0
            || policy.gas_margin_bps > 10_000
        {
            return Err(AccountError::InvalidOperation);
        }
        let mut transaction = QuaiTransaction {
            chain_id: self.store.scope().chain_id,
            nonce: 0,
            to: Some(destination.address()),
            value,
            gas_limit: policy.max_gas,
            gas_price: U256::ZERO,
            data: slippage.to_be_bytes().to_vec(),
            access_list: vec![],
        };
        quai_consensus::QuaiToQiTransaction::new(transaction.clone())
            .map_err(|_| AccountError::InvalidOperation)?;
        self.verify_network().await?;
        let observation = self.observation().await?;
        let pending = self
            .provider
            .transaction_count(sender, observation.0)
            .await?;
        transaction.gas_price = self.provider.gas_price(sender.zone()).await?;
        if transaction.gas_price > policy.max_gas_price {
            return Err(AccountError::FeeLimit);
        }
        // Estimate the pending nonce first; repeat against the actual reserved
        // nonce when concurrent operations advanced the durable cursor.
        transaction.nonce = pending;
        let (mut gas, mut fee) = self
            .quote_conversion(sender, &transaction, policy, observation.0)
            .await?;
        self.verify_observation(observation).await?;
        transaction.nonce = self.store.reserve_nonce(id, sender, pending)?;
        if transaction.nonce != pending {
            (gas, fee) = self
                .quote_conversion(sender, &transaction, policy, observation.0)
                .await?;
        }
        transaction.gas_limit = gas;
        self.verify_observation(observation).await?;
        let digest = transaction
            .signing_digest()
            .map_err(|_| AccountError::InvalidOperation)?;
        Ok(PreparedAccountTransaction {
            instance: self.store.instance(),
            id,
            sender,
            genesis: self.store.scope().genesis,
            transaction,
            maximum_fee: fee,
            digest,
        })
    }
    async fn quote_conversion(
        &self,
        sender: QuaiAddress,
        transaction: &QuaiTransaction,
        policy: FeePolicy,
        block: BlockTag,
    ) -> Result<(u64, U256), AccountError> {
        let typed = quai_consensus::QuaiToQiTransaction::new(transaction.clone())
            .map_err(|_| AccountError::InvalidOperation)?;
        let estimate = self
            .provider
            .estimate_quai_conversion_gas_budget(sender, &typed, block)
            .await?;
        let gas =
            (u128::from(estimate) * (10_000 + u128::from(policy.gas_margin_bps))).div_ceil(10_000);
        if gas == 0 || gas > u128::from(policy.max_gas) {
            return Err(AccountError::FeeLimit);
        }
        let gas = gas as u64;
        let fee = transaction
            .gas_price
            .checked_mul(U256::from(gas))
            .ok_or(AccountError::FeeLimit)?;
        if fee > policy.max_total_fee {
            return Err(AccountError::FeeLimit);
        }
        if fee
            .checked_add(transaction.value)
            .ok_or(AccountError::FeeLimit)?
            > self.provider.balance(sender, block).await?
        {
            return Err(AccountError::InsufficientBalance);
        }
        Ok((gas, fee))
    }
    /// Bind handles without network access. The sender must already be registered
    /// in storage through validated public-key metadata.
    pub fn new(
        provider: &'a Provider<T>,
        signer: &'a S,
        store: &'a mut SqliteStore,
    ) -> Result<Self, AccountError> {
        let sender =
            QuaiAddress::try_from(signer.address()).map_err(|_| AccountError::IdentityMismatch)?;
        let scope = store.scope();
        if signer.chain_id() != scope.chain_id || sender.zone() != scope.zone {
            return Err(AccountError::IdentityMismatch);
        }
        Ok(Self {
            provider,
            signer,
            store,
            observation_policy: AccountObservationPolicy::Pending,
            access_list_policy: AccountAccessListPolicy::Preserve,
        })
    }

    /// Select an explicit preflight state source, including for conversions and deployments.
    pub fn with_observation_policy(mut self, policy: AccountObservationPolicy) -> Self {
        self.observation_policy = policy;
        self
    }
    async fn observation(&self) -> Result<(BlockTag, Option<Hash32>), AccountError> {
        match self.observation_policy {
            AccountObservationPolicy::Pending => Ok((BlockTag::Pending, None)),
            AccountObservationPolicy::PinnedLatest => {
                let header = self
                    .provider
                    .latest_header(self.store.scope().zone)
                    .await?
                    .ok_or(AccountError::ObservationChanged)?;
                Ok((
                    BlockTag::Number(U256::from(header.number)),
                    Some(header.hash),
                ))
            }
        }
    }
    async fn verify_observation(
        &self,
        observation: (BlockTag, Option<Hash32>),
    ) -> Result<(), AccountError> {
        if let Some(hash) = observation.1 {
            let latest = self
                .provider
                .latest_header(self.store.scope().zone)
                .await?
                .ok_or(AccountError::ObservationChanged)?;
            if latest.hash != hash || observation.0 != BlockTag::Number(U256::from(latest.number)) {
                return Err(AccountError::ObservationChanged);
            }
        }
        Ok(())
    }

    async fn verify_network(&self) -> Result<(), AccountError> {
        let scope = self.store.scope();
        if self.provider.chain_id(scope.zone.into()).await? != scope.chain_id
            || self.provider.genesis_hash(scope.zone).await? != scope.genesis
        {
            return Err(AccountError::IdentityMismatch);
        }
        Ok(())
    }

    /// Reserve a nonce before offline CREATE address grinding. The reservation is
    /// durable and remains held if grinding, simulation or fee approval later fails.
    /// An explicit unsigned release does not rewind the cursor; preserve the ID for recovery.
    #[cfg(feature = "abi")]
    pub async fn reserve_deployment_nonce(
        &mut self,
        id: ReservationId,
    ) -> Result<u64, AccountError> {
        let sender = QuaiAddress::try_from(self.signer.address())
            .map_err(|_| AccountError::IdentityMismatch)?;
        self.verify_network().await?;
        let observation = self.observation().await?;
        let pending = self
            .provider
            .transaction_count(sender, observation.0)
            .await?;
        self.verify_observation(observation).await?;
        Ok(self.store.reserve_nonce(id, sender, pending)?)
    }

    /// Estimate and freeze the exact init code, predicted-address access list and
    /// already-reserved nonce from the offline deployment builder. Signing and
    /// broadcasting use the ordinary durable methods after explicit payload review.
    /// Failure leaves the existing reservation intact and never selects another nonce.
    #[cfg(feature = "abi")]
    pub async fn prepare_deployment(
        &mut self,
        id: ReservationId,
        deployment: crate::contracts::PreparedDeployment,
        policy: FeePolicy,
    ) -> Result<PreparedAccountTransaction, AccountError> {
        if policy.max_gas == 0 || policy.gas_margin_bps > 10_000 {
            return Err(AccountError::FeeLimit);
        }
        let sender = QuaiAddress::try_from(self.signer.address())
            .map_err(|_| AccountError::IdentityMismatch)?;
        let predicted = deployment.address();
        let mut transaction = deployment.into_transaction(policy.max_gas, U256::ZERO)?;
        if transaction.chain_id != self.store.scope().chain_id
            || self.store.reserved_nonce(id)? != Some((sender, transaction.nonce))
            || self
                .store
                .reservation(id)?
                .is_none_or(|r| r.state != ReservationState::Reserved)
            || quai_primitives::contract_address(
                sender.address(),
                transaction.nonce,
                &transaction.data,
            ) != predicted.address()
        {
            return Err(AccountError::IdentityMismatch);
        }
        transaction
            .unsigned_bytes()
            .map_err(|_| AccountError::InvalidOperation)?;
        self.verify_network().await?;
        let observation = self.observation().await?;
        if self
            .provider
            .transaction_count(sender, observation.0)
            .await?
            > transaction.nonce
        {
            return Err(AccountError::InvalidOperation);
        }
        let gas_price = self.provider.gas_price(sender.zone()).await?;
        if gas_price > policy.max_gas_price {
            return Err(AccountError::FeeLimit);
        }
        let mut request = CallRequest {
            from: sender,
            to: None,
            gas: Some(policy.max_gas),
            gas_price: Some(gas_price),
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
        self.populate_access(&mut request, &mut transaction, observation.0)
            .await?;
        let estimate = self.provider.estimate_gas(&request, observation.0).await?;
        let gas =
            (u128::from(estimate) * (10_000 + u128::from(policy.gas_margin_bps))).div_ceil(10_000);
        if gas == 0 || gas > u128::from(policy.max_gas) {
            return Err(AccountError::FeeLimit);
        }
        let gas = u64::try_from(gas).map_err(|_| AccountError::FeeLimit)?;
        let fee = gas_price
            .checked_mul(U256::from(gas))
            .ok_or(AccountError::FeeLimit)?;
        if fee > policy.max_total_fee {
            return Err(AccountError::FeeLimit);
        }
        let debit = fee
            .checked_add(transaction.value)
            .ok_or(AccountError::FeeLimit)?;
        if debit > self.provider.balance(sender, observation.0).await? {
            return Err(AccountError::InsufficientBalance);
        }
        transaction.gas_price = gas_price;
        transaction.gas_limit = gas;
        self.verify_observation(observation).await?;
        let digest = transaction
            .signing_digest()
            .map_err(|_| AccountError::InvalidOperation)?;
        Ok(PreparedAccountTransaction {
            instance: self.store.instance(),
            id,
            sender,
            genesis: self.store.scope().genesis,
            transaction,
            maximum_fee: fee,
            digest,
        })
    }

    /// Estimate and freeze an ordinary account transaction, then atomically reserve
    /// its nonce. Preflight rejection occurs before allocation. If the durable nonce
    /// differs from the pending observation, re-estimate that exact nonce and recheck
    /// fees/balance. Failure or cancellation then retains the unsigned reservation.
    /// Estimation is advisory; signing never changes the frozen payload.
    pub async fn prepare(
        &mut self,
        id: ReservationId,
        intent: AccountIntent,
        policy: FeePolicy,
    ) -> Result<PreparedAccountTransaction, AccountError> {
        self.prepare_account(id, intent, policy, false, false).await
    }
    /// Prepare an existing unsigned nonce after restart or failed estimation.
    /// Explicitly reopen a released nonce in storage first. A zero-value self
    /// transfer can fill a gap; the resulting payload still requires review/sign.
    pub async fn prepare_reserved(
        &mut self,
        id: ReservationId,
        intent: AccountIntent,
        policy: FeePolicy,
    ) -> Result<PreparedAccountTransaction, AccountError> {
        self.prepare_account(id, intent, policy, true, false).await
    }
    /// Prepare a native Quai transfer or call to another zone. Simulation and
    /// nonce custody use the sender zone; destination execution is asynchronous
    /// and must be observed separately using the emitted ETX correlation.
    pub async fn prepare_cross_zone(
        &mut self,
        id: ReservationId,
        intent: AccountIntent,
        policy: FeePolicy,
    ) -> Result<PreparedAccountTransaction, AccountError> {
        self.prepare_account(id, intent, policy, false, true).await
    }
    /// Resume an explicitly reserved unsigned cross-zone transaction after restart.
    pub async fn prepare_cross_zone_reserved(
        &mut self,
        id: ReservationId,
        intent: AccountIntent,
        policy: FeePolicy,
    ) -> Result<PreparedAccountTransaction, AccountError> {
        self.prepare_account(id, intent, policy, true, true).await
    }
    async fn prepare_account(
        &mut self,
        id: ReservationId,
        intent: AccountIntent,
        policy: FeePolicy,
        reuse: bool,
        cross_zone: bool,
    ) -> Result<PreparedAccountTransaction, AccountError> {
        let sender = QuaiAddress::try_from(self.signer.address())
            .map_err(|_| AccountError::IdentityMismatch)?;
        if (intent.to.zone() != sender.zone()) != cross_zone {
            return Err(AccountError::IdentityMismatch);
        }
        if policy.max_gas == 0 || policy.gas_margin_bps > 10_000 {
            return Err(AccountError::FeeLimit);
        }
        // Validate shape before cloning the call/access payload or making requests.
        let mut transaction = QuaiTransaction {
            chain_id: self.store.scope().chain_id,
            nonce: u64::MAX,
            to: Some(intent.to.address()),
            value: intent.value,
            gas_limit: policy.max_gas,
            gas_price: U256::MAX,
            data: intent.data.bytes().to_vec(),
            access_list: intent.access_list,
        };
        transaction
            .unsigned_bytes()
            .map_err(|_| AccountError::InvalidOperation)?;
        self.verify_network().await?;
        let observation = self.observation().await?;
        let pending_nonce = self
            .provider
            .transaction_count(sender, observation.0)
            .await?;
        let gas_price = self.provider.gas_price(sender.zone()).await?;
        if gas_price > policy.max_gas_price {
            return Err(AccountError::FeeLimit);
        }
        let reserved_nonce = if reuse {
            if self
                .store
                .reservation(id)?
                .is_none_or(|record| record.state != ReservationState::Reserved)
            {
                return Err(AccountError::InvalidOperation);
            }
            let (owner, nonce) = self
                .store
                .reserved_nonce(id)?
                .ok_or(AccountError::InvalidOperation)?;
            if owner != sender || nonce < pending_nonce {
                return Err(AccountError::InvalidOperation);
            }
            Some(nonce)
        } else {
            None
        };
        let mut request = CallRequest {
            from: sender,
            to: Some(intent.to),
            gas: Some(policy.max_gas),
            gas_price: Some(gas_price),
            value: Some(intent.value),
            nonce: Some(reserved_nonce.unwrap_or(pending_nonce)),
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
        let original_access = request.access_list.clone();
        self.populate_access(&mut request, &mut transaction, observation.0)
            .await?;
        let (mut gas, mut fee) = self.quote_fee(&request, policy, observation.0).await?;
        self.verify_observation(observation).await?;
        transaction.nonce = match reserved_nonce {
            Some(nonce) => nonce,
            None => self.store.reserve_nonce(id, sender, pending_nonce)?,
        };
        if !reuse && transaction.nonce != pending_nonce {
            // Another reservation or a restart may advance the durable cursor beyond
            // the node's pending observation. Never authorize the unestimated nonce.
            let mut actual = request;
            actual.nonce = Some(transaction.nonce);
            actual.access_list = original_access;
            self.populate_access(&mut actual, &mut transaction, observation.0)
                .await?;
            (gas, fee) = self.quote_fee(&actual, policy, observation.0).await?;
        }
        transaction.gas_price = gas_price;
        transaction.gas_limit = gas;
        self.verify_observation(observation).await?;
        let digest = transaction
            .signing_digest()
            .map_err(|_| AccountError::InvalidOperation)?;
        Ok(PreparedAccountTransaction {
            instance: self.store.instance(),
            id,
            sender,
            genesis: self.store.scope().genesis,
            transaction,
            maximum_fee: fee,
            digest,
        })
    }

    async fn quote_fee(
        &self,
        request: &CallRequest,
        policy: FeePolicy,
        block: BlockTag,
    ) -> Result<(u64, U256), AccountError> {
        let estimate = self.provider.estimate_gas(request, block).await?;
        let gas =
            (u128::from(estimate) * (10_000 + u128::from(policy.gas_margin_bps))).div_ceil(10_000);
        if gas == 0 || gas > u128::from(policy.max_gas) {
            return Err(AccountError::FeeLimit);
        }
        let gas = u64::try_from(gas).map_err(|_| AccountError::FeeLimit)?;
        let fee = request
            .gas_price
            .ok_or(AccountError::InvalidOperation)?
            .checked_mul(U256::from(gas))
            .ok_or(AccountError::FeeLimit)?;
        if fee > policy.max_total_fee {
            return Err(AccountError::FeeLimit);
        }
        let maximum_debit = fee
            .checked_add(request.value.unwrap_or(U256::ZERO))
            .ok_or(AccountError::FeeLimit)?;
        if maximum_debit > self.provider.balance(request.from, block).await? {
            return Err(AccountError::InsufficientBalance);
        }
        Ok((gas, fee))
    }

    /// Authorize the exact prepared payload and save its verified signed bytes
    /// before exposing the signature to the caller. No network call occurs.
    pub fn sign(
        &mut self,
        prepared: &PreparedAccountTransaction,
    ) -> Result<SignedQuaiTransaction, AccountError> {
        self.validate_prepared(prepared)?;
        let signed = self.signer.sign_quai(&prepared.transaction)?;
        self.commit_external_signature(prepared, &signed)?;
        Ok(signed)
    }

    /// Persist a remotely or externally signed exact prepared payload. Use a
    /// `WatchOnlySigner` for preparation when the key lives in a remote wallet.
    /// The original handle, reservation, sender and every unsigned field must
    /// still match. No RPC occurs; errors retain the reservation. Retain returned
    /// remote bytes until this durable commit succeeds, including after cancellation.
    pub fn commit_external_signature(
        &mut self,
        prepared: &PreparedAccountTransaction,
        signed: &SignedQuaiTransaction,
    ) -> Result<(), AccountError> {
        self.validate_prepared(prepared)?;
        if signed.transaction() != &prepared.transaction || signed.from() != prepared.sender {
            return Err(AccountError::PayloadMismatch);
        }
        self.store.commit_signed_quai(prepared.id, signed)?;
        Ok(())
    }

    fn validate_prepared(&self, prepared: &PreparedAccountTransaction) -> Result<(), AccountError> {
        if prepared.instance != self.store.instance()
            || prepared.sender.address() != self.signer.address()
            || prepared.genesis != self.store.scope().genesis
            || prepared.transaction.chain_id != self.store.scope().chain_id
        {
            return Err(AccountError::IdentityMismatch);
        }
        let reserved = self
            .store
            .reservation(prepared.id)?
            .ok_or(AccountError::InvalidOperation)?;
        if reserved.state != ReservationState::Reserved
            || self.store.reserved_nonce(prepared.id)?
                != Some((prepared.sender, prepared.transaction.nonce))
        {
            return Err(AccountError::InvalidOperation);
        }
        Ok(())
    }

    /// Submit the exact durable signed bytes once. An explicit repeat submits the
    /// same transaction ID; this method never makes a new payment or retries.
    /// Submitted state is saved before network I/O, retaining claims on ambiguity.
    pub async fn broadcast(&mut self, id: ReservationId) -> Result<BroadcastResult, AccountError> {
        let reservation = self
            .store
            .reservation(id)?
            .ok_or(AccountError::MissingSignedPayload)?;
        if !matches!(
            reservation.state,
            ReservationState::Signed | ReservationState::Submitted
        ) {
            return Err(AccountError::InvalidOperation);
        }
        let bytes = self
            .store
            .signed_payload(id)?
            .ok_or(AccountError::MissingSignedPayload)?;
        let signed =
            SignedQuaiTransaction::decode(&bytes).map_err(|_| AccountError::InvalidOperation)?;
        if signed.from().address() != self.signer.address() {
            return Err(AccountError::IdentityMismatch);
        }
        self.verify_network().await?;
        if reservation.state == ReservationState::Signed {
            self.store.mark_submitted(id)?;
        }
        Ok(self.provider.broadcast(&signed).await?)
    }
}

mod replacements;
pub use replacements::{
    AccountCandidateStatus, AccountFamilyObservation, AccountNonceObservation, AccountNonceOutcome,
    PreparedAccountReplacement, ReplacementPolicy,
};
