//! Verified migration of the pinned quais.js plaintext whole-wallet JSON schema.
//! Legacy JSON cannot carry Rust custody or unexposed burned ranges. It must never
//! replace a live journal directly; import to an authenticated backup and merge.
use super::*;
use crate::state::payment::{StoredPaymentChannel, StoredPaymentExposure};
use crate::{ExtendedPublicKey, HdWallet, Mnemonic};
use quai_crypto::PublicKey;
use quai_payments::{PaymentChannel, PaymentCode, PaymentDirection, PrivatePaymentCode};
mod json;
use json::{Document, Row, Text};

/// Maximum input/output JSON size, including private fields.
pub const MAX_LEGACY_JSON_BYTES: usize = 1024 * 1024;
/// Maximum combined owned addresses and peer send exposures per document.
pub const MAX_LEGACY_ADDRESSES: usize = 1024;
/// Maximum peer codes in a legacy document.
pub const MAX_LEGACY_CHANNELS: usize = 64;
/// Explicit private identity omitted or damaged by the reference serializer.
/// The caller supplies the original mnemonic language/passphrase and a trusted
/// coin-root xpub. Empty/import-only documents cannot silently change identity.
pub struct LegacyWalletIdentity<'a> {
    /// Original mnemonic language, explicitly selected for the phrase in JSON.
    pub language: crate::Language,
    /// Original BIP39 passphrase; never inferred from the legacy document.
    pub passphrase: &'a str,
    /// Trusted coin-root public key. Explicitly neuter the reference
    /// misnamed `xPub()` result before using it; its raw xprv is rejected.
    pub expected_root: &'a ExtendedPublicKey,
}
fn invalid<T>() -> Result<T> {
    Err(WalletBackupError::InvalidInput)
}
fn decode<const N: usize>(text: &str) -> Result<Zeroizing<[u8; N]>> {
    let b = text.as_bytes();
    if b.len() != 2 + N * 2 || &b[..2] != b"0x" {
        return invalid();
    }
    fn nibble(b: u8) -> Result<u8> {
        match b {
            b'0'..=b'9' => Ok(b - b'0'),
            b'a'..=b'f' => Ok(b - b'a' + 10),
            b'A'..=b'F' => Ok(b - b'A' + 10),
            _ => invalid(),
        }
    }
    let mut out = Zeroizing::new([0; N]);
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = nibble(b[2 + i * 2])? * 16 + nibble(b[3 + i * 2])?;
    }
    Ok(out)
}
fn hexadecimal(bytes: &[u8]) -> Text {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = Zeroizing::new(String::with_capacity(2 + bytes.len() * 2));
    out.push_str("0x");
    for b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 15) as usize] as char);
    }
    Text(out)
}
fn row_public(row: &Row, coin: CoinType) -> Result<(PublicKey, Zone)> {
    if row.account >= 1 << 31
        || row.pub_key.len() != 68
        || row.address.len() != 42
        || row.zone.len() != 4
    {
        return invalid();
    }
    let key = PublicKey::from_sec1_bytes(&*decode::<33>(&row.pub_key)?)
        .map_err(|_| WalletBackupError::InvalidInput)?;
    let address: Address = row
        .address
        .parse()
        .map_err(|_| WalletBackupError::InvalidInput)?;
    let zone: Zone = row
        .zone
        .parse()
        .map_err(|_| WalletBackupError::InvalidInput)?;
    if key.address() != address
        || address
            .zone()
            .map_err(|_| WalletBackupError::InvalidInput)?
            != zone
        || address.ledger() != coin.ledger()
    {
        return invalid();
    }
    if let Some(cp) = &row.last_synced_block {
        let _: Hash32 = cp
            .hash
            .parse()
            .map_err(|_| WalletBackupError::InvalidInput)?;
        if cp.number > 9_007_199_254_740_991 {
            return invalid();
        }
    }
    if coin == CoinType::Qi {
        if row.change.is_none() || row.status.is_none() || row.derivation_path.is_none() {
            return invalid();
        }
    } else if row.change.is_some()
        || row.status.is_some()
        || row.derivation_path.is_some()
        || row.last_synced_block.is_some()
    {
        return invalid();
    }
    Ok((key, zone))
}
fn index(row: &Row) -> Result<u32> {
    u32::try_from(row.index)
        .ok()
        .filter(|i| *i < 1 << 31)
        .ok_or(WalletBackupError::InvalidInput)
}
fn scoped(
    states: &mut BTreeMap<u8, ScopeState>,
    network: NetworkScope,
    zone: Zone,
) -> &mut ScopeState {
    states.entry(zone.byte()).or_insert_with(|| ScopeState {
        scope: NetworkScope { zone, ..network },
        addresses: vec![],
        derivation: vec![],
        nonces: vec![],
        operations: vec![],
    })
}
struct Channels {
    owners: BTreeMap<u32, PrivatePaymentCode>,
    channels: BTreeMap<(u32, [u8; 80]), PaymentChannel>,
    exposures: Vec<StoredPaymentExposure>,
    seen: BTreeSet<(u32, [u8; 80], u8, u32)>,
}
impl Channels {
    fn new() -> Self {
        Self {
            owners: BTreeMap::new(),
            channels: BTreeMap::new(),
            exposures: vec![],
            seen: BTreeSet::new(),
        }
    }
    fn open(&mut self, origin: &BackupOrigin, peer: &PaymentCode, account: u32) -> Result<()> {
        if !self.owners.contains_key(&account) {
            if self.owners.len() >= 64 {
                return invalid();
            }
            self.owners.insert(account, origin.payment_code(account)?);
        }
        let owner = self
            .owners
            .get(&account)
            .ok_or(WalletBackupError::InvalidInput)?;
        let key = (account, peer.to_bytes());
        if !self.channels.contains_key(&key) {
            if self.channels.len() >= MAX_LEGACY_CHANNELS {
                return invalid();
            }
            self.channels
                .insert(key, PaymentChannel::new(owner, peer.clone()));
        }
        Ok(())
    }
    fn exposure(
        &mut self,
        origin: &BackupOrigin,
        network: NetworkScope,
        row: &Row,
        key: PublicKey,
        peer: &PaymentCode,
        direction: PaymentDirection,
    ) -> Result<()> {
        if row.change != Some(false) {
            return invalid();
        }
        let zone = key
            .address()
            .zone()
            .map_err(|_| WalletBackupError::InvalidInput)?;
        let index = index(row)?;
        self.open(origin, peer, row.account)?;
        if !self.seen.insert((
            row.account,
            peer.to_bytes(),
            u8::from(direction == PaymentDirection::Receive),
            index,
        )) {
            return invalid();
        }
        let owner = self
            .owners
            .get(&row.account)
            .ok_or(WalletBackupError::InvalidInput)?;
        let expected = match direction {
            PaymentDirection::Send => owner.send_public_key(peer, index),
            PaymentDirection::Receive => owner.receive_public_key(peer, index),
        }
        .map_err(|_| WalletBackupError::Ownership)?;
        if expected != key {
            return Err(WalletBackupError::Ownership);
        }
        let channel = self
            .channels
            .get_mut(&(row.account, peer.to_bytes()))
            .ok_or(WalletBackupError::InvalidInput)?;
        let next = index + 1;
        let previous = channel.next_index(direction, zone).unwrap_or(1 << 31);
        if next > previous {
            channel
                .advance_cursor(
                    owner,
                    direction,
                    zone,
                    if next == 1 << 31 { None } else { Some(next) },
                )
                .map_err(|_| WalletBackupError::InvalidInput)?;
        }
        self.exposures.push(StoredPaymentExposure {
            network: network.key()[..64]
                .try_into()
                .map_err(|_| WalletBackupError::InvalidInput)?,
            local: owner.public_code().to_bytes(),
            peer: peer.to_bytes(),
            account: row.account,
            record: PaymentAddressRecord {
                direction,
                zone,
                index,
                address: key
                    .address()
                    .try_into()
                    .map_err(|_| WalletBackupError::InvalidInput)?,
                public_key: key,
                burned: crate::discovery::IndexRange {
                    start: index,
                    end: next,
                },
            },
        });
        Ok(())
    }
}
/// Import version-one Quai/Qi JSON after proving the explicit original identity
/// and every HD, imported-key and payment send/receive address. Retains known
/// address floors and channel exposures across zones of the supplied network.
/// Ignores cached statuses/checkpoints for recovery, creates no UTXO/nonce claims
/// and performs no storage or RPC writes. Merge the resulting backup into live
/// stores monotonically, then rescan. Missing history/ranges cannot be recovered.
/// Limits: 1 MiB JSON, 1,024 addresses, 64 account-channel pairs and 15 direct
/// imported keys in addition to the seed origin. Duplicate/unknown fields reject.
pub fn import_quais_json(
    document: &[u8],
    network: NetworkScope,
    identity: LegacyWalletIdentity<'_>,
) -> Result<WalletBackup> {
    let doc = json::parse(document)?;
    if doc.version != 1 || network.chain_id == U256::ZERO || network.genesis == Hash32::ZERO {
        return invalid();
    }
    let coin = match doc.coin_type {
        994 => CoinType::Quai,
        969 => CoinType::Qi,
        _ => return invalid(),
    };
    if (coin == CoinType::Qi) != doc.sender_payment_code_info.is_some() {
        return invalid();
    }
    let mnemonic = Mnemonic::parse(identity.language, &doc.phrase.0)
        .map_err(|_| WalletBackupError::InvalidInput)?;
    let wallet = HdWallet::from_mnemonic(&mnemonic, identity.passphrase, coin)
        .map_err(|_| WalletBackupError::InvalidInput)?;
    if wallet.root_public_key().export() != identity.expected_root.export() {
        return Err(WalletBackupError::Ownership);
    }
    let mut origins = vec![BackupOrigin::from_mnemonic(&mnemonic, identity.passphrase)?];
    let mut states = BTreeMap::new();
    scoped(&mut states, network, network.zone);
    let mut accounts = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut channels = Channels::new();
    for row in &doc.addresses.0 {
        let (key, zone) = row_public(row, coin)?;
        if !seen.insert(key.address()) {
            return invalid();
        }
        let path = row.derivation_path.as_ref().map(|p| p.0.as_str());
        let is_hd =
            coin == CoinType::Quai || matches!(path, Some("BIP44:external" | "BIP44:change"));
        let public = if is_hd {
            let change = path == Some("BIP44:change");
            if coin == CoinType::Qi && row.change != Some(change) {
                return invalid();
            }
            let index = index(row)?;
            if !accounts.contains_key(&row.account) {
                if accounts.len() >= 64 {
                    return invalid();
                }
                accounts.insert(
                    row.account,
                    wallet
                        .account_public(row.account)
                        .map_err(|_| WalletBackupError::Ownership)?,
                );
            }
            let account = accounts
                .get(&row.account)
                .ok_or(WalletBackupError::Ownership)?;
            let public = PublicAddress::derive(account, change, index)?;
            if public.public_key() != &key.to_compressed() {
                return Err(WalletBackupError::Ownership);
            }
            let state = scoped(&mut states, network, zone);
            if let Some(old) = state
                .derivation
                .iter_mut()
                .find(|d| d.account == row.account && d.change == change)
            {
                old.next_index = old.next_index.max(index + 1);
            } else {
                state.derivation.push(DerivationState {
                    coin,
                    account: row.account,
                    change,
                    xpub: account.export(),
                    next_index: index + 1,
                });
            }
            public
        } else if path.is_some_and(|p| p.starts_with("0x")) {
            if row.index != -1
                || row.account != 0
                || row.change != Some(false)
                || origins.len() >= MAX_ORIGINS
            {
                return invalid();
            }
            let bytes = decode::<32>(path.ok_or(WalletBackupError::InvalidInput)?)?;
            let secret =
                SecretKey::from_bytes(&bytes).map_err(|_| WalletBackupError::InvalidInput)?;
            if secret.public_key() != key {
                return Err(WalletBackupError::Ownership);
            }
            origins.push(BackupOrigin::from_private_key(&secret));
            PublicAddress::imported(&key)?
        } else {
            let peer = PaymentCode::from_base58(path.ok_or(WalletBackupError::InvalidInput)?)
                .map_err(|_| WalletBackupError::InvalidInput)?;
            channels.exposure(
                &origins[0],
                network,
                row,
                key,
                &peer,
                PaymentDirection::Receive,
            )?;
            PublicAddress::imported(&key)?
        };
        scoped(&mut states, network, zone).addresses.push(public);
    }
    if let Some(peers) = doc.sender_payment_code_info {
        for (code, rows) in peers.0 {
            let peer =
                PaymentCode::from_base58(&code).map_err(|_| WalletBackupError::InvalidInput)?;
            if rows.0.is_empty() && !channels.channels.keys().any(|(_, p)| *p == peer.to_bytes()) {
                channels.open(&origins[0], &peer, 0)?;
            }
            for row in &rows.0 {
                let (key, _) = row_public(row, CoinType::Qi)?;
                if row.derivation_path.as_ref().map(|p| p.0.as_str()) != Some(code.as_str()) {
                    return invalid();
                }
                channels.exposure(
                    &origins[0],
                    network,
                    row,
                    key,
                    &peer,
                    PaymentDirection::Send,
                )?;
            }
        }
    }
    let mut stored = vec![];
    for ((account, _), channel) in channels.channels {
        stored.push(StoredPaymentChannel {
            network: network.key()[..64]
                .try_into()
                .map_err(|_| WalletBackupError::InvalidInput)?,
            local: channel.local_code().to_bytes(),
            peer: channel.counterparty_code().to_bytes(),
            account,
            generation: 0,
            metadata: channel
                .to_json()
                .map_err(|_| WalletBackupError::InvalidInput)?,
        });
    }
    let backup = WalletBackup {
        origins,
        state: PublicWalletState {
            scopes: states.into_values().collect(),
            channels: stored,
            exposures: channels.exposures,
        },
    };
    backup.validate()?;
    Ok(backup)
}
fn make_row(
    public: &PublicAddress,
    account: u32,
    index: i64,
    coin: CoinType,
    path: Option<Text>,
    change: bool,
) -> Result<Row> {
    Ok(Row {
        pub_key: hexadecimal(public.public_key()).0.to_string(),
        address: public.address().to_string(),
        account,
        index,
        zone: format!(
            "0x{:02x}",
            public
                .address()
                .zone()
                .map_err(|_| WalletBackupError::InvalidInput)?
                .byte()
        ),
        change: (coin == CoinType::Qi).then_some(change),
        status: (coin == CoinType::Qi).then_some(json::Status::Unknown),
        derivation_path: path,
        last_synced_block: None,
    })
}
/// Export a representable legacy wallet as explicitly guarded plaintext JSON.
/// Requires the original English mnemonic with an empty passphrase identity,
/// because the pinned JavaScript importer cannot recover other mnemonic contexts.
/// The selected network/coin must have no durable operations, advanced nonce
/// floors or burned HD/payment tails beyond the exported address indexes. Such
/// state rejects instead of silently weakening recovery; use encrypted backups.
/// Direct imported keys are intentionally present in this secret export, never
/// in ordinary public metadata. Output is bounded to 1 MiB and zeroizes on drop.
pub fn export_quais_json(
    backup: &WalletBackup,
    network: NetworkScope,
    coin: CoinType,
    mnemonic: &Mnemonic,
) -> Result<crate::SecretString> {
    if network.chain_id == U256::ZERO
        || network.genesis == Hash32::ZERO
        || mnemonic.language() != crate::Language::English
    {
        return Err(WalletBackupError::Unsupported);
    }
    let seed = mnemonic.to_seed("");
    let wallet =
        HdWallet::from_seed(seed.expose(), coin).map_err(|_| WalletBackupError::InvalidInput)?;
    let root = wallet.root_public_key().export();
    let mut matches = false;
    for origin in &backup.origins {
        if let Ok(master) = origin.export_master_xprv()
            && HdWallet::from_master_xprv(master.expose(), coin)
                .is_ok_and(|w| w.root_public_key().export() == root)
        {
            matches = true;
            break;
        }
    }
    if !matches {
        return Err(WalletBackupError::Ownership);
    }
    backup.validate()?;
    let network_key: [u8; 64] = network.key()[..64]
        .try_into()
        .map_err(|_| WalletBackupError::InvalidInput)?;
    let relevant: Vec<_> = backup
        .state
        .scopes
        .iter()
        .filter(|s| s.scope.key()[..64] == network_key)
        .collect();
    if relevant.is_empty() {
        return invalid();
    }
    let mut rows = vec![];
    let mut senders = BTreeMap::<String, json::List<Row>>::new();
    let mut accounts = BTreeMap::new();
    let exposures: Vec<_> = backup
        .state
        .exposures
        .iter()
        .filter(|e| e.network == network_key)
        .collect();
    for state in relevant {
        if !state.operations.is_empty() || state.nonces.iter().any(|n| n.next_nonce > 0) {
            return Err(WalletBackupError::Unsupported);
        }
        for cursor in state.derivation.iter().filter(|d| d.coin == coin) {
            let maximum = state
                .addresses
                .iter()
                .filter_map(|a| match a.origin() {
                    KeyOrigin::Bip44 {
                        coin: c,
                        account,
                        change,
                        index,
                    } if c == coin && account == cursor.account && change == cursor.change => {
                        Some(index + 1)
                    }
                    _ => None,
                })
                .max()
                .unwrap_or(0);
            if cursor.next_index != maximum {
                return Err(WalletBackupError::Unsupported);
            }
        }
        for public in state
            .addresses
            .iter()
            .filter(|a| a.address().ledger() == coin.ledger())
        {
            if rows.len() >= MAX_LEGACY_ADDRESSES {
                return invalid();
            }
            let row = match public.origin() {
                KeyOrigin::Bip44 {
                    coin: c,
                    account,
                    change,
                    index,
                } => {
                    if c != coin || (coin == CoinType::Quai && change) {
                        return Err(WalletBackupError::Unsupported);
                    }
                    if let std::collections::btree_map::Entry::Vacant(entry) =
                        accounts.entry(account)
                    {
                        entry.insert(
                            wallet
                                .account_public(account)
                                .map_err(|_| WalletBackupError::Ownership)?,
                        );
                    }
                    if PublicAddress::derive(
                        accounts.get(&account).ok_or(WalletBackupError::Ownership)?,
                        change,
                        index,
                    )? != *public
                    {
                        return Err(WalletBackupError::Ownership);
                    }
                    make_row(
                        public,
                        account,
                        i64::from(index),
                        coin,
                        if coin == CoinType::Qi {
                            Some(Text(Zeroizing::new(
                                if change {
                                    "BIP44:change"
                                } else {
                                    "BIP44:external"
                                }
                                .to_owned(),
                            )))
                        } else {
                            None
                        },
                        change,
                    )?
                }
                KeyOrigin::ImportedPublic => {
                    if coin != CoinType::Qi {
                        return Err(WalletBackupError::Unsupported);
                    }
                    let received: Vec<_> = exposures
                        .iter()
                        .filter(|e| {
                            e.record.direction == PaymentDirection::Receive
                                && e.record.address.address() == public.address()
                        })
                        .collect();
                    if received.len() > 1 {
                        return Err(WalletBackupError::Unsupported);
                    }
                    if let Some(e) = received.first() {
                        make_row(
                            public,
                            e.account,
                            i64::from(e.record.index),
                            coin,
                            Some(Text(Zeroizing::new(
                                PaymentCode::from_bytes(&e.peer)
                                    .map_err(|_| WalletBackupError::InvalidInput)?
                                    .to_base58(),
                            ))),
                            false,
                        )?
                    } else {
                        let mut secret = None;
                        for origin in &backup.origins {
                            if let OriginMaterial::Imported(bytes) = &origin.0 {
                                let key = SecretKey::from_bytes(bytes)
                                    .map_err(|_| WalletBackupError::Ownership)?;
                                if key.public_key().to_compressed() == *public.public_key() {
                                    secret = Some(hexadecimal(key.export_bytes().as_bytes()));
                                    break;
                                }
                            }
                        }
                        make_row(
                            public,
                            0,
                            -1,
                            coin,
                            Some(secret.ok_or(WalletBackupError::Ownership)?),
                            false,
                        )?
                    }
                }
            };
            rows.push(row);
        }
    }
    if coin == CoinType::Qi {
        for stored in backup
            .state
            .channels
            .iter()
            .filter(|c| c.network == network_key)
        {
            if senders.len() >= MAX_LEGACY_CHANNELS {
                return invalid();
            }
            let owner = PrivatePaymentCode::from_seed(seed.expose(), stored.account)
                .map_err(|_| WalletBackupError::Ownership)?;
            let channel = stored.checked(&owner)?;
            let retained: Vec<_> = exposures
                .iter()
                .filter(|e| {
                    e.local == stored.local && e.peer == stored.peer && e.account == stored.account
                })
                .collect();
            if stored.account > 0 && retained.is_empty() {
                return Err(WalletBackupError::Unsupported);
            }
            for zone in Zone::ALL {
                for direction in [PaymentDirection::Send, PaymentDirection::Receive] {
                    let floor = retained
                        .iter()
                        .filter(|e| e.record.zone == zone && e.record.direction == direction)
                        .map(|e| e.record.index + 1)
                        .max()
                        .unwrap_or(0);
                    if channel.next_index(direction, zone).unwrap_or(1 << 31) != floor {
                        return Err(WalletBackupError::Unsupported);
                    }
                }
            }
            senders
                .entry(channel.counterparty_code().to_base58())
                .or_insert_with(|| json::List(vec![]));
        }
        let mut total = rows.len();
        for exposure in exposures
            .iter()
            .filter(|e| e.record.direction == PaymentDirection::Send)
        {
            total += 1;
            if total > MAX_LEGACY_ADDRESSES {
                return invalid();
            }
            let code = PaymentCode::from_bytes(&exposure.peer)
                .map_err(|_| WalletBackupError::InvalidInput)?
                .to_base58();
            let public = PublicAddress::imported(&exposure.record.public_key)?;
            let row = make_row(
                &public,
                exposure.account,
                i64::from(exposure.record.index),
                coin,
                Some(Text(Zeroizing::new(code.clone()))),
                false,
            )?;
            senders
                .get_mut(&code)
                .ok_or(WalletBackupError::InvalidInput)?
                .0
                .push(row);
        }
    }
    let doc = Document {
        version: 1,
        phrase: Text(Zeroizing::new(mnemonic.phrase().expose().to_owned())),
        coin_type: coin.number() as u16,
        addresses: json::List(rows),
        sender_payment_code_info: if coin == CoinType::Qi {
            Some(json::Channels(senders))
        } else {
            None
        },
    };
    json::encode(&doc)
}
