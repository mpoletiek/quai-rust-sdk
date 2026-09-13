//! Native ordinary Qi payments with exact fee estimation and durable signed custody.
//!
//! Allocate change first, refresh qualified discovery, prepare, review, sign, then
//! explicitly broadcast. This module does not turn latest-only RPC observations
//! into a pinned UTXO snapshot. Explicit local key resolvers support BIP44,
//! imported and registered payment receive keys; conversions use separate types.
use quai_consensus::{QiInput, QiOutput, QiTransaction, SignedQiTransaction, TransactionError};
use quai_crypto::{PublicKey, SecretKey};
use quai_primitives::{Address, Hash32, QiAddress};
use quai_provider::{BroadcastError, BroadcastResult, Provider, ProviderError};
use quai_rpc::{Transport, U256};
use quai_wallet::discovery::Checkpoint;
use quai_wallet::qi_keys::QiKeyResolver;
use quai_wallet::storage::{
    NetworkScope, PublicAddress, ReservationId, ReservationState, SqliteStore, StorageError,
    StoreInstance,
};
use quai_wallet::{AccountPublic, CoinType, HdWallet, WalletError};
use quai_wallet::{SelectionError, SelectionRequest, select_fewest};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;
mod special;
mod sweep;
pub use special::{PreparedQiOperation, QiSpecialIntent, QiSpecialTransaction};

pub use crate::qi_preflight::{QiIntent, QiPolicy};

/// A one-use, scoped pool of fresh, durably burned BIP44 change addresses.
///
/// There is deliberately no constructor from addresses, clone or deserialization.
/// This pool belongs to its exact opened storage handle; another/reopened handle
/// rejects it even when all public state is identical.
/// Dropped, failed and unused allocations remain burned. Allocate this before
/// refreshing discovery: adding metadata invalidates the old wallet snapshot.
#[derive(Debug)]
pub struct QiChangePool {
    instance: StoreInstance,
    scope: NetworkScope,
    account: AccountPublic,
    burned_through: Option<u32>,
    addresses: Vec<PublicAddress>,
}
impl QiChangePool {
    /// Allocate a bounded number of fresh change addresses before exposing them.
    /// `count <= 1024`, attempts per address is 1..=100,000 and their product is
    /// at most 100,000. Failure never rewinds any already burned range.
    pub fn allocate(
        store: &mut SqliteStore,
        account: &AccountPublic,
        count: usize,
        attempts_per_address: u32,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<Self, QiError> {
        if account.coin_type() != CoinType::Qi
            || count > 1024
            || !(1..=100_000).contains(&attempts_per_address)
            || count.saturating_mul(attempts_per_address as usize) > 100_000
        {
            return Err(QiError::InvalidPolicy);
        }
        let mut addresses = Vec::with_capacity(count);
        let mut burned_through = None;
        for _ in 0..count {
            if cancelled() {
                return Err(QiError::Cancelled);
            }
            let allocated =
                store.allocate_address(account, true, attempts_per_address, &mut cancelled)?;
            burned_through = Some(allocated.burned.end);
            addresses.push(allocated.address);
        }
        Ok(Self {
            instance: store.instance(),
            scope: store.scope(),
            account: account.clone(),
            burned_through,
            addresses,
        })
    }
    /// Persisted metadata to include in the subsequent qualified discovery scan.
    pub fn addresses(&self) -> &[PublicAddress] {
        &self.addresses
    }
}

/// Planning failures do not release signed claims or retry submission.
#[derive(Debug, Error)]
pub enum QiError {
    /// Qi message signature generation failed without exposing backend diagnostics.
    #[error("Qi message signing failed")]
    MessageSigning,
    /// An optional caller-owned address-use query failed; no remote text is kept.
    #[error("Qi address-use check failed")]
    UseCheckFailed,
    /// A checked provider observation failed.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// A durable wallet operation failed.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// Selection, amount arithmetic or fee convergence failed.
    #[error(transparent)]
    Selection(#[from] SelectionError),
    /// Deriving a local BIP44 key failed.
    #[error(transparent)]
    Wallet(#[from] WalletError),
    /// Consensus shape or signature verification failed.
    #[error(transparent)]
    Transaction(#[from] TransactionError),
    /// Submission failed; the embedded error describes acceptance ambiguity.
    #[error(transparent)]
    Broadcast(#[from] BroadcastError),
    /// Wallet ownership, chain, genesis, zone or reservation does not match.
    #[error("Qi wallet, network or reservation identity mismatch")]
    IdentityMismatch,
    /// An explicit resource or fee policy is invalid.
    #[error("invalid Qi operation policy")]
    InvalidPolicy,
    /// Change allocation has not been followed by a qualified discovery snapshot.
    #[error("Qi operation requires a refreshed wallet snapshot")]
    MissingSnapshot,
    /// The checkpoint is no longer canonical or exceeds the explicit age budget.
    #[error("Qi snapshot is stale or noncanonical")]
    StaleSnapshot,
    /// Exact denomination decomposition needs more recipient addresses.
    #[error("insufficient distinct Qi recipient addresses")]
    InsufficientDestinations,
    /// Exact denomination decomposition needs more already allocated change addresses.
    #[error("insufficient preallocated Qi change addresses")]
    InsufficientChange,
    /// Allocation was explicitly cancelled; burned ranges remain consumed.
    #[error("Qi change allocation cancelled")]
    Cancelled,
    /// No valid signed Qi payload is stored for this reservation.
    #[error("no recoverable signed Qi transaction")]
    MissingSignedPayload,
}

/// Immutable reviewed transaction associated with a durable unsigned input claim.
/// Bound to the exact opened storage handle; restart broadcast uses persisted signed
/// bytes instead. An unsigned object cannot move to another/reopened handle.
#[derive(Debug)]
pub struct PreparedQiTransaction {
    instance: StoreInstance,
    id: ReservationId,
    scope: NetworkScope,
    transaction: QiTransaction,
    fee: U256,
    recipient_outputs: usize,
    digest: Hash32,
}
impl PreparedQiTransaction {
    /// Durable restart/reconciliation identifier.
    pub fn reservation_id(&self) -> ReservationId {
        self.id
    }
    /// Exact transaction to review; signing cannot mutate it.
    pub fn transaction(&self) -> &QiTransaction {
        &self.transaction
    }
    /// Source-reported input value minus output value in Qits, possibly above the last quote.
    /// A source lying about input denominations can misstate the on-chain debit.
    pub fn fee(&self) -> U256 {
        self.fee
    }
    /// Number of recipient outputs at the beginning of the ordered output list.
    pub fn recipient_outputs(&self) -> usize {
        self.recipient_outputs
    }
    /// Exact prehash authorized by signing.
    pub fn signing_digest(&self) -> Hash32 {
        self.digest
    }
}

/// Local all-keys-owned BIP44 Qi orchestration using a durable native wallet store.
///
/// Chain and genesis are caller-selected, not hardcoded. Calling `broadcast` is
/// explicit authorization to submit previously signed bytes to that network.
pub struct QiSession<'a, T> {
    provider: &'a Provider<T>,
    wallet: &'a dyn QiKeyResolver,
    store: &'a mut SqliteStore,
}
impl<'a, T: Transport> QiSession<'a, T> {
    /// Bind a local Qi HD wallet. Ownership of every selected input/change key is
    /// checked before reservation. Use `with_keys` for mixed local origins.
    pub fn new(
        provider: &'a Provider<T>,
        wallet: &'a HdWallet,
        store: &'a mut SqliteStore,
    ) -> Result<Self, QiError> {
        if wallet.coin_type() != CoinType::Qi {
            return Err(QiError::IdentityMismatch);
        }
        Ok(Self {
            provider,
            wallet,
            store,
        })
    }
    /// Bind an explicit local resolver, including `QiKeyring` for imported and
    /// BIP47 receive keys. Every resolved public key is independently checked.
    pub fn with_keys(
        provider: &'a Provider<T>,
        keys: &'a dyn QiKeyResolver,
        store: &'a mut SqliteStore,
    ) -> Self {
        Self {
            provider,
            wallet: keys,
            store,
        }
    }
    async fn verify_network(&self) -> Result<(), QiError> {
        let scope = self.store.scope();
        if self.provider.chain_id(scope.zone.into()).await? != scope.chain_id
            || self.provider.genesis_hash(scope.zone).await? != scope.genesis
        {
            return Err(QiError::IdentityMismatch);
        }
        Ok(())
    }
    fn key_for(&self, metadata: &PublicAddress) -> Result<SecretKey, QiError> {
        let key = self
            .wallet
            .resolve(metadata)
            .map_err(|_| QiError::IdentityMismatch)?;
        if key.public_key().to_compressed() != *metadata.public_key()
            || key.public_key().address() != metadata.address()
        {
            return Err(QiError::IdentityMismatch);
        }
        Ok(key)
    }
    async fn candidate_height(
        &mut self,
        generation: u64,
        checkpoint: Checkpoint,
        max_age: u64,
    ) -> Result<U256, QiError> {
        let zone = self.store.scope().zone;
        let canonical = self
            .provider
            .header_at(
                zone,
                u64::try_from(checkpoint.height).map_err(|_| QiError::StaleSnapshot)?,
            )
            .await?
            .map(|header| Checkpoint {
                hash: header.hash,
                height: U256::from(header.number),
            });
        if canonical != Some(checkpoint) {
            self.store.reconcile_checkpoint(generation, canonical)?;
            return Err(QiError::StaleSnapshot);
        }
        let tip = self
            .provider
            .latest_header(zone)
            .await?
            .ok_or(QiError::StaleSnapshot)?;
        let height = U256::from(tip.number);
        let age = height
            .checked_sub(checkpoint.height)
            .ok_or(QiError::StaleSnapshot)?;
        if age > U256::from(max_age) {
            return Err(QiError::StaleSnapshot);
        }
        height
            .checked_add(U256::from(1))
            .ok_or(QiError::StaleSnapshot)
    }
    /// Estimate the exact payload in a bounded monotonic fee loop, freeze it and
    /// atomically claim final inputs. Consume the change pool even on error.
    /// All network reads precede claims; no signing or submission occurs here.
    /// Fees/state can change later; estimates do not guarantee node acceptance.
    pub async fn prepare(
        &mut self,
        id: ReservationId,
        intent: QiIntent,
        policy: QiPolicy,
        change: QiChangePool,
    ) -> Result<PreparedQiTransaction, QiError> {
        self.prepare_transfer(id, intent, policy, change, false)
            .await
    }
    /// Prepare a Qi transfer to another single destination zone. Origin fees
    /// include the exact cross-zone output shape; destination ETX execution and
    /// maturity must be observed separately. No destination success is implied.
    pub async fn prepare_cross_zone(
        &mut self,
        id: ReservationId,
        intent: QiIntent,
        policy: QiPolicy,
        change: QiChangePool,
    ) -> Result<PreparedQiTransaction, QiError> {
        if intent.destinations.first().is_none_or(|first| {
            first.zone() == self.store.scope().zone
                || intent
                    .destinations
                    .iter()
                    .any(|address| address.zone() != first.zone())
        }) {
            return Err(QiError::IdentityMismatch);
        }
        self.prepare_transfer(id, intent, policy, change, true)
            .await
    }
    async fn prepare_transfer(
        &mut self,
        id: ReservationId,
        intent: QiIntent,
        policy: QiPolicy,
        change: QiChangePool,
        cross_zone: bool,
    ) -> Result<PreparedQiTransaction, QiError> {
        if !(1..=1024).contains(&policy.max_inputs)
            || !(1..=1024).contains(&policy.max_outputs)
            || !(1..=32).contains(&policy.max_fee_rounds)
            || policy.initial_fee > policy.max_fee
            || intent.amount == U256::ZERO
            || intent.destinations.is_empty()
            || intent.destinations.len() > 1024
        {
            return Err(QiError::InvalidPolicy);
        }
        let scope = self.store.scope();
        if change.instance != self.store.instance() || change.scope != scope {
            return Err(QiError::IdentityMismatch);
        }
        if let Some(minimum) = change.burned_through
            && self
                .store
                .next_derivation_index(&change.account, true)?
                .is_none_or(|cursor| cursor < minimum)
        {
            return Err(QiError::IdentityMismatch);
        }
        let metadata: BTreeMap<Address, PublicAddress> = self
            .store
            .addresses()?
            .into_iter()
            .map(|entry| (entry.address(), entry))
            .collect();
        let mut seen = BTreeSet::new();
        for destination in &intent.destinations {
            if (!cross_zone && destination.zone() != scope.zone)
                || !seen.insert(destination.address())
            {
                return Err(QiError::IdentityMismatch);
            }
        }
        for address in &change.addresses {
            if address.address().zone().ok() != Some(scope.zone)
                || !seen.insert(address.address())
                || metadata.get(&address.address()) != Some(address)
            {
                return Err(QiError::IdentityMismatch);
            }
            self.key_for(address)?;
        }
        let snapshot = self.store.snapshot()?;
        let checkpoint = snapshot.checkpoint.ok_or(QiError::MissingSnapshot)?;
        self.verify_network().await?;
        let candidate_height = self
            .candidate_height(snapshot.generation, checkpoint, policy.max_snapshot_age)
            .await?;
        let mut fee = policy.initial_fee;
        for _ in 0..policy.max_fee_rounds {
            let selection = select_fewest(
                &snapshot.coins,
                &SelectionRequest {
                    zone: scope.zone,
                    candidate_height,
                    target: intent.amount,
                    fee,
                    max_fee: policy.max_fee,
                    max_inputs: policy.max_inputs,
                    max_outputs: policy.max_outputs,
                },
            )?;
            if selection.spend_outputs.len() > intent.destinations.len() {
                return Err(QiError::InsufficientDestinations);
            }
            if selection.change_outputs.len() > change.addresses.len() {
                return Err(QiError::InsufficientChange);
            }
            let inputs: Vec<QiInput> = selection
                .inputs
                .iter()
                .map(|coin| {
                    let entry = metadata
                        .get(&coin.address.address())
                        .ok_or(QiError::IdentityMismatch)?;
                    let public_key = PublicKey::from_sec1_bytes(entry.public_key())
                        .map_err(|_| QiError::IdentityMismatch)?;
                    Ok(QiInput {
                        previous_output: coin.outpoint,
                        public_key,
                    })
                })
                .collect::<Result<_, QiError>>()?;
            let recipient_outputs = selection.spend_outputs.len();
            let outputs = selection
                .spend_outputs
                .iter()
                .zip(&intent.destinations)
                .map(|(denomination, address)| QiOutput {
                    address: address.address(),
                    denomination: *denomination,
                })
                .chain(selection.change_outputs.iter().zip(&change.addresses).map(
                    |(denomination, address)| QiOutput {
                        address: address.address(),
                        denomination: *denomination,
                    },
                ))
                .collect();
            let transaction = QiTransaction {
                chain_id: scope.chain_id,
                inputs,
                outputs,
                data: Vec::new(),
            };
            // This also rejects recipient/input address reuse and duplicate outputs.
            transaction.unsigned_bytes()?;
            let quote = self.provider.estimate_qi_fee(&transaction).await?;
            if quote > policy.max_fee {
                return Err(SelectionError::FeeBudgetExceeded.into());
            }
            if quote > fee {
                fee = quote;
                continue;
            }
            for coin in &selection.inputs {
                self.key_for(
                    metadata
                        .get(&coin.address.address())
                        .ok_or(QiError::IdentityMismatch)?,
                )?;
            }
            // Recheck canonicality and lock/expiry eligibility after the asynchronous fee loop.
            let final_height = self
                .candidate_height(snapshot.generation, checkpoint, policy.max_snapshot_age)
                .await?;
            if selection.inputs.iter().any(|coin| {
                coin.unlock_height > final_height
                    || coin.expires_at.is_some_and(|expiry| final_height >= expiry)
            }) {
                return Err(QiError::StaleSnapshot);
            }
            let digest = transaction.signing_digest()?;
            let outpoints: Vec<_> = selection.inputs.iter().map(|coin| coin.outpoint).collect();
            self.store
                .reserve_qi(id, snapshot.generation, final_height, &outpoints)?;
            return Ok(PreparedQiTransaction {
                instance: self.store.instance(),
                id,
                scope,
                transaction,
                fee,
                recipient_outputs,
                digest,
            });
        }
        Err(SelectionError::FeeDidNotConverge.into())
    }
    /// Sign only the frozen payload with locally verified ordered keys, committing
    /// canonical signed bytes and claims durably before exposing the signature.
    pub fn sign(
        &mut self,
        prepared: &PreparedQiTransaction,
    ) -> Result<SignedQiTransaction, QiError> {
        if prepared.instance != self.store.instance() || prepared.scope != self.store.scope() {
            return Err(QiError::IdentityMismatch);
        }
        let reservation = self
            .store
            .reservation(prepared.id)?
            .ok_or(QiError::IdentityMismatch)?;
        let mut expected: Vec<_> = prepared
            .transaction
            .inputs
            .iter()
            .map(|input| input.previous_output)
            .collect();
        expected.sort_unstable();
        let mut actual = self.store.reserved_outpoints(prepared.id)?;
        actual.sort_unstable();
        if reservation.state != ReservationState::Reserved || expected != actual {
            return Err(QiError::IdentityMismatch);
        }
        let metadata: BTreeMap<_, _> = self
            .store
            .addresses()?
            .into_iter()
            .map(|entry| (entry.address(), entry))
            .collect();
        let keys = prepared
            .transaction
            .inputs
            .iter()
            .map(|input| {
                let entry = metadata
                    .get(&input.public_key.address())
                    .ok_or(QiError::IdentityMismatch)?;
                self.key_for(entry)
            })
            .collect::<Result<Vec<_>, QiError>>()?;
        let references: Vec<_> = keys.iter().collect();
        let signed = prepared.transaction.sign_local(&references)?;
        self.store.commit_signed_qi(prepared.id, &signed)?;
        Ok(signed)
    }
    /// Explicitly submit persisted bytes once, checking network identity again.
    /// The state becomes Submitted before I/O; cancellation, timeout, malformed
    /// acknowledgement and restart retain signed bytes and all input claims.
    /// Calling again explicitly rebroadcasts the same bytes, without replanning.
    pub async fn broadcast(&mut self, id: ReservationId) -> Result<BroadcastResult, QiError> {
        let reservation = self
            .store
            .reservation(id)?
            .ok_or(QiError::MissingSignedPayload)?;
        if !matches!(
            reservation.state,
            ReservationState::Signed | ReservationState::Submitted
        ) {
            return Err(QiError::MissingSignedPayload);
        }
        let bytes = self
            .store
            .signed_payload(id)?
            .ok_or(QiError::MissingSignedPayload)?;
        let signed = quai_consensus::SignedQiOperation::decode(&bytes)?;
        let metadata: BTreeMap<_, _> = self
            .store
            .addresses()?
            .into_iter()
            .map(|entry| (entry.address(), entry))
            .collect();
        for input in &signed.transaction().inputs {
            self.key_for(
                metadata
                    .get(&input.public_key.address())
                    .ok_or(QiError::IdentityMismatch)?,
            )?;
        }
        self.verify_network().await?;
        self.store.mark_submitted(id)?;
        Ok(match &signed {
            quai_consensus::SignedQiOperation::Transfer(tx) => {
                self.provider.broadcast_qi(tx).await?
            }
            quai_consensus::SignedQiOperation::Conversion(tx) => {
                self.provider.broadcast_qi_conversion(tx).await?
            }
            quai_consensus::SignedQiOperation::Wrapping(tx) => {
                self.provider.broadcast_qi_wrapping(tx).await?
            }
        })
    }
}

mod replacements;
pub use replacements::{
    PreparedQiReplacement, QiCandidateStatus, QiFamilyObservation, QiReplacementIntent,
};

/// Sign the pinned Qi message format using exact owned HD/imported/payment metadata.
/// Independently checks the resolver's full public key and address before signing.
/// This signs Keccak(message) with BIP340; it adds no chain or application domain,
/// performs no RPC, and does not mutate the wallet or its transaction claims.
pub fn sign_message(
    resolver: &impl QiKeyResolver,
    address: &PublicAddress,
    message: &[u8],
) -> Result<quai_crypto::SchnorrSignature, QiError> {
    let key = resolver.resolve(address)?;
    if key.public_key().to_compressed() != *address.public_key()
        || key.public_key().address() != address.address()
        || quai_primitives::QiAddress::try_from(address.address()).is_err()
    {
        return Err(QiError::IdentityMismatch);
    }
    quai_signer::sign_qi_message(&key, message).map_err(|_| QiError::MessageSigning)
}
