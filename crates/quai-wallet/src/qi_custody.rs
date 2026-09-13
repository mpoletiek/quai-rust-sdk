//! Portable Qi input custody. Source observations are explicit and never history proofs.
use crate::CandidateCoin;
use crate::custody_codec::{Reader, Writer};
use crate::discovery::{Checkpoint, NetworkScope};
use crate::metadata::{PublicAddress, StorageError};
pub use crate::state::{ReplacementCandidate, ReservationId, ReservationState};
use quai_consensus::{OutPoint, QiTransaction, SignedQiOperation, U256};
use quai_primitives::{Address, Hash32, QiAddress};
use std::collections::{BTreeMap, BTreeSet};
type Result<T> = std::result::Result<T, StorageError>;
/// Retained IDs, including released unsigned requests; no automatic pruning.
pub const MAX_QI_OPERATIONS: usize = 256;
/// Public origin records retained even when unsigned input claims are released.
pub const MAX_QI_CUSTODY_ADDRESSES: usize = 4096;
/// Aggregate public metadata, input claims and signed-candidate byte bound.
pub const MAX_QI_CUSTODY_BYTES: usize = 16 * 1024 * 1024;
const MAGIC: &[u8; 8] = b"QQICUBK1";
/// Exact input and owner held independently of any current UTXO snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QiInputClaim {
    /// Fixed output identity, including the node-supported u16 index.
    pub outpoint: OutPoint,
    /// Public owner whose exact point and origin remain in the book's inventory.
    pub owner: QiAddress,
}
/// Immutable inspection of one operation. Metadata does not grant key ownership.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QiOperation {
    /// Caller-generated unique operation ID; never reuse after release.
    pub id: ReservationId,
    /// Signed claims remain held after errors, ambiguity, confirmation and reorg.
    pub state: ReservationState,
    /// Source checkpoint used for a new reservation, if retained. It is not a
    /// spendability/finality assertion; native backup imports omit observations.
    pub reservation_checkpoint: Option<Checkpoint>,
    /// Complete original unordered owner/outpoint set; empty only after unsigned release.
    pub claims: Vec<QiInputClaim>,
    /// Immutable root identity, including imported hash-only custody.
    pub transaction: Option<Hash32>,
    /// Canonical signed transfer, conversion or wrapping bytes, when retained.
    pub payload: Option<Vec<u8>>,
    /// At most 32 same-input candidate edges in dependency order.
    pub replacements: Vec<ReplacementCandidate>,
    /// Caller-observed candidate and block, without any authority to release inputs.
    pub inclusion: Option<(Hash32, Checkpoint)>,
}
/// Counts from a successful monotonic custody merge; observations are invalidated.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct QiMergeReport {
    /// Newly retained public address records.
    pub addresses_added: usize,
    /// Newly retained operation IDs, including released IDs.
    pub operations_added: usize,
    /// Newly retained candidate edges across all operations.
    pub candidates_added: usize,
    /// Live inclusion observations discarded for fresh canonical checks.
    pub invalidated_inclusions: usize,
    /// Live source checkpoints discarded for fresh discovery.
    pub invalidated_checkpoints: usize,
}
/// One network/zone and application-chosen wallet namespace's public Qi custody.
/// Every writer for this logical wallet must use the same namespace. This is not
/// an address allocator, UTXO cache, historical index or encrypted private backup.
#[derive(Clone, Debug)]
pub struct QiOperationBook {
    scope: NetworkScope,
    identity: Hash32,
    addresses: BTreeMap<Address, PublicAddress>,
    operations: BTreeMap<[u8; 16], QiOperation>,
}
impl QiOperationBook {
    /// Initialize only with independently trusted network and stable public wallet
    /// identity. Different namespaces cannot prevent one another from spending.
    pub fn new(scope: NetworkScope, identity: Hash32) -> Result<Self> {
        if scope.chain_id == U256::ZERO || scope.genesis == Hash32::ZERO || identity == Hash32::ZERO
        {
            return Err(StorageError::Invalid);
        }
        Ok(Self {
            scope,
            identity,
            addresses: BTreeMap::new(),
            operations: BTreeMap::new(),
        })
    }
    /// Exact network context; matching chain IDs alone are insufficient.
    pub fn scope(&self) -> NetworkScope {
        self.scope
    }
    /// Application-chosen stable public wallet namespace.
    pub fn identity(&self) -> Hash32 {
        self.identity
    }
    /// Domain-separated storage namespace for the application's stable wallet ID.
    pub fn storage_identity(identity: Hash32) -> Hash32 {
        let mut bytes = b"quai-rust/qi-custody/v1".to_vec();
        bytes.extend(identity.bytes());
        quai_crypto::keccak256(&bytes).into()
    }
    /// Retained public origins; even released unsigned operations do not erase them.
    pub fn addresses(&self) -> impl Iterator<Item = &PublicAddress> {
        self.addresses.values()
    }
    /// Public metadata used for exact local-key resolution.
    pub fn address(&self, address: QiAddress) -> Option<&PublicAddress> {
        self.addresses.get(&address.address())
    }
    /// All IDs, including released operations, in canonical lexicographic order.
    pub fn operations(&self) -> impl Iterator<Item = &QiOperation> {
        self.operations.values()
    }
    /// Inspect this same ID after a cancelled or ambiguous storage write.
    pub fn operation(&self, id: ReservationId) -> Option<&QiOperation> {
        self.operations.get(&id.0)
    }
    /// Retained claims override a source's current unreserved/unspent report.
    pub fn claimed(&self, outpoint: OutPoint) -> bool {
        self.operations
            .values()
            .any(|op| op.claims.iter().any(|c| c.outpoint == outpoint))
    }
    /// Snapshot all held references once for efficient selection over many coins.
    pub fn claimed_outpoints(&self) -> BTreeSet<OutPoint> {
        self.operations
            .values()
            .flat_map(|op| op.claims.iter().map(|c| c.outpoint))
            .collect()
    }
    fn add_address(&mut self, address: PublicAddress) -> Result<()> {
        let qi = QiAddress::try_from(address.address()).map_err(|_| StorageError::Invalid)?;
        if qi.zone() != self.scope.zone {
            return Err(StorageError::Invalid);
        }
        if let Some(old) = self.addresses.get(&address.address()) {
            if old != &address {
                return Err(StorageError::Conflict);
            }
        } else {
            if self.addresses.len() >= MAX_QI_CUSTODY_ADDRESSES {
                return Err(StorageError::Invalid);
            }
            self.addresses.insert(address.address(), address);
        }
        Ok(())
    }
    fn valid_outpoint(&self, claim: &QiInputClaim) -> bool {
        let hash = claim.outpoint.transaction_hash.bytes();
        claim.owner.zone() == self.scope.zone
            && hash[2] == self.scope.zone.byte()
            && hash[3] & 0x80 != 0
            && self.addresses.contains_key(&claim.owner.address())
    }
    /// Consume exact selected outputs after checking supplied fixed denominations,
    /// unlock/expiry bounds, public origins and existing local claims. Observations
    /// remain caller supplied; a durable adapter must fence the revision captured
    /// before discovery and commit before returning. This does not prove unspentness.
    pub fn reserve(
        &mut self,
        id: ReservationId,
        checkpoint: Checkpoint,
        candidate_height: U256,
        coins: &[CandidateCoin],
        owners: &[PublicAddress],
    ) -> Result<()> {
        if self.operations.contains_key(&id.0) {
            return Err(StorageError::Conflict);
        }
        if self.operations.len() >= MAX_QI_OPERATIONS
            || checkpoint.hash == Hash32::ZERO
            || candidate_height < checkpoint.height
            || coins.is_empty()
            || coins.len() > 4096
            || owners.len() > MAX_QI_CUSTODY_ADDRESSES
        {
            return Err(StorageError::Invalid);
        }
        let mut candidate = self.clone();
        let mut unique_owners = BTreeSet::new();
        for owner in owners {
            if !unique_owners.insert(owner.address()) {
                return Err(StorageError::Invalid);
            }
            candidate.add_address(owner.clone())?;
        }
        let held = candidate.claimed_outpoints();
        let mut seen = BTreeSet::new();
        let mut claims = Vec::with_capacity(coins.len());
        for coin in coins {
            let claim = QiInputClaim {
                outpoint: coin.outpoint,
                owner: coin.address,
            };
            if !candidate.valid_outpoint(&claim)
                || !seen.insert(claim.outpoint)
                || coin.reserved
                || candidate_height < coin.unlock_height
                || coin
                    .expires_at
                    .is_some_and(|h| h <= coin.unlock_height || candidate_height >= h)
            {
                return Err(StorageError::Invalid);
            }
            if held.contains(&claim.outpoint) {
                return Err(StorageError::Conflict);
            }
            claims.push(claim);
        }
        claims.sort_by_key(|c| c.outpoint);
        candidate.operations.insert(
            id.0,
            QiOperation {
                id,
                state: ReservationState::Reserved,
                reservation_checkpoint: Some(checkpoint),
                claims,
                transaction: None,
                payload: None,
                replacements: vec![],
                inclusion: None,
            },
        );
        candidate.export_state()?;
        *self = candidate;
        Ok(())
    }
    /// Release only a never-signed operation; retain its ID and all public origins.
    /// Qi IDs cannot be reopened: a new request requires a new explicit observation.
    pub fn release_unsigned(&mut self, id: ReservationId) -> Result<()> {
        let op = self
            .operations
            .get_mut(&id.0)
            .ok_or(StorageError::Transition)?;
        if op.state != ReservationState::Reserved {
            return Err(StorageError::Transition);
        }
        op.state = ReservationState::Released;
        op.claims.clear();
        Ok(())
    }
    /// Check the exact complete unordered claim set and public points before local
    /// signing. Input order is retained in the transaction and aggregate signature.
    pub fn check_inputs(&self, id: ReservationId, tx: &QiTransaction) -> Result<()> {
        let op = self.operation(id).ok_or(StorageError::Transition)?;
        if tx.chain_id != self.scope.chain_id
            || tx.inputs.is_empty()
            || tx.inputs.len() != op.claims.len()
        {
            return Err(StorageError::Conflict);
        }
        let claims: BTreeMap<_, _> = op.claims.iter().map(|c| (c.outpoint, c.owner)).collect();
        let mut seen = BTreeSet::new();
        for input in &tx.inputs {
            let owner = claims
                .get(&input.previous_output)
                .ok_or(StorageError::Conflict)?;
            let metadata = self.address(*owner).ok_or(StorageError::Invalid)?;
            if !seen.insert(input.previous_output)
                || input.public_key.address() != owner.address()
                || input.public_key.to_compressed() != *metadata.public_key()
            {
                return Err(StorageError::Conflict);
            }
        }
        Ok(())
    }
    /// Store exact verified ordinary/conversion/wrapping bytes before exposure.
    /// This checks custody and signatures, not node fee/trim/aggregation placement.
    pub fn commit_signed(&mut self, id: ReservationId, signed: &SignedQiOperation) -> Result<()> {
        self.check_inputs(id, signed.transaction())?;
        let hash = signed.hash().map_err(|_| StorageError::Invalid)?;
        if hash.bytes()[0] != self.scope.zone.byte() {
            return Err(StorageError::Invalid);
        }
        let payload = signed.signed_bytes().map_err(|_| StorageError::Invalid)?;
        let mut op = self.operation(id).ok_or(StorageError::Transition)?.clone();
        if op.state == ReservationState::Released || op.transaction.is_some_and(|h| h != hash) {
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
        self.install(op)
    }
    fn install(&mut self, op: QiOperation) -> Result<()> {
        let mut candidate = self.clone();
        candidate.operations.insert(op.id.0, op);
        candidate.export_state()?;
        *self = candidate;
        Ok(())
    }
    /// Persist an explicitly reviewed same-input/data, lower-output-value candidate.
    /// Original claims remain unchanged; at most 32 edges are retained.
    pub fn commit_replacement(
        &mut self,
        id: ReservationId,
        parent: Hash32,
        signed: &SignedQiOperation,
    ) -> Result<()> {
        let mut op = self.operation(id).ok_or(StorageError::Transition)?.clone();
        if !matches!(
            op.state,
            ReservationState::Signed | ReservationState::Submitted
        ) {
            return Err(StorageError::Transition);
        }
        let edge = ReplacementCandidate {
            parent,
            payload: signed.signed_bytes().map_err(|_| StorageError::Invalid)?,
        };
        if op.replacements.contains(&edge) {
            return Ok(());
        }
        op.replacements.push(edge);
        crate::state::replacements::validate_family(
            op.payload.as_deref().ok_or(StorageError::Invalid)?,
            &op.replacements,
        )?;
        self.install(op)
    }
    /// Mark an attempted broadcast before dispatch. No send/retry occurs here.
    pub fn mark_submitted(&mut self, id: ReservationId) -> Result<()> {
        let op = self
            .operations
            .get_mut(&id.0)
            .ok_or(StorageError::Transition)?;
        if op.state == ReservationState::Submitted {
            return Ok(());
        }
        if op.state != ReservationState::Signed {
            return Err(StorageError::Transition);
        }
        op.state = ReservationState::Submitted;
        Ok(())
    }
    fn contains_candidate(op: &QiOperation, hash: Hash32) -> bool {
        op.transaction == Some(hash)
            || op.replacements.iter().any(|e| {
                SignedQiOperation::decode(&e.payload)
                    .and_then(|s| s.hash())
                    .is_ok_and(|h| h == hash)
            })
    }
    /// Record one independently verified candidate/block, under an external durable
    /// revision fence. Confirmation never releases signed outpoint claims.
    pub fn observe_inclusion(
        &mut self,
        id: ReservationId,
        hash: Hash32,
        block: Checkpoint,
    ) -> Result<()> {
        let mut op = self.operation(id).ok_or(StorageError::Transition)?.clone();
        if op.state == ReservationState::Confirmed && op.inclusion == Some((hash, block)) {
            return Ok(());
        }
        if !matches!(
            op.state,
            ReservationState::Signed | ReservationState::Submitted
        ) || !Self::contains_candidate(&op, hash)
            || block.hash == Hash32::ZERO
        {
            return Err(StorageError::Transition);
        }
        op.state = ReservationState::Confirmed;
        op.inclusion = Some((hash, block));
        self.install(op)
    }
    /// Remove only the exact lost-canonicality observation, retaining every input.
    pub fn invalidate_inclusion(
        &mut self,
        id: ReservationId,
        expected: (Hash32, Checkpoint),
    ) -> Result<()> {
        let op = self
            .operations
            .get_mut(&id.0)
            .ok_or(StorageError::Transition)?;
        if op.state != ReservationState::Confirmed || op.inclusion != Some(expected) {
            return Err(StorageError::StaleSnapshot);
        }
        op.state = ReservationState::Submitted;
        op.inclusion = None;
        Ok(())
    }
    /// Initialize a new namespace from authenticated native/portable Qi custody.
    /// Public ownership was checked during backup construction/decryption; the
    /// caller selects its stable wallet namespace. Current UTXOs are not restored.
    pub fn from_backup(
        backup: &crate::full_backup::WalletBackup,
        scope: NetworkScope,
        identity: Hash32,
    ) -> Result<Self> {
        let source = backup.scope_state(scope).ok_or(StorageError::Invalid)?;
        let mut book = Self::new(scope, identity)?;
        for address in source.addresses() {
            if QiAddress::try_from(address.address()).is_ok() {
                book.add_address(address.clone())?;
            }
        }
        for op in source.operations().filter(|op| op.nonce.is_none()) {
            if book.operations.len() >= MAX_QI_OPERATIONS {
                return Err(StorageError::Invalid);
            }
            let mut claims = Vec::new();
            for &(outpoint, owner) in op.qi_claims {
                claims.push(QiInputClaim {
                    outpoint,
                    owner: QiAddress::try_from(owner).map_err(|_| StorageError::Invalid)?,
                });
            }
            claims.sort_by_key(|c| c.outpoint);
            let record = QiOperation {
                id: op.reservation.id,
                state: if op.reservation.state == ReservationState::Confirmed {
                    ReservationState::Submitted
                } else {
                    op.reservation.state
                },
                reservation_checkpoint: None,
                claims,
                transaction: op.reservation.transaction,
                payload: op.signed_payload.map(<[u8]>::to_vec),
                replacements: op.replacements.to_vec(),
                inclusion: None,
            };
            if book.operations.insert(record.id.0, record).is_some() {
                return Err(StorageError::Conflict);
            }
        }
        book.validate()?;
        book.export_state()?;
        Ok(book)
    }
    /// Union authenticated Qi custody without dropping live IDs, claims or signed
    /// candidates. Unsigned backups never release or reopen live operations.
    /// Conflicting owners, ID/input/root assignments or capacity reject atomically.
    /// Every successful merge discards observations for fresh reconciliation.
    pub fn merge_backup(
        &mut self,
        backup: &crate::full_backup::WalletBackup,
    ) -> Result<QiMergeReport> {
        let incoming = Self::from_backup(backup, self.scope, self.identity)?;
        let mut candidate = self.clone();
        let mut report = QiMergeReport::default();
        for public in incoming.addresses.values() {
            if !candidate.addresses.contains_key(&public.address()) {
                report.addresses_added += 1;
            }
            candidate.add_address(public.clone())?;
        }
        for source in incoming.operations.values() {
            let Some(live) = candidate.operations.get_mut(&source.id.0) else {
                report.operations_added += 1;
                report.candidates_added += source.replacements.len();
                candidate.operations.insert(source.id.0, source.clone());
                continue;
            };
            if (!live.claims.is_empty()
                && !source.claims.is_empty()
                && live.claims != source.claims)
                || matches!((live.transaction, source.transaction), (Some(a), Some(b)) if a != b)
                || matches!((&live.payload, &source.payload), (Some(a), Some(b)) if a != b)
            {
                return Err(StorageError::Conflict);
            }
            if source.transaction.is_some() && live.claims.is_empty() {
                live.claims = source.claims.clone();
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
            if op.reservation_checkpoint.take().is_some() {
                report.invalidated_checkpoints += 1;
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
        if self.operations.len() > MAX_QI_OPERATIONS
            || self.addresses.len() > MAX_QI_CUSTODY_ADDRESSES
        {
            return Err(StorageError::Invalid);
        }
        let mut held = BTreeSet::new();
        for op in self.operations.values() {
            let signed = matches!(
                op.state,
                ReservationState::Signed
                    | ReservationState::Submitted
                    | ReservationState::Confirmed
            );
            if signed != op.transaction.is_some()
                || (op.state == ReservationState::Released) != op.claims.is_empty()
                || (op.state == ReservationState::Confirmed) != op.inclusion.is_some()
                || (!signed && (op.payload.is_some() || !op.replacements.is_empty()))
                || op.claims.len() > 4096
                || op
                    .reservation_checkpoint
                    .is_some_and(|b| b.hash == Hash32::ZERO)
            {
                return Err(StorageError::Invalid);
            }
            let mut previous = None;
            for claim in &op.claims {
                if !self.valid_outpoint(claim)
                    || !held.insert(claim.outpoint)
                    || previous.is_some_and(|p| p >= claim.outpoint)
                {
                    return Err(StorageError::Invalid);
                }
                previous = Some(claim.outpoint);
            }
            if let Some(payload) = &op.payload {
                let tx = SignedQiOperation::decode(payload).map_err(|_| StorageError::Invalid)?;
                let hash = tx.hash().map_err(|_| StorageError::Invalid)?;
                if Some(hash) != op.transaction || hash.bytes()[0] != self.scope.zone.byte() {
                    return Err(StorageError::Invalid);
                }
                self.check_inputs(op.id, tx.transaction())?;
                crate::state::replacements::validate_family(payload, &op.replacements)?;
            } else if !op.replacements.is_empty() {
                return Err(StorageError::Invalid);
            }
            if let Some((hash, block)) = op.inclusion
                && (!Self::contains_candidate(op, hash) || block.hash == Hash32::ZERO)
            {
                return Err(StorageError::Invalid);
            }
        }
        Ok(())
    }
    /// Bounded public encoding; contains broadcastable signed bytes, not secrets.
    /// It does not authenticate freshness or permit arbitrary live state replacement.
    pub fn export_state(&self) -> Result<Vec<u8>> {
        let mut w = Writer(Vec::new());
        w.put(MAGIC)?;
        w.put(&self.scope.key())?;
        w.put(self.identity.bytes())?;
        w.put(&(self.addresses.len() as u16).to_be_bytes())?;
        for address in self.addresses.values() {
            w.blob(&address.export_metadata())?;
        }
        w.put(&(self.operations.len() as u16).to_be_bytes())?;
        for op in self.operations.values() {
            w.put(&op.id.0)?;
            w.put(&[op.state as u8])?;
            w.put(&[u8::from(op.reservation_checkpoint.is_some())])?;
            if let Some(b) = op.reservation_checkpoint {
                w.put(b.hash.bytes())?;
                w.put(&b.height.to_be_bytes::<32>())?;
            }
            w.put(&(op.claims.len() as u16).to_be_bytes())?;
            for c in &op.claims {
                w.put(c.outpoint.transaction_hash.bytes())?;
                w.put(&c.outpoint.index.to_be_bytes())?;
                w.put(c.owner.bytes())?;
            }
            w.put(&[u8::from(op.transaction.is_some())])?;
            if let Some(h) = op.transaction {
                w.put(h.bytes())?;
            }
            w.put(&[u8::from(op.inclusion.is_some())])?;
            if let Some((h, b)) = op.inclusion {
                w.put(h.bytes())?;
                w.put(b.hash.bytes())?;
                w.put(&b.height.to_be_bytes::<32>())?;
            }
            w.blob(op.payload.as_deref().unwrap_or_default())?;
            w.put(&[op.replacements.len() as u8])?;
            for edge in &op.replacements {
                w.put(edge.parent.bytes())?;
                w.blob(&edge.payload)?;
            }
        }
        Ok(w.0)
    }
    /// Strict import verifies scope, metadata, unique held inputs, every signature
    /// and candidate edge. Current balances/spendability must be observed separately.
    pub fn from_state(bytes: &[u8], scope: NetworkScope, identity: Hash32) -> Result<Self> {
        if bytes.len() > MAX_QI_CUSTODY_BYTES {
            return Err(StorageError::Invalid);
        }
        let mut r = Reader(bytes);
        if r.take(8)? != MAGIC || r.take(65)? != scope.key() || r.take(32)? != identity.bytes() {
            return Err(StorageError::Invalid);
        }
        let mut book = Self::new(scope, identity)?;
        let count = u16::from_be_bytes(r.array()?) as usize;
        if count > MAX_QI_CUSTODY_ADDRESSES {
            return Err(StorageError::Invalid);
        }
        let mut previous = None;
        for _ in 0..count {
            let a = PublicAddress::from_metadata(r.blob()?)?;
            if previous.is_some_and(|p| p >= a.address()) {
                return Err(StorageError::Invalid);
            }
            previous = Some(a.address());
            book.add_address(a)?;
        }
        let count = u16::from_be_bytes(r.array()?) as usize;
        if count > MAX_QI_OPERATIONS {
            return Err(StorageError::Invalid);
        }
        let mut previous = None;
        for _ in 0..count {
            let id = ReservationId(r.array()?);
            if previous.is_some_and(|p| p >= id.0) {
                return Err(StorageError::Invalid);
            }
            previous = Some(id.0);
            let state = match r.byte()? {
                0 => ReservationState::Reserved,
                1 => ReservationState::Signed,
                2 => ReservationState::Submitted,
                3 => ReservationState::Confirmed,
                4 => ReservationState::Released,
                _ => return Err(StorageError::Invalid),
            };
            let reservation_checkpoint = if r.flag()? {
                Some(Checkpoint {
                    hash: Hash32::from_bytes(r.array()?),
                    height: U256::from_be_bytes(r.array::<32>()?),
                })
            } else {
                None
            };
            let n = u16::from_be_bytes(r.array()?) as usize;
            if n > 4096 {
                return Err(StorageError::Invalid);
            }
            let mut claims = Vec::new();
            for _ in 0..n {
                claims.push(QiInputClaim {
                    outpoint: OutPoint {
                        transaction_hash: Hash32::from_bytes(r.array()?),
                        index: u16::from_be_bytes(r.array()?),
                    },
                    owner: QiAddress::try_from(r.array::<20>()?)
                        .map_err(|_| StorageError::Invalid)?,
                });
            }
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
            let n = r.byte()?;
            if n > 32 {
                return Err(StorageError::Invalid);
            }
            let mut replacements = Vec::new();
            for _ in 0..n {
                replacements.push(ReplacementCandidate {
                    parent: Hash32::from_bytes(r.array()?),
                    payload: r.blob()?.to_vec(),
                });
            }
            book.operations.insert(
                id.0,
                QiOperation {
                    id,
                    state,
                    reservation_checkpoint,
                    claims,
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
