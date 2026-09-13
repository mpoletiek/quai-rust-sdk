//! Bounded public journal for durable HD address allocation.
//!
//! Mutations here are in memory. Persist a reserved range before searching and a
//! completed address before exposing it. The SDK browser adapter enforces those
//! two commit boundaries with IndexedDB compare-and-exchange.
use crate::AccountPublic;
use crate::discovery::{IndexRange, NetworkScope};
use crate::metadata::{PublicAddress, StorageError};
use quai_primitives::Hash32;
use std::collections::{BTreeMap, BTreeSet};
/// Maximum allocation IDs retained in one journal, including abandoned ranges.
pub const MAX_ADDRESS_ALLOCATIONS: usize = 4096;
const HEADER: usize = 123;
/// Maximum encoded journal, including all completed addresses.
pub const MAX_ADDRESS_ALLOCATION_BYTES: usize = HEADER + MAX_ADDRESS_ALLOCATIONS * 30;
/// Caller-generated allocation identity. Never reuse it for a different request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct AddressAllocationId(pub [u8; 16]);
/// Monotonic state of one burned raw range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddressAllocationStatus {
    /// The entire range is consumed; exact search/completion may resume.
    Pending,
    /// This exact address was selected for the allocation.
    Completed(PublicAddress),
    /// Explicitly abandoned; its range and ID remain consumed.
    Abandoned,
}
/// Public allocation record; records are returned by immutable reference.
#[derive(Clone, Debug)]
pub struct AddressAllocation {
    /// Stable caller-supplied identity.
    pub id: AddressAllocationId,
    /// Receive (`false`) or change (`true`) branch.
    pub change: bool,
    /// Whole burned interval, including skipped and unexamined indexes.
    pub range: IndexRange,
    /// Pending, completed or abandoned. No transition rewinds a cursor.
    pub status: AddressAllocationStatus,
}
/// One network/zone and caller-trusted account xpub's public allocation journal.
/// This is an address allocator, not a UTXO/nonce store or an encrypted backup.
#[derive(Clone, Debug)]
pub struct AddressAllocationBook {
    scope: NetworkScope,
    account: AccountPublic,
    identity: Hash32,
    floor: [u32; 2],
    next: [u32; 2],
    allocations: BTreeMap<AddressAllocationId, AddressAllocation>,
}
impl AddressAllocationBook {
    /// Start a new journal above every matching burned cursor and owned HD address
    /// in an authenticated backup. Earlier addresses remain in the backup's address
    /// inventory; this journal tracks subsequent allocation requests. It must never
    /// replace a newer live journal. The supplied account must match stored origins.
    #[cfg(feature = "backup")]
    pub fn from_backup(
        backup: &crate::full_backup::WalletBackup,
        scope: NetworkScope,
        account: AccountPublic,
    ) -> Result<Self, StorageError> {
        let state = backup.scope_state(scope).ok_or(StorageError::Invalid)?;
        let expected_xpub = account.export();
        if !backup.origins().iter().any(|origin| {
            origin
                .account_public(account.coin_type(), account.account_index())
                .is_ok_and(|public| public.export() == expected_xpub)
        }) {
            return Err(StorageError::Invalid);
        }
        let mut floor = [0; 2];
        for cursor in state.derivation_cursors() {
            if cursor.coin == account.coin_type() && cursor.account == account.account_index() {
                if cursor.account_xpub != account.export() {
                    return Err(StorageError::Invalid);
                }
                floor[usize::from(cursor.change)] = cursor.next_index;
            }
        }
        for address in state.addresses() {
            if let crate::metadata::KeyOrigin::Bip44 {
                coin,
                account: number,
                change,
                index,
            } = address.origin()
                && coin == account.coin_type()
                && number == account.account_index()
            {
                if PublicAddress::derive(&account, change, index)? != *address {
                    return Err(StorageError::Invalid);
                }
                floor[usize::from(change)] = floor[usize::from(change)].max(index + 1);
            }
        }
        Self::new(scope, account, floor[0], floor[1])
    }
    /// Initialize with explicit first unconsumed receive/change indexes. They must
    /// include all previously exposed addresses and burned ranges. Zero is correct
    /// only for a fresh account branch, not an empty latest-only UTXO scan.
    pub fn new(
        scope: NetworkScope,
        account: AccountPublic,
        first_receive: u32,
        first_change: u32,
    ) -> Result<Self, StorageError> {
        if scope.chain_id == quai_consensus::U256::ZERO
            || scope.genesis == Hash32::ZERO
            || first_receive > 1 << 31
            || first_change > 1 << 31
        {
            return Err(StorageError::Invalid);
        }
        let floor = [first_receive, first_change];
        Ok(Self {
            scope,
            identity: Self::account_identity(&account),
            account,
            floor,
            next: floor,
            allocations: BTreeMap::new(),
        })
    }
    /// Stable public namespace for this exact coin/account/xpub descriptor. The
    /// network/zone is a separate part of the persisted scope. This is not a secret.
    pub fn account_identity(account: &AccountPublic) -> Hash32 {
        let mut bytes = b"quai-rust/address-allocation/v1".to_vec();
        bytes.extend(account.coin_type().number().to_be_bytes());
        bytes.extend(account.account_index().to_be_bytes());
        bytes.extend(account.export().as_bytes());
        quai_crypto::keccak256(&bytes).into()
    }
    /// Exact configured network/zone.
    pub fn scope(&self) -> NetworkScope {
        self.scope
    }
    /// Caller-trusted public account descriptor.
    pub fn account(&self) -> &AccountPublic {
        &self.account
    }
    /// Public account namespace checked by the codec and browser store.
    pub fn identity(&self) -> Hash32 {
        self.identity
    }
    /// First unconsumed raw index; `2^31` denotes exhausted derivation space.
    pub fn next_index(&self, change: bool) -> u32 {
        self.next[usize::from(change)]
    }
    /// Inspect one exact allocation after restart or a cancelled write.
    pub fn allocation(&self, id: AddressAllocationId) -> Option<&AddressAllocation> {
        self.allocations.get(&id)
    }
    /// All retained IDs ordered by ID, not by allocation time.
    pub fn allocations(&self) -> impl Iterator<Item = &AddressAllocation> {
        self.allocations.values()
    }
    /// Burn 1..=100,000 raw indexes in memory. Persist the updated journal before
    /// deriving or exposing an address. Existing IDs always conflict, even completed
    /// or abandoned ones; use explicit resume for the same request.
    pub fn reserve(
        &mut self,
        id: AddressAllocationId,
        change: bool,
        max_attempts: u32,
    ) -> Result<(), StorageError> {
        if self.allocations.contains_key(&id) {
            return Err(StorageError::Conflict);
        }
        if !(1..=100_000).contains(&max_attempts)
            || self.allocations.len() >= MAX_ADDRESS_ALLOCATIONS
        {
            return Err(StorageError::Invalid);
        }
        let branch = usize::from(change);
        let start = self.next[branch];
        if start >= 1 << 31 {
            return Err(StorageError::DerivationExhausted);
        }
        let end = start.saturating_add(max_attempts).min(1 << 31);
        self.allocations.insert(
            id,
            AddressAllocation {
                id,
                change,
                range: IndexRange { start, end },
                status: AddressAllocationStatus::Pending,
            },
        );
        self.next[branch] = end;
        Ok(())
    }
    /// Complete a pending allocation at an exact index in its range. A repeated
    /// completion at the same index is idempotent. Persist before exposing the
    /// returned address; this method itself performs no I/O or generation fencing.
    pub fn complete(
        &mut self,
        id: AddressAllocationId,
        index: u32,
    ) -> Result<PublicAddress, StorageError> {
        let allocation = self.allocations.get(&id).ok_or(StorageError::Invalid)?;
        if let AddressAllocationStatus::Completed(address) = &allocation.status {
            return if matches!(address.origin(),crate::metadata::KeyOrigin::Bip44{index:old,..} if old==index)
            {
                Ok(address.clone())
            } else {
                Err(StorageError::Transition)
            };
        }
        if !matches!(allocation.status, AddressAllocationStatus::Pending)
            || index < allocation.range.start
            || index >= allocation.range.end
        {
            return Err(StorageError::Transition);
        }
        let address = PublicAddress::derive(&self.account, allocation.change, index)?;
        if address
            .address()
            .zone()
            .map_err(|_| StorageError::Invalid)?
            != self.scope.zone
        {
            return Err(StorageError::Invalid);
        }
        if self.allocations.values().any(|a|matches!(&a.status,AddressAllocationStatus::Completed(old) if old.address()==address.address())) { return Err(StorageError::Conflict); }
        self.allocations
            .get_mut(&id)
            .ok_or(StorageError::Invalid)?
            .status = AddressAllocationStatus::Completed(address.clone());
        Ok(address)
    }
    /// Abandon a pending range without reclaiming any index or the allocation ID.
    /// Completed allocations cannot be abandoned or released.
    pub fn abandon(&mut self, id: AddressAllocationId) -> Result<(), StorageError> {
        let allocation = self.allocations.get_mut(&id).ok_or(StorageError::Invalid)?;
        if !matches!(allocation.status, AddressAllocationStatus::Pending) {
            return Err(StorageError::Transition);
        }
        allocation.status = AddressAllocationStatus::Abandoned;
        Ok(())
    }
    /// Canonical bounded public bytes. No private origin, balance or signed claim
    /// is serialized. Completed public keys are re-derived during validated import.
    pub fn export_state(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(HEADER + self.allocations.len() * 30);
        bytes.extend(b"QADDRBK1");
        bytes.extend(self.scope.key());
        bytes.extend(self.identity.bytes());
        for index in self.floor.into_iter().chain(self.next) {
            bytes.extend(index.to_be_bytes());
        }
        bytes.extend((self.allocations.len() as u16).to_be_bytes());
        for allocation in self.allocations.values() {
            bytes.extend(allocation.id.0);
            bytes.push(u8::from(allocation.change));
            bytes.extend(allocation.range.start.to_be_bytes());
            bytes.extend(allocation.range.end.to_be_bytes());
            match &allocation.status {
                AddressAllocationStatus::Pending => bytes.push(0),
                AddressAllocationStatus::Abandoned => bytes.push(2),
                AddressAllocationStatus::Completed(address) => {
                    bytes.push(1);
                    if let crate::metadata::KeyOrigin::Bip44 { index, .. } = address.origin() {
                        bytes.extend(index.to_be_bytes());
                    }
                }
            }
        }
        bytes
    }
    /// Import against an independently supplied scope and trusted account descriptor.
    /// Checks exact bytes, ID ordering, complete nonoverlapping range coverage and
    /// every completed derivation. It cannot establish that a snapshot is the latest;
    /// persistent revision fencing belongs to the selected storage backend.
    pub fn from_state(
        bytes: &[u8],
        scope: NetworkScope,
        account: AccountPublic,
    ) -> Result<Self, StorageError> {
        if bytes.len() < HEADER
            || bytes.len() > MAX_ADDRESS_ALLOCATION_BYTES
            || &bytes[..8] != b"QADDRBK1"
            || bytes[8..73] != scope.key()
            || bytes[73..105] != *Self::account_identity(&account).bytes()
        {
            return Err(StorageError::Invalid);
        }
        fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, StorageError> {
            Ok(u32::from_be_bytes(
                bytes
                    .get(offset..offset + 4)
                    .ok_or(StorageError::Invalid)?
                    .try_into()
                    .map_err(|_| StorageError::Invalid)?,
            ))
        }
        let mut book = Self::new(scope, account, u32_at(bytes, 105)?, u32_at(bytes, 109)?)?;
        let expected_next = [u32_at(bytes, 113)?, u32_at(bytes, 117)?];
        if expected_next.iter().any(|n| *n > 1 << 31) {
            return Err(StorageError::Invalid);
        }
        let count = usize::from(u16::from_be_bytes(
            bytes[121..123]
                .try_into()
                .map_err(|_| StorageError::Invalid)?,
        ));
        if count > MAX_ADDRESS_ALLOCATIONS || count > (bytes.len() - HEADER) / 26 {
            return Err(StorageError::Invalid);
        }
        let mut offset = HEADER;
        let mut prior = None;
        let mut ranges: [Vec<IndexRange>; 2] = [vec![], vec![]];
        let mut addresses = BTreeSet::new();
        for _ in 0..count {
            let fixed = bytes
                .get(offset..offset + 26)
                .ok_or(StorageError::Invalid)?;
            let id =
                AddressAllocationId(fixed[..16].try_into().map_err(|_| StorageError::Invalid)?);
            if prior.is_some_and(|old| id <= old) || fixed[16] > 1 {
                return Err(StorageError::Invalid);
            }
            prior = Some(id);
            let change = fixed[16] == 1;
            let range = IndexRange {
                start: u32_at(fixed, 17)?,
                end: u32_at(fixed, 21)?,
            };
            if range.start >= range.end || range.end > 1 << 31 || range.end - range.start > 100_000
            {
                return Err(StorageError::Invalid);
            }
            offset += 26;
            let status = match fixed[25] {
                0 => AddressAllocationStatus::Pending,
                2 => AddressAllocationStatus::Abandoned,
                1 => {
                    let index = u32_at(bytes, offset)?;
                    offset += 4;
                    if index < range.start || index >= range.end {
                        return Err(StorageError::Invalid);
                    }
                    let address = PublicAddress::derive(&book.account, change, index)?;
                    if address
                        .address()
                        .zone()
                        .map_err(|_| StorageError::Invalid)?
                        != scope.zone
                        || !addresses.insert(address.address())
                    {
                        return Err(StorageError::Invalid);
                    }
                    AddressAllocationStatus::Completed(address)
                }
                _ => return Err(StorageError::Invalid),
            };
            ranges[usize::from(change)].push(range);
            book.allocations.insert(
                id,
                AddressAllocation {
                    id,
                    change,
                    range,
                    status,
                },
            );
        }
        if offset != bytes.len() {
            return Err(StorageError::Invalid);
        }
        for (branch, ranges) in ranges.iter_mut().enumerate() {
            ranges.sort_unstable_by_key(|r| r.start);
            let mut next = book.floor[branch];
            for range in ranges {
                if range.start != next {
                    return Err(StorageError::Invalid);
                }
                next = range.end;
            }
            if next != expected_next[branch] {
                return Err(StorageError::Invalid);
            }
            book.next[branch] = next;
        }
        Ok(book)
    }
}
