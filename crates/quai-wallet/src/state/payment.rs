//! Public payment records and ownership checks shared by native and portable backups.
use super::*;
pub use crate::payment_allocation::PaymentAddressRecord;
use quai_payments::{PaymentChannel, PaymentCode, PaymentDirection, PrivatePaymentCode};
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
