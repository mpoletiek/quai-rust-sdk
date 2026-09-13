//! Authenticated v2 registered-channel extension. Secret owners never enter SQLite.
use super::*;
use crate::storage::payment::{cursor_value, direction_byte, direction_from, validate_exposure};
use crate::storage::{PaymentAddressRecord, StoredPaymentChannel, StoredPaymentExposure};
use quai_crypto::PublicKey;
use quai_payments::{PaymentCode, PaymentDirection, PrivatePaymentCode};

type PaymentProof = (Vec<PrivatePaymentCode>, BTreeSet<([u8; 65], [u8; 33])>);
impl BackupOrigin {
    /// Derive a BIP47 owner from its effective seed or master xprv. Standalone
    /// scalars have no payment-account ancestry.
    pub fn payment_code(&self, account: u32) -> Result<PrivatePaymentCode> {
        match &self.0 {
            OriginMaterial::Seed(seed) => PrivatePaymentCode::from_seed(seed, account)
                .map_err(|_| WalletBackupError::InvalidInput),
            OriginMaterial::Master(root) => PrivatePaymentCode::from_master_xprv(
                root.export()
                    .map_err(|_| WalletBackupError::InvalidInput)?
                    .expose(),
                account,
            )
            .map_err(|_| WalletBackupError::InvalidInput),
            OriginMaterial::PaymentAccount {
                account: stored,
                key,
            } if account == *stored => PrivatePaymentCode::from_account_xprv(
                key.export()
                    .map_err(|_| WalletBackupError::Resources)?
                    .expose(),
                account,
            )
            .map_err(|_| WalletBackupError::InvalidInput),
            _ => Err(WalletBackupError::Unsupported),
        }
    }
}
impl WalletBackup {
    pub(super) fn payment_proof(&self) -> Result<PaymentProof> {
        if self.state.channels.len() > 1024
            || self.state.channels.len().saturating_mul(self.origins.len()) > 8192
            || self.state.exposures.len() > MAX_RECORDS
        {
            return Err(WalletBackupError::InvalidInput);
        }
        let networks: BTreeSet<[u8; 64]> = self
            .state
            .scopes
            .iter()
            .map(|s| fixed(&s.scope.key()[..64]))
            .collect::<Result<_>>()?;
        let mut owners = Vec::new();
        let mut channels = BTreeMap::new();
        for stored in &self.state.channels {
            if !networks.contains(&stored.network) {
                return Err(WalletBackupError::InvalidInput);
            }
            let mut matched = None;
            for origin in &self.origins {
                if matches!(
                    &origin.0,
                    OriginMaterial::Seed(_) | OriginMaterial::Master(_)
                ) || matches!(&origin.0, OriginMaterial::PaymentAccount { account, .. } if *account == stored.account)
                {
                    let owner = origin.payment_code(stored.account)?;
                    if owner.public_code().to_bytes() == stored.local {
                        matched = Some(owner);
                        break;
                    }
                }
            }
            let owner = matched.ok_or(WalletBackupError::Ownership)?;
            let channel = stored.checked(&owner)?;
            let key = (stored.network, stored.local, stored.peer, stored.account);
            if channels.insert(key, (owners.len(), channel)).is_some() {
                return Err(WalletBackupError::InvalidInput);
            }
            owners.push(owner);
        }
        let imported_addresses: BTreeSet<_> = self
            .state
            .scopes
            .iter()
            .flat_map(|scope| {
                scope
                    .addresses
                    .iter()
                    .filter(|a| a.origin() == KeyOrigin::ImportedPublic)
                    .map(|a| (scope.scope.key(), a.address(), *a.public_key()))
            })
            .collect();
        let mut owned = BTreeSet::new();
        let mut exposures = BTreeSet::new();
        for exposure in &self.state.exposures {
            let record = &exposure.record;
            let key = (
                exposure.network,
                exposure.local,
                exposure.peer,
                exposure.account,
            );
            let (owner_index, channel) = channels.get(&key).ok_or(WalletBackupError::Ownership)?;
            if !exposures.insert((
                key,
                direction_byte(record.direction),
                record.zone.byte(),
                record.index,
            )) || record.burned.end
                > cursor_value(channel.next_index(record.direction, record.zone))
            {
                return Err(WalletBackupError::InvalidInput);
            }
            validate_exposure(&owners[*owner_index], channel.counterparty_code(), record)?;
            if record.direction == PaymentDirection::Receive {
                let mut scope_key = [0u8; 65];
                scope_key[..64].copy_from_slice(&exposure.network);
                scope_key[64] = record.zone.byte();
                if !imported_addresses.contains(&(
                    scope_key,
                    record.address.address(),
                    record.public_key.to_compressed(),
                )) {
                    return Err(WalletBackupError::Ownership);
                }
                owned.insert((scope_key, record.public_key.to_compressed()));
            }
        }
        Ok((owners, owned))
    }
    pub(super) fn version(&self) -> u8 {
        if self.state.scopes.iter().any(|scope| {
            scope
                .operations
                .iter()
                .any(|op| op.kind == 0 && !op.replacements.is_empty())
        }) {
            return 5;
        }
        if self.state.scopes.iter().any(|scope| {
            scope
                .operations
                .iter()
                .any(|op| !op.replacements.is_empty())
        }) {
            return 4;
        }
        if self
            .origins
            .iter()
            .any(|origin| matches!(&origin.0, OriginMaterial::PaymentAccount { .. }))
        {
            3
        } else if self.state.channels.is_empty() && self.state.exposures.is_empty() {
            1
        } else {
            2
        }
    }
    pub(super) fn encode_payments(&self, writer: &mut Writer) -> Result<()> {
        if self.version() == 1 {
            return Ok(());
        }
        writer.u32(self.state.channels.len() as u32)?;
        for channel in &self.state.channels {
            writer.put(&channel.network)?;
            writer.put(&channel.local)?;
            writer.put(&channel.peer)?;
            writer.u32(channel.account)?;
            writer.u64(channel.generation)?;
            writer.short(&channel.metadata)?;
        }
        writer.u32(self.state.exposures.len() as u32)?;
        for exposure in &self.state.exposures {
            writer.put(&exposure.network)?;
            writer.put(&exposure.local)?;
            writer.put(&exposure.peer)?;
            writer.u32(exposure.account)?;
            let r = &exposure.record;
            writer.u8(direction_byte(r.direction))?;
            writer.u8(r.zone.byte())?;
            writer.u32(r.index)?;
            writer.put(&r.public_key.to_compressed())?;
            writer.put(r.address.bytes())?;
            writer.u32(r.burned.start)?;
            writer.u32(r.burned.end)?;
        }
        Ok(())
    }
}
pub(super) fn decode_payments(
    reader: &mut Reader<'_>,
    version: u8,
) -> Result<(Vec<StoredPaymentChannel>, Vec<StoredPaymentExposure>)> {
    if version == 1 {
        return Ok((Vec::new(), Vec::new()));
    }
    let count = reader.count(238)?;
    if count > 1024 {
        return Err(WalletBackupError::InvalidInput);
    }
    let mut channels = Vec::new();
    for _ in 0..count {
        let network = reader.array()?;
        let local = reader.array()?;
        let peer = reader.array()?;
        PaymentCode::from_bytes(&local).map_err(|_| WalletBackupError::InvalidInput)?;
        PaymentCode::from_bytes(&peer).map_err(|_| WalletBackupError::InvalidInput)?;
        channels.push(StoredPaymentChannel {
            network,
            local,
            peer,
            account: reader.u32()?,
            generation: reader.u64()?,
            metadata: reader.short(4096)?.to_vec(),
        });
    }
    let count = reader.count(295)?;
    let mut exposures = Vec::new();
    for _ in 0..count {
        let network = reader.array()?;
        let local = reader.array()?;
        let peer = reader.array()?;
        let account = reader.u32()?;
        let direction = direction_from(reader.u8()?)?;
        let zone = Zone::from_byte(reader.u8()?).map_err(|_| WalletBackupError::InvalidInput)?;
        let index = reader.u32()?;
        let public_key = PublicKey::from_sec1_bytes(reader.take(33)?)
            .map_err(|_| WalletBackupError::InvalidInput)?;
        let address = QiAddress::try_from(reader.array::<20>()?)
            .map_err(|_| WalletBackupError::InvalidInput)?;
        let burned = crate::discovery::IndexRange {
            start: reader.u32()?,
            end: reader.u32()?,
        };
        exposures.push(StoredPaymentExposure {
            network,
            local,
            peer,
            account,
            record: PaymentAddressRecord {
                direction,
                zone,
                index,
                public_key,
                address,
                burned,
            },
        });
    }
    Ok((channels, exposures))
}
