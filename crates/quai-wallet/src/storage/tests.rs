use super::*;
use crate::{HdWallet, Search};
use quai_primitives::Zone;
use std::{
    path::PathBuf,
    sync::{Arc, Barrier, OnceLock},
    thread,
};

struct Database(PathBuf);
impl Database {
    fn new() -> Self {
        let mut suffix = [0; 16];
        getrandom::fill(&mut suffix).unwrap();
        let name: String = suffix.iter().map(|v| format!("{v:02x}")).collect();
        Self(std::env::temp_dir().join(format!("quai-public-storage-{name}.sqlite")))
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
fn id(n: u8) -> ReservationId {
    ReservationId([n; 16])
}
fn block(n: u8) -> Checkpoint {
    Checkpoint {
        hash: Hash32::from_bytes([n; 32]),
        height: U256::from(n),
    }
}
fn metadata() -> &'static [PublicAddress; 2] {
    static METADATA: OnceLock<[PublicAddress; 2]> = OnceLock::new();
    METADATA.get_or_init(|| {
        [CoinType::Qi, CoinType::Quai].map(|coin| {
            // Entirely public deterministic test entropy; never use this wallet for funds.
            let wallet = HdWallet::from_seed(&[0; 16], coin).unwrap();
            let account = wallet.account_public(0).unwrap();
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
fn coins() -> Vec<CandidateCoin> {
    (1..=2)
        .map(|index| {
            let mut hash = [0; 32];
            hash[3] = 0x80;
            hash[31] = index;
            CandidateCoin {
                outpoint: OutPoint {
                    transaction_hash: Hash32::from_bytes(hash),
                    index: 0,
                },
                address: QiAddress::try_from(metadata()[0].address()).unwrap(),
                denomination: Denomination::new(2).unwrap(),
                unlock_height: U256::ZERO,
                expires_at: None,
                reserved: false,
            }
        })
        .collect()
}
fn populate(store: &mut SqliteStore) -> u64 {
    let generation = store.import_metadata(0, metadata()).unwrap();
    store
        .replace_snapshot(&Snapshot {
            scope: scope(),
            generation,
            checkpoint: Some(block(5)),
            coins: coins(),
        })
        .unwrap()
}

#[test]
fn restart_states_and_metadata_invalidation_preserve_ambiguous_claims() {
    let db = Database::new();
    let mut store = db.open();
    let generation = populate(&mut store);
    let outpoint = coins()[0].outpoint;
    store
        .reserve_qi(id(1), generation, U256::from(6), &[outpoint])
        .unwrap();
    let transaction = Hash32::from_bytes([7; 32]);
    store.mark_signed(id(1), transaction).unwrap();
    assert_eq!(store.release_unsigned(id(1)), Err(StorageError::Transition));
    drop(store);
    let mut store = db.open();
    assert_eq!(
        store.reservation(id(1)).unwrap().unwrap().state,
        ReservationState::Signed
    );
    assert!(store.snapshot().unwrap().coins[0].reserved);
    store.mark_submitted(id(1)).unwrap();
    store
        .observe_inclusion(id(1), transaction, block(6))
        .unwrap();
    let next = store.import_metadata(generation, metadata()).unwrap();
    let empty = store.snapshot().unwrap();
    assert_eq!(empty.generation, next);
    assert!(empty.checkpoint.is_none());
    assert!(empty.coins.is_empty());
    assert_eq!(store.release_unsigned(id(1)), Err(StorageError::Transition));
    let next = store
        .replace_snapshot(&Snapshot {
            scope: scope(),
            generation: next,
            checkpoint: Some(block(4)),
            coins: coins(),
        })
        .unwrap();
    assert!(store.snapshot().unwrap().coins[0].reserved);
    assert_eq!(
        store.reserve_qi(id(2), next, U256::from(5), &[outpoint]),
        Err(StorageError::Conflict)
    );
    assert!(store.reservation(id(2)).unwrap().is_none());
    assert_eq!(
        store.reservation(id(1)).unwrap().unwrap().inclusion,
        Some(block(6))
    );
}

#[test]
fn batch_failure_rolls_back_claims_and_snapshot_replacement() {
    let db = Database::new();
    let mut store = db.open();
    let generation = populate(&mut store);
    let mut missing = coins()[1].outpoint;
    missing.index = 100;
    assert_eq!(
        store.reserve_qi(
            id(1),
            generation,
            U256::from(6),
            &[coins()[0].outpoint, missing]
        ),
        Err(StorageError::Invalid)
    );
    assert!(store.reservation(id(1)).unwrap().is_none());
    assert!(!store.snapshot().unwrap().coins[0].reserved);
    let duplicate = vec![coins()[0].clone(), coins()[0].clone()];
    assert_eq!(
        store.replace_snapshot(&Snapshot {
            scope: scope(),
            generation,
            checkpoint: Some(block(6)),
            coins: duplicate
        }),
        Err(StorageError::Invalid)
    );
    let snapshot = store.snapshot().unwrap();
    assert_eq!(snapshot.generation, generation);
    assert_eq!(snapshot.checkpoint, Some(block(5)));
    assert_eq!(snapshot.coins.len(), 2);
    store
        .reserve_qi(id(1), generation, U256::from(6), &[coins()[0].outpoint])
        .unwrap();
    store.release_unsigned(id(1)).unwrap();
    assert_eq!(
        store.reservation(id(1)).unwrap().unwrap().state,
        ReservationState::Released
    );
    assert_eq!(
        store.reserve_qi(id(1), generation, U256::from(6), &[coins()[0].outpoint]),
        Err(StorageError::Conflict)
    );
    store
        .reserve_qi(id(2), generation, U256::from(6), &[coins()[0].outpoint])
        .unwrap();
}

#[test]
fn stale_and_cross_network_snapshots_and_claims_are_isolated() {
    let db = Database::new();
    let mut first = db.open();
    let generation = populate(&mut first);
    let base = first.snapshot().unwrap();
    assert_eq!(
        first.import_metadata(0, metadata()),
        Err(StorageError::StaleSnapshot)
    );
    for changed in [
        NetworkScope {
            chain_id: U256::from(15001),
            ..scope()
        },
        NetworkScope {
            genesis: Hash32::from_bytes([2; 32]),
            ..scope()
        },
        NetworkScope {
            zone: Zone::Cyprus2,
            ..scope()
        },
    ] {
        let mut second = SqliteStore::open(&db.0, changed).unwrap();
        assert!(second.addresses().unwrap().is_empty());
        assert_eq!(second.snapshot().unwrap().generation, 0);
        assert_eq!(
            second.replace_snapshot(&base),
            Err(StorageError::StaleSnapshot)
        );
        assert!(second.reservation(id(1)).unwrap().is_none());
    }
    let mut stale = base.clone();
    stale.checkpoint = Some(block(4));
    assert_eq!(
        first.replace_snapshot(&stale),
        Err(StorageError::StaleSnapshot)
    );
    stale.checkpoint = Some(Checkpoint {
        hash: Hash32::from_bytes([99; 32]),
        height: U256::from(5),
    });
    assert_eq!(
        first.replace_snapshot(&stale),
        Err(StorageError::StaleSnapshot)
    );
    let nonce = QuaiAddress::try_from(metadata()[1].address()).unwrap();
    first.reserve_nonce(id(1), nonce, 42).unwrap();
    let mut other = SqliteStore::open(
        &db.0,
        NetworkScope {
            genesis: Hash32::from_bytes([2; 32]),
            ..scope()
        },
    )
    .unwrap();
    other.import_metadata(0, metadata()).unwrap();
    assert_eq!(other.reserve_nonce(id(1), nonce, 0).unwrap(), 0);
    assert_eq!(first.snapshot().unwrap().generation, generation);
}

#[test]
fn nonce_restart_release_and_overflow_are_monotonic() {
    let db = Database::new();
    let mut store = db.open();
    populate(&mut store);
    let address = QuaiAddress::try_from(metadata()[1].address()).unwrap();
    assert_eq!(store.reserve_nonce(id(1), address, 5).unwrap(), 5);
    store.release_unsigned(id(1)).unwrap();
    drop(store);
    let mut store = db.open();
    assert_eq!(store.reserve_nonce(id(2), address, 1).unwrap(), 6);
    assert_eq!(store.reserve_nonce(id(3), address, 100).unwrap(), 100);
    assert_eq!(
        store.reserve_nonce(id(3), address, 200),
        Err(StorageError::Conflict)
    );
    assert_eq!(store.reserve_nonce(id(4), address, 0).unwrap(), 101);
    assert_eq!(
        store.reserve_nonce(id(5), address, u64::MAX),
        Err(StorageError::Overflow)
    );
    assert!(store.reservation(id(5)).unwrap().is_none());
    assert_eq!(
        store.reserve_nonce(id(5), address, u64::MAX - 1).unwrap(),
        u64::MAX - 1
    );
    assert_eq!(
        store.reserve_nonce(id(6), address, 0),
        Err(StorageError::Overflow)
    );
}

#[test]
fn concurrent_connections_claim_qi_once_and_allocate_unique_nonces() {
    let db = Database::new();
    let mut store = db.open();
    let generation = populate(&mut store);
    let barrier = Arc::new(Barrier::new(8));
    let handles: Vec<_> = (1..=8)
        .map(|n| {
            let path = db.0.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                let mut store = SqliteStore::open(path, scope()).unwrap();
                barrier.wait();
                let claim =
                    store.reserve_qi(id(n), generation, U256::from(6), &[coins()[0].outpoint]);
                let nonce = store
                    .reserve_nonce(
                        id(n + 20),
                        QuaiAddress::try_from(metadata()[1].address()).unwrap(),
                        0,
                    )
                    .unwrap();
                (claim, nonce)
            })
        })
        .collect();
    let results: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|r| r.0.is_ok()).count(), 1);
    assert!(
        results
            .iter()
            .all(|r| matches!(r.0, Ok(()) | Err(StorageError::Conflict)))
    );
    assert_eq!(
        results.iter().map(|r| r.1).collect::<BTreeSet<_>>(),
        (0..8).collect()
    );
}

#[test]
fn foreign_databases_are_unchanged_and_schema_has_no_secret_payload() {
    let db = Database::new();
    let foreign = Connection::open(&db.0).unwrap();
    foreign.execute_batch("CREATE TABLE foreign_data(value TEXT); INSERT INTO foreign_data VALUES('preserve'); PRAGMA user_version=77;").unwrap();
    drop(foreign);
    let before = std::fs::read(&db.0).unwrap();
    assert!(matches!(
        SqliteStore::open(&db.0, scope()),
        Err(StorageError::Schema)
    ));
    assert_eq!(std::fs::read(&db.0).unwrap(), before);
    let db = Database::new();
    let store = db.open();
    let mut stmt = store
        .connection
        .prepare("SELECT name FROM pragma_table_info(?1)")
        .unwrap();
    let tables = [
        (
            "scopes",
            vec!["scope", "generation", "block_hash", "height"],
        ),
        (
            "addresses",
            vec!["scope", "address", "public_key", "origin"],
        ),
        (
            "coins",
            vec![
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
            "reservations",
            vec![
                "scope",
                "id",
                "kind",
                "state",
                "transaction_hash",
                "block_hash",
                "block_height",
            ],
        ),
        (
            "qi_claims",
            vec!["scope", "tx_hash", "output_index", "operation", "address"],
        ),
        (
            "signed_payloads",
            vec!["scope", "operation", "kind", "payload"],
        ),
        ("nonce_cursors", vec!["scope", "address", "next_nonce"]),
        (
            "nonce_claims",
            vec!["scope", "address", "nonce", "operation"],
        ),
    ];
    for (table, expected) in tables {
        let actual: Vec<String> = stmt
            .query_map([table], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(actual, expected);
    }
}

#[test]
fn immutable_origins_and_limits_fail_closed() {
    let db = Database::new();
    let mut store = db.open();
    let generation = populate(&mut store);
    let imported =
        PublicAddress::imported(&PublicKey::from_sec1_bytes(metadata()[0].public_key()).unwrap())
            .unwrap();
    assert_eq!(
        store.import_metadata(generation, &[imported]),
        Err(StorageError::Conflict)
    );
    assert_eq!(store.addresses().unwrap().len(), 2);
    assert_eq!(store.snapshot().unwrap().generation, generation);
    assert_eq!(
        store.reserve_qi(id(1), generation, U256::from(4), &[coins()[0].outpoint]),
        Err(StorageError::StaleSnapshot)
    );
    assert_eq!(store.mark_submitted(id(1)), Err(StorageError::Transition));
    store
        .connection
        .execute(
            "UPDATE scopes SET generation=?2 WHERE scope=?1",
            params![&scope().key()[..], i64::MAX],
        )
        .unwrap();
    assert_eq!(
        store.invalidate_snapshot(i64::MAX as u64),
        Err(StorageError::Overflow)
    );
}

#[test]
fn process_reservation_child() {
    let Ok(path) = std::env::var("QUAI_PUBLIC_STORAGE_CHILD") else {
        return;
    };
    let number: u8 = std::env::var("QUAI_PUBLIC_STORAGE_ID")
        .unwrap()
        .parse()
        .unwrap();
    let mut store = SqliteStore::open(path, scope()).unwrap();
    if std::env::var("QUAI_PUBLIC_STORAGE_ALLOCATE").is_ok() {
        let account = HdWallet::from_seed(&[0; 16], CoinType::Qi)
            .unwrap()
            .account_public(0)
            .unwrap();
        store
            .allocate_address(&account, false, 10000, || false)
            .unwrap();
        return;
    }
    let address = QuaiAddress::try_from(metadata()[1].address()).unwrap();
    store.reserve_nonce(id(number), address, 0).unwrap();
    let generation = store.snapshot().unwrap().generation;
    assert!(matches!(
        store.reserve_qi(
            id(number + 10),
            generation,
            U256::from(6),
            &[coins()[0].outpoint]
        ),
        Ok(()) | Err(StorageError::Conflict)
    ));
}

#[test]
fn independent_processes_allocate_durably() {
    let db = Database::new();
    let mut store = db.open();
    populate(&mut store);
    let executable = std::env::current_exe().unwrap();
    let children: Vec<_> = (1..=3)
        .map(|number| {
            std::process::Command::new(&executable)
                .args([
                    "--exact",
                    "storage::tests::process_reservation_child",
                    "--nocapture",
                ])
                .env("QUAI_PUBLIC_STORAGE_CHILD", &db.0)
                .env("QUAI_PUBLIC_STORAGE_ID", number.to_string())
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect();
    for mut child in children {
        assert!(child.wait().unwrap().success());
    }
    let next = store
        .reserve_nonce(
            id(4),
            QuaiAddress::try_from(metadata()[1].address()).unwrap(),
            0,
        )
        .unwrap();
    assert_eq!(next, 3);
    for number in 1..=3 {
        assert!(store.reservation(id(number)).unwrap().is_some());
        assert!(store.reserved_nonce(id(number)).unwrap().is_some());
    }
    assert_eq!(
        (11..=13)
            .filter(|n| store.reservation(id(*n)).unwrap().is_some())
            .count(),
        1
    );
    let all = store.reservations(None, 100).unwrap();
    assert_eq!(all.len(), 5);
    assert_eq!(store.reservations(Some(all[0].id), 100).unwrap(), all[1..]);
    let winner = all.iter().find(|r| r.id.0[0] > 10).unwrap();
    assert_eq!(
        store.reserved_outpoints(winner.id).unwrap(),
        [coins()[0].outpoint]
    );
}

fn signing_key(which: usize) -> quai_crypto::SecretKey {
    let KeyOrigin::Bip44 {
        coin,
        account,
        change,
        index,
    } = metadata()[which].origin()
    else {
        panic!("HD test metadata")
    };
    HdWallet::from_seed(&[0; 16], coin)
        .unwrap()
        .derive_key(account, change, index)
        .unwrap()
        .secret_key()
        .unwrap()
}
fn account_transaction(nonce: u64) -> quai_consensus::QuaiTransaction {
    quai_consensus::QuaiTransaction {
        chain_id: scope().chain_id,
        nonce,
        to: Some(metadata()[1].address()),
        value: U256::from(1),
        gas_limit: 21000,
        gas_price: U256::from(1),
        data: vec![],
        access_list: vec![],
    }
}
#[test]
fn signed_account_payload_commit_validates_claim_and_recovers_after_restart() {
    let db = Database::new();
    let mut store = db.open();
    populate(&mut store);
    let address = QuaiAddress::try_from(metadata()[1].address()).unwrap();
    let nonce = store.reserve_nonce(id(1), address, 5).unwrap();
    let key = signing_key(1);
    let signed = account_transaction(nonce).sign(&key).unwrap();
    let wrong = account_transaction(nonce + 1).sign(&key).unwrap();
    assert_eq!(
        store.commit_signed_quai(id(1), &wrong),
        Err(StorageError::Conflict)
    );
    let mut wrong_chain = account_transaction(nonce);
    wrong_chain.chain_id = U256::from(1);
    assert_eq!(
        store.commit_signed_quai(id(1), &wrong_chain.sign(&key).unwrap()),
        Err(StorageError::Invalid)
    );
    assert_eq!(
        store.reservation(id(1)).unwrap().unwrap().state,
        ReservationState::Reserved
    );
    assert!(store.signed_payload(id(1)).unwrap().is_none());
    store.connection.execute_batch("CREATE TRIGGER simulate_commit_failure BEFORE UPDATE ON reservations BEGIN SELECT RAISE(ABORT,'test failure'); END;").unwrap();
    assert_eq!(
        store.commit_signed_quai(id(1), &signed),
        Err(StorageError::Database)
    );
    assert!(store.signed_payload(id(1)).unwrap().is_none());
    assert_eq!(
        store.reservation(id(1)).unwrap().unwrap().state,
        ReservationState::Reserved
    );
    store
        .connection
        .execute_batch("DROP TRIGGER simulate_commit_failure;")
        .unwrap();
    store.commit_signed_quai(id(1), &signed).unwrap();
    store.commit_signed_quai(id(1), &signed).unwrap();
    assert_eq!(store.release_unsigned(id(1)), Err(StorageError::Transition));
    drop(store);
    let mut store = db.open();
    assert_eq!(
        store.signed_payload(id(1)).unwrap().unwrap(),
        signed.signed_bytes().unwrap()
    );
    store.mark_submitted(id(1)).unwrap();
    store.invalidate_snapshot(2).unwrap();
    assert_eq!(
        store.signed_payload(id(1)).unwrap().unwrap(),
        signed.signed_bytes().unwrap()
    );
    let mut changed = account_transaction(nonce);
    changed.value = U256::from(2);
    assert_eq!(
        store.commit_signed_quai(id(1), &changed.sign(&key).unwrap()),
        Err(StorageError::Transition)
    );
    store
        .connection
        .execute(
            "UPDATE signed_payloads SET payload=x'00' WHERE scope=?1 AND operation=?2",
            params![&scope().key()[..], &id(1).0[..]],
        )
        .unwrap();
    assert_eq!(store.signed_payload(id(1)), Err(StorageError::Invalid));
}
#[test]
fn signed_qi_requires_exact_inputs_and_persists_across_metadata_import() {
    let db = Database::new();
    let mut store = db.open();
    let generation = populate(&mut store);
    let key = signing_key(0);
    let mut recipient = *metadata()[0].address().bytes();
    recipient[19] ^= 1;
    let recipient = Address::from_bytes(recipient);
    let make = |selected: &[CandidateCoin]| {
        quai_consensus::QiTransaction {
            chain_id: scope().chain_id,
            inputs: selected
                .iter()
                .map(|coin| quai_consensus::QiInput {
                    previous_output: coin.outpoint,
                    public_key: key.public_key(),
                })
                .collect(),
            outputs: vec![quai_consensus::QiOutput {
                address: recipient,
                denomination: Denomination::new(1).unwrap(),
            }],
            data: vec![],
        }
        .sign_local(&vec![&key; selected.len()])
        .unwrap()
    };
    store
        .reserve_qi(
            id(1),
            generation,
            U256::from(6),
            &coins().iter().map(|c| c.outpoint).collect::<Vec<_>>(),
        )
        .unwrap();
    assert_eq!(
        store.commit_signed_qi(id(1), &make(&coins()[..1])),
        Err(StorageError::Conflict)
    );
    assert_eq!(
        store.reservation(id(1)).unwrap().unwrap().state,
        ReservationState::Reserved
    );
    let signed = make(&coins());
    store.commit_signed_qi(id(1), &signed).unwrap();
    store.import_metadata(generation, metadata()).unwrap();
    drop(store);
    let mut store = db.open();
    assert_eq!(
        store.signed_payload(id(1)).unwrap().unwrap(),
        signed.signed_bytes().unwrap()
    );
    assert_eq!(store.release_unsigned(id(1)), Err(StorageError::Transition));
}

#[test]
fn allocation_burns_skipped_cancelled_and_exposed_ranges_across_restart() {
    let db = Database::new();
    let mut store = db.open();
    let account = HdWallet::from_seed(&[0; 16], CoinType::Qi)
        .unwrap()
        .account_public(0)
        .unwrap();
    let mut calls = 0;
    assert!(matches!(
        store.allocate_address(&account, false, 10000, || {
            calls += 1;
            calls > 1
        }),
        Err(StorageError::Cancelled)
    ));
    assert!(store.addresses().unwrap().is_empty());
    drop(store);
    let mut store = db.open();
    let first = store
        .allocate_address(&account, false, 10000, || false)
        .unwrap();
    assert_eq!(first.burned.start, 10000);
    assert_eq!(first.burned.end, 20000);
    let KeyOrigin::Bip44 { index, .. } = first.address.origin() else {
        panic!("HD allocation")
    };
    assert!((10000..20000).contains(&index));
    let second = store
        .allocate_address(&account, false, 10000, || false)
        .unwrap();
    assert_eq!(second.burned.start, 20000);
    assert_ne!(first.address.address(), second.address.address());
    let change = store
        .allocate_address(&account, true, 10000, || false)
        .unwrap();
    assert_eq!(change.burned.start, 0);
    let different = HdWallet::from_seed(&[1; 16], CoinType::Qi)
        .unwrap()
        .account_public(0)
        .unwrap();
    assert!(matches!(
        store.allocate_address(&different, false, 10000, || false),
        Err(StorageError::Conflict)
    ));
    assert!(matches!(
        store.allocate_address(&different, true, 10000, || false),
        Err(StorageError::Conflict)
    ));
    assert!(store.snapshot().unwrap().checkpoint.is_none());
    assert_eq!(store.addresses().unwrap().len(), 3);
}
#[test]
fn allocation_competes_across_processes_without_index_collisions() {
    let db = Database::new();
    let store = db.open();
    drop(store);
    let executable = std::env::current_exe().unwrap();
    let children: Vec<_> = (1..=3)
        .map(|number| {
            std::process::Command::new(&executable)
                .args([
                    "--exact",
                    "storage::tests::process_reservation_child",
                    "--nocapture",
                ])
                .env("QUAI_PUBLIC_STORAGE_CHILD", &db.0)
                .env("QUAI_PUBLIC_STORAGE_ID", number.to_string())
                .env("QUAI_PUBLIC_STORAGE_ALLOCATE", "1")
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect();
    for mut child in children {
        assert!(child.wait().unwrap().success());
    }
    let store = db.open();
    let addresses = store.addresses().unwrap();
    assert_eq!(addresses.len(), 3);
    let ranges: BTreeSet<_> = addresses
        .iter()
        .map(|address| match address.origin() {
            KeyOrigin::Bip44 { index, .. } => index / 10000,
            _ => panic!("HD allocation"),
        })
        .collect();
    assert_eq!(ranges, [0, 1, 2].into_iter().collect());
}
fn ready_scan<F: std::future::Future>(future: F) -> F::Output {
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    let mut future = std::pin::pin!(future);
    match future.as_mut().poll(&mut context) {
        std::task::Poll::Ready(value) => value,
        _ => panic!("immediate fixture source"),
    }
}
struct StorageSource;
impl crate::discovery::ObservationSource for StorageSource {
    fn history_capability(
        &self,
        _: NetworkScope,
        _: CoinType,
    ) -> crate::discovery::HistoryCapability {
        crate::discovery::HistoryCapability::CurrentStateOnly
    }
    async fn tip(
        &self,
        scope: NetworkScope,
    ) -> std::result::Result<crate::discovery::ScopedCheckpoint, crate::discovery::DiscoveryError>
    {
        Ok(crate::discovery::ScopedCheckpoint {
            scope,
            checkpoint: block(5),
        })
    }
    async fn observe(
        &self,
        scope: NetworkScope,
        address: &crate::DerivedAddress,
        checkpoint: Checkpoint,
    ) -> std::result::Result<crate::discovery::AddressObservation, crate::discovery::DiscoveryError>
    {
        Ok(crate::discovery::AddressObservation {
            scope,
            checkpoint,
            address: address.address,
            ever_used: None,
            account_balance: (address.coin == CoinType::Quai).then_some(U256::ZERO),
            account_nonce: (address.coin == CoinType::Quai).then_some(0),
            coins: if address.coin == CoinType::Qi {
                vec![coins()[0].clone()]
            } else {
                vec![]
            },
        })
    }
    async fn canonical(
        &self,
        scope: NetworkScope,
        _: U256,
    ) -> std::result::Result<
        Option<crate::discovery::ScopedCheckpoint>,
        crate::discovery::DiscoveryError,
    > {
        Ok(Some(crate::discovery::ScopedCheckpoint {
            scope,
            checkpoint: block(5),
        }))
    }
}
#[test]
fn discovery_commit_couples_metadata_checkpoint_and_cas_and_reorg_preserves_claims() {
    use crate::discovery::{DiscoveryRequest, IndexRange, discover};
    let db = Database::new();
    let mut store = db.open();
    let reports: Vec<_> = metadata()
        .iter()
        .map(|address| {
            let KeyOrigin::Bip44 { coin, index, .. } = address.origin() else {
                panic!("HD test metadata")
            };
            let account = HdWallet::from_seed(&[0; 16], coin)
                .unwrap()
                .account_public(0)
                .unwrap();
            let request = DiscoveryRequest {
                scope: scope(),
                receive: IndexRange {
                    start: 0,
                    end: index + 1,
                },
                change: IndexRange { start: 0, end: 0 },
                gap_limit: None,
                require_history: false,
                max_addresses: 10,
                max_coins: 10,
            };
            ready_scan(discover(&StorageSource, &account, &request, || false)).unwrap()
        })
        .collect();
    let generation = store.commit_discovery(0, &reports).unwrap();
    assert_eq!(generation, 1);
    assert_eq!(store.addresses().unwrap().len(), 2);
    assert_eq!(store.snapshot().unwrap().checkpoint, Some(block(5)));
    assert_eq!(
        store.commit_discovery(0, &reports),
        Err(StorageError::StaleSnapshot)
    );
    assert_eq!(
        store.commit_discovery(generation, &reports[..1]),
        Err(StorageError::StaleSnapshot)
    );
    assert_eq!(store.snapshot().unwrap().generation, generation);
    store
        .reserve_qi(id(1), generation, U256::from(6), &[coins()[0].outpoint])
        .unwrap();
    store
        .mark_signed(id(1), Hash32::from_bytes([7; 32]))
        .unwrap();
    assert_eq!(
        store
            .reconcile_checkpoint(generation, Some(block(5)))
            .unwrap(),
        CheckpointReconciliation::Matches
    );
    assert_eq!(
        store
            .reconcile_checkpoint(
                generation,
                Some(Checkpoint {
                    hash: Hash32::from_bytes([99; 32]),
                    height: U256::from(5)
                })
            )
            .unwrap(),
        CheckpointReconciliation::Invalidated { generation: 2 }
    );
    assert!(store.snapshot().unwrap().checkpoint.is_none());
    assert_eq!(
        store.reserved_outpoints(id(1)).unwrap(),
        [coins()[0].outpoint]
    );
    assert_eq!(store.release_unsigned(id(1)), Err(StorageError::Transition));
    assert_eq!(
        store.commit_discovery(generation, &reports),
        Err(StorageError::StaleSnapshot)
    );
}

#[test]
fn allocation_skips_imported_metadata_and_invalidates_checkpoint_without_releasing() {
    let db = Database::new();
    let mut store = db.open();
    let generation = populate(&mut store);
    let account = HdWallet::from_seed(&[0; 16], CoinType::Qi)
        .unwrap()
        .account_public(0)
        .unwrap();
    store
        .reserve_qi(id(1), generation, U256::from(6), &[coins()[0].outpoint])
        .unwrap();
    store
        .mark_signed(id(1), Hash32::from_bytes([7; 32]))
        .unwrap();
    let KeyOrigin::Bip44 { index, .. } = metadata()[0].origin() else {
        panic!("HD test metadata")
    };
    let allocated = store
        .allocate_address(&account, false, 10000, || false)
        .unwrap();
    assert_eq!(allocated.burned.start, index + 1);
    assert_eq!(allocated.generation, generation + 1);
    assert_eq!(
        store.next_derivation_index(&account, false).unwrap(),
        Some(allocated.burned.end)
    );
    assert!(store.snapshot().unwrap().checkpoint.is_none());
    assert!(store.snapshot().unwrap().coins.is_empty());
    assert_eq!(
        store.reserved_outpoints(id(1)).unwrap(),
        [coins()[0].outpoint]
    );
    assert_eq!(store.release_unsigned(id(1)), Err(StorageError::Transition));
}

#[test]
fn fee_replacement_graph_is_atomic_immutable_bounded_and_restartable() {
    let db = Database::new();
    let mut store = db.open();
    populate(&mut store);
    let owner = QuaiAddress::try_from(metadata()[1].address()).unwrap();
    let nonce = store.reserve_nonce(id(90), owner, 5).unwrap();
    let key = signing_key(1);
    let root = account_transaction(nonce).sign(&key).unwrap();
    store.commit_signed_quai(id(90), &root).unwrap();
    let parent = root.hash().unwrap();
    for field in 0..5 {
        let mut wrong = root.transaction().clone();
        wrong.gas_price = U256::from(2);
        match field {
            0 => wrong.nonce += 1,
            1 => wrong.value += U256::from(1),
            2 => wrong.gas_limit += 1,
            3 => wrong.data.push(1),
            _ => wrong.chain_id = U256::from(9),
        };
        assert!(
            store
                .commit_quai_replacement(id(90), parent, &wrong.sign(&key).unwrap())
                .is_err()
        );
    }
    let mut tx = root.transaction().clone();
    tx.gas_price = U256::from(2);
    let first = tx.sign(&key).unwrap();
    store.connection.execute_batch("CREATE TRIGGER fail_variant BEFORE INSERT ON quai_replacements BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
    assert!(
        store
            .commit_quai_replacement(id(90), parent, &first)
            .is_err()
    );
    assert!(store.quai_replacements(id(90)).unwrap().is_empty());
    store
        .connection
        .execute_batch("DROP TRIGGER fail_variant")
        .unwrap();
    store
        .commit_quai_replacement(id(90), parent, &first)
        .unwrap();
    store
        .commit_quai_replacement(id(90), parent, &first)
        .unwrap();
    assert_eq!(store.quai_replacements(id(90)).unwrap().len(), 1);
    tx.gas_price = U256::from(3);
    let second = tx.sign(&key).unwrap();
    assert!(
        store
            .commit_quai_replacement(id(90), Hash32::ZERO, &second)
            .is_err()
    );
    store
        .commit_quai_replacement(id(90), first.hash().unwrap(), &second)
        .unwrap();
    drop(store);
    let mut store = db.open();
    assert_eq!(store.quai_replacements(id(90)).unwrap().len(), 2);
    assert_eq!(
        store.signed_payload(id(90)).unwrap().unwrap(),
        root.signed_bytes().unwrap()
    );
    assert!(store.release_unsigned(id(90)).is_err());
    assert_eq!(store.reserved_nonce(id(90)).unwrap(), Some((owner, nonce)));
    let mut parent = second.hash().unwrap();
    for price in 4..=33 {
        tx.gas_price = U256::from(price);
        let next = tx.sign(&key).unwrap();
        store
            .commit_quai_replacement(id(90), parent, &next)
            .unwrap();
        parent = next.hash().unwrap();
    }
    tx.gas_price = U256::from(34);
    assert!(
        store
            .commit_quai_replacement(id(90), parent, &tx.sign(&key).unwrap())
            .is_err()
    );
    assert_eq!(store.quai_replacements(id(90)).unwrap().len(), 32);
}

#[test]
fn observation_cache_cas_slots_restart_and_tombstones_preserve_signed_claims() {
    let db = Database::new();
    let mut store = db.open();
    populate(&mut store);
    let owner = QuaiAddress::try_from(metadata()[1].address()).unwrap();
    let nonce = store.reserve_nonce(id(98), owner, 5).unwrap();
    let root = account_transaction(nonce).sign(&signing_key(1)).unwrap();
    let hash = root.hash().unwrap();
    store.commit_signed_quai(id(98), &root).unwrap();
    assert!(store.observation_cache(id(98), hash, 0).unwrap().is_none());
    assert!(
        store
            .compare_exchange_observation(id(98), Hash32::ZERO, 0, None, Some(b"public"))
            .is_err()
    );
    assert_eq!(
        store
            .compare_exchange_observation(id(98), hash, 0, None, Some(b"public"))
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .compare_exchange_observation(id(98), hash, 1, None, Some(b"second output"))
            .unwrap(),
        1
    );
    let mut other = db.open();
    assert_eq!(
        other
            .observation_cache(id(98), hash, 0)
            .unwrap()
            .unwrap()
            .payload
            .unwrap(),
        b"public"
    );
    assert_eq!(
        other
            .compare_exchange_observation(id(98), hash, 0, Some(1), None)
            .unwrap(),
        2
    );
    assert_eq!(
        store.compare_exchange_observation(id(98), hash, 0, Some(1), Some(b"stale")),
        Err(StorageError::Conflict)
    );
    assert_eq!(
        store.compare_exchange_observation(id(98), hash, 0, None, Some(b"ABA")),
        Err(StorageError::Conflict)
    );
    assert!(
        store
            .observation_cache(id(98), hash, 0)
            .unwrap()
            .unwrap()
            .payload
            .is_none()
    );
    assert!(
        store
            .observation_cache(id(98), hash, 1)
            .unwrap()
            .unwrap()
            .payload
            .is_some()
    );
    assert_eq!(store.reserved_nonce(id(98)).unwrap(), Some((owner, nonce)));
    assert_eq!(
        store.signed_payload(id(98)).unwrap().unwrap(),
        root.signed_bytes().unwrap()
    );
}

#[test]
fn schema_three_migrates_observation_cache_without_changing_signed_state() {
    let db = Database::new();
    let mut store = db.open();
    populate(&mut store);
    let owner = QuaiAddress::try_from(metadata()[1].address()).unwrap();
    let nonce = store.reserve_nonce(id(99), owner, 5).unwrap();
    let signed = account_transaction(nonce).sign(&signing_key(1)).unwrap();
    store.commit_signed_quai(id(99), &signed).unwrap();
    store
        .connection
        .execute_batch("DROP TABLE observation_cache; PRAGMA user_version=3;")
        .unwrap();
    drop(store);
    let mut reopened = db.open();
    assert_eq!(
        reopened.signed_payload(id(99)).unwrap().unwrap(),
        signed.signed_bytes().unwrap()
    );
    assert!(
        reopened
            .observation_cache(id(99), signed.hash().unwrap(), 0)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        reopened.reserved_nonce(id(99)).unwrap(),
        Some((owner, nonce))
    );
}
