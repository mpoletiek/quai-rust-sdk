//! Durable BIP47 channel ranges and exposure metadata; no notification/scanner engine.
use super::*;
use quai_crypto::PublicKey;
use quai_payments::{
    PaymentChannel, PaymentCode, PaymentDirection, PaymentError, PaymentSearch,
    PaymentSearchResult, PrivatePaymentCode,
};
use quai_primitives::Zone;

pub(super) const PAYMENT_SCHEMA:&str="
CREATE TABLE payment_channels(network BLOB NOT NULL CHECK(length(network)=64),local_code BLOB NOT NULL CHECK(length(local_code)=80),peer_code BLOB NOT NULL CHECK(length(peer_code)=80),account INTEGER NOT NULL CHECK(account BETWEEN 0 AND 2147483647),generation INTEGER NOT NULL CHECK(generation>=0),metadata BLOB NOT NULL CHECK(length(metadata) BETWEEN 1 AND 4096),PRIMARY KEY(network,local_code,peer_code,account)) STRICT;
CREATE TABLE payment_exposures(network BLOB NOT NULL,local_code BLOB NOT NULL,peer_code BLOB NOT NULL,account INTEGER NOT NULL,direction INTEGER NOT NULL CHECK(direction IN(0,1)),zone INTEGER NOT NULL CHECK(zone IN(0,1,2,16,17,18,32,33,34)),child_index INTEGER NOT NULL CHECK(child_index BETWEEN 0 AND 2147483647),public_key BLOB NOT NULL CHECK(length(public_key)=33),address BLOB NOT NULL CHECK(length(address)=20),range_start INTEGER NOT NULL CHECK(range_start>=0),range_end INTEGER NOT NULL CHECK(range_end<=2147483648),CHECK(range_start<=child_index AND child_index<range_end),PRIMARY KEY(network,local_code,peer_code,account,direction,zone,child_index),FOREIGN KEY(network,local_code,peer_code,account) REFERENCES payment_channels(network,local_code,peer_code,account)) STRICT;
";
/// Channel metadata with a local database generation used for compare-and-swap imports.
#[derive(Clone, Debug)]
pub struct VersionedPaymentChannel {
    /// Owner-validated public channel metadata, including all zones/directions.
    pub channel: PaymentChannel,
    /// Local monotonic database generation; not a proof that an external backup is latest.
    pub generation: u64,
}
/// Address returned only after its raw range and public exposure record are durable.
#[derive(Clone, Debug)]
pub struct PaymentAddressAllocation {
    /// Exact matched payment address, public point and child index.
    pub found: PaymentSearchResult,
    /// Whole range consumed before search, including unexamined trailing candidates.
    pub burned: crate::discovery::IndexRange,
    /// Channel generation at which the range was reserved.
    pub channel_generation: u64,
    /// Updated owned-address snapshot generation for receive allocations; None for send.
    pub snapshot_generation: Option<u64>,
}
/// Durable public exposure record; peer/owner context is supplied to the lookup API.
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
    pub burned: crate::discovery::IndexRange,
}
#[derive(Clone, Debug)]
pub(crate) struct StoredPaymentChannel {
    pub network: [u8; 64],
    pub local: [u8; 80],
    pub peer: [u8; 80],
    pub account: u32,
    pub generation: u64,
    pub metadata: Vec<u8>,
}
#[derive(Clone, Debug)]
pub(crate) struct StoredPaymentExposure {
    pub network: [u8; 64],
    pub local: [u8; 80],
    pub peer: [u8; 80],
    pub account: u32,
    pub record: PaymentAddressRecord,
}
impl StoredPaymentChannel {
    pub(crate) fn checked(&self, owner: &PrivatePaymentCode) -> Result<PaymentChannel> {
        let channel =
            PaymentChannel::from_json(owner, &self.metadata).map_err(|_| StorageError::Invalid)?;
        if channel.local_code().to_bytes() != self.local
            || channel.counterparty_code().to_bytes() != self.peer
            || channel.account() != self.account
            || self.generation > i64::MAX as u64
        {
            return Err(StorageError::Invalid);
        }
        Ok(channel)
    }
}
impl SqliteStore {
    /// Register receive children found by a bounded recovery scan. Derives and
    /// checks ownership locally, retains existing burned intervals, and advances
    /// the receive cursor monotonically. No untrusted public point is accepted.
    /// Adding addresses invalidates the coin snapshot; refresh before spending.
    pub fn import_payment_receive_indexes(
        &mut self,
        owner: &PrivatePaymentCode,
        peer: &PaymentCode,
        indexes: &[u32],
    ) -> Result<()> {
        if indexes.len() > 1024 || indexes.iter().collect::<BTreeSet<_>>().len() != indexes.len() {
            return Err(StorageError::Invalid);
        }
        let mut derived = Vec::with_capacity(indexes.len());
        for &index in indexes {
            let public = owner
                .receive_public_key(peer, index)
                .map_err(|_| StorageError::Invalid)?;
            let address =
                QiAddress::try_from(public.address()).map_err(|_| StorageError::Invalid)?;
            if address.zone() != self.scope.zone {
                return Err(StorageError::Invalid);
            }
            derived.push((index, public, address));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut stored =
            read_channel(&tx, &self.key[..64], owner, peer)?.ok_or(StorageError::Invalid)?;
        let mut end = cursor_value(
            stored
                .channel
                .next_index(PaymentDirection::Receive, self.scope.zone),
        );
        for (index, public_key, address) in derived {
            end = end.max(index + 1);
            let old: Option<(u32, u32)> = tx.query_row(
                "SELECT range_start,range_end FROM payment_exposures WHERE network=?1 AND local_code=?2 AND peer_code=?3 AND account=?4 AND direction=1 AND zone=?5 AND child_index=?6",
                params![&self.key[..64], &owner.public_code().to_bytes()[..], &peer.to_bytes()[..], owner.account(), self.scope.zone.byte(), index],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional()?;
            let (start, stop) = old.unwrap_or((index, index + 1));
            insert_exposure(
                &tx,
                &StoredPaymentExposure {
                    network: array(&self.key[..64])?,
                    local: owner.public_code().to_bytes(),
                    peer: peer.to_bytes(),
                    account: owner.account(),
                    record: PaymentAddressRecord {
                        direction: PaymentDirection::Receive,
                        zone: self.scope.zone,
                        index,
                        address,
                        public_key,
                        burned: crate::discovery::IndexRange { start, end: stop },
                    },
                },
                true,
            )?;
            insert_address(
                &tx,
                &self.key,
                self.scope,
                &PublicAddress::imported(&public_key)?,
            )?;
        }
        let count: i64 = tx.query_row(
            "SELECT count(*) FROM addresses WHERE scope=?1",
            [&self.key[..]],
            |row| row.get(0),
        )?;
        if count > MAX_COINS as i64 {
            return Err(StorageError::Invalid);
        }
        stored
            .channel
            .advance_cursor(
                owner,
                PaymentDirection::Receive,
                self.scope.zone,
                if end == 1 << 31 { None } else { Some(end) },
            )
            .map_err(|_| StorageError::Invalid)?;
        let generation = stored
            .generation
            .checked_add(1)
            .ok_or(StorageError::Overflow)?;
        write_channel(&tx, &self.key[..64], &stored.channel, generation)?;
        let old = checkpoint_read(&tx, &self.key)?.0;
        let next = next_generation(&tx, &self.key, old as u64)?;
        clear_snapshot(&tx, &self.key, next)?;
        tx.commit()?;
        Ok(())
    }
    /// Load a channel for the bound chain/genesis. Its eighteen cursors cover all zones.
    /// The private owner is required to validate metadata ownership before use.
    pub fn payment_channel(
        &self,
        owner: &PrivatePaymentCode,
        peer: &PaymentCode,
    ) -> Result<Option<VersionedPaymentChannel>> {
        read_channel(&self.connection, &self.key[..64], owner, peer)
    }
    /// Enumerate this owner's registered channels after restart without knowing
    /// their peer codes in advance. Results are ordered by peer-code bytes and
    /// include validated metadata/generations for all zones on this chain/genesis.
    /// At most 1,024 channels are returned; malformed rows fail the whole read.
    /// This is a local read and neither opens channels nor advances cursors.
    pub fn payment_channels(
        &self,
        owner: &PrivatePaymentCode,
    ) -> Result<Vec<VersionedPaymentChannel>> {
        let mut statement = self.connection.prepare(
            "SELECT CASE WHEN length(peer_code)=80 THEN peer_code ELSE NULL END,generation,CASE WHEN length(metadata) BETWEEN 1 AND 4096 THEN metadata ELSE NULL END FROM payment_channels WHERE network=?1 AND local_code=?2 AND account=?3 ORDER BY peer_code LIMIT 1025"
        )?;
        let mut rows = statement.query(params![
            &self.key[..64],
            &owner.public_code().to_bytes()[..],
            owner.account(),
        ])?;
        let mut channels = Vec::new();
        while let Some(row) = rows.next()? {
            if channels.len() == 1024 {
                return Err(StorageError::Invalid);
            }
            let peer: [u8; 80] = array(&row.get::<_, Vec<u8>>(0)?)?;
            let generation: i64 = row.get(1)?;
            let metadata: Vec<u8> = row.get(2)?;
            let channel =
                PaymentChannel::from_json(owner, &metadata).map_err(|_| StorageError::Invalid)?;
            if generation < 0 || channel.counterparty_code().to_bytes() != peer {
                return Err(StorageError::Invalid);
            }
            channels.push(VersionedPaymentChannel {
                channel,
                generation: generation as u64,
            });
        }
        Ok(channels)
    }
    /// Create or CAS-import owner-validated metadata. Use None only for a new channel.
    /// Existing generations and every cursor must be respected; stale imports cannot
    /// rewind either direction/zone or revive exhausted cursors. All network snapshots
    /// are invalidated because new channel metadata expands the known recovery domain.
    pub fn import_payment_channel(
        &mut self,
        owner: &PrivatePaymentCode,
        channel: &PaymentChannel,
        expected_generation: Option<u64>,
    ) -> Result<u64> {
        let encoded = channel.to_json().map_err(|_| StorageError::Invalid)?;
        let incoming =
            PaymentChannel::from_json(owner, &encoded).map_err(|_| StorageError::Invalid)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let old = read_channel(&tx, &self.key[..64], owner, incoming.counterparty_code())?;
        let generation = match old {
            Some(old) => {
                if expected_generation != Some(old.generation) {
                    return Err(StorageError::StaleSnapshot);
                }
                for zone in Zone::ALL {
                    for direction in [PaymentDirection::Send, PaymentDirection::Receive] {
                        if cursor_value(incoming.next_index(direction, zone))
                            < cursor_value(old.channel.next_index(direction, zone))
                        {
                            return Err(StorageError::Conflict);
                        }
                    }
                }
                old.generation
                    .checked_add(1)
                    .filter(|v| *v <= i64::MAX as u64)
                    .ok_or(StorageError::Overflow)?
            }
            None => {
                if expected_generation.is_some() {
                    return Err(StorageError::StaleSnapshot);
                }
                0
            }
        };
        write_channel(&tx, &self.key[..64], &incoming, generation)?;
        invalidate_network(&tx, &self.key[..64], &BTreeSet::new())?;
        tx.commit()?;
        Ok(generation)
    }
    /// Burn a raw range atomically before deriving/exposing a destination. Receive
    /// allocations persist owned metadata and invalidate its zone snapshot; send
    /// allocations persist peer-owned exposure metadata without claiming local ownership.
    /// A cancelled/failed search still consumes its whole committed range.
    pub fn allocate_payment_address(
        &mut self,
        owner: &PrivatePaymentCode,
        peer: &PaymentCode,
        direction: PaymentDirection,
        max_attempts: u32,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<PaymentAddressAllocation> {
        if max_attempts == 0 || max_attempts > quai_payments::MAX_SEARCH_ATTEMPTS {
            return Err(StorageError::Invalid);
        }
        if cancelled() {
            return Err(StorageError::Cancelled);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut stored =
            read_channel(&tx, &self.key[..64], owner, peer)?.ok_or(StorageError::Invalid)?;
        let start = stored
            .channel
            .next_index(direction, self.scope.zone)
            .ok_or(StorageError::DerivationExhausted)?;
        let end = start
            .checked_add(max_attempts)
            .filter(|v| *v <= 1 << 31)
            .ok_or(StorageError::Overflow)?;
        stored
            .channel
            .advance_cursor(
                owner,
                direction,
                self.scope.zone,
                if end == 1 << 31 { None } else { Some(end) },
            )
            .map_err(|_| StorageError::Invalid)?;
        let generation = stored
            .generation
            .checked_add(1)
            .filter(|v| *v <= i64::MAX as u64)
            .ok_or(StorageError::Overflow)?;
        write_channel(&tx, &self.key[..64], &stored.channel, generation)?;
        tx.commit()?;
        let found = owner
            .search(
                peer,
                direction,
                PaymentSearch {
                    zone: self.scope.zone,
                    start_index: start,
                    max_attempts,
                },
                &mut cancelled,
            )
            .map_err(|error| match error {
                PaymentError::SearchCancelled { .. } => StorageError::Cancelled,
                PaymentError::SearchExhausted { .. } => StorageError::DerivationExhausted,
                _ => StorageError::Invalid,
            })?;
        if cancelled() {
            return Err(StorageError::Cancelled);
        }
        let record = PaymentAddressRecord {
            direction,
            zone: self.scope.zone,
            index: found.index,
            address: found.address,
            public_key: found.public_key,
            burned: crate::discovery::IndexRange { start, end },
        };
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current =
            read_channel(&tx, &self.key[..64], owner, peer)?.ok_or(StorageError::Invalid)?;
        if cursor_value(current.channel.next_index(direction, self.scope.zone)) < end {
            return Err(StorageError::Conflict);
        }
        let exposure = StoredPaymentExposure {
            network: array(&self.key[..64])?,
            local: owner.public_code().to_bytes(),
            peer: peer.to_bytes(),
            account: owner.account(),
            record,
        };
        insert_exposure(&tx, &exposure, false)?;
        let snapshot_generation = if direction == PaymentDirection::Receive {
            let address = PublicAddress::imported(&found.public_key)?;
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM addresses WHERE scope=?1 AND address=?2)",
                params![&self.key[..], &address.address().bytes()[..]],
                |row| row.get(0),
            )?;
            if exists {
                return Err(StorageError::Conflict);
            }
            let count: i64 = tx.query_row(
                "SELECT count(*) FROM addresses WHERE scope=?1",
                [&self.key[..]],
                |row| row.get(0),
            )?;
            if count >= MAX_COINS as i64 {
                return Err(StorageError::Invalid);
            }
            insert_address(&tx, &self.key, self.scope, &address)?;
            let old = checkpoint_read(&tx, &self.key)?.0;
            let next = next_generation(&tx, &self.key, old as u64)?;
            clear_snapshot(&tx, &self.key, next)?;
            Some(next as u64)
        } else {
            None
        };
        tx.commit()?;
        Ok(PaymentAddressAllocation {
            found,
            burned: crate::discovery::IndexRange { start, end },
            channel_generation: generation,
            snapshot_generation,
        })
    }
    /// Enumerate recorded exposures across this network's zones, validating owner and
    /// public derivation. Intended for explicit recovery; no remote history is inferred.
    pub fn payment_addresses(
        &self,
        owner: &PrivatePaymentCode,
        peer: &PaymentCode,
        direction: PaymentDirection,
    ) -> Result<Vec<PaymentAddressRecord>> {
        read_channel(&self.connection, &self.key[..64], owner, peer)?
            .ok_or(StorageError::Invalid)?;
        let mut statement=self.connection.prepare("SELECT zone,child_index,public_key,address,range_start,range_end FROM payment_exposures WHERE network=?1 AND local_code=?2 AND peer_code=?3 AND account=?4 AND direction=?5 ORDER BY zone,child_index LIMIT 100001")?;
        let mut rows = statement.query(params![
            &self.key[..64],
            &owner.public_code().to_bytes()[..],
            &peer.to_bytes()[..],
            owner.account(),
            direction_byte(direction)
        ])?;
        let mut records = Vec::new();
        while let Some(row) = rows.next()? {
            if records.len() == 100_000 {
                return Err(StorageError::Invalid);
            }
            let record = record_from_row(row, 0, direction)?;
            validate_exposure(owner, peer, &record)?;
            records.push(record);
        }
        Ok(records)
    }
}
pub(crate) fn direction_byte(direction: PaymentDirection) -> u8 {
    match direction {
        PaymentDirection::Send => 0,
        PaymentDirection::Receive => 1,
    }
}
pub(crate) fn direction_from(value: u8) -> Result<PaymentDirection> {
    match value {
        0 => Ok(PaymentDirection::Send),
        1 => Ok(PaymentDirection::Receive),
        _ => Err(StorageError::Invalid),
    }
}
pub(crate) fn cursor_value(value: Option<u32>) -> u32 {
    value.unwrap_or(1 << 31)
}
pub(super) fn read_channel(
    connection: &Connection,
    network: &[u8],
    owner: &PrivatePaymentCode,
    peer: &PaymentCode,
) -> Result<Option<VersionedPaymentChannel>> {
    let row:Option<(i64,Vec<u8>)>=connection.query_row("SELECT generation,CASE WHEN length(metadata) BETWEEN 1 AND 4096 THEN metadata ELSE NULL END FROM payment_channels WHERE network=?1 AND local_code=?2 AND peer_code=?3 AND account=?4",params![network,&owner.public_code().to_bytes()[..],&peer.to_bytes()[..],owner.account()],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
    row.map(|(generation, bytes)| {
        if generation < 0 {
            return Err(StorageError::Invalid);
        }
        let channel =
            PaymentChannel::from_json(owner, &bytes).map_err(|_| StorageError::Invalid)?;
        if channel.counterparty_code() != peer {
            return Err(StorageError::Invalid);
        }
        Ok(VersionedPaymentChannel {
            channel,
            generation: generation as u64,
        })
    })
    .transpose()
}
pub(super) fn write_channel(
    connection: &Connection,
    network: &[u8],
    channel: &PaymentChannel,
    generation: u64,
) -> Result<()> {
    let metadata = channel.to_json().map_err(|_| StorageError::Invalid)?;
    let count: i64 = connection.query_row("SELECT count(*) FROM payment_channels", [], |row| {
        row.get(0)
    })?;
    connection.execute("INSERT INTO payment_channels VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(network,local_code,peer_code,account) DO UPDATE SET generation=excluded.generation,metadata=excluded.metadata",params![network,&channel.local_code().to_bytes()[..],&channel.counterparty_code().to_bytes()[..],channel.account(),i64::try_from(generation).map_err(|_|StorageError::Overflow)?,metadata])?;
    let new_count: i64 =
        connection.query_row("SELECT count(*) FROM payment_channels", [], |row| {
            row.get(0)
        })?;
    if count > 1024 || new_count > 1024 {
        return Err(StorageError::Invalid);
    }
    Ok(())
}
pub(super) fn insert_exposure(
    connection: &Connection,
    exposure: &StoredPaymentExposure,
    allow_identical: bool,
) -> Result<()> {
    let record = &exposure.record;
    let old:Option<(Vec<u8>,Vec<u8>,u32,u32)>=connection.query_row("SELECT public_key,address,range_start,range_end FROM payment_exposures WHERE network=?1 AND local_code=?2 AND peer_code=?3 AND account=?4 AND direction=?5 AND zone=?6 AND child_index=?7",params![&exposure.network[..],&exposure.local[..],&exposure.peer[..],exposure.account,direction_byte(record.direction),record.zone.byte(),record.index],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional()?;
    if let Some((public, address, start, end)) = old {
        if allow_identical
            && public == record.public_key.to_compressed()
            && address == record.address.bytes()
            && start == record.burned.start
            && end == record.burned.end
        {
            return Ok(());
        }
        return Err(StorageError::Conflict);
    }
    let count: i64 = connection.query_row("SELECT count(*) FROM payment_exposures", [], |row| {
        row.get(0)
    })?;
    if count >= 100_000 {
        return Err(StorageError::Invalid);
    }
    connection.execute(
        "INSERT INTO payment_exposures VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![
            &exposure.network[..],
            &exposure.local[..],
            &exposure.peer[..],
            exposure.account,
            direction_byte(record.direction),
            record.zone.byte(),
            record.index,
            &record.public_key.to_compressed()[..],
            &record.address.bytes()[..],
            record.burned.start,
            record.burned.end
        ],
    )?;
    Ok(())
}
pub(crate) fn validate_exposure(
    owner: &PrivatePaymentCode,
    peer: &PaymentCode,
    record: &PaymentAddressRecord,
) -> Result<()> {
    if record.index < record.burned.start
        || record.index >= record.burned.end
        || record.burned.end > 1 << 31
        || record.address.zone() != record.zone
        || record.public_key.address() != record.address.address()
    {
        return Err(StorageError::Invalid);
    }
    let key = match record.direction {
        PaymentDirection::Send => owner.send_public_key(peer, record.index),
        PaymentDirection::Receive => owner.receive_public_key(peer, record.index),
    }
    .map_err(|_| StorageError::Invalid)?;
    if key != record.public_key {
        return Err(StorageError::Invalid);
    }
    Ok(())
}
pub(super) fn record_from_row(
    row: &rusqlite::Row<'_>,
    offset: usize,
    direction: PaymentDirection,
) -> Result<PaymentAddressRecord> {
    Ok(PaymentAddressRecord {
        direction,
        zone: Zone::from_byte(row.get(offset)?).map_err(|_| StorageError::Invalid)?,
        index: row.get(offset + 1)?,
        public_key: PublicKey::from_sec1_bytes(&row.get::<_, Vec<u8>>(offset + 2)?)
            .map_err(|_| StorageError::Invalid)?,
        address: QiAddress::try_from(array::<20>(&row.get::<_, Vec<u8>>(offset + 3)?)?)
            .map_err(|_| StorageError::Invalid)?,
        burned: crate::discovery::IndexRange {
            start: row.get(offset + 4)?,
            end: row.get(offset + 5)?,
        },
    })
}
pub(super) fn invalidate_network(
    connection: &Connection,
    network: &[u8],
    skip: &BTreeSet<[u8; 65]>,
) -> Result<Vec<(NetworkScope, u64)>> {
    let mut statement = connection.prepare(
        "SELECT scope,generation FROM scopes WHERE substr(scope,1,64)=?1 ORDER BY scope",
    )?;
    let rows: Vec<(Vec<u8>, i64)> = statement
        .query_map([network], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;
    let mut invalidated = Vec::new();
    for (key, generation) in rows {
        let key = array::<65>(&key)?;
        if skip.contains(&key) {
            continue;
        }
        let next = next_generation(
            connection,
            &key,
            u64::try_from(generation).map_err(|_| StorageError::Invalid)?,
        )?;
        clear_snapshot(connection, &key, next)?;
        invalidated.push((
            NetworkScope {
                chain_id: U256::from_be_bytes(array::<32>(&key[..32])?),
                genesis: Hash32::from_bytes(array(&key[32..64])?),
                zone: Zone::from_byte(key[64]).map_err(|_| StorageError::Invalid)?,
            },
            next as u64,
        ));
    }
    Ok(invalidated)
}

pub(super) fn capture_channels(
    connection: &Connection,
    total: &mut usize,
) -> Result<(Vec<StoredPaymentChannel>, Vec<StoredPaymentExposure>)> {
    let invalid: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM payment_channels WHERE length(metadata)>4096 OR length(metadata)<1)", [], |row| row.get(0))?;
    if invalid {
        return Err(StorageError::Invalid);
    }
    let mut channels = Vec::new();
    let mut statement=connection.prepare("SELECT network,local_code,peer_code,account,generation,metadata FROM payment_channels ORDER BY network,local_code,peer_code,account LIMIT 1025")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        *total = total.checked_add(1).ok_or(StorageError::Invalid)?;
        if channels.len() >= 1024 || *total > 100_000 {
            return Err(StorageError::Invalid);
        }
        channels.push(StoredPaymentChannel {
            network: array(&row.get::<_, Vec<u8>>(0)?)?,
            local: array(&row.get::<_, Vec<u8>>(1)?)?,
            peer: array(&row.get::<_, Vec<u8>>(2)?)?,
            account: row.get(3)?,
            generation: u64::try_from(row.get::<_, i64>(4)?).map_err(|_| StorageError::Invalid)?,
            metadata: row.get(5)?,
        });
    }
    drop(rows);
    drop(statement);
    let mut exposures = Vec::new();
    let mut statement=connection.prepare("SELECT network,local_code,peer_code,account,direction,zone,child_index,public_key,address,range_start,range_end FROM payment_exposures ORDER BY network,local_code,peer_code,account,direction,zone,child_index LIMIT 100001")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        *total = total.checked_add(1).ok_or(StorageError::Invalid)?;
        if *total > 100_000 {
            return Err(StorageError::Invalid);
        }
        exposures.push(StoredPaymentExposure {
            network: array(&row.get::<_, Vec<u8>>(0)?)?,
            local: array(&row.get::<_, Vec<u8>>(1)?)?,
            peer: array(&row.get::<_, Vec<u8>>(2)?)?,
            account: row.get(3)?,
            record: record_from_row(row, 5, direction_from(row.get(4)?)?)?,
        });
    }
    Ok((channels, exposures))
}

pub(super) fn restore_channels(
    connection: &Connection,
    state: &PublicWalletState,
    owners: &[PrivatePaymentCode],
    generations: &mut Vec<(NetworkScope, u64)>,
) -> Result<()> {
    if owners.len() != state.channels.len() {
        return Err(StorageError::Invalid);
    }
    let mut networks = BTreeSet::new();
    for (stored, owner) in state.channels.iter().zip(owners) {
        let incoming = stored.checked(owner)?;
        let existing = read_channel(
            connection,
            &stored.network,
            owner,
            incoming.counterparty_code(),
        )?;
        let (mut merged, generation) = match existing {
            Some(old) => (
                old.channel,
                old.generation
                    .checked_add(1)
                    .ok_or(StorageError::Overflow)?,
            ),
            None => (incoming.clone(), 0),
        };
        for direction in [PaymentDirection::Send, PaymentDirection::Receive] {
            for zone in Zone::ALL {
                let wanted = incoming.next_index(direction, zone);
                if cursor_value(wanted) > cursor_value(merged.next_index(direction, zone)) {
                    merged
                        .advance_cursor(owner, direction, zone, wanted)
                        .map_err(|_| StorageError::Invalid)?;
                }
            }
        }
        write_channel(connection, &stored.network, &merged, generation)?;
        networks.insert(stored.network);
    }
    for exposure in &state.exposures {
        insert_exposure(connection, exposure, true)?;
    }
    let skip: BTreeSet<_> = generations.iter().map(|(scope, _)| scope.key()).collect();
    for network in networks {
        generations.extend(invalidate_network(connection, &network, &skip)?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    struct Database(PathBuf);
    impl Database {
        fn new() -> Self {
            let mut id = [0u8; 16];
            getrandom::fill(&mut id).unwrap();
            Self(std::env::temp_dir().join(format!(
                "quai-public-channels-{}.sqlite",
                id.iter().map(|b| format!("{b:02x}")).collect::<String>()
            )))
        }
        fn open(&self) -> SqliteStore {
            SqliteStore::open(&self.0, scope()).unwrap()
        }
    }
    impl Drop for Database {
        fn drop(&mut self) {
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
            }
        }
    }
    fn scope() -> NetworkScope {
        NetworkScope {
            chain_id: U256::from(15000),
            genesis: Hash32::from_bytes([1; 32]),
            zone: Zone::Cyprus1,
        }
    }
    fn pair() -> (PrivatePaymentCode, PrivatePaymentCode) {
        (
            PrivatePaymentCode::from_seed(&[0; 16], 0).unwrap(),
            PrivatePaymentCode::from_seed(&[1; 16], 0).unwrap(),
        )
    }
    #[test]
    fn channel_listing_recovers_peers_scopes_and_cursors_without_mutation() {
        let db = Database::new();
        let (owner, peer) = pair();
        let second = PrivatePaymentCode::from_seed(&[2; 16], 0).unwrap();
        let another_account = PrivatePaymentCode::from_seed(&[0; 16], 1).unwrap();
        let mut store = db.open();
        assert!(store.payment_channels(&owner).unwrap().is_empty());
        for other in [&second, &peer] {
            let mut channel = PaymentChannel::new(&owner, other.public_code().clone());
            channel
                .advance_cursor(&owner, PaymentDirection::Receive, Zone::Cyprus1, Some(27))
                .unwrap();
            store
                .import_payment_channel(&owner, &channel, None)
                .unwrap();
        }
        store
            .import_payment_channel(
                &another_account,
                &PaymentChannel::new(&another_account, peer.public_code().clone()),
                None,
            )
            .unwrap();
        let before = store.snapshot().unwrap().generation;
        let channels = store.payment_channels(&owner).unwrap();
        assert_eq!(channels.len(), 2);
        let actual: Vec<_> = channels
            .iter()
            .map(|v| v.channel.counterparty_code().to_bytes())
            .collect();
        let mut expected = vec![
            peer.public_code().to_bytes(),
            second.public_code().to_bytes(),
        ];
        expected.sort();
        assert_eq!(actual, expected);
        for value in &channels {
            assert_eq!(value.generation, 0);
            assert_eq!(
                value
                    .channel
                    .next_index(PaymentDirection::Receive, Zone::Cyprus1),
                Some(27)
            );
            assert_eq!(value.channel.local_code(), owner.public_code());
        }
        assert_eq!(store.payment_channels(&another_account).unwrap().len(), 1);
        assert!(store.payment_channels(&peer).unwrap().is_empty());
        assert_eq!(store.snapshot().unwrap().generation, before);
        drop(store);
        let reopened = db.open();
        assert_eq!(reopened.payment_channels(&owner).unwrap().len(), 2);
        // Channels are network-wide, while ordinary address scopes include a zone.
        let other_zone = SqliteStore::open(
            &db.0,
            NetworkScope {
                zone: Zone::Paxos1,
                ..scope()
            },
        )
        .unwrap();
        assert_eq!(other_zone.payment_channels(&owner).unwrap().len(), 2);
        for other_scope in [
            NetworkScope {
                chain_id: U256::from(9),
                ..scope()
            },
            NetworkScope {
                genesis: Hash32::from_bytes([2; 32]),
                ..scope()
            },
        ] {
            let other = SqliteStore::open(&db.0, other_scope).unwrap();
            assert!(other.payment_channels(&owner).unwrap().is_empty());
        }
        // Corruption must not return an incomplete list that looks authoritative.
        reopened
            .connection
            .execute(
                "UPDATE payment_channels SET metadata=?1 WHERE peer_code=?2",
                params![b"{}".as_slice(), &peer.public_code().to_bytes()[..]],
            )
            .unwrap();
        assert!(matches!(
            reopened.payment_channels(&owner),
            Err(StorageError::Invalid)
        ));
    }

    #[test]
    fn restart_cancel_owner_and_stale_import_preserve_all_zone_cursors() {
        let db = Database::new();
        let (owner, peer) = pair();
        let mut store = db.open();
        let initial = PaymentChannel::new(&owner, peer.public_code().clone());
        assert!(store.import_payment_channel(&peer, &initial, None).is_err());
        assert_eq!(
            store
                .import_payment_channel(&owner, &initial, None)
                .unwrap(),
            0
        );
        let mut calls = 0;
        assert_eq!(
            store
                .allocate_payment_address(
                    &owner,
                    peer.public_code(),
                    PaymentDirection::Send,
                    10000,
                    || {
                        calls += 1;
                        calls > 1
                    }
                )
                .unwrap_err(),
            StorageError::Cancelled
        );
        drop(store);
        let mut store = db.open();
        let current = store
            .payment_channel(&owner, peer.public_code())
            .unwrap()
            .unwrap();
        assert_eq!(
            current
                .channel
                .next_index(PaymentDirection::Send, Zone::Cyprus1),
            Some(10000)
        );
        assert_eq!(
            store.import_payment_channel(&owner, &initial, Some(0)),
            Err(StorageError::StaleSnapshot)
        );
        assert_eq!(
            store.import_payment_channel(&owner, &initial, Some(current.generation)),
            Err(StorageError::Conflict)
        );
        let received = store
            .allocate_payment_address(
                &owner,
                peer.public_code(),
                PaymentDirection::Receive,
                10000,
                || false,
            )
            .unwrap();
        assert!(received.snapshot_generation.is_some());
        assert_eq!(store.addresses().unwrap().len(), 1);
        assert_eq!(
            store
                .payment_addresses(&owner, peer.public_code(), PaymentDirection::Receive)
                .unwrap()[0]
                .address,
            received.found.address
        );
        let old_generation = store.snapshot().unwrap().generation;
        let sent = store
            .allocate_payment_address(
                &owner,
                peer.public_code(),
                PaymentDirection::Send,
                10000,
                || false,
            )
            .unwrap();
        assert_eq!(sent.burned.start, 10000);
        assert!(sent.snapshot_generation.is_none());
        assert_eq!(store.snapshot().unwrap().generation, old_generation);
        assert_eq!(store.addresses().unwrap().len(), 1);
        let mut other = scope();
        other.zone = Zone::Cyprus2;
        let other = SqliteStore::open(&db.0, other).unwrap();
        assert_eq!(
            other
                .payment_channel(&owner, peer.public_code())
                .unwrap()
                .unwrap()
                .channel
                .next_index(PaymentDirection::Send, Zone::Cyprus1),
            Some(20000)
        );
        let mut other_network = scope();
        other_network.chain_id += U256::from(1);
        assert!(
            SqliteStore::open(&db.0, other_network)
                .unwrap()
                .payment_channel(&owner, peer.public_code())
                .unwrap()
                .is_none()
        );
        store
            .connection
            .execute(
                "UPDATE payment_exposures SET public_key=?1",
                [&peer.public_code().public_key().to_compressed()[..]],
            )
            .unwrap();
        assert!(
            store
                .payment_addresses(&owner, peer.public_code(), PaymentDirection::Receive)
                .is_err()
        );
    }
    #[test]
    fn payment_allocation_child() {
        let Ok(path) = std::env::var("QUAI_PUBLIC_PAYMENT_CHILD") else {
            return;
        };
        let mut store = SqliteStore::open(path, scope()).unwrap();
        let (owner, peer) = pair();
        store
            .allocate_payment_address(
                &owner,
                peer.public_code(),
                PaymentDirection::Send,
                10000,
                || false,
            )
            .unwrap();
    }
    #[test]
    fn competing_processes_burn_disjoint_ranges() {
        let db = Database::new();
        let (owner, peer) = pair();
        db.open()
            .import_payment_channel(
                &owner,
                &PaymentChannel::new(&owner, peer.public_code().clone()),
                None,
            )
            .unwrap();
        let mut children = Vec::new();
        for _ in 0..3 {
            children.push(
                std::process::Command::new(std::env::current_exe().unwrap())
                    .arg("--exact")
                    .arg("storage::payment::tests::payment_allocation_child")
                    .env("QUAI_PUBLIC_PAYMENT_CHILD", &db.0)
                    .spawn()
                    .unwrap(),
            );
        }
        for mut child in children {
            assert!(child.wait().unwrap().success());
        }
        let store = db.open();
        let records = store
            .payment_addresses(&owner, peer.public_code(), PaymentDirection::Send)
            .unwrap();
        assert_eq!(records.len(), 3);
        let starts: BTreeSet<_> = records.iter().map(|r| r.burned.start).collect();
        assert_eq!(starts, BTreeSet::from([0, 10000, 20000]));
        assert_eq!(
            store
                .payment_channel(&owner, peer.public_code())
                .unwrap()
                .unwrap()
                .channel
                .next_index(PaymentDirection::Send, Zone::Cyprus1),
            Some(30000)
        );
    }
    #[test]
    fn schema_one_migration_is_validated_and_atomic() {
        let db = Database::new();
        let connection = Connection::open(&db.0).unwrap();
        connection.execute_batch(super::super::SCHEMA).unwrap();
        connection
            .pragma_update(None, "application_id", super::super::APP_ID)
            .unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();
        drop(connection);
        let store = db.open();
        assert_eq!(
            store
                .connection
                .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
                .unwrap(),
            super::super::VERSION as u32
        );
        drop(store);
        let bad = Database::new();
        let connection = Connection::open(&bad.0).unwrap();
        connection.execute_batch(super::super::SCHEMA).unwrap();
        connection
            .pragma_update(None, "application_id", super::super::APP_ID)
            .unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();
        connection
            .execute_batch("ALTER TABLE addresses ADD COLUMN future_secret BLOB")
            .unwrap();
        assert!(SqliteStore::open(&bad.0, scope()).is_err());
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
                .unwrap(),
            1
        );
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='payment_channels')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!exists);
    }
}
