//! Replacement families retain original durable account nonce or Qi input claims.
use super::*;
use std::collections::BTreeMap;
pub(super) const REPLACEMENT_SCHEMA: &str = "CREATE TABLE quai_replacements(scope BLOB NOT NULL,operation BLOB NOT NULL,sequence INTEGER NOT NULL CHECK(sequence BETWEEN 0 AND 31),parent_hash BLOB NOT NULL CHECK(length(parent_hash)=32),payload BLOB NOT NULL CHECK(length(payload) BETWEEN 1 AND 1048576),PRIMARY KEY(scope,operation,sequence),FOREIGN KEY(scope,operation) REFERENCES reservations(scope,id)) STRICT;";
/// One immutable replacement edge. Its canonical signed bytes are stored before
/// exposure. Inclusion is reconciled separately for every candidate hash.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuaiReplacement {
    /// Original or earlier replacement transaction hash.
    pub parent: Hash32,
    /// Canonical signed bytes; validated on capture, read and restore.
    pub payload: Vec<u8>,
}
/// Both-ledger name for the immutable candidate edge.
pub type ReplacementCandidate = QuaiReplacement;
pub(crate) fn validate_family(root: &[u8], variants: &[QuaiReplacement]) -> Result<()> {
    if let Ok(root) = quai_consensus::SignedQiOperation::decode(root) {
        return validate_qi_family(root, variants);
    }
    if variants.len() > 32 {
        return Err(StorageError::Invalid);
    }
    let root = SignedQuaiTransaction::decode(root).map_err(|_| StorageError::Invalid)?;
    let mut known = BTreeMap::from([(root.hash().map_err(|_| StorageError::Invalid)?, root)]);
    for variant in variants {
        let parent = known.get(&variant.parent).ok_or(StorageError::Invalid)?;
        let signed =
            SignedQuaiTransaction::decode(&variant.payload).map_err(|_| StorageError::Invalid)?;
        let mut expected = parent.transaction().clone();
        if signed.from() != parent.from() || signed.transaction().gas_price <= expected.gas_price {
            return Err(StorageError::Invalid);
        }
        expected.gas_price = signed.transaction().gas_price;
        if signed.transaction() != &expected {
            return Err(StorageError::Invalid);
        }
        let hash = signed.hash().map_err(|_| StorageError::Invalid)?;
        if known.insert(hash, signed).is_some() {
            return Err(StorageError::Invalid);
        }
    }
    Ok(())
}

fn output_value(tx: &quai_consensus::QiTransaction) -> Result<U256> {
    tx.outputs.iter().try_fold(U256::ZERO, |sum, output| {
        sum.checked_add(U256::from(output.denomination.value()))
            .ok_or(StorageError::Invalid)
    })
}
fn validate_qi_family(
    root: quai_consensus::SignedQiOperation,
    variants: &[QuaiReplacement],
) -> Result<()> {
    if variants.len() > 32 {
        return Err(StorageError::Invalid);
    }
    let mut known = BTreeMap::from([(root.hash().map_err(|_| StorageError::Invalid)?, root)]);
    for variant in variants {
        let parent = known.get(&variant.parent).ok_or(StorageError::Invalid)?;
        let signed = quai_consensus::SignedQiOperation::decode(&variant.payload)
            .map_err(|_| StorageError::Invalid)?;
        let old = parent.transaction();
        let tx = signed.transaction();
        if tx.chain_id != old.chain_id
            || tx.inputs != old.inputs
            || tx.data != old.data
            || output_value(tx)? >= output_value(old)?
        {
            return Err(StorageError::Invalid);
        }
        let hash = signed.hash().map_err(|_| StorageError::Invalid)?;
        if known.insert(hash, signed).is_some() {
            return Err(StorageError::Invalid);
        }
    }
    Ok(())
}

pub(super) fn read_variants(
    connection: &Connection,
    key: &[u8],
    id: ReservationId,
) -> Result<Vec<QuaiReplacement>> {
    let mut statement=connection.prepare("SELECT sequence,parent_hash,payload FROM quai_replacements WHERE scope=?1 AND operation=?2 ORDER BY sequence LIMIT 33")?;
    let mut rows = statement.query(params![key, &id.0[..]])?;
    let mut variants = Vec::new();
    while let Some(row) = rows.next()? {
        if variants.len() >= 32 || row.get::<_, u32>(0)? as usize != variants.len() {
            return Err(StorageError::Invalid);
        }
        variants.push(QuaiReplacement {
            parent: Hash32::from_bytes(array(&row.get::<_, Vec<u8>>(1)?)?),
            payload: row.get(2)?,
        });
    }
    Ok(variants)
}
pub(super) fn insert_variants(
    connection: &Connection,
    key: &[u8],
    id: ReservationId,
    variants: &[QuaiReplacement],
) -> Result<()> {
    for (sequence, variant) in variants.iter().enumerate() {
        connection.execute(
            "INSERT INTO quai_replacements VALUES(?1,?2,?3,?4,?5)",
            params![
                key,
                &id.0[..],
                sequence as u32,
                &variant.parent.bytes()[..],
                variant.payload
            ],
        )?;
    }
    Ok(())
}
impl SqliteStore {
    /// Read and verify all replacement edges, oldest first. Original bytes and
    /// nonce claims stay immutable; no candidate is discarded when another is sent.
    pub fn quai_replacements(&mut self, id: ReservationId) -> Result<Vec<QuaiReplacement>> {
        let root = self.signed_payload(id)?.ok_or(StorageError::Invalid)?;
        SignedQuaiTransaction::decode(&root).map_err(|_| StorageError::Invalid)?;
        self.replacement_candidates(id)
    }
    /// Read validated candidate edges for either ledger, retaining all original claims.
    pub fn replacement_candidates(
        &mut self,
        id: ReservationId,
    ) -> Result<Vec<ReplacementCandidate>> {
        let tx = self.connection.transaction()?;
        let root =
            signed_payload_read(&tx, &self.key, self.scope, id)?.ok_or(StorageError::Invalid)?;
        let variants = read_variants(&tx, &self.key, id)?;
        validate_family(&root, &variants)?;
        tx.commit()?;
        Ok(variants)
    }
    /// Persist an explicitly reviewed fee-only replacement before exposing its
    /// signature. Requires the original held operation, same sender/nonce and all
    /// original fields except a strictly increased gas price. At most 32 edges.
    /// Node pool bump policy and observed balance are checked by the SDK session.
    pub fn commit_quai_replacement(
        &mut self,
        id: ReservationId,
        parent: Hash32,
        signed: &SignedQuaiTransaction,
    ) -> Result<()> {
        self.commit_replacement_payload(
            id,
            parent,
            signed.signed_bytes().map_err(|_| StorageError::Invalid)?,
        )
    }
    /// Persist a reviewed conflicting Qi candidate with exactly the parent's
    /// ordered inputs/data and strictly lower total output value. This preserves
    /// shared claims; it does not guarantee a miner chooses the higher-fee hash.
    pub fn commit_qi_replacement(
        &mut self,
        id: ReservationId,
        parent: Hash32,
        signed: &quai_consensus::SignedQiOperation,
    ) -> Result<()> {
        self.commit_replacement_payload(
            id,
            parent,
            signed.signed_bytes().map_err(|_| StorageError::Invalid)?,
        )
    }
    fn commit_replacement_payload(
        &mut self,
        id: ReservationId,
        parent: Hash32,
        payload: Vec<u8>,
    ) -> Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let record = reservation_read(&tx, &self.key, id)?.ok_or(StorageError::Invalid)?;
        if !matches!(
            record.state,
            ReservationState::Signed | ReservationState::Submitted
        ) {
            return Err(StorageError::Transition);
        }
        let root =
            signed_payload_read(&tx, &self.key, self.scope, id)?.ok_or(StorageError::Invalid)?;
        let mut variants = read_variants(&tx, &self.key, id)?;
        let new = QuaiReplacement { parent, payload };
        if variants.contains(&new) {
            validate_family(&root, &variants)?;
            tx.commit()?;
            return Ok(());
        }
        variants.push(new);
        validate_family(&root, &variants)?;
        let sequence = variants.len() - 1;
        let new = &variants[sequence];
        tx.execute(
            "INSERT INTO quai_replacements VALUES(?1,?2,?3,?4,?5)",
            params![
                &self.key[..],
                &id.0[..],
                sequence as u32,
                &parent.bytes()[..],
                new.payload
            ],
        )?;
        // A family view omitting this new candidate is stale immediately. Keep
        // its revision as a tombstone under the same writer lock as the append.
        // SQLite STRICT integer storage also rejects revision overflow, rolling
        // back the entire operation before any new signature is exposed.
        tx.execute(
            "UPDATE observation_cache SET revision=revision+1,payload=NULL WHERE scope=?1 AND operation=?2 AND candidate=?3 AND slot=65535",
            params![&self.key[..], &id.0[..], &record.transaction.ok_or(StorageError::Invalid)?.bytes()[..]],
        )?;
        tx.commit()?;
        Ok(())
    }
}
