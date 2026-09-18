//! Outgoing activity as a wallet lists it, projected from durable custody.
//!
//! Every entry comes from the store's own records: reservations, validated
//! signed payloads, replacement edges and caller-observed inclusion. Nothing is
//! read from the network. Incoming history is deliberately absent: Quai has no
//! by-address transaction query, and past Qi receipts need the node's outpoint
//! history (`Provider::outpoint_deltas`) or an indexer; the current coin
//! snapshot shows holdings, not history.
use super::*;
use std::collections::BTreeSet;

/// Where an outgoing operation stands, from the wallet's own records.
///
/// Exhaustive on purpose: a new status should fail to compile in a wallet's
/// display and retry logic rather than fall into a wildcard arm.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivityStatus {
    /// Reserved and unsigned: inputs or a nonce are held while preparing.
    Preparing,
    /// Released while unsigned. Nothing was signed or sent.
    Cancelled,
    /// Signed, not yet recorded as submitted.
    Signed,
    /// Submitted; inclusion not observed yet, or a reorg removed it.
    Pending,
    /// Observed included at this block. A caller observation, not finality.
    Included(Checkpoint),
}

/// Which Qi operation an entry is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum QiActivityKind {
    /// Ordinary transfer.
    Transfer,
    /// Qi-to-Quai conversion.
    Conversion,
    /// Deposit to a wrapping contract.
    Wrapping,
}

/// What an outgoing operation does, decoded from its signed payload.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ActivityDetail {
    /// No signed payload yet.
    Unsigned,
    /// An account transaction.
    Account {
        /// Sending account.
        from: QuaiAddress,
        /// Recipient; None for a contract creation.
        to: Option<Address>,
        /// Value sent, in base units.
        value: U256,
        /// Account nonce.
        nonce: u64,
        /// Gas limit times gas price: the most the fee can be.
        max_fee: U256,
    },
    /// A Qi operation.
    Qi {
        /// Operation type.
        kind: QiActivityKind,
        /// Total of outputs to addresses this store does not hold, in Qits.
        sent: U256,
        /// Total of outputs back to addresses this store holds, in Qits.
        change: U256,
        /// Number of outputs.
        outputs: usize,
    },
}

/// One outgoing operation.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ActivityEntry {
    /// Caller-assigned operation ID.
    pub id: ReservationId,
    /// Current status.
    pub status: ActivityStatus,
    /// Original transaction identity, once signed.
    pub transaction: Option<Hash32>,
    /// Decoded payload.
    pub detail: ActivityDetail,
    /// Fee-replacement candidates recorded for this operation.
    pub replacements: usize,
}

impl SqliteStore {
    /// Outgoing operations by ID, including cancelled ones, paged like
    /// [`Self::reservations`]: pass the last returned ID as `after`. Limit 1
    /// through 1000. IDs are application-chosen, so time-ordered IDs give a
    /// chronological list.
    pub fn activity(
        &mut self,
        after: Option<ReservationId>,
        limit: u16,
    ) -> Result<Vec<ActivityEntry>> {
        let reservations = self.reservations(after, limit)?;
        let owned: BTreeSet<Address> = self.addresses()?.iter().map(|a| a.address()).collect();
        reservations
            .into_iter()
            .map(|reservation| {
                let status = match (reservation.state, reservation.inclusion) {
                    (ReservationState::Reserved, _) => ActivityStatus::Preparing,
                    (ReservationState::Released, _) => ActivityStatus::Cancelled,
                    (ReservationState::Signed, _) => ActivityStatus::Signed,
                    (ReservationState::Confirmed, Some(block)) => ActivityStatus::Included(block),
                    (ReservationState::Submitted | ReservationState::Confirmed, _) => {
                        ActivityStatus::Pending
                    }
                };
                let (detail, replacements) = match self.signed_payload(reservation.id)? {
                    None => (ActivityDetail::Unsigned, 0),
                    Some(payload) => (
                        decode_detail(&payload, &owned)?,
                        self.replacement_candidates(reservation.id)?.len(),
                    ),
                };
                Ok(ActivityEntry {
                    id: reservation.id,
                    status,
                    transaction: reservation.transaction,
                    detail,
                    replacements,
                })
            })
            .collect()
    }
}

/// Decode a validated signed payload. Account payloads start with the
/// transaction-type field of an account transaction; anything else here is a
/// Qi operation, which the store has already validated as one.
fn decode_detail(payload: &[u8], owned: &BTreeSet<Address>) -> Result<ActivityDetail> {
    if let Ok(signed) = SignedQuaiTransaction::decode(payload) {
        let tx = signed.transaction();
        return Ok(ActivityDetail::Account {
            from: signed.from(),
            to: tx.to,
            value: tx.value,
            nonce: tx.nonce,
            max_fee: tx
                .gas_price
                .checked_mul(U256::from(tx.gas_limit))
                .ok_or(StorageError::Invalid)?,
        });
    }
    let signed =
        quai_consensus::SignedQiOperation::decode(payload).map_err(|_| StorageError::Invalid)?;
    let kind = match &signed {
        quai_consensus::SignedQiOperation::Transfer(_) => QiActivityKind::Transfer,
        quai_consensus::SignedQiOperation::Conversion(_) => QiActivityKind::Conversion,
        quai_consensus::SignedQiOperation::Wrapping(_) => QiActivityKind::Wrapping,
    };
    let outputs = &signed.transaction().outputs;
    let (mut sent, mut change) = (U256::ZERO, U256::ZERO);
    for output in outputs {
        let value = U256::from(output.denomination.value());
        let total = if owned.contains(&output.address) {
            &mut change
        } else {
            &mut sent
        };
        *total = total.checked_add(value).ok_or(StorageError::Invalid)?;
    }
    Ok(ActivityDetail::Qi {
        kind,
        sent,
        change,
        outputs: outputs.len(),
    })
}
