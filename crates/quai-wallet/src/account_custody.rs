//! Portable account nonce and signed-candidate custody, with bounded public encoding.
//!
//! These mutations are in memory. A durable adapter must commit the complete book
//! before exposing a nonce or signed payload. Current balances, pending nonces and
//! canonical inclusion are caller observations, never established by this journal.
use crate::custody_codec::{Reader, Writer};
use crate::discovery::{Checkpoint, NetworkScope};
use crate::metadata::StorageError;
pub use crate::state::{QuaiReplacement, ReservationId, ReservationState};
use quai_consensus::{SignedQuaiTransaction, U256};
use quai_crypto::PublicKey;
use quai_primitives::{Hash32, QuaiAddress};
use std::collections::{BTreeMap, BTreeSet};

type Result<T> = std::result::Result<T, StorageError>;
/// Retained operation IDs, including released nonce gaps; exhaustion fails closed.
pub const MAX_ACCOUNT_OPERATIONS: usize = 256;
/// Aggregate encoded custody bound, including all signed candidates.
pub const MAX_ACCOUNT_CUSTODY_BYTES: usize = 16 * 1024 * 1024;
const MAGIC: &[u8; 8] = b"QACCTBK1";

/// Immutable inspection of an allocated nonce and its candidate family.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountOperation {
    /// Caller-generated ID, never reused for a different operation.
    pub id: ReservationId,
    /// Consumed account nonce, retained even after unsigned release.
    pub nonce: u64,
    /// Signed claims remain held after failed submission or reorg.
    pub state: ReservationState,
    /// Original transaction identity, including imported hash-only custody.
    pub transaction: Option<Hash32>,
    /// Exact canonical root bytes, if retained. Public transaction data, not secrets.
    pub payload: Option<Vec<u8>>,
    /// Ordered immutable fee-only replacement edges, at most 32.
    pub replacements: Vec<QuaiReplacement>,
    /// Caller-observed candidate and block. Does not prove finality or free the nonce.
    pub inclusion: Option<(Hash32, Checkpoint)>,
}
/// One account's nonce custody in an exact chain/genesis/zone namespace.
/// The private fields prevent callers from removing claims or rewinding cursors.
#[derive(Clone, Debug)]
pub struct AccountOperationBook {
    scope: NetworkScope,
    owner: PublicKey,
    address: QuaiAddress,
    next_nonce: u64,
    operations: BTreeMap<[u8; 16], AccountOperation>,
}
/// Result of a conservative authenticated backup union; no claim is released.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccountMergeReport {
    /// Newly retained IDs; existing IDs are never removed or reassigned.
    pub operations_added: usize,
    /// Newly retained replacement edges across all operations.
    pub candidates_added: usize,
    /// Live inclusion observations discarded for fresh canonical reconciliation.
    pub invalidated_inclusions: usize,
    /// Maximum retained next-nonce floor after the merge.
    pub next_nonce: u64,
}
impl AccountOperationBook {
    /// Start a never-written book with an explicit prior nonce floor. A remote
    /// pending nonce alone cannot recover previously exposed local transactions.
    pub fn new(scope: NetworkScope, owner: PublicKey, first_nonce: u64) -> Result<Self> {
        let address = QuaiAddress::try_from(owner.address()).map_err(|_| StorageError::Invalid)?;
        if scope.chain_id == U256::ZERO
            || scope.genesis == Hash32::ZERO
            || address.zone() != scope.zone
        {
            return Err(StorageError::Invalid);
        }
        Ok(Self {
            scope,
            owner,
            address,
            next_nonce: first_nonce,
            operations: BTreeMap::new(),
        })
    }
    /// Domain-separated public namespace; the network is stored separately.
    pub fn account_identity(owner: PublicKey) -> Hash32 {
        let mut bytes = b"quai-rust/account-custody/v1".to_vec();
        bytes.extend(owner.to_compressed());
        quai_crypto::keccak256(&bytes).into()
    }
    /// Exact configured network.
    pub fn scope(&self) -> NetworkScope {
        self.scope
    }
    /// Exact account whose nonces are held.
    pub fn address(&self) -> QuaiAddress {
        self.address
    }
    /// Exact compressed public-key identity; this does not expose signing material.
    pub fn owner(&self) -> PublicKey {
        self.owner
    }
    /// Next nonce floor; `u64::MAX` denotes exhausted allocation.
    pub fn next_nonce(&self) -> u64 {
        self.next_nonce
    }
    /// Retained operations in lexicographic ID order.
    pub fn operations(&self) -> impl Iterator<Item = &AccountOperation> {
        self.operations.values()
    }
    /// Inspect after cancellation or a lost write acknowledgement before resuming.
    pub fn operation(&self, id: ReservationId) -> Option<&AccountOperation> {
        self.operations.get(&id.0)
    }
    fn get_mut(&mut self, id: ReservationId) -> Result<&mut AccountOperation> {
        self.operations
            .get_mut(&id.0)
            .ok_or(StorageError::Transition)
    }
    /// Allocate max(explicit pending observation, retained floor). No cursor rewind
    /// or ID reuse; callers must persist this result before constructing a payment.
    pub fn reserve_nonce(&mut self, id: ReservationId, remote_pending: u64) -> Result<u64> {
        if self.operations.contains_key(&id.0) {
            return Err(StorageError::Conflict);
        }
        if self.operations.len() >= MAX_ACCOUNT_OPERATIONS {
            return Err(StorageError::Invalid);
        }
        let nonce = self.next_nonce.max(remote_pending);
        let next = nonce.checked_add(1).ok_or(StorageError::Overflow)?;
        let op = AccountOperation {
            id,
            nonce,
            state: ReservationState::Reserved,
            transaction: None,
            payload: None,
            replacements: vec![],
            inclusion: None,
        };
        self.install(op, next)?;
        Ok(nonce)
    }
    fn install(&mut self, op: AccountOperation, next_nonce: u64) -> Result<()> {
        let mut candidate = self.clone();
        candidate.operations.insert(op.id.0, op);
        candidate.next_nonce = next_nonce;
        candidate.export_state()?;
        *self = candidate;
        Ok(())
    }
    /// Release only a never-signed operation, retaining its ID and exact nonce.
    pub fn release_unsigned(&mut self, id: ReservationId) -> Result<()> {
        let op = self.get_mut(id)?;
        if op.state != ReservationState::Reserved {
            return Err(StorageError::Transition);
        }
        op.state = ReservationState::Released;
        Ok(())
    }
    /// Explicit nonce-gap repair under the original ID; never reopens signed custody.
    pub fn reopen_unsigned(&mut self, id: ReservationId) -> Result<()> {
        let op = self.get_mut(id)?;
        if op.state != ReservationState::Released {
            return Err(StorageError::Transition);
        }
        op.state = ReservationState::Reserved;
        Ok(())
    }
    fn checked_payload(&self, nonce: u64, payload: &[u8]) -> Result<Hash32> {
        let signed = SignedQuaiTransaction::decode(payload).map_err(|_| StorageError::Invalid)?;
        if signed.from() != self.address
            || signed.transaction().nonce != nonce
            || signed.transaction().chain_id != self.scope.chain_id
        {
            return Err(StorageError::Conflict);
        }
        signed.hash().map_err(|_| StorageError::Invalid)
    }
    /// Retain exact verified root bytes before exposing them. Repeating identical
    /// bytes is idempotent; a different root or released operation conflicts.
    pub fn commit_signed(
        &mut self,
        id: ReservationId,
        signed: &SignedQuaiTransaction,
    ) -> Result<()> {
        let mut op = self.operation(id).ok_or(StorageError::Transition)?.clone();
        let payload = signed.signed_bytes().map_err(|_| StorageError::Invalid)?;
        let hash = self.checked_payload(op.nonce, &payload)?;
        if op.state == ReservationState::Released || op.transaction.is_some_and(|old| old != hash) {
            return Err(StorageError::Transition);
        }
        if let Some(old) = &op.payload {
            return if *old == payload {
                Ok(())
            } else {
                Err(StorageError::Conflict)
            };
        }
        op.payload = Some(payload);
        op.transaction = Some(hash);
        if op.state == ReservationState::Reserved {
            op.state = ReservationState::Signed;
        }
        self.install(op, self.next_nonce)
    }
    /// Append a same-sender/nonce fee-only candidate before exposure. No pool bump
    /// policy is inferred. Original bytes and all prior candidates remain held.
    pub fn commit_replacement(
        &mut self,
        id: ReservationId,
        parent: Hash32,
        signed: &SignedQuaiTransaction,
    ) -> Result<()> {
        let mut op = self.operation(id).ok_or(StorageError::Transition)?.clone();
        if !matches!(
            op.state,
            ReservationState::Signed | ReservationState::Submitted
        ) {
            return Err(StorageError::Transition);
        }
        let root = op.payload.as_deref().ok_or(StorageError::Invalid)?;
        let new = QuaiReplacement {
            parent,
            payload: signed.signed_bytes().map_err(|_| StorageError::Invalid)?,
        };
        if op.replacements.contains(&new) {
            return Ok(());
        }
        op.replacements.push(new);
        crate::state::replacements::validate_family(root, &op.replacements)?;
        self.install(op, self.next_nonce)
    }
    /// Record a submission attempt before dispatch. Failed/ambiguous sends never
    /// release custody, and this method does not broadcast or retry anything.
    pub fn mark_submitted(&mut self, id: ReservationId) -> Result<()> {
        let op = self.get_mut(id)?;
        if op.state == ReservationState::Submitted {
            return Ok(());
        }
        if op.state != ReservationState::Signed {
            return Err(StorageError::Transition);
        }
        op.state = ReservationState::Submitted;
        Ok(())
    }
    fn contains_candidate(op: &AccountOperation, hash: Hash32) -> bool {
        op.transaction == Some(hash)
            || op.replacements.iter().any(|v| {
                SignedQuaiTransaction::decode(&v.payload)
                    .and_then(|s| s.hash())
                    .is_ok_and(|h| h == hash)
            })
    }
    /// Record an independently verified exact candidate/block observation. A durable
    /// caller must fence against the revision captured before asynchronous reads.
    pub fn observe_inclusion(
        &mut self,
        id: ReservationId,
        hash: Hash32,
        checkpoint: Checkpoint,
    ) -> Result<()> {
        let mut op = self.operation(id).ok_or(StorageError::Transition)?.clone();
        if op.state == ReservationState::Confirmed && op.inclusion == Some((hash, checkpoint)) {
            return Ok(());
        }
        if !matches!(
            op.state,
            ReservationState::Signed | ReservationState::Submitted
        ) || !Self::contains_candidate(&op, hash)
            || checkpoint.hash == Hash32::ZERO
        {
            return Err(StorageError::Transition);
        }
        op.state = ReservationState::Confirmed;
        op.inclusion = Some((hash, checkpoint));
        self.install(op, self.next_nonce)
    }
    /// Remove the exact lost-canonicality observation; preserve every signed claim.
    pub fn invalidate_inclusion(
        &mut self,
        id: ReservationId,
        expected: (Hash32, Checkpoint),
    ) -> Result<()> {
        let op = self.get_mut(id)?;
        if op.state != ReservationState::Confirmed || op.inclusion != Some(expected) {
            return Err(StorageError::StaleSnapshot);
        }
        op.state = ReservationState::Submitted;
        op.inclusion = None;
        Ok(())
    }
    /// Initialize from authenticated custody for this exact owned account. The
    /// backup need not be current; never overwrite a newer live journal with it.
    pub fn from_backup(
        backup: &crate::full_backup::WalletBackup,
        scope: NetworkScope,
        owner: PublicKey,
    ) -> Result<Self> {
        let state = backup.scope_state(scope).ok_or(StorageError::Invalid)?;
        if !state
            .addresses()
            .iter()
            .any(|a| *a.public_key() == owner.to_compressed())
        {
            return Err(StorageError::Invalid);
        }
        let mut book = Self::new(scope, owner, 0)?;
        book.next_nonce = state
            .nonce_cursors()
            .find(|(a, _)| *a == book.address)
            .map_or(0, |(_, n)| n);
        for source in state.operations() {
            let Some((address, nonce)) = source.nonce else {
                continue;
            };
            if address != book.address {
                continue;
            }
            let mut status = source.reservation.state;
            // A backup's inclusion cannot establish current canonicality.
            if status == ReservationState::Confirmed {
                status = ReservationState::Submitted;
            }
            let op = AccountOperation {
                id: source.reservation.id,
                nonce,
                state: status,
                transaction: source.reservation.transaction,
                payload: source.signed_payload.map(<[u8]>::to_vec),
                replacements: source.replacements.to_vec(),
                inclusion: None,
            };
            if book.operations.insert(op.id.0, op).is_some() {
                return Err(StorageError::Conflict);
            }
        }
        book.validate()?;
        book.export_state()?;
        Ok(book)
    }
    /// Atomically union authenticated account custody without dropping IDs, signed
    /// bytes, candidates or nonce floors. Unsigned backup states never release or
    /// reopen a live operation; signed evidence always preserves a held claim.
    /// Conflicting nonce assignments, root bytes/hashes or candidate families reject
    /// the entire merge. Successful merges invalidate every live inclusion.
    pub fn merge_backup(
        &mut self,
        backup: &crate::full_backup::WalletBackup,
    ) -> Result<AccountMergeReport> {
        let incoming = Self::from_backup(backup, self.scope, self.owner)?;
        let mut candidate = self.clone();
        let mut report = AccountMergeReport {
            operations_added: 0,
            candidates_added: 0,
            invalidated_inclusions: 0,
            next_nonce: self.next_nonce.max(incoming.next_nonce),
        };
        candidate.next_nonce = report.next_nonce;
        for source in incoming.operations.values() {
            let Some(live) = candidate.operations.get_mut(&source.id.0) else {
                report.operations_added += 1;
                report.candidates_added += source.replacements.len();
                candidate.operations.insert(source.id.0, source.clone());
                continue;
            };
            if live.nonce != source.nonce
                || matches!((live.transaction, source.transaction), (Some(a), Some(b)) if a != b)
                || matches!((&live.payload, &source.payload), (Some(a), Some(b)) if a != b)
            {
                return Err(StorageError::Conflict);
            }
            live.transaction = live.transaction.or(source.transaction);
            if live.payload.is_none() {
                live.payload = source.payload.clone();
            }
            for edge in &source.replacements {
                if !live.replacements.contains(edge) {
                    live.replacements.push(edge.clone());
                    report.candidates_added += 1;
                }
            }
            if live.transaction.is_some() {
                live.state = if matches!(
                    live.state,
                    ReservationState::Submitted | ReservationState::Confirmed
                ) || source.state == ReservationState::Submitted
                {
                    ReservationState::Submitted
                } else {
                    ReservationState::Signed
                };
            }
        }
        for op in candidate.operations.values_mut() {
            if op.inclusion.take().is_some() {
                report.invalidated_inclusions += 1;
            }
            if op.state == ReservationState::Confirmed {
                op.state = ReservationState::Submitted;
            }
        }
        candidate.validate()?;
        candidate.export_state()?;
        *self = candidate;
        Ok(report)
    }
    fn validate(&self) -> Result<()> {
        if self.operations.len() > MAX_ACCOUNT_OPERATIONS {
            return Err(StorageError::Invalid);
        }
        let mut nonces = BTreeSet::new();
        for op in self.operations.values() {
            if op.nonce >= self.next_nonce || !nonces.insert(op.nonce) {
                return Err(StorageError::Invalid);
            }
            let signed = matches!(
                op.state,
                ReservationState::Signed
                    | ReservationState::Submitted
                    | ReservationState::Confirmed
            );
            if signed != op.transaction.is_some()
                || (op.state == ReservationState::Confirmed) != op.inclusion.is_some()
                || (!signed && (op.payload.is_some() || !op.replacements.is_empty()))
            {
                return Err(StorageError::Invalid);
            }
            if let Some(payload) = &op.payload {
                if Some(self.checked_payload(op.nonce, payload)?) != op.transaction {
                    return Err(StorageError::Invalid);
                }
                crate::state::replacements::validate_family(payload, &op.replacements)?;
            } else if !op.replacements.is_empty() {
                return Err(StorageError::Invalid);
            }
            if let Some((hash, block)) = op.inclusion
                && (block.hash == Hash32::ZERO || !Self::contains_candidate(op, hash))
            {
                return Err(StorageError::Invalid);
            }
        }
        Ok(())
    }
    /// Deterministic public custody encoding. This contains broadcastable signed
    /// bytes; it is neither encrypted nor an authenticated backup of private keys.
    pub fn export_state(&self) -> Result<Vec<u8>> {
        let mut out = Writer(Vec::new());
        out.put(MAGIC)?;
        out.put(&self.scope.key())?;
        out.put(&self.owner.to_compressed())?;
        out.put(&self.next_nonce.to_be_bytes())?;
        out.put(&(self.operations.len() as u16).to_be_bytes())?;
        for op in self.operations.values() {
            out.put(&op.id.0)?;
            out.put(&op.nonce.to_be_bytes())?;
            out.put(&[op.state as u8])?;
            out.put(&[u8::from(op.transaction.is_some())])?;
            if let Some(hash) = op.transaction {
                out.put(hash.bytes())?;
            }
            out.put(&[u8::from(op.inclusion.is_some())])?;
            if let Some((hash, block)) = op.inclusion {
                out.put(hash.bytes())?;
                out.put(block.hash.bytes())?;
                out.put(&block.height.to_be_bytes::<32>())?;
            }
            out.blob(op.payload.as_deref().unwrap_or_default())?;
            out.put(&[op.replacements.len() as u8])?;
            for edge in &op.replacements {
                out.put(edge.parent.bytes())?;
                out.blob(&edge.payload)?;
            }
        }
        Ok(out.0)
    }
    /// Strict bounded import, rechecking every signature, nonce, family and scope.
    /// Public bytes do not authenticate freshness; use a trusted live revision or
    /// authenticated recovery process, never a last-writer-wins overwrite.
    pub fn from_state(bytes: &[u8], scope: NetworkScope, owner: PublicKey) -> Result<Self> {
        if bytes.len() > MAX_ACCOUNT_CUSTODY_BYTES {
            return Err(StorageError::Invalid);
        }
        let mut r = Reader(bytes);
        if r.take(8)? != MAGIC || r.take(65)? != scope.key() || r.take(33)? != owner.to_compressed()
        {
            return Err(StorageError::Invalid);
        }
        let mut book = Self::new(scope, owner, u64::from_be_bytes(r.array()?))?;
        let count = u16::from_be_bytes(r.array()?) as usize;
        if count > MAX_ACCOUNT_OPERATIONS {
            return Err(StorageError::Invalid);
        }
        let mut previous = None;
        for _ in 0..count {
            let id = ReservationId(r.array()?);
            if previous.is_some_and(|old| old >= id.0) {
                return Err(StorageError::Invalid);
            }
            previous = Some(id.0);
            let nonce = u64::from_be_bytes(r.array()?);
            let state = match r.byte()? {
                0 => ReservationState::Reserved,
                1 => ReservationState::Signed,
                2 => ReservationState::Submitted,
                3 => ReservationState::Confirmed,
                4 => ReservationState::Released,
                _ => return Err(StorageError::Invalid),
            };
            let transaction = if r.flag()? {
                Some(Hash32::from_bytes(r.array()?))
            } else {
                None
            };
            let inclusion = if r.flag()? {
                Some((
                    Hash32::from_bytes(r.array()?),
                    Checkpoint {
                        hash: Hash32::from_bytes(r.array()?),
                        height: U256::from_be_bytes(r.array::<32>()?),
                    },
                ))
            } else {
                None
            };
            let payload = r.blob()?;
            let payload = if payload.is_empty() {
                None
            } else {
                Some(payload.to_vec())
            };
            let count = r.byte()?;
            if count > 32 {
                return Err(StorageError::Invalid);
            }
            let mut replacements = Vec::new();
            for _ in 0..count {
                replacements.push(QuaiReplacement {
                    parent: Hash32::from_bytes(r.array()?),
                    payload: r.blob()?.to_vec(),
                });
            }
            book.operations.insert(
                id.0,
                AccountOperation {
                    id,
                    nonce,
                    state,
                    transaction,
                    payload,
                    replacements,
                    inclusion,
                },
            );
        }
        if !r.0.is_empty() {
            return Err(StorageError::Invalid);
        }
        book.validate()?;
        Ok(book)
    }
}
