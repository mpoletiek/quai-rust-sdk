//! Backend-independent public wallet records shared with authenticated backups.
use crate::discovery::{Checkpoint, NetworkScope};
use crate::{
    CoinType,
    metadata::{PublicAddress, StorageError},
};
use quai_consensus::{OutPoint, U256};
use quai_primitives::{Address, Hash32, QuaiAddress};
type Result<T> = std::result::Result<T, StorageError>;
pub(crate) mod payment;
pub(crate) mod replacements;
use payment::{StoredPaymentChannel, StoredPaymentExposure};
pub use replacements::{QuaiReplacement, ReplacementCandidate};
/// Caller-generated unique 128-bit operation identifier; never reuse, including after release.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReservationId(pub [u8; 16]);
/// Durable monotonic operation state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i64)]
pub enum ReservationState {
    /// Unsigned; may be explicitly released before exposing a signed transaction.
    Reserved = 0,
    /// Signed transaction may have escaped; claim cannot be released.
    Signed = 1,
    /// Submission was attempted/observed; claim cannot be released.
    Submitted = 2,
    /// Caller observed inclusion; claim remains held across reorgs.
    Confirmed = 3,
    /// Explicitly released while unsigned. ID remains consumed.
    Released = 4,
}
/// Durable operation record with optional public transaction/block observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reservation {
    /// Unique caller-assigned operation ID.
    pub id: ReservationId,
    /// Current state.
    pub state: ReservationState,
    /// Immutable transaction hash recorded before a signature is exposed.
    pub transaction: Option<Hash32>,
    /// Caller-observed inclusion, without a storage-layer proof of finality.
    pub inclusion: Option<Checkpoint>,
}

#[derive(Clone, Debug)]
pub(crate) struct PublicWalletState {
    pub scopes: Vec<ScopeState>,
    pub channels: Vec<StoredPaymentChannel>,
    pub exposures: Vec<StoredPaymentExposure>,
}
#[derive(Clone, Debug)]
pub(crate) struct ScopeState {
    pub scope: NetworkScope,
    pub addresses: Vec<PublicAddress>,
    pub derivation: Vec<DerivationState>,
    pub nonces: Vec<NonceState>,
    pub operations: Vec<OperationState>,
}
#[derive(Clone, Debug)]
pub(crate) struct DerivationState {
    pub coin: CoinType,
    pub account: u32,
    pub change: bool,
    pub xpub: String,
    pub next_index: u32,
}
#[derive(Clone, Debug)]
pub(crate) struct NonceState {
    pub address: QuaiAddress,
    pub next_nonce: u64,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OperationState {
    pub record: Reservation,
    pub kind: u8,
    pub qi: Vec<(OutPoint, Address)>,
    pub nonce: Option<(QuaiAddress, u64)>,
    pub payload: Option<Vec<u8>>,
    pub replacements: Vec<QuaiReplacement>,
}
