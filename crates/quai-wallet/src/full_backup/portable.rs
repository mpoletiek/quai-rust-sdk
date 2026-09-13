//! Ownership-proved recovery capture from explicitly frozen portable journals.
use super::*;
use crate::account_custody::AccountOperationBook;
use crate::allocation::{AddressAllocationBook, AddressAllocationStatus};
use crate::payment_allocation::{PaymentAllocationBook, PaymentAllocationStatus};
use crate::qi_custody::QiOperationBook;
use crate::state::payment::{StoredPaymentChannel, StoredPaymentExposure};
use quai_payments::{PaymentChannel, PaymentDirection};

/// Maximum combined journal descriptors in one portable recovery capture.
pub const MAX_PORTABLE_CAPTURE_JOURNALS: usize = 128;
/// One account journal and its explicit owned public origin descriptor.
#[derive(Clone, Copy)]
pub struct AccountCustodyCapture<'a> {
    /// Frozen account custody snapshot.
    pub book: &'a AccountOperationBook,
    /// Exact public metadata, proved by the supplied private origins.
    pub address: &'a PublicAddress,
}
/// Explicit frozen inputs for a recovery backup spanning portable wallet journals.
/// Callers must provide every relevant journal and known address, and establish a
/// consistent snapshot before capture. Missing inputs cannot be discovered here.
#[derive(Default)]
pub struct PortableWalletCapture<'a> {
    /// HD receive/change burned cursors and completed owned addresses.
    pub allocations: &'a [&'a AddressAllocationBook],
    /// Account nonce floors, exact signed roots/candidates and consumed IDs.
    pub accounts: &'a [AccountCustodyCapture<'a>],
    /// Exact Qi claims, public origins and signed roots/candidates.
    pub qi: &'a [&'a QiOperationBook],
    /// Payment channel cursors and completed send/receive exposures.
    pub payments: &'a [&'a PaymentAllocationBook],
    /// Carry forward owned address inventory, HD burned floors, payment channels
    /// and exposures from an authenticated earlier backup. Custody must still be
    /// supplied through every current account/Qi journal; old operation records and
    /// nonce claims are deliberately not copied by this inventory-only field.
    pub previous_inventory: Option<&'a WalletBackup>,
    /// Previously known owned addresses absent from the current journals, for
    /// example inventory retained after initializing allocators from an old backup.
    pub additional_addresses: &'a [(NetworkScope, PublicAddress)],
}
fn scope_state(
    scopes: &mut BTreeMap<[u8; 65], ScopeState>,
    scope: NetworkScope,
) -> Result<&mut ScopeState> {
    if scope.chain_id == U256::ZERO || scope.genesis == Hash32::ZERO {
        return Err(WalletBackupError::InvalidInput);
    }
    if !scopes.contains_key(&scope.key()) && scopes.len() >= 64 {
        return Err(WalletBackupError::InvalidInput);
    }
    Ok(scopes.entry(scope.key()).or_insert_with(|| ScopeState {
        scope,
        addresses: vec![],
        derivation: vec![],
        nonces: vec![],
        operations: vec![],
    }))
}
fn record(id: ReservationId, state: ReservationState, transaction: Option<Hash32>) -> Reservation {
    Reservation {
        id,
        state: if state == ReservationState::Confirmed {
            ReservationState::Submitted
        } else {
            state
        },
        transaction,
        inclusion: None,
    }
}
impl WalletBackup {
    /// Capture explicit frozen HD/account/Qi/payment journals into the existing
    /// authenticated recovery format. Proves private ownership before returning;
    /// no password KDF, network or storage mutation occurs here.
    ///
    /// All burned cursors survive, including pending/abandoned ranges. Completed
    /// addresses and payment exposures remain in the inventory. Allocation request
    /// IDs and pending search work are not encoded by QUAIWALT v1–v5: restore starts
    /// new allocation journals above retained floors, using new request IDs.
    /// Account/Qi operation IDs, exact claims and candidates are retained.
    ///
    /// Duplicate journal namespaces, conflicting origins or operation IDs, unowned
    /// addresses and full-backup bounds reject the whole capture. Frozen input
    /// journals plus the optional previous inventory's encoded backup total at
    /// most 16 MiB and 128 journal descriptors; normal backup limits also apply. This method cannot establish freshness of independently read stores.
    pub fn capture_portable(
        inputs: PortableWalletCapture<'_>,
        origins: Vec<BackupOrigin>,
    ) -> Result<Self> {
        let count = inputs
            .allocations
            .len()
            .checked_add(inputs.accounts.len())
            .and_then(|n| n.checked_add(inputs.qi.len()))
            .and_then(|n| n.checked_add(inputs.payments.len()))
            .ok_or(WalletBackupError::InvalidInput)?;
        if count > MAX_PORTABLE_CAPTURE_JOURNALS
            || inputs.additional_addresses.len() > MAX_RECORDS
            || origins.is_empty()
            || origins.len() > MAX_ORIGINS
        {
            return Err(WalletBackupError::InvalidInput);
        }
        let mut seen = BTreeSet::new();
        let mut bytes = inputs
            .previous_inventory
            .map(|b| b.encode().map(|bytes| bytes.len()))
            .transpose()?
            .unwrap_or(0);
        let mut include =
            |kind: u8, scope: NetworkScope, identity: Hash32, size: usize| -> Result<()> {
                bytes = bytes
                    .checked_add(size)
                    .ok_or(WalletBackupError::InvalidInput)?;
                if bytes > MAX_PLAINTEXT || !seen.insert((kind, scope.key(), identity)) {
                    return Err(WalletBackupError::InvalidInput);
                }
                Ok(())
            };
        for b in inputs.allocations {
            include(0, b.scope(), b.identity(), b.export_state().len())?;
        }
        for c in inputs.accounts {
            include(
                1,
                c.book.scope(),
                AccountOperationBook::account_identity(c.book.owner()),
                c.book.export_state()?.len(),
            )?;
        }
        for b in inputs.qi {
            include(2, b.scope(), b.identity(), b.export_state()?.len())?;
        }
        for b in inputs.payments {
            include(3, b.scope(), b.identity(), b.export_state().len())?;
        }
        let mut scopes = BTreeMap::new();
        if let Some(previous) = inputs.previous_inventory {
            for old in &previous.state.scopes {
                let scope = scope_state(&mut scopes, old.scope)?;
                scope.addresses.extend(old.addresses.iter().cloned());
                scope.derivation.extend(old.derivation.iter().cloned());
            }
        }
        for b in inputs.allocations {
            let scope = scope_state(&mut scopes, b.scope())?;
            for change in [false, true] {
                scope.derivation.push(DerivationState {
                    coin: b.account().coin_type(),
                    account: b.account().account_index(),
                    change,
                    xpub: b.account().export(),
                    next_index: b.next_index(change),
                });
            }
            scope
                .addresses
                .extend(b.allocations().filter_map(|a| match &a.status {
                    AddressAllocationStatus::Completed(public) => Some(public.clone()),
                    _ => None,
                }));
        }
        for c in inputs.accounts {
            let b = c.book;
            if c.address.address() != b.address().address()
                || *c.address.public_key() != b.owner().to_compressed()
            {
                return Err(WalletBackupError::Ownership);
            }
            let scope = scope_state(&mut scopes, b.scope())?;
            scope.addresses.push(c.address.clone());
            scope.nonces.push(NonceState {
                address: b.address(),
                next_nonce: b.next_nonce(),
            });
            scope
                .operations
                .extend(b.operations().map(|op| OperationState {
                    record: record(op.id, op.state, op.transaction),
                    kind: 1,
                    qi: vec![],
                    nonce: Some((b.address(), op.nonce)),
                    payload: op.payload.clone(),
                    replacements: op.replacements.clone(),
                }));
        }
        for b in inputs.qi {
            let scope = scope_state(&mut scopes, b.scope())?;
            scope.addresses.extend(b.addresses().cloned());
            scope.operations.extend(b.operations().map(|op| {
                OperationState {
                    record: record(op.id, op.state, op.transaction),
                    kind: 0,
                    qi: op
                        .claims
                        .iter()
                        .map(|c| (c.outpoint, c.owner.address()))
                        .collect(),
                    nonce: None,
                    payload: op.payload.clone(),
                    replacements: op.replacements.clone(),
                }
            }));
        }
        let mut channels = BTreeMap::new();
        let mut exposures = Vec::new();
        if let Some(previous) = inputs.previous_inventory {
            for stored in &previous.state.channels {
                let owner = origins
                    .iter()
                    .filter_map(|o| o.payment_code(stored.account).ok())
                    .find(|o| o.public_code().to_bytes() == stored.local)
                    .ok_or(WalletBackupError::Ownership)?;
                let key = (stored.network, stored.local, stored.peer, stored.account);
                channels.insert(key, stored.checked(&owner)?);
            }
            exposures.extend(previous.state.exposures.iter().cloned());
        }
        for b in inputs.payments {
            let owner = origins
                .iter()
                .filter_map(|o| o.payment_code(b.account()).ok())
                .find(|o| o.public_code() == b.local_code())
                .ok_or(WalletBackupError::Ownership)?;
            let network: [u8; 64] = b.scope().key()[..64]
                .try_into()
                .map_err(|_| WalletBackupError::InvalidInput)?;
            let key = (
                network,
                b.local_code().to_bytes(),
                b.peer().to_bytes(),
                b.account(),
            );
            let channel = channels
                .entry(key)
                .or_insert_with(|| PaymentChannel::new(&owner, b.peer().clone()));
            let next = match channel.next_index(b.direction(), b.scope().zone) {
                Some(old) if b.next_index() < 1 << 31 => Some(old.max(b.next_index())),
                _ => None,
            };
            channel
                .advance_cursor(&owner, b.direction(), b.scope().zone, next)
                .map_err(|_| WalletBackupError::InvalidInput)?;
            let scope = scope_state(&mut scopes, b.scope())?;
            for allocation in b.allocations() {
                if let PaymentAllocationStatus::Completed(record) = &allocation.status {
                    if record.direction == PaymentDirection::Receive {
                        scope
                            .addresses
                            .push(PublicAddress::imported(&record.public_key)?);
                    }
                    exposures.push(StoredPaymentExposure {
                        network,
                        local: key.1,
                        peer: key.2,
                        account: key.3,
                        record: record.clone(),
                    });
                }
            }
        }
        for (scope, public) in inputs.additional_addresses {
            scope_state(&mut scopes, *scope)?
                .addresses
                .push(public.clone());
        }
        for scope in scopes.values_mut() {
            let mut addresses = BTreeMap::new();
            for public in scope.addresses.drain(..) {
                if addresses
                    .get(&public.address())
                    .is_some_and(|old| old != &public)
                {
                    return Err(WalletBackupError::Ownership);
                }
                addresses.insert(public.address(), public);
            }
            scope.addresses = addresses.into_values().collect();
            let mut derivation: BTreeMap<_, DerivationState> = BTreeMap::new();
            for cursor in scope.derivation.drain(..) {
                let key = (cursor.coin.number(), cursor.account, cursor.change);
                if let Some(old) = derivation.get_mut(&key) {
                    if old.xpub != cursor.xpub {
                        return Err(WalletBackupError::Ownership);
                    }
                    old.next_index = old.next_index.max(cursor.next_index);
                } else {
                    derivation.insert(key, cursor);
                }
            }
            scope.derivation = derivation.into_values().collect();
            scope.nonces.sort_by_key(|n| n.address);
            scope.operations.sort_by_key(|o| o.record.id.0);
        }
        let mut unique_exposures: BTreeMap<_, StoredPaymentExposure> = BTreeMap::new();
        for exposure in exposures {
            let key = (
                exposure.network,
                exposure.local,
                exposure.peer,
                exposure.account,
                crate::state::payment::direction_byte(exposure.record.direction),
                exposure.record.zone.byte(),
                exposure.record.index,
            );
            if unique_exposures
                .get(&key)
                .is_some_and(|old| old.record != exposure.record)
            {
                return Err(WalletBackupError::InvalidInput);
            }
            unique_exposures.insert(key, exposure);
        }
        let exposures = unique_exposures.into_values().collect();
        let channels = channels
            .into_iter()
            .map(|((network, local, peer, account), channel)| {
                Ok(StoredPaymentChannel {
                    network,
                    local,
                    peer,
                    account,
                    generation: 0,
                    metadata: channel
                        .to_json()
                        .map_err(|_| WalletBackupError::InvalidInput)?,
                })
            })
            .collect::<Result<_>>()?;
        let backup = Self {
            origins,
            state: PublicWalletState {
                scopes: scopes.into_values().collect(),
                channels,
                exposures,
            },
        };
        backup.validate()?;
        // Includes private-origin and inventory framing, beyond the public journal cap.
        backup.encode()?;
        Ok(backup)
    }
}
