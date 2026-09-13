//! Borrowed inspection of authenticated backup records, never live spendability.
use super::*;
use quai_payments::{PaymentChannel, PrivatePaymentCode};

/// Public records from one authenticated backup scope. These are historical
/// custody/cursor records, not a current balance or finality assertion.
#[derive(Clone, Copy)]
pub struct BackupScope<'a>(&'a ScopeState);
impl<'a> BackupScope<'a> {
    /// Exact chain/genesis/zone recorded in this backup.
    pub fn scope(&self) -> NetworkScope {
        self.0.scope
    }
    /// Owned public addresses whose origins were checked during capture/decryption.
    pub fn addresses(&self) -> &'a [PublicAddress] {
        &self.0.addresses
    }
    /// Burned raw derivation cursors, including zone-grinding skips.
    pub fn derivation_cursors(&self) -> impl Iterator<Item = BackupDerivationCursor<'a>> {
        self.0
            .derivation
            .iter()
            .map(|cursor| BackupDerivationCursor {
                coin: cursor.coin,
                account: cursor.account,
                change: cursor.change,
                account_xpub: &cursor.xpub,
                next_index: cursor.next_index,
            })
    }
    /// Account and next allocated nonce, including unused reserved gaps.
    pub fn nonce_cursors(&self) -> impl Iterator<Item = (QuaiAddress, u64)> {
        self.0
            .nonces
            .iter()
            .map(|cursor| (cursor.address, cursor.next_nonce))
    }
    /// Original immutable claims and all retained replacement candidates.
    /// Inclusion observations may be stale; signed claims remain held after restore.
    pub fn operations(&self) -> impl Iterator<Item = BackupOperation<'a>> {
        self.0.operations.iter().map(|operation| BackupOperation {
            reservation: &operation.record,
            qi_claims: &operation.qi,
            nonce: operation.nonce,
            signed_payload: operation.payload.as_deref(),
            replacements: &operation.replacements,
        })
    }
}
/// One authenticated public derivation cursor; `2^31` means exhausted.
#[derive(Clone, Copy, Debug)]
pub struct BackupDerivationCursor<'a> {
    /// Quai or Qi BIP44 coin.
    pub coin: CoinType,
    /// Unhardened account number.
    pub account: u32,
    /// Receive (`false`) or change (`true`) branch.
    pub change: bool,
    /// Bound account xpub checked against a retained private origin.
    pub account_xpub: &'a str,
    /// First raw index not consumed by this backup; never rewind live state to it.
    pub next_index: u32,
}
/// Borrowed custody record. Exposed signed bytes are public transaction data,
/// not a request to submit them or evidence that their claims are spendable.
#[derive(Clone, Copy, Debug)]
pub struct BackupOperation<'a> {
    /// Monotonic state, transaction ID and any caller-observed inclusion.
    pub reservation: &'a Reservation,
    /// Original Qi outpoints and their independently checked owner addresses.
    pub qi_claims: &'a [(OutPoint, Address)],
    /// Original account nonce claim; absent for a Qi operation.
    pub nonce: Option<(QuaiAddress, u64)>,
    /// Canonical original signed bytes, if retained. Hash-only records stay explicit.
    pub signed_payload: Option<&'a [u8]>,
    /// All same-claim candidate edges in original order.
    pub replacements: &'a [QuaiReplacement],
}
/// Borrowed registered payment channel from an authenticated backup.
#[derive(Clone, Copy)]
pub struct BackupPaymentChannel<'a>(&'a crate::state::payment::StoredPaymentChannel);
impl BackupPaymentChannel<'_> {
    /// Exact chain ID followed by genesis hash (32 big-endian bytes each).
    pub fn network_key(&self) -> &[u8; 64] {
        &self.0.network
    }
    /// Monotonic local channel revision recorded by the backup, not its freshness.
    pub fn generation(&self) -> u64 {
        self.0.generation
    }
    /// BIP47 account number.
    pub fn account(&self) -> u32 {
        self.0.account
    }
    /// Canonical public owner code bytes.
    pub fn local_code_bytes(&self) -> &[u8; 80] {
        &self.0.local
    }
    /// Canonical public peer code bytes.
    pub fn peer_code_bytes(&self) -> &[u8; 80] {
        &self.0.peer
    }
    /// Reconstruct this public channel for its explicit private owner; verifies
    /// code/account identity and preserves all send/receive burned ranges.
    pub fn channel(&self, owner: &PrivatePaymentCode) -> Result<PaymentChannel> {
        self.0
            .checked(owner)
            .map_err(|_| WalletBackupError::Ownership)
    }
}
/// Borrowed payment exposure with exact network and peer/owner context.
#[derive(Clone, Copy, Debug)]
pub struct BackupPaymentExposure<'a> {
    /// Chain ID followed by genesis hash, 32 big-endian bytes each.
    pub network_key: &'a [u8; 64],
    /// Canonical owner payment-code bytes.
    pub local_code_bytes: &'a [u8; 80],
    /// Canonical counterparty payment-code bytes.
    pub peer_code_bytes: &'a [u8; 80],
    /// Asserted BIP47 account number, proved by the supplied owner.
    pub account: u32,
    /// Direction, zone, exact child, point/address and entire burned range.
    pub record: &'a PaymentAddressRecord,
}
impl WalletBackup {
    /// Inspect one exact authenticated backup scope without storage or network I/O.
    /// A live store must merge cursors/claims monotonically, never overwrite itself
    /// with an older backup. Absence does not authorize releasing any live claim.
    pub fn scope_state(&self, scope: NetworkScope) -> Option<BackupScope<'_>> {
        self.state
            .scopes
            .iter()
            .find(|state| state.scope == scope)
            .map(BackupScope)
    }
    /// Inspect registered channels across every recorded network.
    pub fn payment_channels(&self) -> impl Iterator<Item = BackupPaymentChannel<'_>> {
        self.state.channels.iter().map(BackupPaymentChannel)
    }
    /// Inspect recorded exposures without exposing private owner keys.
    pub fn payment_exposures(&self) -> impl Iterator<Item = BackupPaymentExposure<'_>> {
        self.state
            .exposures
            .iter()
            .map(|exposure| BackupPaymentExposure {
                network_key: &exposure.network,
                local_code_bytes: &exposure.local,
                peer_code_bytes: &exposure.peer,
                account: exposure.account,
                record: &exposure.record,
            })
    }
}
