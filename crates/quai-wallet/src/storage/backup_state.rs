//! Strictly typed public storage export/import; never stores secret origin bytes.
use super::*;
pub(crate) use crate::state::{
    DerivationState, NonceState, OperationState, PublicWalletState, ScopeState,
};

const MAX_SCOPES: usize = 64;
const MAX_RECORDS: usize = 100_000;

impl SqliteStore {
    pub(crate) fn capture_public_state(&mut self) -> Result<PublicWalletState> {
        let tx = self.connection.transaction()?;
        validate_native_schema(&tx)?;
        // Bound aggregate signed bytes before loading any payload-bearing operation.
        let payload_bytes: i64 = tx.query_row(
            "SELECT (SELECT coalesce(sum(length(payload)),0) FROM signed_payloads)+(SELECT coalesce(sum(length(payload)),0) FROM quai_replacements)",
            [],
            |r| r.get(0),
        )?;
        if !(0..=16 * 1024 * 1024).contains(&payload_bytes) {
            return Err(StorageError::Invalid);
        }
        let mut statement = tx.prepare("SELECT scope FROM scopes ORDER BY scope LIMIT 65")?;
        let keys: Vec<Vec<u8>> = statement
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        drop(statement);
        if keys.len() > MAX_SCOPES {
            return Err(StorageError::Invalid);
        }
        let mut scopes = Vec::new();
        let mut total = 0usize;
        for key in keys {
            if key.len() != 65 {
                return Err(StorageError::Invalid);
            }
            let scope = NetworkScope {
                chain_id: U256::from_be_bytes(array::<32>(&key[..32])?),
                genesis: Hash32::from_bytes(array(&key[32..64])?),
                zone: quai_primitives::Zone::from_byte(key[64])
                    .map_err(|_| StorageError::Invalid)?,
            };
            let mut state = ScopeState {
                scope,
                addresses: vec![],
                derivation: vec![],
                nonces: vec![],
                operations: vec![],
            };
            let mut statement=tx.prepare("SELECT address,public_key,origin FROM addresses WHERE scope=?1 ORDER BY address LIMIT 100001")?;
            let mut rows = statement.query([&key])?;
            while let Some(row) = rows.next()? {
                count(&mut total)?;
                state.addresses.push(PublicAddress::from_backup_parts(
                    Address::from_bytes(array(&row.get::<_, Vec<u8>>(0)?)?),
                    array(&row.get::<_, Vec<u8>>(1)?)?,
                    KeyOrigin::decode(&row.get::<_, Vec<u8>>(2)?)?,
                )?);
            }
            drop(rows);
            drop(statement);
            let mut statement=tx.prepare("SELECT coin,account,change_branch,account_xpub,next_index FROM derivation_cursors WHERE scope=?1 ORDER BY coin,account,change_branch LIMIT 100001")?;
            let mut rows = statement.query([&key])?;
            while let Some(row) = rows.next()? {
                count(&mut total)?;
                let coin = match row.get::<_, u32>(0)? {
                    994 => CoinType::Quai,
                    969 => CoinType::Qi,
                    _ => return Err(StorageError::Invalid),
                };
                state.derivation.push(DerivationState {
                    coin,
                    account: row.get(1)?,
                    change: row.get(2)?,
                    xpub: row.get(3)?,
                    next_index: row.get(4)?,
                });
            }
            drop(rows);
            drop(statement);
            let mut statement=tx.prepare("SELECT address,next_nonce FROM nonce_cursors WHERE scope=?1 ORDER BY address LIMIT 100001")?;
            let mut rows = statement.query([&key])?;
            while let Some(row) = rows.next()? {
                count(&mut total)?;
                state.nonces.push(NonceState {
                    address: QuaiAddress::try_from(array::<20>(&row.get::<_, Vec<u8>>(0)?)?)
                        .map_err(|_| StorageError::Invalid)?,
                    next_nonce: u64::from_be_bytes(array(&row.get::<_, Vec<u8>>(1)?)?),
                });
            }
            drop(rows);
            drop(statement);
            let mut statement =
                tx.prepare("SELECT id FROM reservations WHERE scope=?1 ORDER BY id LIMIT 100001")?;
            let mut rows = statement.query([&key])?;
            while let Some(row) = rows.next()? {
                count(&mut total)?;
                let id = ReservationId(array(&row.get::<_, Vec<u8>>(0)?)?);
                let operation =
                    read_operation(&tx, &key, scope, id)?.ok_or(StorageError::Invalid)?;
                total = total
                    .checked_add(operation.qi.len())
                    .ok_or(StorageError::Invalid)?;
                if total > MAX_RECORDS {
                    return Err(StorageError::Invalid);
                }
                state.operations.push(operation);
            }
            scopes.push(state);
        }
        let (channels, exposures) = payment::capture_channels(&tx, &mut total)?;
        tx.commit()?;
        Ok(PublicWalletState {
            scopes,
            channels,
            exposures,
        })
    }
    pub(crate) fn restore_public_state(
        &mut self,
        state: &PublicWalletState,
        owners: &[quai_payments::PrivatePaymentCode],
    ) -> Result<Vec<(NetworkScope, u64)>> {
        if state.scopes.len() > MAX_SCOPES || !state.scopes.iter().any(|s| s.scope == self.scope) {
            return Err(StorageError::Invalid);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate_native_schema(&tx)?;
        let mut generations = Vec::new();
        for scope_state in &state.scopes {
            let key = scope_state.scope.key();
            tx.execute(
                "INSERT OR IGNORE INTO scopes(scope,generation) VALUES(?1,0)",
                [&key[..]],
            )?;
            // Cached source observations are disposable and must be revalidated
            // after restoration, just like the UTXO snapshot checkpoint.
            tx.execute("DELETE FROM observation_cache WHERE scope=?1", [&key[..]])?;
            // Another device may have signed with a released address: burn them.
            tx.execute("DELETE FROM released_change WHERE scope=?1", [&key[..]])?;
            let current = checkpoint_read(&tx, &key)?.0;
            let next = next_generation(&tx, &key, current as u64)?;
            for address in &scope_state.addresses {
                insert_address(&tx, &key, scope_state.scope, address)?;
            }
            let total: i64 = tx.query_row(
                "SELECT count(*) FROM addresses WHERE scope=?1",
                [&key[..]],
                |r| r.get(0),
            )?;
            if total > MAX_RECORDS as i64 {
                return Err(StorageError::Invalid);
            }
            for cursor in &scope_state.derivation {
                let old:Option<String>=tx.query_row("SELECT account_xpub FROM derivation_cursors WHERE scope=?1 AND coin=?2 AND account=?3 LIMIT 1",params![&key[..],cursor.coin.number(),cursor.account],|r|r.get(0)).optional()?;
                if old.as_ref().is_some_and(|xpub| xpub != &cursor.xpub) {
                    return Err(StorageError::Conflict);
                }
                if old.is_none() {
                    let account = AccountPublic::import(&cursor.xpub, cursor.coin, cursor.account)
                        .map_err(|_| StorageError::Invalid)?;
                    let mut statement = tx.prepare(
                        "SELECT origin,address,public_key FROM addresses WHERE scope=?1",
                    )?;
                    let mut rows = statement.query([&key[..]])?;
                    while let Some(row) = rows.next()? {
                        if let KeyOrigin::Bip44 {
                            coin,
                            account: account_index,
                            change,
                            index,
                        } = KeyOrigin::decode(&row.get::<_, Vec<u8>>(0)?)?
                            && coin == cursor.coin
                            && account_index == cursor.account
                        {
                            let expected = account
                                .derive_address(change, index)
                                .map_err(|_| StorageError::Conflict)?;
                            if row.get::<_, Vec<u8>>(1)? != expected.address.bytes()
                                || row.get::<_, Vec<u8>>(2)? != expected.public_key
                            {
                                return Err(StorageError::Conflict);
                            }
                        }
                    }
                }
                tx.execute("INSERT INTO derivation_cursors VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(scope,coin,account,change_branch) DO UPDATE SET next_index=max(next_index,excluded.next_index)",params![&key[..],cursor.coin.number(),cursor.account,cursor.change,cursor.xpub,cursor.next_index])?;
            }
            // Cursors remain above both incoming and already existing public metadata.
            let mut statement = tx.prepare("SELECT origin FROM addresses WHERE scope=?1")?;
            let mut rows = statement.query([&key[..]])?;
            while let Some(row) = rows.next()? {
                if let KeyOrigin::Bip44 {
                    coin,
                    account,
                    change,
                    index,
                } = KeyOrigin::decode(&row.get::<_, Vec<u8>>(0)?)?
                {
                    tx.execute("UPDATE derivation_cursors SET next_index=max(next_index,?5) WHERE scope=?1 AND coin=?2 AND account=?3 AND change_branch=?4",params![&key[..],coin.number(),account,change,index+1])?;
                }
            }
            drop(rows);
            drop(statement);
            for nonce in &scope_state.nonces {
                let old: Option<Vec<u8>> = tx
                    .query_row(
                        "SELECT next_nonce FROM nonce_cursors WHERE scope=?1 AND address=?2",
                        params![&key[..], &nonce.address.bytes()[..]],
                        |r| r.get(0),
                    )
                    .optional()?;
                let value = nonce.next_nonce.max(
                    old.map(|v| array(&v).map(u64::from_be_bytes))
                        .transpose()?
                        .unwrap_or(0),
                );
                tx.execute("INSERT INTO nonce_cursors VALUES(?1,?2,?3) ON CONFLICT(scope,address) DO UPDATE SET next_nonce=excluded.next_nonce",params![&key[..],&nonce.address.bytes()[..],&value.to_be_bytes()[..]])?;
            }
            for operation in &scope_state.operations {
                let record = &operation.record;
                if let Some(old) = read_operation(&tx, &key, scope_state.scope, record.id)? {
                    // A backup import cannot downgrade, release or replace a live operation.
                    if old != *operation {
                        return Err(StorageError::Conflict);
                    }
                    continue;
                }
                tx.execute(
                    "INSERT INTO reservations VALUES(?1,?2,?3,?4,?5,?6,?7)",
                    params![
                        &key[..],
                        &record.id.0[..],
                        operation.kind,
                        record.state as i64,
                        record.transaction.as_ref().map(|v| &v.bytes()[..]),
                        record.inclusion.as_ref().map(|v| &v.hash.bytes()[..]),
                        record
                            .inclusion
                            .map(|v| v.height.to_be_bytes::<32>())
                            .as_ref()
                            .map(|v| &v[..])
                    ],
                )?;
                for (outpoint, address) in &operation.qi {
                    tx.execute(
                        "INSERT INTO qi_claims VALUES(?1,?2,?3,?4,?5)",
                        params![
                            &key[..],
                            &outpoint.transaction_hash.bytes()[..],
                            outpoint.index,
                            &record.id.0[..],
                            &address.bytes()[..]
                        ],
                    )?;
                }
                if let Some((address, nonce)) = operation.nonce {
                    tx.execute(
                        "INSERT INTO nonce_claims VALUES(?1,?2,?3,?4)",
                        params![
                            &key[..],
                            &address.bytes()[..],
                            &nonce.to_be_bytes()[..],
                            &record.id.0[..]
                        ],
                    )?;
                }
                if let Some(payload) = &operation.payload {
                    tx.execute(
                        "INSERT INTO signed_payloads VALUES(?1,?2,?3,?4)",
                        params![&key[..], &record.id.0[..], operation.kind, payload],
                    )?;
                    signed_payload_read(&tx, &key, scope_state.scope, record.id)?
                        .ok_or(StorageError::Invalid)?;
                }
                replacements::insert_variants(&tx, &key, record.id, &operation.replacements)?;
            }
            head_state::clear(&tx, &key)?;
            clear_snapshot(&tx, &key, next)?;
            generations.push((scope_state.scope, next as u64));
        }
        payment::restore_channels(&tx, state, owners, &mut generations)?;
        tx.commit()?;
        Ok(generations)
    }
}
fn count(total: &mut usize) -> Result<()> {
    *total = total.checked_add(1).ok_or(StorageError::Invalid)?;
    if *total > MAX_RECORDS {
        Err(StorageError::Invalid)
    } else {
        Ok(())
    }
}
fn read_operation(
    connection: &Connection,
    key: &[u8],
    scope: NetworkScope,
    id: ReservationId,
) -> Result<Option<OperationState>> {
    let Some(record) = reservation_read(connection, key, id)? else {
        return Ok(None);
    };
    let kind: u8 = connection.query_row(
        "SELECT kind FROM reservations WHERE scope=?1 AND id=?2",
        params![key, &id.0[..]],
        |r| r.get(0),
    )?;
    let mut qi = Vec::new();
    let mut statement=connection.prepare("SELECT tx_hash,output_index,address FROM qi_claims WHERE scope=?1 AND operation=?2 ORDER BY tx_hash,output_index LIMIT 4097")?;
    let mut rows = statement.query(params![key, &id.0[..]])?;
    while let Some(row) = rows.next()? {
        if qi.len() == 4096 {
            return Err(StorageError::Invalid);
        }
        qi.push((
            OutPoint {
                transaction_hash: Hash32::from_bytes(array(&row.get::<_, Vec<u8>>(0)?)?),
                index: row.get(1)?,
            },
            Address::from_bytes(array(&row.get::<_, Vec<u8>>(2)?)?),
        ));
    }
    let nonce: Option<(Vec<u8>, Vec<u8>)> = connection
        .query_row(
            "SELECT address,nonce FROM nonce_claims WHERE scope=?1 AND operation=?2",
            params![key, &id.0[..]],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let nonce = nonce
        .map(|(address, nonce)| {
            Ok::<_, StorageError>((
                QuaiAddress::try_from(array::<20>(&address)?).map_err(|_| StorageError::Invalid)?,
                u64::from_be_bytes(array(&nonce)?),
            ))
        })
        .transpose()?;
    let payload = signed_payload_read(connection, key, scope, id)?;
    let replacements = replacements::read_variants(connection, key, id)?;
    if !replacements.is_empty() {
        replacements::validate_family(
            payload.as_deref().ok_or(StorageError::Invalid)?,
            &replacements,
        )?;
    }
    Ok(Some(OperationState {
        record,
        kind,
        qi,
        nonce,
        payload,
        replacements,
    }))
}

fn validate_native_schema(connection: &Connection) -> Result<()> {
    validate_native_schema_version(connection, 6)
}
pub(super) fn validate_native_schema_version(connection: &Connection, version: u8) -> Result<()> {
    // Refuse future tables/columns rather than silently omitting channel or other state.
    const TABLES: &[(&str, &[&str])] = &[
        ("addresses", &["scope", "address", "public_key", "origin"]),
        (
            "coins",
            &[
                "scope",
                "tx_hash",
                "output_index",
                "generation",
                "address",
                "denomination",
                "unlock_height",
                "expires_at",
            ],
        ),
        (
            "derivation_cursors",
            &[
                "scope",
                "coin",
                "account",
                "change_branch",
                "account_xpub",
                "next_index",
            ],
        ),
        ("head_replay", &["scope", "revision", "payload"]),
        ("nonce_claims", &["scope", "address", "nonce", "operation"]),
        ("nonce_cursors", &["scope", "address", "next_nonce"]),
        (
            "observation_cache",
            &[
                "scope",
                "operation",
                "candidate",
                "slot",
                "revision",
                "payload",
            ],
        ),
        (
            "payment_channels",
            &[
                "network",
                "local_code",
                "peer_code",
                "account",
                "generation",
                "metadata",
            ],
        ),
        (
            "payment_exposures",
            &[
                "network",
                "local_code",
                "peer_code",
                "account",
                "direction",
                "zone",
                "child_index",
                "public_key",
                "address",
                "range_start",
                "range_end",
            ],
        ),
        (
            "qi_claims",
            &["scope", "tx_hash", "output_index", "operation", "address"],
        ),
        (
            "quai_replacements",
            &["scope", "operation", "sequence", "parent_hash", "payload"],
        ),
        (
            "released_change",
            &["scope", "account", "child_index", "address"],
        ),
        (
            "reservations",
            &[
                "scope",
                "id",
                "kind",
                "state",
                "transaction_hash",
                "block_hash",
                "block_height",
            ],
        ),
        ("scopes", &["scope", "generation", "block_hash", "height"]),
        (
            "signed_payloads",
            &["scope", "operation", "kind", "payload"],
        ),
    ];
    let mut statement=connection.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name LIMIT 16")?;
    let names: Vec<String> = statement
        .query_map([], |row| row.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    let tables: Vec<_> = TABLES
        .iter()
        .filter(|(name, _)| {
            (version >= 2 || !name.starts_with("payment_"))
                && (version >= 3 || *name != "quai_replacements")
                && (version >= 4 || *name != "observation_cache")
                && (version >= 5 || *name != "head_replay")
                && (version >= 6 || *name != "released_change")
        })
        .collect();
    if names != tables.iter().map(|(name, _)| *name).collect::<Vec<_>>() {
        return Err(StorageError::Schema);
    }
    for (table, columns) in tables {
        let mut statement =
            connection.prepare("SELECT name FROM pragma_table_info(?1) ORDER BY cid LIMIT 32")?;
        let names: Vec<String> = statement
            .query_map([table], |row| row.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        if names != *columns {
            return Err(StorageError::Schema);
        }
    }
    Ok(())
}
