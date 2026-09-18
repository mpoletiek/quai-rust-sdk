//! Browser Qi allocation, current discovery, exact planning and durable signing.
use crate::browser_addresses::{BrowserAddressBook, BrowserAddressError, BrowserAddressSnapshot};
use crate::browser_qi::{BrowserQiBook, BrowserQiError, BrowserQiSnapshot};
use crate::browser_recovery::{BrowserRecoveryError, BrowserRecoverySession};
use crate::discovery::{QiDiscoveryError, QiDiscoveryOptions, discover_qi};
use crate::qi_preflight::{
    QiFeeMode, QiOperationIntent, QiPolicy, QiPreflightError, QiQuote, QiQuoteRequest, QiSource,
    include_known_qi_addresses, quote_qi,
};
use quai_consensus::{QiTransaction, SignedQiOperation};
use quai_primitives::Hash32;
use quai_provider::{BroadcastResult, Provider};
use quai_rpc::{Transport, U256};
use quai_wallet::allocation::AddressAllocationId;
use quai_wallet::discovery::NetworkScope;
use quai_wallet::metadata::{PublicAddress, StorageError};
use quai_wallet::qi_custody::ReservationId;
use quai_wallet::qi_keys::QiKeyResolver;
use quai_wallet::{AccountPublic, CoinType};

/// Allocation/preparation failures retain burned ranges and signed custody.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BrowserQiTransactionError {
    /// Browser input custody failed or another writer changed the revision.
    #[error(transparent)]
    Custody(#[from] BrowserQiError),
    /// Fresh change allocation failed; inspect the supplied allocation IDs.
    #[error(transparent)]
    Allocation(#[from] BrowserAddressError),
    /// Source, identity, selection, fee or canonical observation failed.
    #[error(transparent)]
    Preflight(#[from] QiPreflightError),
    /// Current gap/deep scan failed.
    #[error(transparent)]
    Discovery(#[from] QiDiscoveryError),
    /// Explicit persisted submission failed or was ambiguous.
    #[error(transparent)]
    Recovery(#[from] BrowserRecoveryError),
}
/// One-use fresh HD change capacity, with ranges and completions durably recorded.
/// No clone, deserialization or constructor from arbitrary addresses. Dropping a
/// pool or failing preparation never rewinds its consumed allocation IDs/ranges.
pub struct BrowserQiChangePool {
    scope: NetworkScope,
    allocation_book: BrowserAddressBook,
    addresses: Vec<PublicAddress>,
}
impl BrowserQiChangePool {
    /// Allocate before discovery; at most 1024 unique IDs, 100,000 total raw
    /// attempts. Reusing an old ID fails even if its completed address is known.
    /// Cancellation retains every dispatched reservation and completed address.
    pub async fn allocate(
        book: &BrowserAddressBook,
        ids: &[AddressAllocationId],
        attempts_per_address: u32,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<Self, BrowserQiTransactionError> {
        if ids.len() > 1024
            || !(1..=100_000).contains(&attempts_per_address)
            || ids.len().saturating_mul(attempts_per_address as usize) > 100_000
            || ids
                .iter()
                .map(|id| id.0)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != ids.len()
        {
            return Err(QiPreflightError::Invalid.into());
        }
        let snapshot = book.snapshot().await?;
        if snapshot.book.account().coin_type() != CoinType::Qi {
            return Err(QiPreflightError::Invalid.into());
        }
        let mut addresses = Vec::with_capacity(ids.len());
        for id in ids {
            addresses.push(
                book.allocate(*id, true, attempts_per_address, &mut cancelled)
                    .await?,
            );
        }
        Ok(Self {
            scope: snapshot.book.scope(),
            allocation_book: book.clone(),
            addresses,
        })
    }
    /// Public completions, including capacity later left unused by denomination selection.
    pub fn addresses(&self) -> &[PublicAddress] {
        &self.addresses
    }
}
/// Exact operation and explicit planning policies, with no implicit fee default.
pub struct BrowserQiRequest {
    /// Typed transfer/conversion/wrapping/sweep intent.
    pub intent: QiOperationIntent,
    /// Amount-independent fee/resource/age limits.
    pub policy: QiPolicy,
    /// Explicit ordinary node, specialized profile or caller-specified fee source.
    pub fees: QiFeeMode,
}
/// Immutable reviewed fields, bound to the same book that claimed their inputs.
pub struct PreparedBrowserQiTransaction {
    book: BrowserQiBook,
    id: ReservationId,
    quote: QiQuote,
}
impl PreparedBrowserQiTransaction {
    /// Original operation ID for restart inspection and explicit submission.
    pub fn reservation_id(&self) -> ReservationId {
        self.id
    }
    /// Exact ordered inputs/outputs and typed operation data to review.
    pub fn transaction(&self) -> &QiTransaction {
        self.quote.transaction()
    }
    /// Source-reported fee in Qits, not a cryptographic proof of input values.
    pub fn fee(&self) -> U256 {
        self.quote.fee()
    }
    /// Exact operation-specific digest.
    pub fn signing_digest(&self) -> Hash32 {
        self.quote.signing_digest()
    }
    /// Recipient outputs precede change outputs.
    pub fn recipient_outputs(&self) -> usize {
        self.quote.recipient_outputs()
    }
    /// Resolve exact keys and persist canonical signed bytes before returning them.
    /// No network call occurs and no field is repopulated at signing time.
    pub async fn sign(
        &self,
        resolver: &(impl QiKeyResolver + ?Sized),
    ) -> Result<SignedQiOperation, BrowserQiTransactionError> {
        Ok(self
            .book
            .sign(self.id, self.quote.transaction(), resolver)
            .await?)
    }
}
/// Same-input Qi replacement bound to the original durable family.
pub struct PreparedBrowserQiReplacement {
    book: BrowserQiBook,
    id: ReservationId,
    quote: crate::qi_replacement::QiReplacementQuote,
}
impl PreparedBrowserQiReplacement {
    /// Exact outputs/data after reducing only selected owned change.
    pub fn transaction(&self) -> &QiTransaction {
        self.quote.transaction()
    }
    /// Selected immutable parent, which may still mine.
    pub fn parent_hash(&self) -> Hash32 {
        self.quote.parent_hash()
    }
    /// Original input-claim family; no new inputs were reserved.
    pub fn reservation_id(&self) -> ReservationId {
        self.id
    }
    /// Source-reported total fee in Qits.
    pub fn fee(&self) -> U256 {
        self.quote.fee()
    }
    /// Exact reviewed signing digest.
    pub fn signing_digest(&self) -> Hash32 {
        self.quote.signing_digest()
    }
    /// Persist the verified candidate before returning. Repeating an identical
    /// parent/payload returns existing bytes, including randomized MuSig signatures.
    pub async fn sign(
        &self,
        resolver: &(impl QiKeyResolver + ?Sized),
    ) -> Result<SignedQiOperation, BrowserQiTransactionError> {
        let mut snapshot = self.book.snapshot().await?;
        let op = snapshot
            .book
            .operation(self.id)
            .filter(|op| {
                matches!(
                    op.state,
                    quai_wallet::qi_custody::ReservationState::Signed
                        | quai_wallet::qi_custody::ReservationState::Submitted
                )
            })
            .ok_or(QiPreflightError::Invalid)?;
        for edge in &op.replacements {
            let existing =
                SignedQiOperation::decode(&edge.payload).map_err(BrowserQiError::from)?;
            if edge.parent == self.parent_hash() && existing.transaction() == self.transaction() {
                return Ok(existing);
            }
        }
        let present = op.transaction == Some(self.parent_hash())
            || op.replacements.iter().any(|edge| {
                SignedQiOperation::decode(&edge.payload)
                    .and_then(|tx| tx.hash())
                    .ok()
                    == Some(self.parent_hash())
            });
        if !present {
            return Err(QiPreflightError::Invalid.into());
        }
        snapshot
            .book
            .check_inputs(self.id, self.transaction())
            .map_err(BrowserQiError::from)?;
        // Verify both input ownership and the approved reduction of local change.
        for owner in self.quote.required_owners() {
            let key = resolver.resolve(owner).map_err(BrowserQiError::from)?;
            if key.public_key().to_compressed() != *owner.public_key() {
                return Err(QiPreflightError::Invalid.into());
            }
        }
        let mut keys = Vec::with_capacity(self.transaction().inputs.len());
        for input in &self.transaction().inputs {
            let address = input
                .public_key
                .address()
                .try_into()
                .map_err(|_| QiPreflightError::Invalid)?;
            let owner = snapshot
                .book
                .address(address)
                .ok_or(QiPreflightError::Invalid)?;
            let key = resolver.resolve(owner).map_err(BrowserQiError::from)?;
            if key.public_key() != input.public_key {
                return Err(QiPreflightError::Invalid.into());
            }
            keys.push(key);
        }
        let keys: Vec<_> = keys.iter().collect();
        let signed = match self.transaction().data.len() {
            0 => SignedQiOperation::Transfer(
                self.transaction()
                    .sign_local(&keys)
                    .map_err(BrowserQiError::from)?,
            ),
            20 => SignedQiOperation::Wrapping(
                quai_consensus::QiWrappingTransaction::from_transaction(self.transaction().clone())
                    .map_err(BrowserQiError::from)?
                    .sign_local(&keys)
                    .map_err(BrowserQiError::from)?,
            ),
            22 => SignedQiOperation::Conversion(
                quai_consensus::QiConversionTransaction::from_transaction(
                    self.transaction().clone(),
                )
                .map_err(BrowserQiError::from)?
                .sign_local(&keys)
                .map_err(BrowserQiError::from)?,
            ),
            _ => return Err(QiPreflightError::Invalid.into()),
        };
        snapshot
            .book
            .commit_replacement(self.id, self.parent_hash(), &signed)
            .map_err(BrowserQiError::from)?;
        self.book
            .store
            .compare_exchange(
                Some(snapshot.revision),
                Some(&snapshot.book.export_state().map_err(BrowserQiError::from)?),
            )
            .await
            .map_err(BrowserQiError::from)?;
        Ok(signed)
    }
}
/// Browser preparation over a scoped input journal and explicit local key resolver.
/// The resolver supports HD, imported and registered payment-code receive origins.
pub struct BrowserQiSession<'a, T> {
    provider: &'a Provider<T>,
    book: &'a BrowserQiBook,
    keys: &'a dyn QiKeyResolver,
}
impl<'a, T: Transport> BrowserQiSession<'a, T> {
    /// Bind configured objects without prompting, storage reads or network access.
    pub fn new(
        provider: &'a Provider<T>,
        book: &'a BrowserQiBook,
        keys: &'a dyn QiKeyResolver,
    ) -> Self {
        Self {
            provider,
            book,
            keys,
        }
    }
    /// Scan current HD receive/change ranges (default gap 50), overlay all local
    /// claims, quote and reserve against the revision captured before discovery.
    /// Explicit deep ranges retain their ordinary discovery semantics. Persisted
    /// custody owners and all completed addresses in the change allocation journal
    /// are queried beyond the scan. Both journals must use the same named database;
    /// their pre-read revisions are fenced together when committing the input claim.
    #[allow(clippy::too_many_arguments)]
    pub async fn prepare_discovered(
        &self,
        id: ReservationId,
        account: &AccountPublic,
        options: &QiDiscoveryOptions,
        request: BrowserQiRequest,
        change: BrowserQiChangePool,
        cancelled: impl FnMut() -> bool,
    ) -> Result<PreparedBrowserQiTransaction, BrowserQiTransactionError> {
        let snapshot = self.book.snapshot().await?;
        let allocations = change.allocation_book.snapshot().await?;
        let mut known: Vec<_> = snapshot.book.addresses().cloned().collect();
        known.extend(
            allocations
                .book
                .allocations()
                .filter_map(|a| match &a.status {
                    quai_wallet::allocation::AddressAllocationStatus::Completed(owner) => {
                        Some(owner.clone())
                    }
                    _ => None,
                }),
        );
        let report = discover_qi(
            self.provider,
            snapshot.book.scope(),
            account,
            options,
            cancelled,
        )
        .await?;
        let mut source = QiSource::from_discovery(&report, account)?;
        include_known_qi_addresses(
            self.provider,
            &mut source,
            &known,
            request.policy.max_snapshot_age,
        )
        .await?;
        self.prepare_snapshot(snapshot, id, source, request, change, Some(allocations))
            .await
    }
    /// Plan over explicit current observations, including imported/payment keys.
    /// Capture `book.snapshot().revision` before source reads; a changed revision
    /// rejects. Latest-only observations do not become a historical/atomic snapshot.
    pub async fn prepare_observed(
        &self,
        id: ReservationId,
        expected_revision: u64,
        source: QiSource,
        request: BrowserQiRequest,
        change: BrowserQiChangePool,
    ) -> Result<PreparedBrowserQiTransaction, BrowserQiTransactionError> {
        let snapshot = self.book.snapshot().await?;
        if snapshot.revision != expected_revision {
            return Err(BrowserQiError::State(StorageError::StaleSnapshot).into());
        }
        self.prepare_snapshot(snapshot, id, source, request, change, None)
            .await
    }
    #[allow(clippy::too_many_arguments)]
    async fn prepare_snapshot(
        &self,
        mut snapshot: BrowserQiSnapshot,
        id: ReservationId,
        mut source: QiSource,
        request: BrowserQiRequest,
        change: BrowserQiChangePool,
        allocations: Option<BrowserAddressSnapshot>,
    ) -> Result<PreparedBrowserQiTransaction, BrowserQiTransactionError> {
        if source.scope != snapshot.book.scope()
            || change.scope != source.scope
            || snapshot.book.operation(id).is_some()
        {
            return Err(QiPreflightError::Invalid.into());
        }
        let claims = snapshot.book.claimed_outpoints();
        for coin in &mut source.coins {
            coin.reserved |= claims.contains(&coin.outpoint);
        }
        for address in &change.addresses {
            if source
                .coins
                .iter()
                .any(|c| c.address.address() == address.address())
            {
                return Err(QiPreflightError::Invalid.into());
            }
            self.check_key(address)?;
        }
        let quote = quote_qi(
            self.provider,
            QiQuoteRequest {
                source: &source,
                intent: request.intent,
                policy: request.policy,
                fees: request.fees,
                change: &change.addresses,
            },
        )
        .await?;
        for owner in quote.selected_owners() {
            self.check_key(owner)?;
        }
        snapshot
            .book
            .reserve(
                id,
                source.checkpoint,
                quote.candidate_height(),
                quote.selected_inputs(),
                quote.selected_owners(),
            )
            .map_err(BrowserQiError::from)?;
        let bytes = snapshot.book.export_state().map_err(BrowserQiError::from)?;
        if let Some(allocations) = allocations {
            let allocated = allocations.book.export_state();
            quai_browser::compare_exchange_snapshots(&[
                quai_browser::BrowserSnapshotUpdate {
                    store: &self.book.store,
                    expected: Some(snapshot.revision),
                    bytes: Some(&bytes),
                },
                quai_browser::BrowserSnapshotUpdate {
                    store: &change.allocation_book.store,
                    expected: Some(allocations.revision),
                    bytes: Some(&allocated),
                },
            ])
            .await
            .map_err(BrowserQiError::from)?;
        } else {
            self.book
                .store
                .compare_exchange(Some(snapshot.revision), Some(&bytes))
                .await
                .map_err(BrowserQiError::from)?;
        }
        Ok(PreparedBrowserQiTransaction {
            book: self.book.clone(),
            id,
            quote,
        })
    }
    /// Prepare a same-input candidate funded only from selected owned change.
    /// Capture the custody revision before refreshing source input observations;
    /// both the parent and original claim set are revalidated. The fixed candidate
    /// is estimated once; a fee deficit rejects without changing its outputs.
    #[allow(clippy::too_many_arguments)]
    pub async fn prepare_replacement(
        &self,
        id: ReservationId,
        expected_revision: u64,
        source: QiSource,
        intent: crate::qi_replacement::QiReplacementIntent,
        policy: QiPolicy,
        fees: QiFeeMode,
    ) -> Result<PreparedBrowserQiReplacement, BrowserQiTransactionError> {
        let snapshot = self.book.snapshot().await?;
        if snapshot.revision != expected_revision || source.scope != snapshot.book.scope() {
            return Err(QiPreflightError::Stale.into());
        }
        let op = snapshot
            .book
            .operation(id)
            .filter(|op| {
                matches!(
                    op.state,
                    quai_wallet::qi_custody::ReservationState::Signed
                        | quai_wallet::qi_custody::ReservationState::Submitted
                ) && op.replacements.len() < 32
            })
            .ok_or(QiPreflightError::Invalid)?;
        let parent = std::iter::once(op.payload.as_deref().ok_or(QiPreflightError::Invalid)?)
            .chain(op.replacements.iter().map(|r| r.payload.as_slice()))
            .find_map(|bytes| {
                SignedQiOperation::decode(bytes)
                    .ok()
                    .filter(|tx| tx.hash().ok() == Some(intent.parent))
            })
            .ok_or(QiPreflightError::Invalid)?;
        snapshot
            .book
            .check_inputs(id, parent.transaction())
            .map_err(BrowserQiError::from)?;
        let quote = crate::qi_replacement::quote_qi_replacement(
            self.provider,
            &source,
            &parent,
            intent,
            policy,
            fees,
        )
        .await?;
        for owner in quote.required_owners() {
            self.check_key(owner)?;
        }
        self.book
            .store
            .compare_exchange(
                Some(snapshot.revision),
                Some(&snapshot.book.export_state().map_err(BrowserQiError::from)?),
            )
            .await
            .map_err(BrowserQiError::from)?;
        Ok(PreparedBrowserQiReplacement {
            book: self.book.clone(),
            id,
            quote,
        })
    }
    fn check_key(&self, owner: &PublicAddress) -> Result<(), BrowserQiTransactionError> {
        let key = self.keys.resolve(owner).map_err(BrowserQiError::from)?;
        if key.public_key().address() != owner.address()
            || key.public_key().to_compressed() != *owner.public_key()
        {
            return Err(QiPreflightError::Invalid.into());
        }
        Ok(())
    }
    /// Explicitly submit the original persisted bytes once. Candidate selection
    /// and canonical reconciliation are available through BrowserRecoverySession.
    pub async fn broadcast(
        &self,
        id: ReservationId,
    ) -> Result<BroadcastResult, BrowserQiTransactionError> {
        Ok(BrowserRecoverySession::for_qi(self.provider, self.book)
            .broadcast_root(id)
            .await?)
    }
}
