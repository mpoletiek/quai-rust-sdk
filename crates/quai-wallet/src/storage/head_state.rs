//! Scoped public head cursor, atomically coupled to conservative reorg rollback.
use super::*;
/// Maximum public ancestry payload (the provider's 4,096-anchor cursor format).
pub const MAX_HEAD_REPLAY_BYTES: usize = 163_887;
pub(super) const HEAD_SCHEMA: &str = "CREATE TABLE head_replay(scope BLOB PRIMARY KEY,revision INTEGER NOT NULL CHECK(revision>0),payload BLOB CHECK(payload IS NULL OR length(payload) BETWEEN 1 AND 163887),FOREIGN KEY(scope) REFERENCES scopes(scope)) STRICT;";
/// Public ancestry bytes with a monotonic revision; tombstones prevent stale reinsertion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeadReplayState {
    /// Revision for compare-and-exchange, including cleared state.
    pub revision: u64,
    /// Application-validated public cursor, absent after explicit reset or backup restore.
    pub payload: Option<Vec<u8>>,
}
/// Result of atomically persisting a replay cursor and any required rollback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeadReplayCommit {
    /// New durable cursor revision.
    pub revision: u64,
    /// Required reorg invalidation performed inside the same transaction.
    pub invalidation: Option<ReorgInvalidation>,
}
fn read(connection: &Connection, key: &[u8]) -> Result<Option<HeadReplayState>> {
    let row: Option<(i64, Option<Vec<u8>>)> = connection
        .query_row(
            "SELECT revision,payload FROM head_replay WHERE scope=?1",
            [key],
            |r| {
                // Check SQLite's borrowed value before cloning it into Rust.
                let payload = match r.get_ref(1)? {
                    rusqlite::types::ValueRef::Null => None,
                    rusqlite::types::ValueRef::Blob(bytes)
                        if !bytes.is_empty() && bytes.len() <= MAX_HEAD_REPLAY_BYTES =>
                    {
                        Some(bytes.to_vec())
                    }
                    _ => return Err(rusqlite::Error::InvalidQuery),
                };
                Ok((r.get(0)?, payload))
            },
        )
        .optional()?;
    row.map(|(revision, payload)| {
        if revision <= 0
            || payload
                .as_ref()
                .is_some_and(|b| b.is_empty() || b.len() > MAX_HEAD_REPLAY_BYTES)
        {
            return Err(StorageError::Invalid);
        }
        Ok(HeadReplayState {
            revision: revision as u64,
            payload,
        })
    })
    .transpose()
}
pub(super) fn clear(connection: &Connection, key: &[u8]) -> Result<()> {
    if let Some(state) = read(connection, key)? {
        let revision = (state.revision as i64)
            .checked_add(1)
            .ok_or(StorageError::Overflow)?;
        connection.execute(
            "UPDATE head_replay SET revision=?2,payload=NULL WHERE scope=?1",
            params![key, revision],
        )?;
    }
    Ok(())
}
impl SqliteStore {
    /// Read the scope's public ancestry cursor. Its bytes require provider-level
    /// decoding and fresh canonical checks; they are not trusted chain evidence.
    pub fn head_replay_state(&self) -> Result<Option<HeadReplayState>> {
        read(&self.connection, &self.key)
    }
    /// Compare-and-exchange public ancestry and optionally invalidate a reorg suffix
    /// in the SAME writer transaction. Both generation and cursor revision must
    /// match pre-observation reads. Errors roll back the cursor and all wallet changes.
    /// The caller establishes the fork and validates the public cursor; this storage
    /// layer only bounds bytes. Never put keys here. `None` writes a tombstone.
    /// Signed custody, claims, exposures and monotonic nonce cursors are retained.
    pub fn commit_head_replay(
        &mut self,
        expected_generation: u64,
        expected_revision: Option<u64>,
        payload: Option<&[u8]>,
        first_removed_height: Option<U256>,
    ) -> Result<HeadReplayCommit> {
        if payload.is_some_and(|b| b.is_empty() || b.len() > MAX_HEAD_REPLAY_BYTES) {
            return Err(StorageError::Invalid);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if checkpoint_read(&tx, &self.key)?.0 as u64 != expected_generation {
            return Err(StorageError::StaleSnapshot);
        }
        let current = read(&tx, &self.key)?;
        if current.as_ref().map(|s| s.revision) != expected_revision {
            return Err(StorageError::StaleSnapshot);
        }
        let revision = current
            .map_or(0, |s| s.revision as i64)
            .checked_add(1)
            .ok_or(StorageError::Overflow)?;
        let invalidation = first_removed_height
            .map(|height| replay::invalidate(&tx, &self.key, expected_generation, height))
            .transpose()?;
        tx.execute("INSERT INTO head_replay(scope,revision,payload) VALUES(?1,?2,?3) ON CONFLICT(scope) DO UPDATE SET revision=excluded.revision,payload=excluded.payload", params![&self.key[..], revision, payload])?;
        tx.commit()?;
        Ok(HeadReplayCommit {
            revision: revision as u64,
            invalidation,
        })
    }
}
