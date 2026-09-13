//! Atomic conservative rollback of source observations; never roll back custody.
use super::*;

/// Public state invalidated in one SQLite transaction following a trusted reorg.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReorgInvalidation {
    /// New scope generation; old discovery and observation writers are stale.
    pub generation: u64,
    /// Origin inclusion records reverted to Submitted, retaining exact bytes.
    pub inclusions: usize,
    /// Public cache slots tombstoned, including old tombstones whose revisions advance.
    pub caches: usize,
}
impl SqliteStore {
    /// Invalidate a reorganized suffix beginning at this nonzero zone height.
    /// The caller must first establish the fork through canonical node reads.
    /// All affected origin inclusions, all scope observation caches and the entire
    /// current coin snapshot change atomically. Signed bytes, candidates, claims,
    /// addresses and derivation/nonce cursors are retained. Requery discoveries
    /// and operation observations before presenting current balances/settlement.
    /// This conservative operation may invalidate a concurrently refreshed valid
    /// inclusion in the same suffix; it never makes signed inputs reusable.
    pub fn invalidate_reorg_from(
        &mut self,
        expected_generation: u64,
        first_removed_height: U256,
    ) -> Result<ReorgInvalidation> {
        if first_removed_height == U256::ZERO {
            return Err(StorageError::Invalid);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = invalidate(&tx, &self.key, expected_generation, first_removed_height)?;
        tx.commit()?;
        Ok(result)
    }
}

pub(super) fn invalidate(
    connection: &Connection,
    key: &[u8],
    expected_generation: u64,
    first_removed_height: U256,
) -> Result<ReorgInvalidation> {
    if first_removed_height == U256::ZERO {
        return Err(StorageError::Invalid);
    }
    let next = next_generation(connection, key, expected_generation)?;
    let overflow: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM observation_cache WHERE scope=?1 AND revision=9223372036854775807)",
            [key], |r| r.get(0),
        )?;
    if overflow {
        return Err(StorageError::Overflow);
    }
    let inclusions = connection.execute(
            "UPDATE reservations SET state=2,block_hash=NULL,block_height=NULL WHERE scope=?1 AND state=3 AND block_height>=?2",
            params![key, &first_removed_height.to_be_bytes::<32>()[..]],
        )?;
    let caches = connection.execute(
        "UPDATE observation_cache SET revision=revision+1,payload=NULL WHERE scope=?1",
        [key],
    )?;
    clear_snapshot(connection, key, next)?;
    Ok(ReorgInvalidation {
        generation: next as u64,
        inclusions,
        caches,
    })
}
