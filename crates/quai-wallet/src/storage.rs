//! Native SQLite storage primitives, not a scanner or verified reconciliation engine.
//!
//! A database contains public metadata only. Caller-observed checkpoints are not
//! chain proofs. Signed claims never become reusable through this API. Keep the
//! database and WAL on a local filesystem with reliable locks; protect their privacy.

pub use crate::discovery::{Checkpoint, NetworkScope};
pub use crate::metadata::{KeyOrigin, PublicAddress, StorageError};
pub use crate::state::{Reservation, ReservationId, ReservationState};
use crate::{AccountPublic, CandidateCoin, CoinType};
use quai_consensus::{
    Denomination, MAX_TRANSACTION_BYTES, OutPoint, SignedQiTransaction, SignedQuaiTransaction, U256,
};
use quai_crypto::PublicKey;
use quai_primitives::{Address, Hash32, QiAddress, QuaiAddress};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

const APP_ID: i64 = 0x51574149;
const VERSION: i64 = 6;
const MAX_COINS: usize = 100_000;

impl From<rusqlite::Error> for StorageError {
    fn from(_: rusqlite::Error) -> Self {
        Self::Database
    }
}
type Result<T> = std::result::Result<T, StorageError>;
mod activity;
pub use activity::{ActivityDetail, ActivityEntry, ActivityStatus, QiActivityKind};
mod backup_state;
mod observations;
mod released;
pub use observations::ObservationCache;
mod replay;
pub use replay::ReorgInvalidation;
mod head_state;
pub use head_state::{HeadReplayCommit, HeadReplayState, MAX_HEAD_REPLAY_BYTES};
pub(crate) mod replacements;
pub use replacements::{QuaiReplacement, ReplacementCandidate};
pub(crate) mod payment;
pub(crate) use backup_state::PublicWalletState;
pub use payment::{PaymentAddressAllocation, PaymentAddressRecord, VersionedPaymentChannel};

/// Atomic scanner snapshot. Importing this requires exact scope and prior generation.
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Network scope, checked on replacement.
    pub scope: NetworkScope,
    /// Current generation for reads, expected previous generation for replacement.
    pub generation: u64,
    /// None denotes invalidated/unscanned metadata.
    pub checkpoint: Option<Checkpoint>,
    /// Public UTXO candidates; reserved flags are computed from durable claims on reads.
    pub coins: Vec<CandidateCoin>,
}
/// A fresh public address, returned only after its range and metadata are durable.
#[derive(Clone, Debug)]
pub struct AllocatedAddress {
    /// Public address and immutable exact derivation origin.
    pub address: PublicAddress,
    /// Raw range consumed, including skipped indexes, and the unexamined tail
    /// when a concurrent write prevented giving it back.
    pub burned: crate::discovery::IndexRange,
    /// Snapshot generation after metadata invalidation.
    pub generation: u64,
}
/// Conservative canonical-checkpoint reconciliation result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckpointReconciliation {
    /// No usable checkpoint exists yet.
    Unscanned,
    /// Source still reports the same block; this is not a finality proof.
    Matches,
    /// Reorg/unknown observation invalidated state while preserving claims.
    Invalidated {
        /// New generation after invalidation.
        generation: u64,
    },
}
/// Opaque identity of one opened native storage handle. It is process-local and
/// cannot be constructed or deserialized externally. Reopening the same file gets
/// a different identity; this binds live unsigned objects, not persisted backups.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StoreInstance(usize);
impl StoreInstance {
    fn allocate() -> Result<Self> {
        static NEXT: AtomicUsize = AtomicUsize::new(1);
        NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            value.checked_add(1)
        })
        .map(Self)
        .map_err(|_| StorageError::Overflow)
    }
}
/// Scoped native database connection. Separate instances safely compete via SQLite transactions.
pub struct SqliteStore {
    instance: StoreInstance,
    connection: Connection,
    scope: NetworkScope,
    key: [u8; 65],
}
impl SqliteStore {
    /// Open/create schema v6, atomically migrating validated v1 to v5 state. Foreign application IDs and nonempty unknown databases
    /// are rejected before persistent writes. WAL requires a local filesystem.
    pub fn open(path: impl AsRef<Path>, scope: NetworkScope) -> Result<Self> {
        Self::open_with_busy_timeout(path, scope, Duration::from_secs(5))
    }

    /// Open with an explicit SQLite lock-wait budget in whole milliseconds,
    /// between zero (fail immediately on contention) and 60 seconds. The default
    /// open method uses five seconds. This changes only local lock waiting;
    /// database errors never trigger automatic reservation or broadcast retries.
    pub fn open_with_busy_timeout(
        path: impl AsRef<Path>,
        scope: NetworkScope,
        busy_timeout: Duration,
    ) -> Result<Self> {
        if busy_timeout > Duration::from_secs(60)
            || !busy_timeout.subsec_nanos().is_multiple_of(1_000_000)
        {
            return Err(StorageError::Invalid);
        }
        if scope.genesis.bytes() == &[0; 32] {
            return Err(StorageError::Invalid);
        }
        let instance = StoreInstance::allocate()?;
        let mut connection = Connection::open(path)?;
        connection.busy_timeout(busy_timeout)?;
        // Validate inside a writer transaction to serialize first-open schema creation.
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let app: i64 = tx.pragma_query_value(None, "application_id", |r| r.get(0))?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let objects: i64 = tx.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |r| r.get(0),
        )?;
        if app == 0 && version == 0 && objects == 0 {
            tx.execute_batch(SCHEMA)?;
            tx.execute_batch(payment::PAYMENT_SCHEMA)?;
            tx.execute_batch(replacements::REPLACEMENT_SCHEMA)?;
            tx.execute_batch(observations::OBSERVATION_SCHEMA)?;
            tx.execute_batch(head_state::HEAD_SCHEMA)?;
            tx.execute_batch(released::RELEASED_SCHEMA)?;
            tx.pragma_update(None, "application_id", APP_ID)?;
            tx.pragma_update(None, "user_version", VERSION)?;
        } else if app == APP_ID && matches!(version, 1..=5) {
            backup_state::validate_native_schema_version(&tx, version as u8)?;
            if version == 1 {
                tx.execute_batch(payment::PAYMENT_SCHEMA)?;
            }
            if version < 3 {
                tx.execute_batch(replacements::REPLACEMENT_SCHEMA)?;
            }
            if version < 4 {
                tx.execute_batch(observations::OBSERVATION_SCHEMA)?;
            }
            if version < 5 {
                tx.execute_batch(head_state::HEAD_SCHEMA)?;
            }
            tx.execute_batch(released::RELEASED_SCHEMA)?;
            tx.pragma_update(None, "user_version", VERSION)?;
        } else if app != APP_ID || version != VERSION {
            return Err(StorageError::Schema);
        }
        tx.commit()?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        let mode: String = connection.pragma_query_value(None, "journal_mode", |r| r.get(0))?;
        if mode != "wal" {
            return Err(StorageError::Database);
        }
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.pragma_update(None, "foreign_keys", true)?;
        connection.pragma_update(None, "trusted_schema", false)?;
        let key = scope.key();
        connection.execute(
            "INSERT OR IGNORE INTO scopes(scope,generation) VALUES(?1,0)",
            [&key[..]],
        )?;
        Ok(Self {
            instance,
            connection,
            scope,
            key,
        })
    }
    /// Identity for binding live unsigned operations to this exact opened handle.
    pub fn instance(&self) -> StoreInstance {
        self.instance
    }

    /// Bound network identity.
    pub fn scope(&self) -> NetworkScope {
        self.scope
    }

    /// Import public metadata atomically and invalidate ALL checkpoint/UTXO state.
    /// Existing origins are immutable; reservations and nonce cursors survive.
    /// Even an empty or idempotent import increments generation and invalidates.
    pub fn import_metadata(
        &mut self,
        expected_generation: u64,
        addresses: &[PublicAddress],
    ) -> Result<u64> {
        if addresses.len() > MAX_COINS {
            return Err(StorageError::Invalid);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let next = next_generation(&tx, &self.key, expected_generation)?;
        for address in addresses {
            insert_address(&tx, &self.key, self.scope, address)?;
        }
        let count: i64 = tx.query_row(
            "SELECT count(*) FROM addresses WHERE scope=?1",
            [&self.key[..]],
            |r| r.get(0),
        )?;
        if count > MAX_COINS as i64 {
            return Err(StorageError::Invalid);
        }
        clear_snapshot(&tx, &self.key, next)?;
        tx.commit()?;
        Ok(next as u64)
    }
    /// Explicit invalidation primitive for reorgs/recovery; preserves ambiguous claims.
    pub fn invalidate_snapshot(&mut self, expected_generation: u64) -> Result<u64> {
        self.import_metadata(expected_generation, &[])
    }
    /// Read a bound branch's durable next raw index, including cancelled/crashed
    /// and unused trailing allocations. None means no range was allocated yet;
    /// initialization will still skip all matching imported/discovered metadata.
    pub fn next_derivation_index(
        &self,
        account: &AccountPublic,
        change: bool,
    ) -> Result<Option<u32>> {
        let row:Option<(String,u32)>=self.connection.query_row("SELECT account_xpub,next_index FROM derivation_cursors WHERE scope=?1 AND coin=?2 AND account=?3 AND change_branch=?4",params![&self.key[..],account.coin_type().number(),account.account_index(),change],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        match row {
            Some((xpub, index)) if xpub == account.export() => Ok(Some(index)),
            Some(_) => Err(StorageError::Conflict),
            None => Ok(None),
        }
    }
    /// Atomically burn a bounded raw child range, then derive and persist a fresh
    /// address before returning it. The search runs without holding the write
    /// lock. Every index in that range stays consumed after cancellation,
    /// failure, process death or restart. On success, the range past the
    /// returned address is released again unless the store changed meanwhile. `max_attempts` must be 1..=100,000.
    pub fn allocate_address(
        &mut self,
        account: &AccountPublic,
        change: bool,
        max_attempts: u32,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<AllocatedAddress> {
        self.allocate_address_inner(account, change, (max_attempts, false), &mut cancelled)
    }

    /// Derive under the SQLite write lock and commit only the examined range.
    /// No address escapes before its cursor and metadata are durable. Failed or
    /// cancelled searches expose nothing and roll back; previously committed
    /// allocations are never reused. The bounded search holds the write lock.
    pub fn allocate_address_compact(
        &mut self,
        account: &AccountPublic,
        change: bool,
        max_attempts: u32,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<AllocatedAddress> {
        self.allocate_address_inner(account, change, (max_attempts, true), &mut cancelled)
    }

    fn allocate_address_inner(
        &mut self,
        account: &AccountPublic,
        change: bool,
        search: (u32, bool),
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<AllocatedAddress> {
        use crate::{Search, WalletError};
        let (max_attempts, compact) = search;
        if max_attempts == 0 || max_attempts > 100_000 {
            return Err(StorageError::Invalid);
        }
        if cancelled() {
            return Err(StorageError::Cancelled);
        }
        let coin = account.coin_type().number();
        let account_index = account.account_index();
        let xpub = account.export();
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let bound:Option<String>=tx.query_row("SELECT account_xpub FROM derivation_cursors WHERE scope=?1 AND coin=?2 AND account=?3 LIMIT 1",params![&self.key[..],coin,account_index],|r|r.get(0)).optional()?;
        if bound.as_ref().is_some_and(|previous| previous != &xpub) {
            return Err(StorageError::Conflict);
        }
        let old: Option<(String,u32)> = tx.query_row("SELECT account_xpub,next_index FROM derivation_cursors WHERE scope=?1 AND coin=?2 AND account=?3 AND change_branch=?4",params![&self.key[..],coin,account_index,change],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let initialized = old.is_some();
        let mut start = if let Some((previous, index)) = old {
            if previous != xpub {
                return Err(StorageError::Conflict);
            }
            index
        } else {
            0
        };
        if !initialized {
            // Imported/discovered public metadata is conservatively treated as exposed.
            let mut statement =
                tx.prepare("SELECT origin,address,public_key FROM addresses WHERE scope=?1")?;
            let mut rows = statement.query([&self.key[..]])?;
            while let Some(row) = rows.next()? {
                if let KeyOrigin::Bip44 {
                    coin: origin_coin,
                    account: origin_account,
                    change: origin_change,
                    index,
                } = KeyOrigin::decode(&row.get::<_, Vec<u8>>(0)?)?
                    && origin_coin == account.coin_type()
                    && origin_account == account_index
                {
                    if cancelled() {
                        return Err(StorageError::Cancelled);
                    }
                    if bound.is_none() {
                        let expected = account
                            .derive_address(origin_change, index)
                            .map_err(|_| StorageError::Conflict)?;
                        if row.get::<_, Vec<u8>>(1)? != expected.address.bytes()
                            || row.get::<_, Vec<u8>>(2)? != expected.public_key
                        {
                            return Err(StorageError::Conflict);
                        }
                    }
                    if origin_change == change {
                        start = start.max(index + 1);
                    }
                }
            }
            drop(rows);
            drop(statement);
        }
        let limit = start
            .checked_add(max_attempts)
            .filter(|end| *end <= 1 << 31)
            .ok_or(StorageError::Overflow)?;
        let mut derive = || {
            account
                .search(
                    change,
                    Search {
                        zone: self.scope.zone,
                        start_index: start,
                        max_attempts,
                    },
                    &mut cancelled,
                )
                .map_err(|error| match error {
                    WalletError::Cancelled { .. } => StorageError::Cancelled,
                    WalletError::SearchExhausted { .. } => StorageError::DerivationExhausted,
                    _ => StorageError::Invalid,
                })
        };
        let found = if compact { Some(derive()?) } else { None };
        let mut end = found
            .as_ref()
            .map_or(limit, |found| found.address.index + 1);
        tx.execute("INSERT INTO derivation_cursors VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(scope,coin,account,change_branch) DO UPDATE SET next_index=excluded.next_index",params![&self.key[..],coin,account_index,change,xpub,end])?;
        // For compact allocation, retain the same lock until metadata is committed.
        // Legacy allocation deliberately burns its full range before searching.
        let (tx, found) = match found {
            Some(found) => (tx, found),
            None => {
                let reserved_generation = checkpoint_read(&tx, &self.key)?.0;
                tx.commit()?;
                let found = derive()?;
                let tx = self
                    .connection
                    .transaction_with_behavior(TransactionBehavior::Immediate)?;
                // Give back the unexamined tail of the burn when the store is
                // unchanged since the reservation. The cursor value alone
                // cannot show that: imports, discovery and backup merges raise
                // it with max(), so one that records an address inside the
                // tail leaves it equal to `limit`, and rewinding would issue
                // that address again. Every writer that records an address or
                // moves a cursor bumps the scope generation (only a refresh
                // with unchanged coins keeps it), so an unchanged generation
                // does prove nothing was recorded.
                // Keeping a full burn would skip about max_attempts / 512
                // matching addresses, enough past a few thousand attempts to
                // put the next address beyond a default restore gap.
                if tx.execute(
                    "UPDATE derivation_cursors SET next_index=?1 WHERE scope=?2 AND coin=?3 AND account=?4 AND change_branch=?5 AND next_index=?6 AND (SELECT generation FROM scopes WHERE scope=?2)=?7",
                    params![found.address.index + 1, &self.key[..], coin, account_index, change, limit, reserved_generation],
                )? == 1
                {
                    end = found.address.index + 1;
                }
                (tx, found)
            }
        };
        if cancelled() {
            return Err(StorageError::Cancelled);
        }
        let address = PublicAddress::derive(account, change, found.address.index)?;
        // Import+checkpoint invalidation is committed before any address leaves this API.
        let generation = checkpoint_read(&tx, &self.key)?.0;
        let next = next_generation(&tx, &self.key, generation as u64)?;
        let count: i64 = tx.query_row(
            "SELECT count(*) FROM addresses WHERE scope=?1",
            [&self.key[..]],
            |r| r.get(0),
        )?;
        if count >= MAX_COINS as i64 {
            return Err(StorageError::Invalid);
        }
        insert_address(&tx, &self.key, self.scope, &address)?;
        clear_snapshot(&tx, &self.key, next)?;
        tx.commit()?;
        Ok(AllocatedAddress {
            address,
            burned: crate::discovery::IndexRange { start, end },
            generation: next as u64,
        })
    }
    /// Observe canonical identity at the stored checkpoint's exact height. A mismatch
    /// or unavailable hash invalidates coins/checkpoint atomically, preserving claims.
    /// The caller is responsible for obtaining this observation from its trusted source.
    pub fn reconcile_checkpoint(
        &mut self,
        expected_generation: u64,
        canonical: Option<Checkpoint>,
    ) -> Result<CheckpointReconciliation> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (generation, previous) = checkpoint_read(&tx, &self.key)?;
        if generation as u64 != expected_generation {
            return Err(StorageError::StaleSnapshot);
        }
        let Some(previous) = previous else {
            return Ok(CheckpointReconciliation::Unscanned);
        };
        if canonical.is_some_and(|value| value.height != previous.height) {
            return Err(StorageError::Invalid);
        }
        if canonical == Some(previous) {
            return Ok(CheckpointReconciliation::Matches);
        }
        let next = next_generation(&tx, &self.key, expected_generation)?;
        clear_snapshot(&tx, &self.key, next)?;
        tx.commit()?;
        Ok(CheckpointReconciliation::Invalidated {
            generation: next as u64,
        })
    }
    /// Commit a set of consistent discovery reports and public metadata in one CAS
    /// transaction. Every previously stored address MUST be observed by this batch;
    /// partial account scans cannot relabel older, unobserved coins with a new checkpoint.
    /// Gap/range coverage remains bounded source claims, not proof of complete recovery.
    pub fn commit_discovery(
        &mut self,
        expected_generation: u64,
        reports: &[crate::discovery::DiscoveryReport],
    ) -> Result<u64> {
        use crate::discovery::{CanonicalStatus, ScanStop};
        if reports.is_empty() || reports.len() > 1000 {
            return Err(StorageError::Invalid);
        }
        let checkpoint = reports[0].checkpoint;
        let mut public = Vec::new();
        let mut coins = Vec::new();
        let mut seen = BTreeSet::new();
        for report in reports {
            if report.scope != self.scope
                || report.checkpoint != checkpoint
                || report.canonical != CanonicalStatus::Matches
                || report
                    .coverage
                    .iter()
                    .any(|c| matches!(c.stop, ScanStop::Cancelled))
            {
                return Err(StorageError::StaleSnapshot);
            }
            let account = AccountPublic::import(&report.account_xpub, report.coin, report.account)
                .map_err(|_| StorageError::Invalid)?;
            for observed in &report.addresses {
                if public.len() == MAX_COINS {
                    return Err(StorageError::Invalid);
                }
                let derived = PublicAddress::derive(
                    &account,
                    observed.derived.change,
                    observed.derived.index,
                )?;
                if derived.address != observed.derived.address
                    || derived.public_key != observed.derived.public_key
                    || observed.observation.address != derived.address
                    || observed.observation.scope != self.scope
                    || observed.observation.checkpoint != checkpoint
                    || !seen.insert(derived.address)
                {
                    return Err(StorageError::Invalid);
                }
                for coin in &observed.observation.coins {
                    if coin.address.address() != derived.address || coins.len() == MAX_COINS {
                        return Err(StorageError::Invalid);
                    }
                    coins.push(coin.clone());
                }
                public.push(derived);
            }
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let next = next_generation(&tx, &self.key, expected_generation)?;
        if let Some(old) = checkpoint_read(&tx, &self.key)?.1
            && (old.height > checkpoint.height
                || old.height == checkpoint.height && old.hash != checkpoint.hash)
        {
            return Err(StorageError::StaleSnapshot);
        }
        let mut statement = tx.prepare("SELECT address FROM addresses WHERE scope=?1")?;
        let mut rows = statement.query([&self.key[..]])?;
        while let Some(row) = rows.next()? {
            let address = Address::from_bytes(array(&row.get::<_, Vec<u8>>(0)?)?);
            if !seen.contains(&address) {
                return Err(StorageError::StaleSnapshot);
            }
        }
        drop(rows);
        drop(statement);
        for address in &public {
            insert_address(&tx, &self.key, self.scope, address)?;
        }
        tx.execute("DELETE FROM coins WHERE scope=?1", [&self.key[..]])?;
        let mut outpoints = BTreeSet::new();
        for coin in coins {
            let hash = coin.outpoint.transaction_hash.bytes();
            if coin.address.zone() != self.scope.zone
                || hash[2] != self.scope.zone.byte()
                || *hash == [0; 32]
                || !outpoints.insert(coin.outpoint)
                || coin
                    .expires_at
                    .is_some_and(|height| height <= coin.unlock_height)
            {
                return Err(StorageError::Invalid);
            }
            let expiry = coin.expires_at.map(|v| v.to_be_bytes::<32>());
            tx.execute(
                "INSERT INTO coins VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    &self.key[..],
                    &hash[..],
                    coin.outpoint.index,
                    next,
                    &coin.address.bytes()[..],
                    coin.denomination.index(),
                    &coin.unlock_height.to_be_bytes::<32>()[..],
                    expiry.as_ref().map(|v| &v[..])
                ],
            )?;
        }
        tx.execute(
            "UPDATE scopes SET generation=?2,block_hash=?3,height=?4 WHERE scope=?1",
            params![
                &self.key[..],
                next,
                &checkpoint.hash.bytes()[..],
                &checkpoint.height.to_be_bytes::<32>()[..]
            ],
        )?;
        tx.commit()?;
        Ok(next as u64)
    }
    /// Read immutable public metadata in address-byte order.
    pub fn addresses(&self) -> Result<Vec<PublicAddress>> {
        let mut statement = self.connection.prepare("SELECT address,public_key,origin FROM addresses WHERE scope=?1 ORDER BY address LIMIT 100001")?;
        let mut rows = statement.query([&self.key[..]])?;
        let mut result = Vec::new();
        while let Some(row) = rows.next()? {
            let address = Address::from_bytes(array(&row.get::<_, Vec<u8>>(0)?)?);
            let public_key = array(&row.get::<_, Vec<u8>>(1)?)?;
            let origin = KeyOrigin::decode(&row.get::<_, Vec<u8>>(2)?)?;
            let key = PublicKey::from_sec1_bytes(&public_key).map_err(|_| StorageError::Invalid)?;
            if key.address() != address
                || address.zone().map_err(|_| StorageError::Invalid)? != self.scope.zone
                || result.len() == MAX_COINS
            {
                return Err(StorageError::Invalid);
            }
            result.push(PublicAddress {
                address,
                public_key,
                origin,
            });
        }
        Ok(result)
    }
    /// Atomically replace coins and checkpoint using generation compare-and-swap.
    /// Rewinds or same-height hash changes require explicit invalidation first.
    /// When a checkpoint is stored and the coins equal the stored ones (outpoint,
    /// owner, denomination, lock and expiry), only the checkpoint advances and
    /// the generation is kept, so reservations and observers fenced on it stay
    /// valid; an unchanged checkpoint writes nothing. Returns the generation
    /// after the commit.
    pub fn replace_snapshot(&mut self, snapshot: &Snapshot) -> Result<u64> {
        if snapshot.scope != self.scope {
            return Err(StorageError::StaleSnapshot);
        }
        let checkpoint = snapshot.checkpoint.ok_or(StorageError::Invalid)?;
        if snapshot.coins.len() > MAX_COINS {
            return Err(StorageError::Invalid);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let next = next_generation(&tx, &self.key, snapshot.generation)?;
        let old = checkpoint_read(&tx, &self.key)?.1;
        if let Some(old) = old
            && (old.height > checkpoint.height
                || (old.height == checkpoint.height && old.hash != checkpoint.hash))
        {
            return Err(StorageError::StaleSnapshot);
        }
        let mut rows = CoinRows::new();
        for coin in &snapshot.coins {
            let hash = coin.outpoint.transaction_hash.bytes();
            if coin.address.zone() != self.scope.zone
                || hash[2] != self.scope.zone.byte()
                || *hash == [0; 32]
                || coin
                    .expires_at
                    .is_some_and(|height| height <= coin.unlock_height)
            {
                return Err(StorageError::Invalid);
            }
            require_address(&tx, &self.key, coin.address.address())?;
            let row = (
                *coin.address.bytes(),
                coin.denomination.index(),
                coin.unlock_height.to_be_bytes::<32>(),
                coin.expires_at.map(|v| v.to_be_bytes::<32>()),
            );
            if rows.insert((*hash, coin.outpoint.index), row).is_some() {
                return Err(StorageError::Invalid);
            }
        }
        // A stored checkpoint means the stored coins are a complete view at
        // this generation, so an identical set needs no rewrite.
        if old.is_some() && coin_rows(&tx, &self.key)? == rows {
            if old != Some(checkpoint) {
                tx.execute(
                    "UPDATE scopes SET block_hash=?2,height=?3 WHERE scope=?1",
                    params![
                        &self.key[..],
                        &checkpoint.hash.bytes()[..],
                        &checkpoint.height.to_be_bytes::<32>()[..]
                    ],
                )?;
                tx.commit()?;
            }
            return Ok(snapshot.generation);
        }
        tx.execute("DELETE FROM coins WHERE scope=?1", [&self.key[..]])?;
        for ((hash, index), (address, denomination, unlock, expiry)) in &rows {
            // Cached: this runs once per coin, and a refresh replaces them all.
            tx.prepare_cached("INSERT INTO coins VALUES(?1,?2,?3,?4,?5,?6,?7,?8)")?
                .execute(params![
                    &self.key[..],
                    &hash[..],
                    index,
                    next,
                    &address[..],
                    denomination,
                    &unlock[..],
                    expiry.as_ref().map(|v| &v[..])
                ])?;
        }
        tx.execute(
            "UPDATE scopes SET generation=?2,block_hash=?3,height=?4 WHERE scope=?1",
            params![
                &self.key[..],
                next,
                &checkpoint.hash.bytes()[..],
                &checkpoint.height.to_be_bytes::<32>()[..]
            ],
        )?;
        tx.commit()?;
        Ok(next as u64)
    }
    /// Consistent snapshot read; durable claims override caller-supplied reserved flags.
    pub fn snapshot(&mut self) -> Result<Snapshot> {
        let tx = self.connection.transaction()?;
        let (generation, checkpoint) = checkpoint_read(&tx, &self.key)?;
        let mut coins = Vec::new();
        {
            let mut statement = tx.prepare("SELECT c.tx_hash,c.output_index,c.generation,c.address,c.denomination,c.unlock_height,c.expires_at,EXISTS(SELECT 1 FROM qi_claims q WHERE q.scope=c.scope AND q.tx_hash=c.tx_hash AND q.output_index=c.output_index) FROM coins c WHERE c.scope=?1 ORDER BY c.tx_hash,c.output_index LIMIT 100001")?;
            let mut rows = statement.query([&self.key[..]])?;
            while let Some(row) = rows.next()? {
                if coins.len() == MAX_COINS
                    || row.get::<_, i64>(2)? != generation
                    || checkpoint.is_none()
                {
                    return Err(StorageError::Invalid);
                }
                let hash = Hash32::from_bytes(array(&row.get::<_, Vec<u8>>(0)?)?);
                let address = QiAddress::try_from(array::<20>(&row.get::<_, Vec<u8>>(3)?)?)
                    .map_err(|_| StorageError::Invalid)?;
                if address.zone() != self.scope.zone
                    || hash.bytes()[2] != self.scope.zone.byte()
                    || hash == Hash32::ZERO
                {
                    return Err(StorageError::Invalid);
                }
                coins.push(CandidateCoin {
                    outpoint: OutPoint {
                        transaction_hash: hash,
                        index: row.get(1)?,
                    },
                    address,
                    denomination: Denomination::new(row.get(4)?)
                        .map_err(|_| StorageError::Invalid)?,
                    unlock_height: U256::from_be_bytes(array::<32>(&row.get::<_, Vec<u8>>(5)?)?),
                    expires_at: row
                        .get::<_, Option<Vec<u8>>>(6)?
                        .map(|v| array::<32>(&v).map(U256::from_be_bytes))
                        .transpose()?,
                    reserved: row.get(7)?,
                });
            }
        }
        tx.commit()?;
        Ok(Snapshot {
            scope: self.scope,
            generation: generation as u64,
            checkpoint,
            coins,
        })
    }
    /// Atomically claim a nonempty batch of spendable Qi outputs from a current snapshot.
    /// Caller must mark Signed BEFORE exposing a signature or attempting broadcast.
    pub fn reserve_qi(
        &mut self,
        id: ReservationId,
        generation: u64,
        candidate_height: U256,
        outpoints: &[OutPoint],
    ) -> Result<()> {
        if outpoints.is_empty() || outpoints.len() > 4096 {
            return Err(StorageError::Invalid);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (current, checkpoint) = checkpoint_read(&tx, &self.key)?;
        if u64::try_from(current).ok() != Some(generation)
            || checkpoint.is_none_or(|c| c.height > candidate_height)
        {
            return Err(StorageError::StaleSnapshot);
        }
        insert_reservation(&tx, &self.key, id, 0)?;
        let mut seen = BTreeSet::new();
        for outpoint in outpoints {
            if !seen.insert(*outpoint) {
                return Err(StorageError::Invalid);
            }
            type CoinBounds = (Vec<u8>, Option<Vec<u8>>, Vec<u8>);
            let bounds: Option<CoinBounds> = tx.query_row("SELECT unlock_height,expires_at,address FROM coins WHERE scope=?1 AND tx_hash=?2 AND output_index=?3", params![&self.key[..], &outpoint.transaction_hash.bytes()[..], outpoint.index], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            let (unlock, expiry, owner) = bounds.ok_or(StorageError::Invalid)?;
            if U256::from_be_bytes(array::<32>(&unlock)?) > candidate_height
                || expiry
                    .as_ref()
                    .map(|v| array::<32>(v).map(U256::from_be_bytes))
                    .transpose()?
                    .is_some_and(|height| candidate_height >= height)
            {
                return Err(StorageError::Invalid);
            }
            let changed = tx.execute(
                "INSERT OR IGNORE INTO qi_claims VALUES(?1,?2,?3,?4,?5)",
                params![
                    &self.key[..],
                    &outpoint.transaction_hash.bytes()[..],
                    outpoint.index,
                    &id.0[..],
                    owner
                ],
            )?;
            if changed != 1 {
                return Err(StorageError::Conflict);
            }
        }
        tx.commit()?;
        Ok(())
    }
    /// Allocate max(remote_pending, persisted_next), atomically across connections.
    /// Remote pending nonce must come from a trusted provider for this exact scope.
    /// Released operations never rewind this cursor; u64::MAX cannot be allocated.
    pub fn reserve_nonce(
        &mut self,
        id: ReservationId,
        address: QuaiAddress,
        remote_pending: u64,
    ) -> Result<u64> {
        if address.zone() != self.scope.zone {
            return Err(StorageError::Invalid);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_address(&tx, &self.key, address.address())?;
        let previous: Option<Vec<u8>> = tx
            .query_row(
                "SELECT next_nonce FROM nonce_cursors WHERE scope=?1 AND address=?2",
                params![&self.key[..], &address.bytes()[..]],
                |r| r.get(0),
            )
            .optional()?;
        let nonce = remote_pending.max(
            previous
                .map(|v| array::<8>(&v).map(u64::from_be_bytes))
                .transpose()?
                .unwrap_or(0),
        );
        let next = nonce.checked_add(1).ok_or(StorageError::Overflow)?;
        insert_reservation(&tx, &self.key, id, 1)?;
        tx.execute(
            "INSERT INTO nonce_claims VALUES(?1,?2,?3,?4)",
            params![
                &self.key[..],
                &address.bytes()[..],
                &nonce.to_be_bytes()[..],
                &id.0[..]
            ],
        )?;
        tx.execute("INSERT INTO nonce_cursors VALUES(?1,?2,?3) ON CONFLICT(scope,address) DO UPDATE SET next_nonce=excluded.next_nonce", params![&self.key[..], &address.bytes()[..], &next.to_be_bytes()[..]])?;
        tx.commit()?;
        Ok(nonce)
    }
    /// Look up a durable operation after restart.
    pub fn reservation(&self, id: ReservationId) -> Result<Option<Reservation>> {
        reservation_read(&self.connection, &self.key, id)
    }
    /// Enumerate durable operations by ID, including released records. Use the last
    /// returned ID as the next cursor. Limit must be 1 through 1000.
    pub fn reservations(
        &self,
        after: Option<ReservationId>,
        limit: u16,
    ) -> Result<Vec<Reservation>> {
        if limit == 0 || limit > 1000 {
            return Err(StorageError::Invalid);
        }
        let mut statement = self.connection.prepare("SELECT id FROM reservations WHERE scope=?1 AND (?2 IS NULL OR id>?2) ORDER BY id LIMIT ?3")?;
        let cursor = after.map(|id| id.0);
        let mut rows = statement.query(params![
            &self.key[..],
            cursor.as_ref().map(|v| &v[..]),
            limit
        ])?;
        let mut result = Vec::new();
        while let Some(row) = rows.next()? {
            let id = ReservationId(array(&row.get::<_, Vec<u8>>(0)?)?);
            result.push(
                reservation_read(&self.connection, &self.key, id)?.ok_or(StorageError::Invalid)?,
            );
        }
        Ok(result)
    }
    /// Recover currently held Qi claims for an operation. Released unsigned claims
    /// are removed; signed/confirmed claims remain recoverable after invalidation.
    pub fn reserved_outpoints(&self, id: ReservationId) -> Result<Vec<OutPoint>> {
        let mut statement = self.connection.prepare("SELECT tx_hash,output_index FROM qi_claims WHERE scope=?1 AND operation=?2 ORDER BY tx_hash,output_index LIMIT 4097")?;
        let mut rows = statement.query(params![&self.key[..], &id.0[..]])?;
        let mut result = Vec::new();
        while let Some(row) = rows.next()? {
            if result.len() == 4096 {
                return Err(StorageError::Invalid);
            }
            result.push(OutPoint {
                transaction_hash: Hash32::from_bytes(array(&row.get::<_, Vec<u8>>(0)?)?),
                index: row.get(1)?,
            });
        }
        Ok(result)
    }
    /// Recover an allocated account nonce after restart, including an unsigned
    /// released operation. Nonce cursor/claim records never rewind or disappear.
    pub fn reserved_nonce(&self, id: ReservationId) -> Result<Option<(QuaiAddress, u64)>> {
        let raw: Option<(Vec<u8>, Vec<u8>)> = self
            .connection
            .query_row(
                "SELECT address,nonce FROM nonce_claims WHERE scope=?1 AND operation=?2",
                params![&self.key[..], &id.0[..]],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        raw.map(|(address, nonce)| {
            let address =
                QuaiAddress::try_from(array::<20>(&address)?).map_err(|_| StorageError::Invalid)?;
            if address.zone() != self.scope.zone {
                return Err(StorageError::Invalid);
            }
            Ok((address, u64::from_be_bytes(array(&nonce)?)))
        })
        .transpose()
    }
    /// Atomically persist canonical signed account bytes and transition to Signed.
    /// Chain, recovered sender and nonce must exactly match this operation's claim.
    pub fn commit_signed_quai(
        &mut self,
        id: ReservationId,
        signed: &SignedQuaiTransaction,
    ) -> Result<()> {
        let payload = signed.signed_bytes().map_err(|_| StorageError::Invalid)?;
        let hash = signed.hash().map_err(|_| StorageError::Invalid)?;
        if signed.transaction().chain_id != self.scope.chain_id
            || signed.from().zone() != self.scope.zone
        {
            return Err(StorageError::Invalid);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate_quai_claim(&tx, &self.key, id, signed)?;
        commit_payload(&tx, &self.key, id, hash, 1, &payload)?;
        tx.commit()?;
        Ok(())
    }
    /// Atomically persist canonical signed Qi bytes and transition to Signed.
    /// Chain and the complete unordered outpoint/owner set must match held claims.
    /// Input order remains unchanged in the persisted signed bytes.
    pub fn commit_signed_qi(
        &mut self,
        id: ReservationId,
        signed: &SignedQiTransaction,
    ) -> Result<()> {
        self.commit_signed_qi_operation(
            id,
            &quai_consensus::SignedQiOperation::Transfer(signed.clone()),
        )
    }
    /// Persist a verified ordinary, conversion or wrapping payload against its
    /// exact Qi claims. Backups and restart validation preserve its operation type.
    pub fn commit_signed_qi_operation(
        &mut self,
        id: ReservationId,
        signed: &quai_consensus::SignedQiOperation,
    ) -> Result<()> {
        let payload = signed.signed_bytes().map_err(|_| StorageError::Invalid)?;
        let hash = signed.hash().map_err(|_| StorageError::Invalid)?;
        if signed.transaction().chain_id != self.scope.chain_id
            || hash.bytes()[0] != self.scope.zone.byte()
        {
            return Err(StorageError::Invalid);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate_qi_claim(&tx, &self.key, id, signed.transaction())?;
        commit_payload(&tx, &self.key, id, hash, 0, &payload)?;
        tx.commit()?;
        Ok(())
    }
    /// Recover bounded canonical signed bytes for explicit rebroadcast after restart.
    /// Signature, transaction identity, scope and original claims are revalidated.
    /// None means no payload was committed (including hash-only mark_signed usage).
    pub fn signed_payload(&mut self, id: ReservationId) -> Result<Option<Vec<u8>>> {
        let tx = self.connection.transaction()?;
        let result = signed_payload_read(&tx, &self.key, self.scope, id)?;
        tx.commit()?;
        Ok(result)
    }
    /// Record transaction identity before returning signed bytes to another component.
    /// This transition permanently prevents unsigned-release through this API.
    pub fn mark_signed(&mut self, id: ReservationId, transaction: Hash32) -> Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let old = reservation_read(&tx, &self.key, id)?.ok_or(StorageError::Transition)?;
        if old.state == ReservationState::Signed && old.transaction == Some(transaction) {
            return Ok(());
        }
        if old.state != ReservationState::Reserved || old.transaction.is_some() {
            return Err(StorageError::Transition);
        }
        tx.execute(
            "UPDATE reservations SET state=1,transaction_hash=?3 WHERE scope=?1 AND id=?2",
            params![&self.key[..], &id.0[..], &transaction.bytes()[..]],
        )?;
        tx.commit()?;
        Ok(())
    }
    /// Record a broadcast attempt. Failures/timeouts MUST NOT trigger release.
    pub fn mark_submitted(&mut self, id: ReservationId) -> Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let old = reservation_read(&tx, &self.key, id)?.ok_or(StorageError::Transition)?;
        if old.state == ReservationState::Submitted {
            return Ok(());
        }
        if old.state != ReservationState::Signed {
            return Err(StorageError::Transition);
        }
        tx.execute(
            "UPDATE reservations SET state=2 WHERE scope=?1 AND id=?2",
            params![&self.key[..], &id.0[..]],
        )?;
        tx.commit()?;
        Ok(())
    }
    /// Record caller-verified inclusion of the exact signed transaction. This does
    /// not validate a chain proof, release claims, or imply finality across reorgs.
    pub fn observe_inclusion(
        &mut self,
        id: ReservationId,
        transaction: Hash32,
        block: Checkpoint,
    ) -> Result<()> {
        self.observe_inclusion_scoped(self.observation_generation()?, id, transaction, block)
    }
    /// Record an inclusion only while the scope generation captured before its
    /// asynchronous canonical reads still matches. Reorg and restore invalidation
    /// prevent a delayed observer from reinstating an old-chain inclusion.
    pub fn observe_inclusion_scoped(
        &mut self,
        expected_generation: u64,
        id: ReservationId,
        transaction: Hash32,
        block: Checkpoint,
    ) -> Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if checkpoint_read(&tx, &self.key)?.0 as u64 != expected_generation {
            return Err(StorageError::StaleSnapshot);
        }
        let old = reservation_read(&tx, &self.key, id)?.ok_or(StorageError::Transition)?;
        if old.state == ReservationState::Confirmed
            && old.transaction == Some(transaction)
            && old.inclusion == Some(block)
        {
            return Ok(());
        }
        if !matches!(
            old.state,
            ReservationState::Signed | ReservationState::Submitted
        ) || old.transaction != Some(transaction)
        {
            return Err(StorageError::Transition);
        }
        tx.execute("UPDATE reservations SET state=3,block_hash=?3,block_height=?4 WHERE scope=?1 AND id=?2", params![&self.key[..], &id.0[..], &block.hash.bytes()[..], &block.height.to_be_bytes::<32>()[..]])?;
        tx.commit()?;
        Ok(())
    }
    /// Remove a previously recorded inclusion after its canonicality is lost.
    /// Compare the exact old observation under the writer lock; retain signed
    /// bytes, input/nonce claims and cursors. No funds are made reusable.
    pub fn invalidate_inclusion(&mut self, id: ReservationId, expected: Checkpoint) -> Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let old = reservation_read(&tx, &self.key, id)?.ok_or(StorageError::Transition)?;
        if old.state != ReservationState::Confirmed || old.inclusion != Some(expected) {
            return Err(StorageError::StaleSnapshot);
        }
        tx.execute("UPDATE reservations SET state=2,block_hash=NULL,block_height=NULL WHERE scope=?1 AND id=?2", params![&self.key[..], &id.0[..]])?;
        tx.commit()?;
        Ok(())
    }
    /// Reopen an explicitly released, never-signed account nonce for gap repair.
    /// Retains the original operation and nonce claim; no cursor rewinds or
    /// arbitrary nonce takeover. Signed operations are never eligible.
    pub fn reopen_unsigned_nonce(&mut self, id: ReservationId) -> Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let old = reservation_read(&tx, &self.key, id)?.ok_or(StorageError::Transition)?;
        if old.state != ReservationState::Released || old.transaction.is_some() {
            return Err(StorageError::Transition);
        }
        let count: i64 = tx.query_row(
            "SELECT count(*) FROM nonce_claims WHERE scope=?1 AND operation=?2",
            params![&self.key[..], &id.0[..]],
            |row| row.get(0),
        )?;
        if count != 1 {
            return Err(StorageError::Transition);
        }
        tx.execute(
            "UPDATE reservations SET state=0 WHERE scope=?1 AND id=?2",
            params![&self.key[..], &id.0[..]],
        )?;
        tx.commit()?;
        Ok(())
    }
    /// Explicitly cancel an unsigned, unexposed reservation. Signed/submitted/
    /// confirmed operations cannot be released, including after snapshot invalidation.
    /// Account nonce cursors never rewind; applications must handle any nonce gap.
    pub fn release_unsigned(&mut self, id: ReservationId) -> Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let old = reservation_read(&tx, &self.key, id)?.ok_or(StorageError::Transition)?;
        if old.state != ReservationState::Reserved || old.transaction.is_some() {
            return Err(StorageError::Transition);
        }
        tx.execute(
            "DELETE FROM qi_claims WHERE scope=?1 AND operation=?2",
            params![&self.key[..], &id.0[..]],
        )?;
        tx.execute(
            "UPDATE reservations SET state=4 WHERE scope=?1 AND id=?2",
            params![&self.key[..], &id.0[..]],
        )?;
        tx.commit()?;
        Ok(())
    }
}
fn signed_payload_read(
    connection: &Connection,
    key: &[u8],
    scope: NetworkScope,
    id: ReservationId,
) -> Result<Option<Vec<u8>>> {
    let length: Option<i64> = connection
        .query_row(
            "SELECT length(payload) FROM signed_payloads WHERE scope=?1 AND operation=?2",
            params![key, &id.0[..]],
            |r| r.get(0),
        )
        .optional()?;
    let Some(length) = length else {
        return Ok(None);
    };
    if length <= 0 || length > MAX_TRANSACTION_BYTES as i64 {
        return Err(StorageError::Invalid);
    }
    let (kind, payload): (i64, Vec<u8>) = connection.query_row(
        "SELECT kind,payload FROM signed_payloads WHERE scope=?1 AND operation=?2",
        params![key, &id.0[..]],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let operation = reservation_read(connection, key, id)?.ok_or(StorageError::Invalid)?;
    let hash = if kind == 1 {
        let signed = SignedQuaiTransaction::decode(&payload).map_err(|_| StorageError::Invalid)?;
        if signed.transaction().chain_id != scope.chain_id || signed.from().zone() != scope.zone {
            return Err(StorageError::Invalid);
        }
        validate_quai_claim(connection, key, id, &signed)?;
        signed.hash().map_err(|_| StorageError::Invalid)?
    } else if kind == 0 {
        let signed = quai_consensus::SignedQiOperation::decode(&payload)
            .map_err(|_| StorageError::Invalid)?;
        let hash = signed.hash().map_err(|_| StorageError::Invalid)?;
        if signed.transaction().chain_id != scope.chain_id || hash.bytes()[0] != scope.zone.byte() {
            return Err(StorageError::Invalid);
        }
        validate_qi_claim(connection, key, id, signed.transaction())?;
        hash
    } else {
        return Err(StorageError::Invalid);
    };
    if operation.transaction != Some(hash)
        || matches!(
            operation.state,
            ReservationState::Reserved | ReservationState::Released
        )
    {
        return Err(StorageError::Invalid);
    }
    Ok(Some(payload))
}
fn insert_address(
    connection: &Connection,
    key: &[u8],
    scope: NetworkScope,
    address: &PublicAddress,
) -> Result<()> {
    if address.address.zone().map_err(|_| StorageError::Invalid)? != scope.zone {
        return Err(StorageError::Invalid);
    }
    let origin = address.origin.encode();
    let old: Option<(Vec<u8>, Vec<u8>)> = connection
        .prepare_cached("SELECT public_key,origin FROM addresses WHERE scope=?1 AND address=?2")?
        .query_row(params![key, &address.address.bytes()[..]], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?;
    if let Some((old_key, old_origin)) = old {
        if old_key != address.public_key || old_origin != origin {
            return Err(StorageError::Conflict);
        }
    } else {
        connection.execute(
            "INSERT INTO addresses VALUES(?1,?2,?3,?4)",
            params![
                key,
                &address.address.bytes()[..],
                &address.public_key[..],
                origin
            ],
        )?;
    }
    if let KeyOrigin::Bip44 {
        coin,
        account,
        change,
        index,
    } = address.origin
    {
        let xpub:Option<String>=connection.query_row("SELECT account_xpub FROM derivation_cursors WHERE scope=?1 AND coin=?2 AND account=?3 LIMIT 1",params![key,coin.number(),account],|r|r.get(0)).optional()?;
        if let Some(xpub) = xpub {
            let node =
                AccountPublic::import(&xpub, coin, account).map_err(|_| StorageError::Invalid)?;
            let expected = node
                .derive_address(change, index)
                .map_err(|_| StorageError::Conflict)?;
            if expected.address != address.address || expected.public_key != address.public_key {
                return Err(StorageError::Conflict);
            }
        }
        connection.execute("UPDATE derivation_cursors SET next_index=max(next_index,?5) WHERE scope=?1 AND coin=?2 AND account=?3 AND change_branch=?4",params![key,coin.number(),account,change,index+1])?;
    }
    Ok(())
}
fn validate_quai_claim(
    connection: &Connection,
    key: &[u8],
    id: ReservationId,
    signed: &SignedQuaiTransaction,
) -> Result<()> {
    let count: i64 = connection.query_row("SELECT count(*) FROM nonce_claims WHERE scope=?1 AND operation=?2 AND address=?3 AND nonce=?4", params![key, &id.0[..], &signed.from().bytes()[..], &signed.transaction().nonce.to_be_bytes()[..]], |r| r.get(0))?;
    if count != 1 {
        return Err(StorageError::Conflict);
    }
    Ok(())
}
fn validate_qi_claim(
    connection: &Connection,
    key: &[u8],
    id: ReservationId,
    transaction: &quai_consensus::QiTransaction,
) -> Result<()> {
    let count: i64 = connection.query_row(
        "SELECT count(*) FROM qi_claims WHERE scope=?1 AND operation=?2",
        params![key, &id.0[..]],
        |r| r.get(0),
    )?;
    if count as usize != transaction.inputs.len() || count == 0 {
        return Err(StorageError::Conflict);
    }
    for input in &transaction.inputs {
        let owner: Option<Vec<u8>> = connection.query_row("SELECT address FROM qi_claims WHERE scope=?1 AND operation=?2 AND tx_hash=?3 AND output_index=?4", params![key,&id.0[..],&input.previous_output.transaction_hash.bytes()[..],input.previous_output.index], |r|r.get(0)).optional()?;
        if owner.as_deref() != Some(&input.public_key.address().bytes()[..]) {
            return Err(StorageError::Conflict);
        }
    }
    Ok(())
}
fn commit_payload(
    connection: &Connection,
    key: &[u8],
    id: ReservationId,
    hash: Hash32,
    kind: i64,
    payload: &[u8],
) -> Result<()> {
    if payload.is_empty() || payload.len() > MAX_TRANSACTION_BYTES {
        return Err(StorageError::Invalid);
    }
    let operation = reservation_read(connection, key, id)?.ok_or(StorageError::Transition)?;
    let expected_kind: i64 = connection.query_row(
        "SELECT kind FROM reservations WHERE scope=?1 AND id=?2",
        params![key, &id.0[..]],
        |r| r.get(0),
    )?;
    if expected_kind != kind
        || operation
            .transaction
            .is_some_and(|previous| previous != hash)
        || operation.state == ReservationState::Released
    {
        return Err(StorageError::Transition);
    }
    let previous: Option<bool> = connection
        .query_row(
            "SELECT payload=?3 FROM signed_payloads WHERE scope=?1 AND operation=?2",
            params![key, &id.0[..], payload],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(previous) = previous {
        if !previous {
            return Err(StorageError::Conflict);
        }
        return Ok(());
    }
    connection.execute(
        "INSERT INTO signed_payloads VALUES(?1,?2,?3,?4)",
        params![key, &id.0[..], kind, payload],
    )?;
    released::burn_signed(connection, key, payload)?;
    if operation.state == ReservationState::Reserved {
        connection.execute(
            "UPDATE reservations SET state=1,transaction_hash=?3 WHERE scope=?1 AND id=?2",
            params![key, &id.0[..], &hash.bytes()[..]],
        )?;
    }
    Ok(())
}
fn array<const N: usize>(bytes: &[u8]) -> Result<[u8; N]> {
    bytes.try_into().map_err(|_| StorageError::Invalid)
}
fn checkpoint_read(connection: &Connection, key: &[u8]) -> Result<(i64, Option<Checkpoint>)> {
    let (generation, hash, height): (i64, Option<Vec<u8>>, Option<Vec<u8>>) = connection
        .query_row(
            "SELECT generation,block_hash,height FROM scopes WHERE scope=?1",
            [key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
    if generation < 0 {
        return Err(StorageError::Invalid);
    }
    let checkpoint = match (hash, height) {
        (None, None) => None,
        (Some(hash), Some(height)) => Some(Checkpoint {
            hash: Hash32::from_bytes(array(&hash)?),
            height: U256::from_be_bytes(array::<32>(&height)?),
        }),
        _ => return Err(StorageError::Invalid),
    };
    Ok((generation, checkpoint))
}
fn next_generation(connection: &Connection, key: &[u8], expected: u64) -> Result<i64> {
    let current = checkpoint_read(connection, key)?.0;
    if u64::try_from(current).ok() != Some(expected) {
        return Err(StorageError::StaleSnapshot);
    }
    current.checked_add(1).ok_or(StorageError::Overflow)
}
/// Stored coin columns by outpoint: owner, denomination, unlock height, expiry.
type CoinRows = BTreeMap<([u8; 32], u16), ([u8; 20], u8, [u8; 32], Option<[u8; 32]>)>;
fn coin_rows(connection: &Connection, key: &[u8]) -> Result<CoinRows> {
    let mut statement = connection.prepare("SELECT tx_hash,output_index,address,denomination,unlock_height,expires_at FROM coins WHERE scope=?1")?;
    let mut rows = statement.query([key])?;
    let mut result = CoinRows::new();
    while let Some(row) = rows.next()? {
        result.insert(
            (array(&row.get::<_, Vec<u8>>(0)?)?, row.get(1)?),
            (
                array(&row.get::<_, Vec<u8>>(2)?)?,
                row.get(3)?,
                array(&row.get::<_, Vec<u8>>(4)?)?,
                row.get::<_, Option<Vec<u8>>>(5)?
                    .map(|v| array(&v))
                    .transpose()?,
            ),
        );
    }
    Ok(result)
}
fn clear_snapshot(connection: &Connection, key: &[u8], next: i64) -> Result<()> {
    connection.execute("DELETE FROM coins WHERE scope=?1", [key])?;
    connection.execute(
        "UPDATE scopes SET generation=?2,block_hash=NULL,height=NULL WHERE scope=?1",
        params![key, next],
    )?;
    Ok(())
}
fn require_address(connection: &Connection, key: &[u8], address: Address) -> Result<()> {
    let exists: bool = connection
        .prepare_cached("SELECT EXISTS(SELECT 1 FROM addresses WHERE scope=?1 AND address=?2)")?
        .query_row(params![key, &address.bytes()[..]], |r| r.get(0))?;
    if exists {
        Ok(())
    } else {
        Err(StorageError::Invalid)
    }
}
fn insert_reservation(
    connection: &Connection,
    key: &[u8],
    id: ReservationId,
    kind: i64,
) -> Result<()> {
    let changed = connection.execute(
        "INSERT OR IGNORE INTO reservations(scope,id,kind,state) VALUES(?1,?2,?3,0)",
        params![key, &id.0[..], kind],
    )?;
    if changed == 1 {
        Ok(())
    } else {
        Err(StorageError::Conflict)
    }
}
fn reservation_read(
    connection: &Connection,
    key: &[u8],
    id: ReservationId,
) -> Result<Option<Reservation>> {
    type RawReservation = (i64, Option<Vec<u8>>, Option<Vec<u8>>, Option<Vec<u8>>);
    let raw: Option<RawReservation> = connection.query_row("SELECT state,transaction_hash,block_hash,block_height FROM reservations WHERE scope=?1 AND id=?2", params![key,&id.0[..]], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let Some((state, transaction, hash, height)) = raw else {
        return Ok(None);
    };
    let state = match state {
        0 => ReservationState::Reserved,
        1 => ReservationState::Signed,
        2 => ReservationState::Submitted,
        3 => ReservationState::Confirmed,
        4 => ReservationState::Released,
        _ => return Err(StorageError::Invalid),
    };
    let transaction = transaction
        .map(|v| array(&v).map(Hash32::from_bytes))
        .transpose()?;
    let inclusion = match (hash, height) {
        (None, None) => None,
        (Some(hash), Some(height)) => Some(Checkpoint {
            hash: Hash32::from_bytes(array(&hash)?),
            height: U256::from_be_bytes(array::<32>(&height)?),
        }),
        _ => return Err(StorageError::Invalid),
    };
    if matches!(
        state,
        ReservationState::Reserved | ReservationState::Released
    ) != transaction.is_none()
        || (state == ReservationState::Confirmed) != inclusion.is_some()
    {
        return Err(StorageError::Invalid);
    }
    Ok(Some(Reservation {
        id,
        state,
        transaction,
        inclusion,
    }))
}
const SCHEMA: &str = "
CREATE TABLE scopes(scope BLOB PRIMARY KEY CHECK(length(scope)=65),generation INTEGER NOT NULL CHECK(generation>=0),block_hash BLOB CHECK(length(block_hash)=32),height BLOB CHECK(length(height)=32),CHECK((block_hash IS NULL)=(height IS NULL))) STRICT;
CREATE TABLE addresses(scope BLOB NOT NULL REFERENCES scopes(scope),address BLOB NOT NULL CHECK(length(address)=20),public_key BLOB NOT NULL CHECK(length(public_key)=33),origin BLOB NOT NULL CHECK(length(origin) IN(1,11)),PRIMARY KEY(scope,address)) STRICT;
CREATE TABLE coins(scope BLOB NOT NULL,tx_hash BLOB NOT NULL CHECK(length(tx_hash)=32),output_index INTEGER NOT NULL CHECK(output_index BETWEEN 0 AND 65535),generation INTEGER NOT NULL,address BLOB NOT NULL,denomination INTEGER NOT NULL CHECK(denomination BETWEEN 0 AND 14),unlock_height BLOB NOT NULL CHECK(length(unlock_height)=32),expires_at BLOB CHECK(length(expires_at)=32),PRIMARY KEY(scope,tx_hash,output_index),FOREIGN KEY(scope,address) REFERENCES addresses(scope,address)) STRICT;
CREATE TABLE reservations(scope BLOB NOT NULL REFERENCES scopes(scope),id BLOB NOT NULL CHECK(length(id)=16),kind INTEGER NOT NULL CHECK(kind IN(0,1)),state INTEGER NOT NULL CHECK(state BETWEEN 0 AND 4),transaction_hash BLOB CHECK(length(transaction_hash)=32),block_hash BLOB CHECK(length(block_hash)=32),block_height BLOB CHECK(length(block_height)=32),PRIMARY KEY(scope,id),CHECK((state IN(0,4))=(transaction_hash IS NULL)),CHECK((state=3)=(block_hash IS NOT NULL)),CHECK((block_hash IS NULL)=(block_height IS NULL))) STRICT;
CREATE TABLE qi_claims(scope BLOB NOT NULL,tx_hash BLOB NOT NULL CHECK(length(tx_hash)=32),output_index INTEGER NOT NULL CHECK(output_index BETWEEN 0 AND 65535),operation BLOB NOT NULL,address BLOB NOT NULL CHECK(length(address)=20),PRIMARY KEY(scope,tx_hash,output_index),FOREIGN KEY(scope,operation) REFERENCES reservations(scope,id),FOREIGN KEY(scope,address) REFERENCES addresses(scope,address)) STRICT;
CREATE TABLE nonce_cursors(scope BLOB NOT NULL,address BLOB NOT NULL,next_nonce BLOB NOT NULL CHECK(length(next_nonce)=8),PRIMARY KEY(scope,address),FOREIGN KEY(scope,address) REFERENCES addresses(scope,address)) STRICT;
CREATE TABLE nonce_claims(scope BLOB NOT NULL,address BLOB NOT NULL,nonce BLOB NOT NULL CHECK(length(nonce)=8),operation BLOB NOT NULL,PRIMARY KEY(scope,address,nonce),FOREIGN KEY(scope,operation) REFERENCES reservations(scope,id),FOREIGN KEY(scope,address) REFERENCES addresses(scope,address)) STRICT;
CREATE TABLE signed_payloads(scope BLOB NOT NULL,operation BLOB NOT NULL,kind INTEGER NOT NULL CHECK(kind IN(0,1)),payload BLOB NOT NULL CHECK(length(payload) BETWEEN 1 AND 1048576),PRIMARY KEY(scope,operation),FOREIGN KEY(scope,operation) REFERENCES reservations(scope,id)) STRICT;
CREATE TABLE derivation_cursors(scope BLOB NOT NULL REFERENCES scopes(scope),coin INTEGER NOT NULL CHECK(coin IN(994,969)),account INTEGER NOT NULL CHECK(account BETWEEN 0 AND 2147483647),change_branch INTEGER NOT NULL CHECK(change_branch IN(0,1)),account_xpub TEXT NOT NULL CHECK(length(account_xpub) BETWEEN 100 AND 112),next_index INTEGER NOT NULL CHECK(next_index BETWEEN 0 AND 2147483648),PRIMARY KEY(scope,coin,account,change_branch)) STRICT;
CREATE UNIQUE INDEX nonce_claim_operations ON nonce_claims(scope,operation);
CREATE UNIQUE INDEX address_origins ON addresses(scope,origin) WHERE length(origin)=11;
CREATE INDEX qi_claim_operations ON qi_claims(scope,operation);
";

#[cfg(test)]
mod tests;
