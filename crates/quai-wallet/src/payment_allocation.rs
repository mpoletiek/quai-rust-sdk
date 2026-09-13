//! Public, bounded payment-code allocation journal. Persist ranges before search
//! and completions before exposure; the browser adapter supplies atomic commits.
use crate::discovery::{IndexRange, NetworkScope};
use crate::metadata::StorageError;
use quai_crypto::PublicKey;
use quai_payments::{PaymentChannel, PaymentCode, PaymentDirection, PrivatePaymentCode};
use quai_primitives::{Hash32, QiAddress, Zone};
use std::collections::{BTreeMap, BTreeSet};

/// Maximum retained IDs, including abandoned requests. No automatic reclamation.
pub const MAX_PAYMENT_ALLOCATIONS: usize = 1024;
const HEADER: usize = 115;
/// Maximum canonical journal length, with every request completed.
pub const MAX_PAYMENT_ALLOCATION_BYTES: usize = HEADER + 29 * MAX_PAYMENT_ALLOCATIONS;
/// Caller-generated identity retained across cancellation, restart and abandonment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PaymentAllocationId(pub [u8; 16]);
/// Public exposure; its owner/peer/network context belongs to the enclosing book.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaymentAddressRecord {
    /// Send or receive relative to the local owner.
    pub direction: PaymentDirection,
    /// Address zone.
    pub zone: Zone,
    /// Exact raw payment child index.
    pub index: u32,
    /// Validated Qi address.
    pub address: QiAddress,
    /// Matching full public point.
    pub public_key: PublicKey,
    /// Entire consumed raw interval.
    pub burned: IndexRange,
}
/// Monotonic state of a consumed raw interval.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaymentAllocationStatus {
    /// Range committed; search/completion may resume.
    Pending,
    /// Exact public exposure selected. A send destination is not locally spendable.
    Completed(PaymentAddressRecord),
    /// Range and ID remain consumed permanently.
    Abandoned,
}
/// One allocation request, inspected through immutable references.
#[derive(Clone, Debug)]
pub struct PaymentAllocation {
    /// Stable caller identity.
    pub id: PaymentAllocationId,
    /// Whole consumed interval, including unexamined candidates.
    pub range: IndexRange,
    /// Pending, completed or abandoned.
    pub status: PaymentAllocationStatus,
}
/// One network/zone, owner account, peer and direction. Contains no private keys.
/// Independent directions/zones have distinct identities and cursor histories.
#[derive(Clone)]
pub struct PaymentAllocationBook {
    scope: NetworkScope,
    local: PaymentCode,
    peer: PaymentCode,
    account: u32,
    direction: PaymentDirection,
    identity: Hash32,
    floor: u32,
    next: u32,
    allocations: BTreeMap<PaymentAllocationId, PaymentAllocation>,
}
impl core::fmt::Debug for PaymentAllocationBook {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("PaymentAllocationBook([REDACTED])")
    }
}
impl PaymentAllocationBook {
    /// Start at an explicit prior raw cursor, including all exposed and burned
    /// indexes. `2^31` means exhausted. Empty latest UTXOs never justify zero.
    pub fn new(
        scope: NetworkScope,
        owner: &PrivatePaymentCode,
        peer: PaymentCode,
        direction: PaymentDirection,
        first_index: u32,
    ) -> Result<Self, StorageError> {
        if scope.chain_id == quai_consensus::U256::ZERO
            || scope.genesis == Hash32::ZERO
            || first_index > 1 << 31
        {
            return Err(StorageError::Invalid);
        }
        Ok(Self {
            scope,
            local: owner.public_code().clone(),
            account: owner.account(),
            identity: Self::channel_identity(
                owner.public_code(),
                owner.account(),
                &peer,
                direction,
            ),
            peer,
            direction,
            floor: first_index,
            next: first_index,
            allocations: BTreeMap::new(),
        })
    }
    /// Initialize from retained channel cursors, proving the local private owner.
    /// The caller must establish the channel's network and freshness independently;
    /// this method performs no merge and cannot replace a newer live journal.
    pub fn from_channel(
        scope: NetworkScope,
        owner: &PrivatePaymentCode,
        channel: &PaymentChannel,
        direction: PaymentDirection,
    ) -> Result<Self, StorageError> {
        if channel.local_code() != owner.public_code() || channel.account() != owner.account() {
            return Err(StorageError::Invalid);
        }
        Self::new(
            scope,
            owner,
            channel.counterparty_code().clone(),
            direction,
            channel.next_index(direction, scope.zone).unwrap_or(1 << 31),
        )
    }
    /// Initialize from the exact authenticated backup channel and network. Earlier
    /// exposures remain in that backup; live journals must never be overwritten.
    #[cfg(feature = "backup")]
    pub fn from_backup(
        backup: &crate::full_backup::WalletBackup,
        scope: NetworkScope,
        owner: &PrivatePaymentCode,
        peer: PaymentCode,
        direction: PaymentDirection,
    ) -> Result<Self, StorageError> {
        let network = scope.key();
        let channel = backup
            .payment_channels()
            .find(|c| {
                c.network_key()[..] == network[..64]
                    && *c.local_code_bytes() == owner.public_code().to_bytes()
                    && *c.peer_code_bytes() == peer.to_bytes()
                    && c.account() == owner.account()
            })
            .ok_or(StorageError::Invalid)?;
        Self::from_channel(
            scope,
            owner,
            &channel.channel(owner).map_err(|_| StorageError::Invalid)?,
            direction,
        )
    }
    /// Public namespace; network/zone is supplied separately to the storage layer.
    pub fn channel_identity(
        local: &PaymentCode,
        account: u32,
        peer: &PaymentCode,
        direction: PaymentDirection,
    ) -> Hash32 {
        let mut bytes = b"quai-rust/payment-allocation/v1".to_vec();
        bytes.extend(local.to_bytes());
        bytes.extend(account.to_be_bytes());
        bytes.extend(peer.to_bytes());
        bytes.push(match direction {
            PaymentDirection::Send => 0,
            PaymentDirection::Receive => 1,
        });
        quai_crypto::keccak256(&bytes).into()
    }
    /// Exact network and zone.
    pub fn scope(&self) -> NetworkScope {
        self.scope
    }
    /// Public namespace bound to owner account, peer and direction.
    pub fn identity(&self) -> Hash32 {
        self.identity
    }
    /// Public local owner code.
    pub fn local_code(&self) -> &PaymentCode {
        &self.local
    }
    /// Exact local account number.
    pub fn account(&self) -> u32 {
        self.account
    }
    /// Public peer code.
    pub fn peer(&self) -> &PaymentCode {
        &self.peer
    }
    /// Send or receive relative to the local owner.
    pub fn direction(&self) -> PaymentDirection {
        self.direction
    }
    /// First unconsumed raw index; `2^31` denotes exhaustion.
    pub fn next_index(&self) -> u32 {
        self.next
    }
    /// Inspect a request after restart or ambiguous cancellation.
    pub fn allocation(&self, id: PaymentAllocationId) -> Option<&PaymentAllocation> {
        self.allocations.get(&id)
    }
    /// Retained records in ID order, including abandoned ranges.
    pub fn allocations(&self) -> impl Iterator<Item = &PaymentAllocation> {
        self.allocations.values()
    }
    fn check_owner(&self, owner: &PrivatePaymentCode) -> Result<(), StorageError> {
        if self.local != *owner.public_code() || self.account != owner.account() {
            return Err(StorageError::Invalid);
        }
        Ok(())
    }
    /// Burn an entire range in memory. Persist it before any secret-assisted search.
    /// Reused IDs always conflict, including completed and abandoned requests.
    pub fn reserve(
        &mut self,
        id: PaymentAllocationId,
        max_attempts: u32,
    ) -> Result<(), StorageError> {
        if self.allocations.contains_key(&id) {
            return Err(StorageError::Conflict);
        }
        if self.allocations.len() >= MAX_PAYMENT_ALLOCATIONS
            || !(1..=100_000).contains(&max_attempts)
        {
            return Err(StorageError::Invalid);
        }
        if self.next >= 1 << 31 {
            return Err(StorageError::DerivationExhausted);
        }
        let range = IndexRange {
            start: self.next,
            end: self.next.saturating_add(max_attempts).min(1 << 31),
        };
        self.allocations.insert(
            id,
            PaymentAllocation {
                id,
                range,
                status: PaymentAllocationStatus::Pending,
            },
        );
        self.next = range.end;
        Ok(())
    }
    fn exposure(
        &self,
        owner: &PrivatePaymentCode,
        range: IndexRange,
        index: u32,
    ) -> Result<PaymentAddressRecord, StorageError> {
        self.check_owner(owner)?;
        if index < range.start || index >= range.end {
            return Err(StorageError::Invalid);
        }
        let public_key = match self.direction {
            PaymentDirection::Send => owner.send_public_key(&self.peer, index),
            PaymentDirection::Receive => owner.receive_public_key(&self.peer, index),
        }
        .map_err(|_| StorageError::Invalid)?;
        let address =
            QiAddress::try_from(public_key.address()).map_err(|_| StorageError::Invalid)?;
        if address.zone() != self.scope.zone {
            return Err(StorageError::Invalid);
        }
        Ok(PaymentAddressRecord {
            direction: self.direction,
            zone: self.scope.zone,
            index,
            address,
            public_key,
            burned: range,
        })
    }
    /// Verify owner and exact child, then complete in memory. Persist the journal
    /// before returning this destination to another component or peer.
    pub fn complete(
        &mut self,
        owner: &PrivatePaymentCode,
        id: PaymentAllocationId,
        index: u32,
    ) -> Result<PaymentAddressRecord, StorageError> {
        self.check_owner(owner)?;
        let allocation = self.allocations.get(&id).ok_or(StorageError::Invalid)?;
        if let PaymentAllocationStatus::Completed(record) = &allocation.status {
            return if record.index == index {
                Ok(record.clone())
            } else {
                Err(StorageError::Transition)
            };
        }
        if !matches!(allocation.status, PaymentAllocationStatus::Pending)
            || index < allocation.range.start
            || index >= allocation.range.end
        {
            return Err(StorageError::Transition);
        }
        let record = self.exposure(owner, allocation.range, index)?;
        if self.allocations.values().any(|a| matches!(&a.status, PaymentAllocationStatus::Completed(old) if old.address == record.address)) {
            return Err(StorageError::Conflict);
        }
        self.allocations
            .get_mut(&id)
            .ok_or(StorageError::Invalid)?
            .status = PaymentAllocationStatus::Completed(record.clone());
        Ok(record)
    }
    /// Abandon only a pending request. Neither cursor nor retained ID is reclaimed.
    pub fn abandon(&mut self, id: PaymentAllocationId) -> Result<(), StorageError> {
        let a = self.allocations.get_mut(&id).ok_or(StorageError::Invalid)?;
        if !matches!(a.status, PaymentAllocationStatus::Pending) {
            return Err(StorageError::Transition);
        }
        a.status = PaymentAllocationStatus::Abandoned;
        Ok(())
    }
    /// Canonical public metadata. Payment relationships are privacy sensitive;
    /// these bytes contain no secret and provide no authentication or freshness.
    pub fn export_state(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(HEADER + 29 * self.allocations.len());
        bytes.extend(b"QPAYABK1");
        bytes.extend(self.scope.key());
        bytes.extend(self.identity.bytes());
        bytes.extend(self.floor.to_be_bytes());
        bytes.extend(self.next.to_be_bytes());
        bytes.extend((self.allocations.len() as u16).to_be_bytes());
        for a in self.allocations.values() {
            bytes.extend(a.id.0);
            bytes.extend(a.range.start.to_be_bytes());
            bytes.extend(a.range.end.to_be_bytes());
            match &a.status {
                PaymentAllocationStatus::Pending => bytes.push(0),
                PaymentAllocationStatus::Abandoned => bytes.push(2),
                PaymentAllocationStatus::Completed(record) => {
                    bytes.push(1);
                    bytes.extend(record.index.to_be_bytes());
                }
            }
        }
        bytes
    }
    /// Bounded exact import against independently supplied context and private
    /// owner. Every completed child is re-derived; no claimed point is trusted.
    /// Revision fencing and protection from rollback belong to the storage layer.
    pub fn from_state(
        bytes: &[u8],
        scope: NetworkScope,
        owner: &PrivatePaymentCode,
        peer: PaymentCode,
        direction: PaymentDirection,
    ) -> Result<Self, StorageError> {
        if bytes.len() < HEADER
            || bytes.len() > MAX_PAYMENT_ALLOCATION_BYTES
            || &bytes[..8] != b"QPAYABK1"
            || bytes[8..73] != scope.key()
            || bytes[73..105]
                != *Self::channel_identity(owner.public_code(), owner.account(), &peer, direction)
                    .bytes()
        {
            return Err(StorageError::Invalid);
        }
        fn number(bytes: &[u8], at: usize) -> Result<u32, StorageError> {
            Ok(u32::from_be_bytes(
                bytes
                    .get(at..at + 4)
                    .ok_or(StorageError::Invalid)?
                    .try_into()
                    .map_err(|_| StorageError::Invalid)?,
            ))
        }
        let mut book = Self::new(scope, owner, peer, direction, number(bytes, 105)?)?;
        let expected = number(bytes, 109)?;
        let count = usize::from(u16::from_be_bytes(
            bytes[113..115]
                .try_into()
                .map_err(|_| StorageError::Invalid)?,
        ));
        if expected > 1 << 31
            || count > MAX_PAYMENT_ALLOCATIONS
            || count > (bytes.len() - HEADER) / 25
        {
            return Err(StorageError::Invalid);
        }
        let mut at = HEADER;
        let mut prior = None;
        let mut ranges = Vec::with_capacity(count);
        let mut addresses = BTreeSet::new();
        for _ in 0..count {
            let fixed = bytes.get(at..at + 25).ok_or(StorageError::Invalid)?;
            let id =
                PaymentAllocationId(fixed[..16].try_into().map_err(|_| StorageError::Invalid)?);
            if prior.is_some_and(|old| id <= old) {
                return Err(StorageError::Invalid);
            }
            prior = Some(id);
            let range = IndexRange {
                start: number(fixed, 16)?,
                end: number(fixed, 20)?,
            };
            if range.start >= range.end || range.end > 1 << 31 || range.end - range.start > 100_000
            {
                return Err(StorageError::Invalid);
            }
            at += 25;
            let status = match fixed[24] {
                0 => PaymentAllocationStatus::Pending,
                2 => PaymentAllocationStatus::Abandoned,
                1 => {
                    let index = number(bytes, at)?;
                    at += 4;
                    let record = book.exposure(owner, range, index)?;
                    if !addresses.insert(record.address) {
                        return Err(StorageError::Invalid);
                    }
                    PaymentAllocationStatus::Completed(record)
                }
                _ => return Err(StorageError::Invalid),
            };
            ranges.push(range);
            book.allocations
                .insert(id, PaymentAllocation { id, range, status });
        }
        if at != bytes.len() {
            return Err(StorageError::Invalid);
        }
        ranges.sort_unstable_by_key(|r| r.start);
        for range in ranges {
            if range.start != book.next {
                return Err(StorageError::Invalid);
            }
            book.next = range.end;
        }
        if book.next != expected {
            return Err(StorageError::Invalid);
        }
        Ok(book)
    }
}
