use super::*;
use crate::storage::Snapshot;
use crate::{HdWallet, Language, Mnemonic, Search};
use quai_consensus::{Denomination, QuaiTransaction};
use std::{path::PathBuf, sync::OnceLock};

struct Database(PathBuf);
impl Database {
    fn new() -> Self {
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).unwrap();
        let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        Self(std::env::temp_dir().join(format!("quai-public-full-backup-{suffix}.sqlite")))
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
fn block() -> Checkpoint {
    Checkpoint {
        hash: Hash32::from_bytes([5; 32]),
        height: U256::from(5),
    }
}
fn fixture_accounts() -> &'static [PublicAddress; 2] {
    static METADATA: OnceLock<[PublicAddress; 2]> = OnceLock::new();
    METADATA.get_or_init(|| {
        [CoinType::Qi, CoinType::Quai].map(|coin| {
            let account = HdWallet::from_seed(&[0; 16], coin)
                .unwrap()
                .account_public(0)
                .unwrap();
            let found = account
                .search(
                    false,
                    Search {
                        zone: scope().zone,
                        start_index: 0,
                        max_attempts: 10000,
                    },
                    || false,
                )
                .unwrap();
            PublicAddress::derive(&account, false, found.address.index).unwrap()
        })
    })
}
fn key_for(address: &PublicAddress) -> SecretKey {
    let KeyOrigin::Bip44 {
        coin,
        account,
        change,
        index,
    } = address.origin()
    else {
        panic!("public HD fixture")
    };
    HdWallet::from_seed(&[0; 16], coin)
        .unwrap()
        .derive_key(account, change, index)
        .unwrap()
        .secret_key()
        .unwrap()
}
fn id(value: u8) -> ReservationId {
    ReservationId([value; 16])
}
fn populate(store: &mut SqliteStore) -> u64 {
    let generation = store.import_metadata(0, fixture_accounts()).unwrap();
    let mut hash = [0; 32];
    hash[3] = 0x80;
    let coin = crate::CandidateCoin {
        outpoint: OutPoint {
            transaction_hash: Hash32::from_bytes(hash),
            index: 0,
        },
        address: QiAddress::try_from(fixture_accounts()[0].address()).unwrap(),
        denomination: Denomination::new(2).unwrap(),
        unlock_height: U256::ZERO,
        expires_at: None,
        reserved: false,
    };
    store
        .replace_snapshot(&Snapshot {
            scope: scope(),
            generation,
            checkpoint: Some(block()),
            coins: vec![coin],
        })
        .unwrap()
}
fn seed_origin() -> BackupOrigin {
    BackupOrigin::from_seed(&[0; 16]).unwrap()
}

#[test]
fn effective_seed_identity_and_xprv_and_generated_imported_keys_roundtrip() {
    for language in [Language::English, Language::Japanese, Language::Portuguese] {
        let mnemonic = Mnemonic::from_entropy(language, &[0; 16]).unwrap();
        let effective = mnemonic.to_seed("pássphrase 日本語");
        let db = Database::new();
        let mut store = db.open();
        let account = HdWallet::from_seed(effective.expose(), CoinType::Quai)
            .unwrap()
            .account_public(7)
            .unwrap();
        store
            .allocate_address(&account, false, 10000, || false)
            .unwrap();
        let backup = WalletBackup::capture(
            &mut store,
            vec![BackupOrigin::from_mnemonic(&mnemonic, "pássphrase 日本語").unwrap()],
        )
        .unwrap();
        let encoded = backup.encode().unwrap();
        let restored = WalletBackup::decode(&encoded).unwrap();
        // Compare public derivations; never emit the effective secret seed on assertion failure.
        assert_eq!(
            restored.origins()[0]
                .account_public(CoinType::Quai, 7)
                .unwrap()
                .export(),
            account.export()
        );
        assert!(restored.origins()[0].expose_seed().unwrap() == effective.expose());
    }
    let db = Database::new();
    let mut store = db.open();
    populate(&mut store);
    let master = ExtendedPrivateKey::from_seed(&[0; 16])
        .unwrap()
        .export()
        .unwrap();
    let backup = WalletBackup::capture(
        &mut store,
        vec![BackupOrigin::from_master_xprv(master.expose()).unwrap()],
    )
    .unwrap();
    let restored = WalletBackup::decode(&backup.encode().unwrap()).unwrap();
    assert_eq!(restored.origins()[0].kind(), BackupOriginKind::MasterXprv);
    assert!(restored.origins()[0].export_master_xprv().unwrap().expose() == master.expose());
    assert_eq!(
        restored.origins()[0]
            .derive_key(
                CoinType::Quai,
                0,
                false,
                match fixture_accounts()[1].origin() {
                    KeyOrigin::Bip44 { index, .. } => index,
                    _ => 0,
                }
            )
            .unwrap()
            .public_key(),
        key_for(&fixture_accounts()[1]).public_key()
    );
    let db = Database::new();
    let mut store = db.open();
    // Generate a fresh key and use its actual zone, never funding it.
    let key = (0..4096)
        .find_map(|_| {
            let candidate = SecretKey::generate().unwrap();
            candidate
                .public_key()
                .address()
                .zone()
                .is_ok()
                .then_some(candidate)
        })
        .expect("bounded public test key generation");
    let own_scope = NetworkScope {
        zone: key.public_key().address().zone().unwrap(),
        ..scope()
    };
    let mut own = SqliteStore::open(&db.0, own_scope).unwrap();
    let address = PublicAddress::imported(&key.public_key()).unwrap();
    own.import_metadata(0, &[address]).unwrap();
    let backup =
        WalletBackup::capture(&mut store, vec![BackupOrigin::from_private_key(&key)]).unwrap();
    let restored = WalletBackup::decode(&backup.encode().unwrap()).unwrap();
    assert_eq!(
        restored.origins()[0].imported_key().unwrap().public_key(),
        key.public_key()
    );
    assert_eq!(format!("{backup:?}"), "WalletBackup([REDACTED])");
    assert_eq!(
        format!("{:?}", backup.origins()[0]),
        "BackupOrigin([REDACTED])"
    );
}

#[test]
fn restored_ranges_and_nonces_are_monotonic_and_signed_claims_and_bytes_survive() {
    let source = Database::new();
    let mut store = source.open();
    let generation = populate(&mut store);
    let outpoint = store.snapshot().unwrap().coins[0].outpoint;
    store
        .reserve_qi(id(1), generation, U256::from(6), &[outpoint])
        .unwrap();
    store
        .mark_signed(id(1), Hash32::from_bytes([7; 32]))
        .unwrap();
    let address = QuaiAddress::try_from(fixture_accounts()[1].address()).unwrap();
    let nonce = store.reserve_nonce(id(2), address, 10).unwrap();
    let signed = QuaiTransaction {
        chain_id: scope().chain_id,
        nonce,
        to: Some(address.address()),
        value: U256::from(1),
        gas_limit: 21000,
        gas_price: U256::from(1),
        data: vec![],
        access_list: vec![],
    }
    .sign(&key_for(&fixture_accounts()[1]))
    .unwrap();
    store.commit_signed_quai(id(2), &signed).unwrap();
    store.mark_submitted(id(2)).unwrap();
    let account = HdWallet::from_seed(&[0; 16], CoinType::Qi)
        .unwrap()
        .account_public(0)
        .unwrap();
    let allocated = store
        .allocate_address(&account, false, 10000, || false)
        .unwrap();
    let backup = WalletBackup::capture(&mut store, vec![seed_origin()]).unwrap();
    let decoded = WalletBackup::decode(&backup.encode().unwrap()).unwrap();
    let target = Database::new();
    let mut restored = target.open();
    let report = decoded.restore(&mut restored).unwrap();
    assert_eq!(report.retained_signed_operations, 2);
    assert_eq!(report.hash_only_operations, 1);
    assert!(report.reconciliation_required && report.rescan_required);
    assert!(restored.snapshot().unwrap().checkpoint.is_none());
    assert!(restored.snapshot().unwrap().coins.is_empty());
    assert_eq!(
        restored.next_derivation_index(&account, false).unwrap(),
        Some(allocated.burned.end)
    );
    assert_eq!(restored.reserved_outpoints(id(1)).unwrap(), [outpoint]);
    assert_eq!(
        restored.release_unsigned(id(1)),
        Err(StorageError::Transition)
    );
    assert_eq!(
        restored.signed_payload(id(2)).unwrap().unwrap(),
        signed.signed_bytes().unwrap()
    );
    assert_eq!(restored.reserve_nonce(id(3), address, 0).unwrap(), 11);
    let next = restored
        .allocate_address(&account, false, 10000, || false)
        .unwrap();
    assert_eq!(next.burned.start, allocated.burned.end);
    decoded.restore(&mut restored).unwrap();
    assert_eq!(
        restored.next_derivation_index(&account, false).unwrap(),
        Some(next.burned.end)
    );
    assert_eq!(restored.reserve_nonce(id(4), address, 0).unwrap(), 12);
}

#[test]
fn ownership_extensions_corruption_and_conflict_restore_fail_without_partial_changes() {
    let db = Database::new();
    let mut store = db.open();
    let generation = populate(&mut store);
    assert!(matches!(
        WalletBackup::capture(&mut store, vec![BackupOrigin::from_seed(&[1; 16]).unwrap()]),
        Err(WalletBackupError::Ownership)
    ));
    assert_eq!(store.snapshot().unwrap().generation, generation);
    let master = ExtendedPrivateKey::from_seed(&[0; 16]).unwrap();
    let child = master.derive_child(44, true).unwrap().export().unwrap();
    assert!(matches!(
        BackupOrigin::from_master_xprv(child.expose()),
        Err(WalletBackupError::Unsupported)
    ));
    let backup = WalletBackup::capture(&mut store, vec![seed_origin()]).unwrap();
    let mut encoded = backup.encode().unwrap();
    encoded[3] = 1;
    assert!(matches!(
        WalletBackup::decode(&encoded),
        Err(WalletBackupError::Unsupported)
    ));
    for length in [0, 1, 4, 8, 30] {
        assert!(WalletBackup::decode(&encoded[..length.min(encoded.len())]).is_err());
    }
    let connection = rusqlite::Connection::open(&db.0).unwrap();
    connection
        .execute("CREATE TABLE future_channels(channel BLOB)", [])
        .unwrap();
    assert!(matches!(
        WalletBackup::capture(&mut store, vec![seed_origin()]),
        Err(WalletBackupError::Storage(StorageError::Schema))
    ));
    connection
        .execute("DROP TABLE future_channels", [])
        .unwrap();
    let address = QuaiAddress::try_from(fixture_accounts()[1].address()).unwrap();
    store.reserve_nonce(id(1), address, 5).unwrap();
    let account = HdWallet::from_seed(&[0; 16], CoinType::Qi)
        .unwrap()
        .account_public(0)
        .unwrap();
    store
        .allocate_address(&account, true, 10000, || false)
        .unwrap();
    let old = WalletBackup::capture(&mut store, vec![seed_origin()]).unwrap();
    let target = Database::new();
    let mut target = target.open();
    populate(&mut target);
    target.reserve_nonce(id(1), address, 5).unwrap();
    target
        .mark_signed(id(1), Hash32::from_bytes([9; 32]))
        .unwrap();
    let before = target.snapshot().unwrap().generation;
    assert!(matches!(
        old.restore(&mut target),
        Err(WalletBackupError::Storage(StorageError::Conflict))
    ));
    assert_eq!(target.snapshot().unwrap().generation, before);
    assert_eq!(target.addresses().unwrap().len(), 2);
    assert_eq!(target.next_derivation_index(&account, true).unwrap(), None);
    assert_eq!(
        target.reservation(id(1)).unwrap().unwrap().state,
        ReservationState::Signed
    );
}

#[test]
fn malformed_cursor_payload_and_unknown_ownership_are_rejected_before_restore() {
    let db = Database::new();
    let mut store = db.open();
    populate(&mut store);
    let account = HdWallet::from_seed(&[0; 16], CoinType::Qi)
        .unwrap()
        .account_public(0)
        .unwrap();
    store
        .allocate_address(&account, false, 10000, || false)
        .unwrap();
    let mut backup = WalletBackup::capture(&mut store, vec![seed_origin()]).unwrap();
    backup.state.scopes[0].derivation[0].next_index = 0;
    let target = Database::new();
    let mut restored = target.open();
    assert!(matches!(
        backup.restore(&mut restored),
        Err(WalletBackupError::InvalidInput)
    ));
    assert!(restored.addresses().unwrap().is_empty());
    assert_eq!(restored.snapshot().unwrap().generation, 0);
    let mut writer = Writer::new().unwrap();
    writer.put(&vec![0u8; MAX_PLAINTEXT]).unwrap();
    assert!(writer.put(&[1]).is_err());
}

fn vector() -> serde_json::Value {
    serde_json::from_str(include_str!("../../tests/full-backup-vector.json")).unwrap()
}
fn unhex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
fn fixture() -> EncryptedWalletBackup {
    EncryptedWalletBackup::from_bytes(&unhex(vector()["envelope"].as_str().unwrap())).unwrap()
}
const PASSWORD: &[u8] = b"public-full-wallet-vector-password";
#[test]
fn independent_full_envelope_vector_matches_and_authenticates() {
    let vector = vector();
    let seed = Zeroizing::new(unhex(vector["seed"].as_str().unwrap()));
    let db = Database::new();
    let mut store = db.open();
    let backup =
        WalletBackup::capture(&mut store, vec![BackupOrigin::from_seed(&seed).unwrap()]).unwrap();
    let encoded = backup.encode().unwrap();
    assert!(encoded.as_slice() == unhex(vector["plaintext"].as_str().unwrap()));
    let salt = fixed(&unhex(vector["salt"].as_str().unwrap())).unwrap();
    let nonce = fixed(&unhex(vector["nonce"].as_str().unwrap())).unwrap();
    let encrypted = backup
        .encrypt_with_randomness(PASSWORD, BackupKdf::default(), salt, nonce)
        .unwrap();
    assert_eq!(encrypted.as_bytes(), fixture().as_bytes());
    let restored = fixture().decrypt(PASSWORD).unwrap();
    assert!(restored.origins()[0].expose_seed().unwrap() == seed.as_slice());
    assert_eq!(restored.scopes(), [scope()]);
}
#[test]
fn password_tampering_and_authenticated_unsupported_channels_share_unlock_error() {
    assert_eq!(
        fixture().decrypt(b"incorrect public password").unwrap_err(),
        WalletBackupError::UnlockFailed
    );
    let mut corrupted = fixture().0;
    corrupted[HEADER] ^= 1;
    assert_eq!(
        EncryptedWalletBackup::from_bytes(&corrupted)
            .unwrap()
            .decrypt(PASSWORD)
            .unwrap_err(),
        WalletBackupError::UnlockFailed
    );
    // Build an authenticated unsupported extension using only the public test key.
    let vector = vector();
    let key = Zeroizing::new(unhex(vector["key"].as_str().unwrap()));
    let cipher = XChaCha20Poly1305::new_from_slice(&key).unwrap();
    let mut envelope = fixture().0;
    let nonce = XNonce::from(fixed::<24>(&envelope[40..64]).unwrap());
    let mut plaintext = Zeroizing::new(unhex(vector["plaintext"].as_str().unwrap()));
    plaintext[3] = 1;
    let tag = cipher
        .encrypt_inout_detached(&nonce, &envelope[..HEADER], (&mut plaintext[..]).into())
        .unwrap();
    let end = HEADER + plaintext.len();
    envelope[HEADER..end].copy_from_slice(&plaintext);
    envelope[end..].copy_from_slice(&tag);
    assert_eq!(
        EncryptedWalletBackup::from_bytes(&envelope)
            .unwrap()
            .decrypt(PASSWORD)
            .unwrap_err(),
        WalletBackupError::UnlockFailed
    );
}
#[test]
fn fresh_salts_nonces_and_header_bounds_are_enforced() {
    let db = Database::new();
    let mut store = db.open();
    let backup = WalletBackup::capture(&mut store, vec![seed_origin()]).unwrap();
    let first = backup.encrypt(PASSWORD, BackupKdf::default()).unwrap();
    let second = backup.encrypt(PASSWORD, BackupKdf::default()).unwrap();
    assert_ne!(&first.as_bytes()[24..40], &second.as_bytes()[24..40]);
    assert_ne!(&first.as_bytes()[40..64], &second.as_bytes()[40..64]);
    for offset in [0, 8, 9, 10, 11, 12, 64] {
        let mut bytes = fixture().0;
        bytes[offset] ^= 0x80;
        assert_eq!(
            EncryptedWalletBackup::from_bytes(&bytes).unwrap_err(),
            WalletBackupError::UnlockFailed
        );
    }
    assert_eq!(
        EncryptedWalletBackup::from_bytes(&vec![0; HEADER + MAX_PLAINTEXT + 17]).unwrap_err(),
        WalletBackupError::UnlockFailed
    );
    assert!(EncryptedWalletBackup::from_bytes(&fixture().as_bytes()[..HEADER]).is_err());
}

#[test]
fn authenticated_registered_channels_restore_ownership_and_never_rewind() {
    use quai_payments::{PaymentChannel, PaymentDirection, PrivatePaymentCode};
    let db = Database::new();
    let mut store = db.open();
    let owner = PrivatePaymentCode::from_seed(&[0; 16], 0).unwrap();
    let peer = PrivatePaymentCode::from_seed(&[1; 16], 0).unwrap();
    store
        .import_payment_channel(
            &owner,
            &PaymentChannel::new(&owner, peer.public_code().clone()),
            None,
        )
        .unwrap();
    let first = store
        .allocate_payment_address(
            &owner,
            peer.public_code(),
            PaymentDirection::Receive,
            10000,
            || false,
        )
        .unwrap();
    let mut backup =
        WalletBackup::capture(&mut store, vec![BackupOrigin::from_seed(&[0; 16]).unwrap()])
            .unwrap();
    assert_eq!(backup.version(), 2);
    let encoded = backup.encode().unwrap();
    assert!(WalletBackup::decode(&encoded).is_err());
    let restored = WalletBackup::decode_version(&encoded, 2).unwrap();
    let target = Database::new();
    let target_path = target.0.clone();
    let mut target = target.open();
    restored.restore(&mut target).unwrap();
    assert_eq!(
        target
            .payment_addresses(&owner, peer.public_code(), PaymentDirection::Receive)
            .unwrap()[0]
            .address,
        first.found.address
    );
    assert_eq!(target.addresses().unwrap().len(), 1);
    let second = target
        .allocate_payment_address(
            &owner,
            peer.public_code(),
            PaymentDirection::Receive,
            10000,
            || false,
        )
        .unwrap();
    restored.restore(&mut target).unwrap();
    assert_eq!(
        target
            .payment_channel(&owner, peer.public_code())
            .unwrap()
            .unwrap()
            .channel
            .next_index(PaymentDirection::Receive, scope().zone),
        Some(20000)
    );
    assert_eq!(
        target
            .payment_addresses(&owner, peer.public_code(), PaymentDirection::Receive)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(second.burned.start, 10000);
    // A conflicting exposure rolls back earlier metadata/channel generation writes.
    let before = target.snapshot().unwrap().generation;
    let channel_before = target
        .payment_channel(&owner, peer.public_code())
        .unwrap()
        .unwrap()
        .generation;
    let connection = rusqlite::Connection::open(&target_path).unwrap();
    connection
        .execute(
            "UPDATE payment_exposures SET range_end=range_end-1 WHERE child_index=?1",
            [first.found.index],
        )
        .unwrap();
    assert!(restored.restore(&mut target).is_err());
    assert_eq!(target.snapshot().unwrap().generation, before);
    assert_eq!(
        target
            .payment_channel(&owner, peer.public_code())
            .unwrap()
            .unwrap()
            .generation,
        channel_before
    );
    connection
        .execute(
            "UPDATE payment_exposures SET range_end=range_end+1 WHERE child_index=?1",
            [first.found.index],
        )
        .unwrap();
    // A legacy v1 import retains all target channel state and newer exposures.
    let empty = Database::new();
    let legacy = WalletBackup::capture(
        &mut empty.open(),
        vec![BackupOrigin::from_seed(&[0; 16]).unwrap()],
    )
    .unwrap();
    assert_eq!(legacy.version(), 1);
    legacy.restore(&mut target).unwrap();
    assert_eq!(
        target
            .payment_channel(&owner, peer.public_code())
            .unwrap()
            .unwrap()
            .channel
            .next_index(PaymentDirection::Receive, scope().zone),
        Some(20000)
    );
    // Authentication does not replace semantic ownership checks.
    backup.state.exposures[0].record.index += 1;
    assert!(backup.validate().is_err());
    backup.state.exposures[0].record.index -= 1;
    backup.state.channels[0].metadata[0] ^= 1;
    assert!(backup.validate().is_err());
    backup.state.channels[0].metadata[0] ^= 1;
    backup.origins = vec![BackupOrigin::from_seed(&[2; 16]).unwrap()];
    assert!(backup.validate().is_err());
    assert!(
        WalletBackup::capture(
            &mut store,
            vec![
                BackupOrigin::from_master_xprv(
                    BackupOrigin::from_seed(&[0; 16])
                        .unwrap()
                        .export_master_xprv()
                        .unwrap()
                        .expose()
                )
                .unwrap()
            ]
        )
        .is_ok()
    );
}

#[test]
fn independent_v2_channel_vector_authenticates_and_matches_legacy_key_derivation() {
    use quai_payments::{PaymentChannel, PaymentCode, PaymentDirection};
    let vector: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/full-backup-v2-vector.json")).unwrap();
    let seed = Zeroizing::new(unhex(vector["seed"].as_str().unwrap()));
    let origin = BackupOrigin::from_seed(&seed).unwrap();
    let owner = origin.payment_code(0).unwrap();
    let peer = PaymentCode::from_base58(vector["peerCode"].as_str().unwrap()).unwrap();
    let db = Database::new();
    let mut store = db.open();
    store
        .import_payment_channel(&owner, &PaymentChannel::new(&owner, peer.clone()), None)
        .unwrap();
    let mut channel = store
        .payment_channel(&owner, &peer)
        .unwrap()
        .unwrap()
        .channel;
    channel
        .advance_cursor(&owner, PaymentDirection::Send, scope().zone, Some(10000))
        .unwrap();
    store
        .import_payment_channel(&owner, &channel, Some(0))
        .unwrap();
    let backup = WalletBackup::capture(&mut store, vec![origin]).unwrap();
    assert!(backup.encode().unwrap().as_slice() == unhex(vector["plaintext"].as_str().unwrap()));
    let encrypted = backup
        .encrypt_with_randomness(
            PASSWORD,
            BackupKdf::default(),
            fixed(&unhex(vector["salt"].as_str().unwrap())).unwrap(),
            fixed(&unhex(vector["nonce"].as_str().unwrap())).unwrap(),
        )
        .unwrap();
    assert_eq!(
        encrypted.as_bytes(),
        unhex(vector["envelope"].as_str().unwrap())
    );
    let restored = encrypted.decrypt(PASSWORD).unwrap();
    let target = Database::new();
    let mut target = target.open();
    restored.restore(&mut target).unwrap();
    assert_eq!(
        target
            .payment_channel(&owner, &peer)
            .unwrap()
            .unwrap()
            .channel
            .next_index(PaymentDirection::Send, scope().zone),
        Some(10000)
    );
    assert_eq!(
        encrypted.decrypt(b"wrong public password").unwrap_err(),
        WalletBackupError::UnlockFailed
    );
    let mut downgraded = encrypted.0;
    downgraded[8] = 1;
    assert_eq!(
        EncryptedWalletBackup::from_bytes(&downgraded)
            .unwrap()
            .decrypt(PASSWORD)
            .unwrap_err(),
        WalletBackupError::UnlockFailed
    );
}

#[test]
fn signed_specialized_qi_backup_restores_exact_payload_and_claims() {
    use quai_consensus::{
        ConversionSlippage, QiConversionIntent, QiConversionTransaction, QiInput, QiWrappingIntent,
        QiWrappingTransaction, SignedQiOperation,
    };
    for wrapping in [false, true] {
        let db = Database::new();
        let mut store = db.open();
        populate(&mut store);
        let snapshot = store.snapshot().unwrap();
        let coin = &snapshot.coins[0];
        store
            .reserve_qi(id(80), snapshot.generation, U256::from(6), &[coin.outpoint])
            .unwrap();
        let key = key_for(&fixture_accounts()[0]);
        let inputs = vec![QiInput {
            previous_output: coin.outpoint,
            public_key: key.public_key(),
        }];
        let destination = fixture_accounts()[1].address().try_into().unwrap();
        let signed = if wrapping {
            SignedQiOperation::Wrapping(
                QiWrappingTransaction::new(
                    scope().chain_id,
                    inputs,
                    vec![Denomination::new(1).unwrap()],
                    vec![],
                    QiWrappingIntent {
                        destination,
                        owner_contract: "0x002b2596EcF05C93a31ff916E8b456DF6C77c750"
                            .parse()
                            .unwrap(),
                    },
                )
                .unwrap()
                .sign_single(&key)
                .unwrap(),
            )
        } else {
            SignedQiOperation::Conversion(
                QiConversionTransaction::new(
                    scope().chain_id,
                    inputs,
                    vec![Denomination::new(1).unwrap()],
                    vec![],
                    QiConversionIntent {
                        destination,
                        refund: "0x0080000000000000000000000000000000000001"
                            .parse()
                            .unwrap(),
                        slippage: ConversionSlippage::new(100).unwrap(),
                    },
                )
                .unwrap()
                .sign_single(&key)
                .unwrap(),
            )
        };
        store.commit_signed_qi_operation(id(80), &signed).unwrap();
        let backup = WalletBackup::capture(&mut store, vec![seed_origin()]).unwrap();
        let decoded = WalletBackup::decode(&backup.encode().unwrap()).unwrap();
        let target = Database::new();
        let mut restored = target.open();
        decoded.restore(&mut restored).unwrap();
        assert_eq!(
            restored.signed_payload(id(80)).unwrap().unwrap(),
            signed.signed_bytes().unwrap()
        );
        assert_eq!(
            restored.reserved_outpoints(id(80)).unwrap(),
            vec![coin.outpoint]
        );
        assert!(restored.release_unsigned(id(80)).is_err());
    }
}

#[test]
fn imported_payment_account_origin_v3_roundtrips_and_cannot_be_downgraded() {
    use quai_payments::{PaymentChannel, PaymentDirection};
    let account = ExtendedPrivateKey::from_seed(&[4; 32])
        .unwrap()
        .derive_child(47, true)
        .unwrap()
        .derive_child(969, true)
        .unwrap()
        .derive_child(7, true)
        .unwrap();
    let encoded = account.export().unwrap();
    let origin = BackupOrigin::from_payment_account_xprv(encoded.expose(), 7).unwrap();
    assert_eq!(origin.kind(), BackupOriginKind::PaymentAccountXprv);
    assert!(origin.export_master_xprv().is_err());
    assert!(origin.account_public(CoinType::Qi, 7).is_err());
    assert!(BackupOrigin::from_payment_account_xprv(encoded.expose(), 8).is_err());
    let owner = origin.payment_code(7).unwrap();
    let peer = quai_payments::PrivatePaymentCode::from_seed(&[5; 32], 0).unwrap();
    let db = Database::new();
    let mut store = db.open();
    store
        .import_payment_channel(
            &owner,
            &PaymentChannel::new(&owner, peer.public_code().clone()),
            None,
        )
        .unwrap();
    let allocation = store
        .allocate_payment_address(
            &owner,
            peer.public_code(),
            PaymentDirection::Receive,
            10000,
            || false,
        )
        .unwrap();
    let backup = WalletBackup::capture(&mut store, vec![origin]).unwrap();
    assert_eq!(backup.version(), 3);
    let plaintext = backup.encode().unwrap();
    assert!(WalletBackup::decode_version(&plaintext, 2).is_err());
    let restored = WalletBackup::decode_version(&plaintext, 3).unwrap();
    let (index, exported) = restored.origins()[0].export_payment_account_xprv().unwrap();
    assert_eq!(index, 7);
    assert!(exported.expose() == encoded.expose());
    assert_eq!(
        restored.origins()[0].payment_code(7).unwrap().public_code(),
        owner.public_code()
    );
    let target = Database::new();
    let mut target = target.open();
    restored.restore(&mut target).unwrap();
    assert_eq!(
        target
            .payment_addresses(&owner, peer.public_code(), PaymentDirection::Receive)
            .unwrap()[0]
            .address,
        allocation.found.address
    );
    let envelope = backup
        .encrypt(b"public test password", BackupKdf::default())
        .unwrap();
    assert_eq!(envelope.as_bytes()[8], 3);
    assert_eq!(
        envelope.decrypt(b"public test password").unwrap().version(),
        3
    );
    let mut downgraded = envelope.as_bytes().to_vec();
    downgraded[8] = 2;
    assert!(
        EncryptedWalletBackup::from_bytes(&downgraded)
            .unwrap()
            .decrypt(b"public test password")
            .is_err()
    );
}
