use crate::{HARDENED_INDEX, PaymentCode, PaymentError, PrivatePaymentCode};
use quai_crypto::PublicKey;
use quai_primitives::{Ledger, QiAddress, Zone};
use serde::{Deserialize, Serialize};

/// Local maximum work per explicit search call, independent of consensus rules.
pub const MAX_SEARCH_ATTEMPTS: u32 = 100_000;
/// Maximum serialized public channel metadata size before parsing.
pub const MAX_CHANNEL_BYTES: usize = 4096;
/// Versioned metadata scheme identifying the exact derivation path and ledger.
pub const CHANNEL_SCHEME: &str = "bip47-quai-969-v1";

/// Direction relative to the local payment-code owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaymentDirection {
    /// Derive a recipient's destination using the local notification secret.
    Send,
    /// Derive a local spendable destination using the peer notification point.
    Receive,
}
/// Explicit candidate range for grinding a Qi address in one zone.
#[derive(Clone, Copy, Debug)]
pub struct PaymentSearch {
    /// Required deployed zone.
    pub zone: Zone,
    /// First exact nonhardened receiver child index to examine.
    pub start_index: u32,
    /// Maximum candidates, from one through MAX_SEARCH_ATTEMPTS.
    pub max_attempts: u32,
}
/// Public result of a successful bounded search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaymentSearchResult {
    /// Validated Qi destination.
    pub address: QiAddress,
    /// Full compressed secp256k1 point for signing/input metadata.
    pub public_key: PublicKey,
    /// Exact child index, retaining all nonmatching candidates skipped before it.
    pub index: u32,
    /// Number of candidates examined, including this match.
    pub attempts: u32,
    /// First unexamined index; None denotes exhausted nonhardened index space.
    pub next_index: Option<u32>,
}
impl PrivatePaymentCode {
    /// Search a bounded range, polling cancellation before every derivation.
    /// No cursor or network state is mutated. Receiving secrets stay guarded.
    pub fn search(
        &self,
        peer: &PaymentCode,
        direction: PaymentDirection,
        search: PaymentSearch,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<PaymentSearchResult, PaymentError> {
        if search.start_index >= HARDENED_INDEX {
            return Err(PaymentError::InvalidIndex);
        }
        if search.max_attempts == 0 || search.max_attempts > MAX_SEARCH_ATTEMPTS {
            return Err(PaymentError::Limit);
        }
        let mut index = search.start_index;
        for attempts in 0..search.max_attempts {
            if cancelled() {
                return Err(PaymentError::SearchCancelled {
                    attempts,
                    next_index: Some(index),
                });
            }
            let public_key = match direction {
                PaymentDirection::Send => self.send_public_key(peer, index)?,
                PaymentDirection::Receive => self.receive_public_key(peer, index)?,
            };
            let address = public_key.address();
            let next = index.checked_add(1).filter(|i| *i < HARDENED_INDEX);
            if address.ledger() == Ledger::Qi && address.zone() == Ok(search.zone) {
                return Ok(PaymentSearchResult {
                    address: QiAddress::try_from(address).map_err(|_| PaymentError::Derivation)?,
                    public_key,
                    index,
                    attempts: attempts + 1,
                    next_index: next,
                });
            }
            index = next.ok_or(PaymentError::SearchExhausted {
                attempts: attempts + 1,
                next_index: None,
            })?;
        }
        Err(PaymentError::SearchExhausted {
            attempts: search.max_attempts,
            next_index: Some(index),
        })
    }
}

/// Public recoverable channel state bound to an exact private owner account.
///
/// Contains no private keys, but reveals peer relationships and derivation cursors.
/// Persist authenticated metadata before using reserved destinations externally.
/// Cursor provenance/rollback protection belongs to the wallet storage layer.
#[derive(Clone)]
pub struct PaymentChannel {
    local: PaymentCode,
    peer: PaymentCode,
    account: u32,
    send_next: [Option<u32>; 9],
    receive_next: [Option<u32>; 9],
}
impl core::fmt::Debug for PaymentChannel {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("PaymentChannel([REDACTED])")
    }
}
impl PaymentChannel {
    /// Open a local context; this does not publish a notification or contact nodes.
    pub fn new(owner: &PrivatePaymentCode, peer: PaymentCode) -> Self {
        Self {
            local: owner.public_code().clone(),
            peer,
            account: owner.account(),
            send_next: [Some(0); 9],
            receive_next: [Some(0); 9],
        }
    }
    /// Restore bounded versioned metadata and verify exact code/account ownership.
    /// Unknown/duplicate fields, malformed codes and hardened cursors are rejected.
    pub fn from_json(owner: &PrivatePaymentCode, bytes: &[u8]) -> Result<Self, PaymentError> {
        if bytes.len() > MAX_CHANNEL_BYTES {
            return Err(PaymentError::Limit);
        }
        let dto: ChannelDto =
            serde_json::from_slice(bytes).map_err(|_| PaymentError::InvalidChannel)?;
        if dto.schema_version != 1 || dto.scheme != CHANNEL_SCHEME {
            return Err(PaymentError::InvalidChannel);
        }
        let local = PaymentCode::from_base58(&dto.local_payment_code)?;
        if dto.account != owner.account() || local != *owner.public_code() {
            return Err(PaymentError::OwnerMismatch);
        }
        let peer = PaymentCode::from_base58(&dto.counterparty_payment_code)?;
        if dto
            .send_next
            .iter()
            .chain(&dto.receive_next)
            .flatten()
            .any(|i| *i >= HARDENED_INDEX)
        {
            return Err(PaymentError::InvalidIndex);
        }
        Ok(Self {
            local,
            peer,
            account: dto.account,
            send_next: dto.send_next,
            receive_next: dto.receive_next,
        })
    }
    /// Export public but privacy-sensitive metadata for authenticated backup storage.
    pub fn to_json(&self) -> Result<Vec<u8>, PaymentError> {
        let dto = ChannelDto {
            schema_version: 1,
            scheme: CHANNEL_SCHEME.into(),
            account: self.account,
            local_payment_code: self.local.to_base58(),
            counterparty_payment_code: self.peer.to_base58(),
            send_next: self.send_next,
            receive_next: self.receive_next,
        };
        serde_json::to_vec(&dto).map_err(|_| PaymentError::InvalidChannel)
    }
    /// Local ownership binding; contains public extended-key information.
    pub const fn local_code(&self) -> &PaymentCode {
        &self.local
    }
    /// Counterparty public code.
    pub const fn counterparty_code(&self) -> &PaymentCode {
        &self.peer
    }
    /// Local hardened account index before its hardening bit.
    pub const fn account(&self) -> u32 {
        self.account
    }
    /// First unreserved candidate for a direction/zone, or None if exhausted.
    pub fn next_index(&self, direction: PaymentDirection, zone: Zone) -> Option<u32> {
        let i = zone_index(zone);
        match direction {
            PaymentDirection::Send => self.send_next[i],
            PaymentDirection::Receive => self.receive_next[i],
        }
    }
    /// Monotonically advance one cursor after an external durable range reservation.
    /// Equal cursors are idempotent; `None` permanently exhausts that direction/zone.
    /// This changes only in-memory metadata: persist before exposing any address.
    pub fn advance_cursor(
        &mut self,
        owner: &PrivatePaymentCode,
        direction: PaymentDirection,
        zone: Zone,
        next: Option<u32>,
    ) -> Result<(), PaymentError> {
        self.check_owner(owner)?;
        if next.is_some_and(|index| index >= HARDENED_INDEX) {
            return Err(PaymentError::InvalidIndex);
        }
        let previous = self.next_index(direction, zone);
        if matches!((previous, next), (None, Some(_)))
            || matches!((previous,next),(Some(old),Some(new)) if new<old)
        {
            return Err(PaymentError::InvalidIndex);
        }
        let index = zone_index(zone);
        match direction {
            PaymentDirection::Send => self.send_next[index] = next,
            PaymentDirection::Receive => self.receive_next[index] = next,
        }
        Ok(())
    }
    /// Reserve the next matching destination and advance its cursor.
    ///
    /// This explicitly differs from JS's reuse-until-status-changes convenience
    /// method: successful calls reserve distinct indices. Cancellation/exhaustion
    /// retains safe continuation progress past already checked nonmatching keys.
    /// Persist the resulting state before publishing or using the destination.
    pub fn reserve_next(
        &mut self,
        owner: &PrivatePaymentCode,
        direction: PaymentDirection,
        zone: Zone,
        max_attempts: u32,
        cancelled: impl FnMut() -> bool,
    ) -> Result<PaymentSearchResult, PaymentError> {
        self.check_owner(owner)?;
        let start_index =
            self.next_index(direction, zone)
                .ok_or(PaymentError::SearchExhausted {
                    attempts: 0,
                    next_index: None,
                })?;
        let result = owner.search(
            &self.peer,
            direction,
            PaymentSearch {
                zone,
                start_index,
                max_attempts,
            },
            cancelled,
        );
        let next = match &result {
            Ok(found) => Some(found.next_index),
            Err(
                PaymentError::SearchCancelled { next_index, .. }
                | PaymentError::SearchExhausted { next_index, .. },
            ) => Some(*next_index),
            _ => None,
        };
        if let Some(next) = next {
            let i = zone_index(zone);
            match direction {
                PaymentDirection::Send => self.send_next[i] = next,
                PaymentDirection::Receive => self.receive_next[i] = next,
            };
        }
        result
    }
    /// Derive a receiving secret only after verifying the supplied owner's identity.
    pub fn receive_key(
        &self,
        owner: &PrivatePaymentCode,
        index: u32,
    ) -> Result<quai_crypto::SecretKey, PaymentError> {
        self.check_owner(owner)?;
        owner.receive_key(&self.peer, index)
    }
    fn check_owner(&self, owner: &PrivatePaymentCode) -> Result<(), PaymentError> {
        if self.account != owner.account() || self.local != *owner.public_code() {
            Err(PaymentError::OwnerMismatch)
        } else {
            Ok(())
        }
    }
}
fn zone_index(zone: Zone) -> usize {
    let b = zone.byte();
    usize::from(b >> 4) * 3 + usize::from(b & 15)
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ChannelDto {
    schema_version: u8,
    scheme: String,
    account: u32,
    local_payment_code: String,
    counterparty_payment_code: String,
    send_next: [Option<u32>; 9],
    receive_next: [Option<u32>; 9],
}

#[cfg(test)]
mod advance_tests {
    use super::*;
    #[test]
    fn external_cursor_advance_is_owner_checked_and_irreversible() {
        let owner = PrivatePaymentCode::from_seed(&[0; 16], 0).unwrap();
        let peer = PrivatePaymentCode::from_seed(&[1; 16], 0).unwrap();
        let mut channel = PaymentChannel::new(&owner, peer.public_code().clone());
        let direction = PaymentDirection::Send;
        let zone = Zone::Cyprus1;
        assert!(
            channel
                .advance_cursor(&peer, direction, zone, Some(10))
                .is_err()
        );
        channel
            .advance_cursor(&owner, direction, zone, Some(10))
            .unwrap();
        channel
            .advance_cursor(&owner, direction, zone, Some(10))
            .unwrap();
        assert!(
            channel
                .advance_cursor(&owner, direction, zone, Some(9))
                .is_err()
        );
        assert!(
            channel
                .advance_cursor(&owner, direction, zone, Some(1 << 31))
                .is_err()
        );
        channel
            .advance_cursor(&owner, direction, zone, None)
            .unwrap();
        assert!(
            channel
                .advance_cursor(&owner, direction, zone, Some(10))
                .is_err()
        );
        channel
            .advance_cursor(&owner, direction, zone, None)
            .unwrap();
        assert_eq!(channel.next_index(PaymentDirection::Receive, zone), Some(0));
    }
}
