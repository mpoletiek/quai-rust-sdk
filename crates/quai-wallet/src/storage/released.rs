//! Qi change addresses returned before any signature, reissued lowest first.
//! See docs/QI_CHANGE_REUSE.md for the review of this relaxation.
use super::*;
pub(super) const RELEASED_SCHEMA: &str = "CREATE TABLE released_change(scope BLOB NOT NULL,account INTEGER NOT NULL CHECK(account BETWEEN 0 AND 2147483647),child_index INTEGER NOT NULL CHECK(child_index BETWEEN 0 AND 2147483647),address BLOB NOT NULL CHECK(length(address)=20),PRIMARY KEY(scope,account,child_index),FOREIGN KEY(scope,address) REFERENCES addresses(scope,address)) STRICT;";
/// Most addresses one release or take handles, the largest change pool.
const MAX_RELEASE: usize = 1024;

impl SqliteStore {
    /// Return Qi change addresses that never appeared in a signed payload, so
    /// [`Self::take_released_change`] hands them out again, lowest index first.
    /// Each must be this account's stored BIP44 change address below the
    /// branch cursor. An address whose bytes appear in any stored signed
    /// payload or replacement is refused with `Transition`; this check reads
    /// every stored payload once per address. At most 1,024 addresses; nothing
    /// is written on error. Released addresses are not backed up, and a
    /// restore clears them, so they stay burned on another device.
    pub fn release_change(
        &mut self,
        account: &AccountPublic,
        addresses: &[PublicAddress],
    ) -> Result<()> {
        if account.coin_type() != CoinType::Qi || addresses.len() > MAX_RELEASE {
            return Err(StorageError::Invalid);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let cursor = change_cursor(&tx, &self.key, account)?.ok_or(StorageError::Invalid)?;
        for address in addresses {
            let KeyOrigin::Bip44 {
                coin: CoinType::Qi,
                account: owner,
                change: true,
                index,
            } = address.origin
            else {
                return Err(StorageError::Invalid);
            };
            if owner != account.account_index()
                || index >= cursor
                || PublicAddress::derive(account, true, index)? != *address
            {
                return Err(StorageError::Invalid);
            }
            require_address(&tx, &self.key, address.address)?;
            let signed: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM signed_payloads WHERE scope=?1 AND instr(payload,?2)>0) OR EXISTS(SELECT 1 FROM quai_replacements WHERE scope=?1 AND instr(payload,?2)>0)",
                params![&self.key[..], &address.address.bytes()[..]],
                |r| r.get(0),
            )?;
            if signed {
                return Err(StorageError::Transition);
            }
            tx.execute(
                "INSERT OR IGNORE INTO released_change VALUES(?1,?2,?3,?4)",
                params![&self.key[..], owner, index, &address.address.bytes()[..]],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    /// Take up to `max` released change addresses of this account, lowest index
    /// first. They are already stored metadata, so unlike a fresh allocation
    /// this neither invalidates the snapshot nor moves the generation. Each
    /// is removed under the write lock, so no two handles get the same one.
    pub fn take_released_change(
        &mut self,
        account: &AccountPublic,
        max: usize,
    ) -> Result<Vec<PublicAddress>> {
        if account.coin_type() != CoinType::Qi || max > MAX_RELEASE {
            return Err(StorageError::Invalid);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if max == 0 || change_cursor(&tx, &self.key, account)?.is_none() {
            return Ok(Vec::new());
        }
        let indexes: Vec<u32> = tx
            .prepare("SELECT child_index FROM released_change WHERE scope=?1 AND account=?2 ORDER BY child_index LIMIT ?3")?
            .query_map(params![&self.key[..], account.account_index(), max as i64], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        let mut taken = Vec::with_capacity(indexes.len());
        for index in indexes {
            let address = PublicAddress::derive(account, true, index)?;
            let removed = tx.execute(
                "DELETE FROM released_change WHERE scope=?1 AND account=?2 AND child_index=?3 AND address=?4",
                params![
                    &self.key[..],
                    account.account_index(),
                    index,
                    &address.address.bytes()[..]
                ],
            )?;
            if removed != 1 {
                return Err(StorageError::Invalid);
            }
            taken.push(address);
        }
        tx.commit()?;
        Ok(taken)
    }
}

/// The account's change-branch cursor, checked against its bound xpub.
fn change_cursor(
    connection: &Connection,
    key: &[u8],
    account: &AccountPublic,
) -> Result<Option<u32>> {
    let row: Option<(String, u32)> = connection
        .query_row(
            "SELECT account_xpub,next_index FROM derivation_cursors WHERE scope=?1 AND coin=?2 AND account=?3 AND change_branch=1",
            params![key, CoinType::Qi.number(), account.account_index()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    match row {
        Some((xpub, _)) if xpub != account.export() => Err(StorageError::Conflict),
        row => Ok(row.map(|(_, next)| next)),
    }
}

/// Signing any payload burns every released address its bytes contain, so an
/// address that reached a signature by any path is never handed out again.
pub(super) fn burn_signed(connection: &Connection, key: &[u8], payload: &[u8]) -> Result<()> {
    connection.execute(
        "DELETE FROM released_change WHERE scope=?1 AND instr(?2,address)>0",
        params![key, payload],
    )?;
    Ok(())
}
