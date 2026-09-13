//! Disposable public observation caches. Signed claims remain the recovery authority.
use super::*;
pub(super) const OBSERVATION_SCHEMA: &str = "CREATE TABLE observation_cache(scope BLOB NOT NULL,operation BLOB NOT NULL,candidate BLOB NOT NULL CHECK(length(candidate)=32),slot INTEGER NOT NULL CHECK(slot BETWEEN 0 AND 65535),revision INTEGER NOT NULL CHECK(revision>0),payload BLOB CHECK(payload IS NULL OR length(payload) BETWEEN 1 AND 4096),PRIMARY KEY(scope,operation,candidate,slot),FOREIGN KEY(scope,operation) REFERENCES reservations(scope,id)) STRICT;";
/// Versioned public source observation. A tombstone retains its revision to avoid ABA.
/// Never use cached data as proof of canonicality, spendability or claim release.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservationCache {
    /// Monotonic local revision, including invalidations.
    pub revision: u64,
    /// Application-encoded public observation, or an explicit invalidation tombstone.
    pub payload: Option<Vec<u8>>,
}
fn candidate_exists(
    connection: &Connection,
    key: &[u8],
    scope: NetworkScope,
    id: ReservationId,
    candidate: Hash32,
) -> Result<()> {
    let root = signed_payload_read(connection, key, scope, id)?.ok_or(StorageError::Invalid)?;
    let variants = replacements::read_variants(connection, key, id)?;
    replacements::validate_family(&root, &variants)?;
    let hash = |bytes: &[u8]| -> Result<Hash32> {
        if let Ok(qi) = quai_consensus::SignedQiOperation::decode(bytes) {
            return qi.hash().map_err(|_| StorageError::Invalid);
        }
        SignedQuaiTransaction::decode(bytes)
            .and_then(|tx| tx.hash())
            .map_err(|_| StorageError::Invalid)
    };
    if hash(&root)? == candidate
        || variants
            .iter()
            .any(|v| hash(&v.payload).ok() == Some(candidate))
    {
        Ok(())
    } else {
        Err(StorageError::Invalid)
    }
}
impl SqliteStore {
    /// Current scope generation without copying its UTXO snapshot. Capture this
    /// before asynchronous observations and use the scoped CAS methods below.
    pub fn observation_generation(&self) -> Result<u64> {
        Ok(checkpoint_read(&self.connection, &self.key)?.0 as u64)
    }
    /// Read a public observation for an exact persisted signed candidate and slot
    /// (for example, its external output index).
    /// Caches are excluded from full backups and cleared on restore; reconstruct
    /// references from signed bytes and requery canonical state after recovery.
    pub fn observation_cache(
        &mut self,
        id: ReservationId,
        candidate: Hash32,
        slot: u16,
    ) -> Result<Option<ObservationCache>> {
        let tx = self.connection.transaction()?;
        candidate_exists(&tx, &self.key, self.scope, id, candidate)?;
        let value = tx.query_row("SELECT revision,payload FROM observation_cache WHERE scope=?1 AND operation=?2 AND candidate=?3 AND slot=?4", params![&self.key[..], &id.0[..], &candidate.bytes()[..], slot], |r| Ok(ObservationCache { revision: r.get::<_, i64>(0)?.try_into().map_err(|_| rusqlite::Error::InvalidQuery)?, payload: r.get(1)? })).optional()?;
        tx.commit()?;
        Ok(value)
    }
    /// Atomically replace a public observation of at most 4096 bytes after source
    /// revalidation. At most 2048 candidate/slot caches are held per operation.
    /// `None` expected revision means never written; `None` payload invalidates.
    /// Concurrent observers must reread/revalidate after a revision conflict.
    /// No keys, credentials or private application content belong in this cache.
    /// This slot-only convenience does not fence asynchronous scope changes;
    /// capture `observation_generation` and use the scoped method for node reads.
    pub fn compare_exchange_observation(
        &mut self,
        id: ReservationId,
        candidate: Hash32,
        slot: u16,
        expected_revision: Option<u64>,
        payload: Option<&[u8]>,
    ) -> Result<u64> {
        self.compare_exchange_observation_scoped(
            id,
            candidate,
            slot,
            (self.observation_generation()?, expected_revision),
            payload,
        )
    }
    /// Commit only if both the pre-RPC scope generation and cache revision still
    /// match. Reorg invalidation also fences observers whose cache slot has never
    /// existed; a slot-only compare-and-swap cannot detect that race.
    pub fn compare_exchange_observation_scoped(
        &mut self,
        id: ReservationId,
        candidate: Hash32,
        slot: u16,
        expected: (u64, Option<u64>),
        payload: Option<&[u8]>,
    ) -> Result<u64> {
        if payload.is_some_and(|bytes| bytes.is_empty() || bytes.len() > 4096) {
            return Err(StorageError::Invalid);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if checkpoint_read(&tx, &self.key)?.0 as u64 != expected.0 {
            return Err(StorageError::StaleSnapshot);
        }
        candidate_exists(&tx, &self.key, self.scope, id, candidate)?;
        let revision = write_observation(&tx, &self.key, id, candidate, slot, expected.1, payload)?;
        tx.commit()?;
        Ok(revision)
    }
    /// Save a family summary only if its ordered root/replacement identities and
    /// old cache revision still match under the same SQLite writer lock.
    /// Slot 65535 on the root candidate is reserved for these summaries.
    /// Signed claims and payloads are never changed by this cache operation.
    /// Use the scoped variant when a node observation spans asynchronous work.
    pub fn compare_exchange_family_observation(
        &mut self,
        id: ReservationId,
        expected_candidates: &[Hash32],
        expected_revision: Option<u64>,
        payload: Option<&[u8]>,
    ) -> Result<u64> {
        self.compare_exchange_family_observation_scoped(
            id,
            expected_candidates,
            (self.observation_generation()?, expected_revision),
            payload,
        )
    }
    /// Family CAS including the scope generation captured before node reads.
    /// Rejects stale first-time observers after a reorg or wallet restore.
    pub fn compare_exchange_family_observation_scoped(
        &mut self,
        id: ReservationId,
        expected_candidates: &[Hash32],
        expected: (u64, Option<u64>),
        payload: Option<&[u8]>,
    ) -> Result<u64> {
        if expected_candidates.is_empty()
            || expected_candidates.len() > 33
            || payload.is_some_and(|p| p.is_empty() || p.len() > 4096)
        {
            return Err(StorageError::Invalid);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if checkpoint_read(&tx, &self.key)?.0 as u64 != expected.0 {
            return Err(StorageError::StaleSnapshot);
        }
        let root =
            signed_payload_read(&tx, &self.key, self.scope, id)?.ok_or(StorageError::Invalid)?;
        let variants = replacements::read_variants(&tx, &self.key, id)?;
        replacements::validate_family(&root, &variants)?;
        let actual: Vec<_> = std::iter::once(root)
            .chain(variants.into_iter().map(|v| v.payload))
            .map(|bytes| {
                if let Ok(qi) = quai_consensus::SignedQiOperation::decode(&bytes) {
                    qi.hash().map_err(|_| StorageError::Invalid)
                } else {
                    SignedQuaiTransaction::decode(&bytes)
                        .and_then(|tx| tx.hash())
                        .map_err(|_| StorageError::Invalid)
                }
            })
            .collect::<Result<_>>()?;
        if actual != expected_candidates {
            return Err(StorageError::Conflict);
        }
        let revision = write_observation(
            &tx,
            &self.key,
            id,
            expected_candidates[0],
            u16::MAX,
            expected.1,
            payload,
        )?;
        tx.commit()?;
        Ok(revision)
    }
}

fn write_observation(
    connection: &Connection,
    key: &[u8],
    id: ReservationId,
    candidate: Hash32,
    slot: u16,
    expected_revision: Option<u64>,
    payload: Option<&[u8]>,
) -> Result<u64> {
    let old: Option<i64> = connection.query_row("SELECT revision FROM observation_cache WHERE scope=?1 AND operation=?2 AND candidate=?3 AND slot=?4", params![key, &id.0[..], &candidate.bytes()[..], slot], |r| r.get(0)).optional()?;
    let old = old
        .map(u64::try_from)
        .transpose()
        .map_err(|_| StorageError::Invalid)?;
    if old != expected_revision {
        return Err(StorageError::Conflict);
    }
    if old.is_none() {
        let count: i64 = connection.query_row(
            "SELECT count(*) FROM observation_cache WHERE scope=?1 AND operation=?2",
            params![key, &id.0[..]],
            |r| r.get(0),
        )?;
        if count >= 2048 {
            return Err(StorageError::Invalid);
        }
    }
    let revision = old
        .unwrap_or(0)
        .checked_add(1)
        .filter(|n| *n <= i64::MAX as u64)
        .ok_or(StorageError::Overflow)?;
    connection.execute("INSERT INTO observation_cache VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(scope,operation,candidate,slot) DO UPDATE SET revision=excluded.revision,payload=excluded.payload", params![key, &id.0[..], &candidate.bytes()[..], slot, revision as i64, payload])?;
    Ok(revision)
}
