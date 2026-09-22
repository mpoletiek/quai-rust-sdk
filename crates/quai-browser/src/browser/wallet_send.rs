//! Explicit wallet-mediated sends; reported hashes are verified by later reads.
use super::*;
use quai_consensus::{QuaiTransaction, SignedQuaiTransaction};
use quai_primitives::Hash32;
use quai_provider::{Inclusion, Provider, ProviderError};

/// Original request identity to retain before awaiting a wallet-mediated send.
/// The signing digest is NOT a transaction ID. A wallet can change fields in its
/// approval flow; after an unknown outcome this identity alone cannot prove reuse safe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct WalletSendIdentity {
    /// Requested exposed account.
    pub from: QuaiAddress,
    /// Requested network.
    pub chain_id: U256,
    /// Requested nonce; observed wallet behavior is checked separately.
    pub nonce: u64,
    /// Digest of the complete requested unsigned transaction.
    pub signing_digest: Hash32,
}
impl WalletSendIdentity {
    /// Compute public recovery metadata without any wallet request.
    pub fn new(from: QuaiAddress, transaction: &QuaiTransaction) -> Result<Self, BrowserError> {
        Ok(Self {
            from,
            chain_id: transaction.chain_id,
            nonce: transaction.nonce,
            signing_digest: transaction
                .signing_digest()
                .map_err(|_| BrowserError::InvalidConfig)?,
        })
    }
}
/// Whether the adapter failed before dispatch or after submission could occur.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WalletSendError {
    /// No wallet transaction request was dispatched.
    #[error("wallet transaction preflight failed: {0}")]
    Preflight(#[source] BrowserError),
    /// Wallet dispatch began. No retry or nonce/claim release is implied.
    #[error("wallet transaction outcome is ambiguous: {source}")]
    Ambiguous {
        /// Original request metadata, not an expected signed transaction ID.
        identity: WalletSendIdentity,
        /// Hash returned before the failure, if it was structurally parseable.
        /// It may have failed origin/ledger checks and is not a verified identity.
        reported_hash: Option<Hash32>,
        /// Sanitized cause, including numeric rejection or context change.
        #[source]
        source: BrowserError,
    },
}
impl WalletSendError {
    /// True after wallet dispatch may have occurred, including remote rejection.
    pub fn acceptance_is_ambiguous(&self) -> bool {
        matches!(self, Self::Ambiguous { .. })
    }
}
/// Structurally valid wallet acknowledgement, pending independent transaction verification.
/// Unlike raw submission, there was no locally signed payload from which to compute its ID.
#[derive(Clone)]
pub struct WalletSendAcknowledgement {
    identity: WalletSendIdentity,
    reported_hash: Hash32,
    requested: QuaiTransaction,
}
impl fmt::Debug for WalletSendAcknowledgement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WalletSendAcknowledgement")
            .field("identity", &self.identity)
            .field("reported_hash", &self.reported_hash)
            .finish()
    }
}
impl WalletSendAcknowledgement {
    /// Restore an unverified wallet-reported hash and original request after
    /// restart or an ambiguous error. This does not assert that submission occurred.
    /// Validates bounds and hash routing; `observe` still verifies signed identity.
    pub fn from_reported_hash(
        from: QuaiAddress,
        requested: QuaiTransaction,
        reported_hash: Hash32,
    ) -> Result<Self, BrowserError> {
        if requested.chain_id == U256::ZERO || !valid_hash(from, reported_hash) {
            return Err(BrowserError::InvalidResult);
        }
        let identity = WalletSendIdentity::new(from, &requested)?;
        Ok(Self {
            identity,
            reported_hash,
            requested,
        })
    }
    /// Original request metadata; no wallet changes have been accepted implicitly.
    pub fn identity(&self) -> WalletSendIdentity {
        self.identity
    }
    /// Wallet-reported hash for readonly lookup; not yet independently verified.
    pub fn reported_hash(&self) -> Hash32 {
        self.reported_hash
    }
    /// Exact original request, retained for comparison and application recovery.
    pub fn requested_transaction(&self) -> &QuaiTransaction {
        &self.requested
    }
    /// One readonly transaction lookup. None means not indexed yet, not dropped.
    /// Cryptographically reconstructs the returned transaction and compares every
    /// unsigned field and sender. No unbounded polling, retry or submission occurs.
    pub async fn observe<T: Transport>(
        &self,
        provider: &Provider<T>,
    ) -> Result<Option<WalletSendObservation>, ProviderError> {
        let zone = self.identity.from.zone();
        let actual = provider.chain_id(zone.into()).await?;
        if actual != self.identity.chain_id {
            return Err(ProviderError::ChainMismatch {
                expected: self.identity.chain_id,
                actual,
            });
        }
        let Some(observed) = provider.transaction(zone, self.reported_hash).await? else {
            return Ok(None);
        };
        let signed = observed.verified_quai()?;
        let matches_request =
            signed.from() == self.identity.from && signed.transaction() == &self.requested;
        Ok(Some(WalletSendObservation {
            signed,
            matches_request,
            inclusion: observed.inclusion,
        }))
    }
}
/// Independently verified transaction bytes and explicit comparison to the request.
#[derive(Debug)]
pub struct WalletSendObservation {
    signed: SignedQuaiTransaction,
    matches_request: bool,
    inclusion: Option<Inclusion>,
}
impl WalletSendObservation {
    /// Actual canonical signed transaction, verified against the reported hash.
    pub fn signed(&self) -> &SignedQuaiTransaction {
        &self.signed
    }
    /// Whether the wallet preserved every requested field and the requested sender.
    pub fn matches_request(&self) -> bool {
        self.matches_request
    }
    /// Node-claimed location only; canonical receipt/confirmation checks are separate.
    pub fn inclusion(&self) -> Option<Inclusion> {
        self.inclusion
    }
}
impl InjectedProvider {
    /// Explicitly ask the selected wallet to authorize, sign and send a populated
    /// type-0 request using `quai_sendTransaction`. No offline-signing support is
    /// required. Retain account/nonce/digest before awaiting in case of cancellation.
    /// Wallets may alter fields in their approval flow: the returned acknowledgement
    /// does not assert exact signing; call `observe` to verify and compare the result.
    pub async fn send_quai_transaction(
        &self,
        address: QuaiAddress,
        transaction: &QuaiTransaction,
    ) -> Result<WalletSendAcknowledgement, WalletSendError> {
        let params = self
            .transaction_params(address, transaction, "quai_sendTransaction")
            .map_err(WalletSendError::Preflight)?;
        let identity =
            WalletSendIdentity::new(address, transaction).map_err(WalletSendError::Preflight)?;
        let revision = self.context_revision();
        self.check_chain()
            .await
            .map_err(WalletSendError::Preflight)?;
        let accounts = Self::accounts_from(
            self.raw("quai_accounts", json!([]))
                .await
                .map_err(WalletSendError::Preflight)?,
        )
        .map_err(WalletSendError::Preflight)?;
        if !accounts.contains(&address) {
            return Err(WalletSendError::Preflight(BrowserError::AccountUnavailable));
        }
        if provider_changed(&self.watch.0, revision) {
            return Err(WalletSendError::Preflight(BrowserError::ContextChanged));
        }
        let value = self
            .raw("quai_sendTransaction", params)
            .await
            .map_err(|source| WalletSendError::Ambiguous {
                identity,
                reported_hash: None,
                source,
            })?;
        let reported_hash = value.as_str().and_then(|s| s.parse::<Hash32>().ok());
        self.verify_context(revision, Some(address))
            .await
            .map_err(|source| WalletSendError::Ambiguous {
                identity,
                reported_hash,
                source,
            })?;
        let valid = reported_hash
            .filter(|hash| valid_hash(address, *hash))
            .ok_or(WalletSendError::Ambiguous {
                identity,
                reported_hash,
                source: BrowserError::InvalidResult,
            })?;
        Ok(WalletSendAcknowledgement {
            identity,
            reported_hash: valid,
            requested: transaction.clone(),
        })
    }
}
fn valid_hash(address: QuaiAddress, hash: Hash32) -> bool {
    let bytes = hash.bytes();
    hash != Hash32::ZERO
        && bytes[0] == address.zone().byte()
        && bytes[2] == address.zone().byte()
        && bytes[1] & 0x80 == 0
        && bytes[3] & 0x80 == 0
}
