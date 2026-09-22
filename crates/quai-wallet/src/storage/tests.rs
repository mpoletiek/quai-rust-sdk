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
                // Keep eight real competing writers. Loaded Windows runners may
                // need more than the production default five-second lock budget.
                let mut store =
                    SqliteStore::open_with_busy_timeout(path, scope(), Duration::from_secs(30))
                        .unwrap();
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
    // The cancelled attempt's burn stays consumed; the successful one gives
    // back everything past its address.
    assert_eq!(first.burned.start, 10000);
    let KeyOrigin::Bip44 { index, .. } = first.address.origin() else {
        panic!("HD allocation")
    };
    assert!((10000..20000).contains(&index));
    assert_eq!(first.burned.end, index + 1);
    let second = store
        .allocate_address(&account, false, 10000, || false)
        .unwrap();
    assert_eq!(second.burned.start, index + 1);
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
    let indexes: BTreeSet<_> = addresses
        .iter()
        .map(|address| match address.origin() {
            KeyOrigin::Bip44 { index, .. } => index,
            _ => panic!("HD allocation"),
        })
        .collect();
    assert_eq!(indexes.len(), 3, "no two processes issued the same index");
}

#[test]
fn consecutive_allocations_do_not_skip_matching_addresses() {
    // A large bound used to burn its whole range, skipping ~bound/512 matching
    // addresses between allocations: at 100,000, about 190, far past a default
    // gap of 50, so a mnemonic-only restore stopped before the second address.
    let db = Database::new();
    let mut store = db.open();
    let account = HdWallet::from_seed(&[0; 16], CoinType::Qi)
        .unwrap()
        .account_public(0)
        .unwrap();
    let index = |allocated: &AllocatedAddress| match allocated.address.origin() {
        KeyOrigin::Bip44 { index, .. } => index,
        _ => panic!("HD allocation"),
    };
    let first = store
        .allocate_address(&account, false, 100_000, || false)
        .unwrap();
    let second = store
        .allocate_address(&account, false, 100_000, || false)
        .unwrap();
    let next = account
        .search(
            false,
            crate::Search {
                zone: scope().zone,
                start_index: index(&first) + 1,
                max_attempts: 100_000,
            },
            || false,
        )
        .unwrap();
    assert_eq!(index(&second), next.address.index, "the very next match");
}
fn ready_scan<F: std::future::Future>(future: F) -> F::Output {
    crate::discovery::tests::ready(future)
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
                grinding: crate::Grinding::Sequential,
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
    // Losing the race is Stale, not Invalid: the loser re-reads and retries.
    let raced = store.compare_exchange_observation(id(98), hash, 0, Some(1), Some(b"stale"));
    assert_eq!(raced, Err(StorageError::ObservationRaced));
    assert_eq!(
        raced.unwrap_err().class(),
        quai_primitives::ErrorClass::Stale
    );
    assert_eq!(
        store.compare_exchange_observation(id(98), hash, 0, None, Some(b"ABA")),
        Err(StorageError::ObservationRaced)
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
        .execute_batch(
            "DROP TABLE released_change; DROP TABLE head_replay; DROP TABLE observation_cache; PRAGMA user_version=3;",
        )
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

#[test]
fn reorg_invalidation_is_atomic_preserves_custody_and_fences_first_observers() {
    let db = Database::new();
    let mut store = db.open();
    let generation = populate(&mut store);
    let owner = QuaiAddress::try_from(metadata()[1].address()).unwrap();
    let mut signed = Vec::new();
    for (operation, inclusion) in [(id(94), block(4)), (id(95), block(6))] {
        let nonce = store.reserve_nonce(operation, owner, 5).unwrap();
        let root = account_transaction(nonce).sign(&signing_key(1)).unwrap();
        store.commit_signed_quai(operation, &root).unwrap();
        store
            .observe_inclusion(operation, root.hash().unwrap(), inclusion)
            .unwrap();
        signed.push(root);
    }
    let outpoint = coins()[0].outpoint;
    store
        .reserve_qi(id(96), generation, U256::from(6), &[outpoint])
        .unwrap();
    store
        .mark_signed(id(96), Hash32::from_bytes([96; 32]))
        .unwrap();
    let hash = signed[1].hash().unwrap();
    store
        .compare_exchange_observation(id(95), hash, 0, None, Some(b"old chain"))
        .unwrap();
    store
        .compare_exchange_observation(id(95), hash, 1, None, None)
        .unwrap();
    let mut other = db.open();
    let update = other
        .invalidate_reorg_from(generation, U256::from(5))
        .unwrap();
    assert_eq!(
        (update.generation, update.inclusions, update.caches),
        (generation + 1, 1, 2)
    );
    assert!(store.snapshot().unwrap().checkpoint.is_none());
    assert!(store.snapshot().unwrap().coins.is_empty());
    assert_eq!(
        store.reservation(id(94)).unwrap().unwrap().state,
        ReservationState::Confirmed
    );
    assert_eq!(
        store.reservation(id(95)).unwrap().unwrap().state,
        ReservationState::Submitted
    );
    assert_eq!(
        store.reservation(id(96)).unwrap().unwrap().state,
        ReservationState::Signed
    );
    for slot in [0, 1] {
        let cache = store
            .observation_cache(id(95), hash, slot)
            .unwrap()
            .unwrap();
        assert_eq!(cache.revision, 2);
        assert!(cache.payload.is_none());
    }
    // A never-written slot has no revision tombstone. Scope generation must
    // reject an observer which started before rollback nevertheless.
    assert_eq!(
        store.observe_inclusion_scoped(generation, id(95), hash, block(6)),
        Err(StorageError::StaleSnapshot)
    );
    assert_eq!(
        store.compare_exchange_observation_scoped(
            id(95),
            hash,
            2,
            (generation, None),
            Some(b"stale")
        ),
        Err(StorageError::StaleSnapshot)
    );
    assert_eq!(
        store.compare_exchange_family_observation_scoped(
            id(95),
            &[hash],
            (generation, None),
            Some(b"stale family")
        ),
        Err(StorageError::StaleSnapshot)
    );
    assert_eq!(
        store.invalidate_reorg_from(generation, U256::from(5)),
        Err(StorageError::StaleSnapshot)
    );
    assert_eq!(
        store.invalidate_reorg_from(update.generation, U256::ZERO),
        Err(StorageError::Invalid)
    );
    drop(store);
    let mut store = db.open();
    for (operation, root) in [id(94), id(95)].into_iter().zip(signed) {
        assert_eq!(
            store.signed_payload(operation).unwrap().unwrap(),
            root.signed_bytes().unwrap()
        );
        assert!(store.reserved_nonce(operation).unwrap().is_some());
        assert_eq!(
            store.release_unsigned(operation),
            Err(StorageError::Transition)
        );
    }
    store
        .replace_snapshot(&Snapshot {
            scope: scope(),
            generation: update.generation,
            checkpoint: Some(block(7)),
            coins: coins(),
        })
        .unwrap();
    assert!(store.snapshot().unwrap().coins[0].reserved);
    assert_eq!(
        store.release_unsigned(id(96)),
        Err(StorageError::Transition)
    );
}

#[test]
fn reorg_invalidation_rolls_back_on_write_fault_or_revision_exhaustion() {
    let db = Database::new();
    let mut store = db.open();
    let generation = populate(&mut store);
    let owner = QuaiAddress::try_from(metadata()[1].address()).unwrap();
    let nonce = store.reserve_nonce(id(97), owner, 5).unwrap();
    let root = account_transaction(nonce).sign(&signing_key(1)).unwrap();
    let hash = root.hash().unwrap();
    store.commit_signed_quai(id(97), &root).unwrap();
    store.observe_inclusion(id(97), hash, block(6)).unwrap();
    store
        .compare_exchange_observation(id(97), hash, 0, None, Some(b"old"))
        .unwrap();
    store.connection.execute_batch("CREATE TRIGGER injected_reorg_fault BEFORE UPDATE ON observation_cache BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert_eq!(
        store.invalidate_reorg_from(generation, U256::from(5)),
        Err(StorageError::Database)
    );
    assert_eq!(
        store.reservation(id(97)).unwrap().unwrap().inclusion,
        Some(block(6))
    );
    assert_eq!(store.snapshot().unwrap().generation, generation);
    assert_eq!(store.snapshot().unwrap().coins.len(), 2);
    store.connection.execute_batch("DROP TRIGGER injected_reorg_fault; UPDATE observation_cache SET revision=9223372036854775807;").unwrap();
    assert_eq!(
        store.invalidate_reorg_from(generation, U256::from(5)),
        Err(StorageError::Overflow)
    );
    assert_eq!(
        store.reservation(id(97)).unwrap().unwrap().inclusion,
        Some(block(6))
    );
    assert_eq!(store.snapshot().unwrap().checkpoint, Some(block(5)));
}

#[test]
fn head_replay_commits_cursor_with_rollback_and_fences_stale_writers() {
    let db = Database::new();
    let mut store = db.open();
    let generation = populate(&mut store);
    let owner = QuaiAddress::try_from(metadata()[1].address()).unwrap();
    let nonce = store.reserve_nonce(id(101), owner, 5).unwrap();
    let signed = account_transaction(nonce).sign(&signing_key(1)).unwrap();
    store.commit_signed_quai(id(101), &signed).unwrap();
    store
        .observe_inclusion(id(101), signed.hash().unwrap(), block(6))
        .unwrap();
    let first = store
        .commit_head_replay(generation, None, Some(b"public cursor 1"), None)
        .unwrap();
    assert_eq!(first.revision, 1);
    assert!(first.invalidation.is_none());
    drop(store);
    let mut store = db.open();
    let before = store.head_replay_state().unwrap().unwrap();
    assert_eq!(
        before.payload.as_deref(),
        Some(b"public cursor 1".as_slice())
    );
    // A failure at the cursor write follows the attempted wallet rollback and
    // must still undo the entire transaction, including coins and inclusions.
    store.connection.execute_batch("CREATE TEMP TRIGGER fail_cursor BEFORE UPDATE ON head_replay BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
    assert!(
        store
            .commit_head_replay(
                generation,
                Some(1),
                Some(b"public cursor 2"),
                Some(U256::from(5))
            )
            .is_err()
    );
    assert_eq!(store.observation_generation().unwrap(), generation);
    assert_eq!(
        store.reservation(id(101)).unwrap().unwrap().state,
        ReservationState::Confirmed
    );
    assert_eq!(store.head_replay_state().unwrap().unwrap(), before);
    assert!(!store.snapshot().unwrap().coins.is_empty());
    store
        .connection
        .execute_batch("DROP TRIGGER fail_cursor;")
        .unwrap();
    let committed = store
        .commit_head_replay(
            generation,
            Some(1),
            Some(b"public cursor 2"),
            Some(U256::from(5)),
        )
        .unwrap();
    assert_eq!(committed.revision, 2);
    assert_eq!(committed.invalidation.unwrap().inclusions, 1);
    assert_eq!(
        store.reservation(id(101)).unwrap().unwrap().state,
        ReservationState::Submitted
    );
    assert!(store.snapshot().unwrap().coins.is_empty());
    assert_eq!(
        store.signed_payload(id(101)).unwrap().unwrap(),
        signed.signed_bytes().unwrap()
    );
    assert_eq!(store.reserved_nonce(id(101)).unwrap(), Some((owner, nonce)));
    let mut other = db.open();
    for (gen_value, revision) in [
        (generation, Some(2)),
        (generation + 1, Some(1)),
        (generation + 1, None),
    ] {
        assert_eq!(
            other
                .commit_head_replay(gen_value, revision, Some(b"stale"), None)
                .unwrap_err(),
            StorageError::StaleSnapshot
        );
    }
    other
        .commit_head_replay(generation + 1, Some(2), None, None)
        .unwrap();
    assert_eq!(
        store.head_replay_state().unwrap().unwrap(),
        HeadReplayState {
            revision: 3,
            payload: None
        }
    );
    assert!(
        store
            .commit_head_replay(generation + 1, Some(2), Some(b"late"), None)
            .is_err()
    );
    let mut foreign = SqliteStore::open(
        &db.0,
        NetworkScope {
            chain_id: U256::from(10),
            ..scope()
        },
    )
    .unwrap();
    assert!(foreign.head_replay_state().unwrap().is_none());
    foreign
        .commit_head_replay(0, None, Some(b"separate network"), None)
        .unwrap();
    assert!(
        store
            .head_replay_state()
            .unwrap()
            .unwrap()
            .payload
            .is_none()
    );
}

#[test]
fn head_replay_migrates_v4_and_rejects_bounds_overflow_and_failed_resets() {
    let db = Database::new();
    let mut store = db.open();
    let generation = populate(&mut store);
    store
        .connection
        .execute_batch("DROP TABLE released_change; DROP TABLE head_replay; PRAGMA user_version=4;")
        .unwrap();
    drop(store);
    let mut store = db.open();
    assert!(store.head_replay_state().unwrap().is_none());
    assert_eq!(store.observation_generation().unwrap(), generation);
    for bytes in [vec![], vec![0; MAX_HEAD_REPLAY_BYTES + 1]] {
        assert_eq!(
            store
                .commit_head_replay(generation, None, Some(&bytes), None)
                .unwrap_err(),
            StorageError::Invalid
        );
    }
    assert!(
        store
            .commit_head_replay(generation, None, Some(b"cursor"), Some(U256::ZERO))
            .is_err()
    );
    assert!(store.head_replay_state().unwrap().is_none());
    store
        .commit_head_replay(
            generation,
            None,
            Some(&vec![1; MAX_HEAD_REPLAY_BYTES]),
            None,
        )
        .unwrap();
    store
        .connection
        .execute("UPDATE head_replay SET revision=9223372036854775807", [])
        .unwrap();
    assert_eq!(
        store
            .commit_head_replay(generation, Some(i64::MAX as u64), None, Some(U256::from(1)))
            .unwrap_err(),
        StorageError::Overflow
    );
    assert_eq!(store.observation_generation().unwrap(), generation);
    assert!(head_state::clear(&store.connection, &store.key).is_err());
    assert_eq!(
        store
            .head_replay_state()
            .unwrap()
            .unwrap()
            .payload
            .unwrap()
            .len(),
        MAX_HEAD_REPLAY_BYTES
    );
}

#[test]
fn explicit_busy_budget_is_bounded_and_failed_nonce_claims_leave_no_state() {
    let db = Database::new();
    for timeout in [Duration::from_secs(61), Duration::from_nanos(1)] {
        assert!(matches!(
            SqliteStore::open_with_busy_timeout(&db.0, scope(), timeout),
            Err(StorageError::Invalid)
        ));
        assert!(!db.0.exists());
    }
    let mut store =
        SqliteStore::open_with_busy_timeout(&db.0, scope(), Duration::from_millis(10)).unwrap();
    populate(&mut store);
    let address = QuaiAddress::try_from(metadata()[1].address()).unwrap();
    let mut blocker = Connection::open(&db.0).unwrap();
    let tx = blocker
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    assert_eq!(
        store.reserve_nonce(id(220), address, 0),
        Err(StorageError::Database)
    );
    tx.rollback().unwrap();
    assert!(store.reservation(id(220)).unwrap().is_none());
    assert_eq!(store.reserve_nonce(id(220), address, 0).unwrap(), 0);
    drop(store);
    let mut store = SqliteStore::open_with_busy_timeout(&db.0, scope(), Duration::ZERO).unwrap();
    assert_eq!(store.reserve_nonce(id(221), address, 0).unwrap(), 1);
}

#[test]
fn compact_hd_allocations_commit_only_examined_children_and_preserve_old_floors() {
    let db = Database::new();
    let account = HdWallet::from_seed(&[0; 16], CoinType::Qi)
        .unwrap()
        .account_public(0)
        .unwrap();
    let mut store = db.open();
    let legacy = store
        .allocate_address(&account, false, 10000, || false)
        .unwrap();
    let mut calls = 0;
    assert!(matches!(
        store.allocate_address_compact(&account, false, 10000, || {
            calls += 1;
            calls > 2
        }),
        Err(StorageError::Cancelled)
    ));
    assert_eq!(
        store.next_derivation_index(&account, false).unwrap(),
        Some(legacy.burned.end)
    );
    let first = store
        .allocate_address_compact(&account, false, 10000, || false)
        .unwrap();
    let KeyOrigin::Bip44 { index, .. } = first.address.origin() else {
        panic!("HD origin")
    };
    assert_eq!(first.burned.start, legacy.burned.end);
    assert_eq!(first.burned.end, index + 1);
    drop(store);
    let mut store = db.open();
    let next = store
        .allocate_address_compact(&account, false, 10000, || false)
        .unwrap();
    assert_eq!(next.burned.start, first.burned.end);
    assert_ne!(next.address, first.address);
    assert_eq!(store.addresses().unwrap().len(), 3);
}

/// Two connections to one store, plus a hook that runs `during` while the
/// first connection's legacy allocation is searching outside the write lock.
fn allocate_with_concurrent_write(
    during: impl FnOnce(&mut SqliteStore),
) -> (Database, AllocatedAddress, SqliteStore, AccountPublic) {
    let db = Database::new();
    let mut store = db.open();
    let mut other = Some((db.open(), during));
    let account = HdWallet::from_seed(&[0; 16], CoinType::Qi)
        .unwrap()
        .account_public(0)
        .unwrap();
    // The first check runs before the reservation; the second is the search's
    // first candidate, after the reservation committed and the lock dropped.
    let mut calls = 0;
    let allocated = store
        .allocate_address(&account, false, 100_000, || {
            calls += 1;
            if calls == 2
                && let Some((mut other, during)) = other.take()
            {
                during(&mut other);
            }
            false
        })
        .unwrap();
    (db, allocated, store, account)
}

#[test]
fn giveback_never_rewinds_below_an_address_recorded_meanwhile() {
    // Discovery on another connection records the next matching address,
    // inside this allocation's reserved tail. That raises the cursor with
    // max(), leaving it equal to the reserved limit, so a cursor-only check
    // rewound past it and the next allocation issued the same address.
    let account = HdWallet::from_seed(&[0; 16], CoinType::Qi)
        .unwrap()
        .account_public(0)
        .unwrap();
    let search = |start| {
        account
            .search(
                false,
                crate::Search {
                    zone: scope().zone,
                    start_index: start,
                    max_attempts: 100_000,
                },
                || false,
            )
            .unwrap()
            .address
            .index
    };
    let second = search(search(0) + 1);
    let recorded = PublicAddress::derive(&account, false, second).unwrap();
    let copy = recorded.clone();
    let (_db, first, mut store, account) = allocate_with_concurrent_write(move |other| {
        let generation = other.snapshot().unwrap().generation;
        other.import_metadata(generation, &[copy]).unwrap();
    });
    assert_eq!(
        first.burned.end,
        first.burned.start + 100_000,
        "full burn kept"
    );
    let next = store
        .allocate_address_compact(&account, false, 100_000, || false)
        .unwrap();
    assert_ne!(next.address.address(), recorded.address());
}

#[test]
fn giveback_keeps_the_full_burn_after_a_concurrent_allocation() {
    let (_db, first, mut store, account) = allocate_with_concurrent_write(|other| {
        let account = HdWallet::from_seed(&[0; 16], CoinType::Qi)
            .unwrap()
            .account_public(0)
            .unwrap();
        other
            .allocate_address_compact(&account, false, 100_000, || false)
            .unwrap();
    });
    assert_eq!(
        first.burned.end,
        first.burned.start + 100_000,
        "full burn kept"
    );
    let issued: BTreeSet<_> = store
        .addresses()
        .unwrap()
        .iter()
        .map(|a| a.address())
        .collect();
    assert_eq!(issued.len(), 2);
    let next = store
        .allocate_address_compact(&account, false, 100_000, || false)
        .unwrap();
    assert!(!issued.contains(&next.address.address()));
}

#[test]
fn activity_lists_outgoing_operations_with_status_and_decoded_amounts() {
    use crate::storage::{ActivityDetail, ActivityStatus, QiActivityKind};
    let db = Database::new();
    let mut store = db.open();
    let generation = populate(&mut store);
    // Account: reserved, signed, submitted, then observed included.
    let account = QuaiAddress::try_from(metadata()[1].address()).unwrap();
    let nonce = store.reserve_nonce(id(1), account, 5).unwrap();
    let transfer = account_transaction(nonce).sign(&signing_key(1)).unwrap();
    store.commit_signed_quai(id(1), &transfer).unwrap();
    store.mark_submitted(id(1)).unwrap();
    store
        .observe_inclusion(id(1), transfer.hash().unwrap(), block(9))
        .unwrap();
    // Account: reserved, then released unsigned.
    store.reserve_nonce(id(2), account, 0).unwrap();
    store.release_unsigned(id(2)).unwrap();
    // Qi: one output to someone else, one back to a second address this
    // wallet holds (outputs may not reuse an input's address).
    let qi_account = HdWallet::from_seed(&[0; 16], CoinType::Qi)
        .unwrap()
        .account_public(0)
        .unwrap();
    let KeyOrigin::Bip44 { index: first, .. } = metadata()[0].origin() else {
        panic!("HD test metadata")
    };
    let second = qi_account
        .search(
            false,
            Search {
                zone: scope().zone,
                start_index: first + 1,
                max_attempts: 10_000,
            },
            || false,
        )
        .unwrap();
    let change_address = PublicAddress::derive(&qi_account, false, second.address.index).unwrap();
    let key = signing_key(0);
    let mut stranger = *metadata()[0].address().bytes();
    stranger[19] ^= 1;
    // A watched key the store holds without HD ancestry is not change.
    let other = HdWallet::from_seed(&[1; 16], CoinType::Qi).unwrap();
    let found = other
        .account_public(0)
        .unwrap()
        .search(
            false,
            Search {
                zone: scope().zone,
                start_index: 0,
                max_attempts: 10_000,
            },
            || false,
        )
        .unwrap();
    let watched_key = other
        .derive_key(0, false, found.address.index)
        .unwrap()
        .secret_key()
        .unwrap()
        .public_key();
    let watched = PublicAddress::imported(&watched_key).unwrap();
    let qi = quai_consensus::QiTransaction {
        chain_id: scope().chain_id,
        inputs: coins()
            .iter()
            .map(|coin| quai_consensus::QiInput {
                previous_output: coin.outpoint,
                public_key: key.public_key(),
            })
            .collect(),
        outputs: vec![
            quai_consensus::QiOutput {
                address: Address::from_bytes(stranger),
                denomination: Denomination::new(1).unwrap(),
            },
            quai_consensus::QiOutput {
                address: change_address.address(),
                denomination: Denomination::new(0).unwrap(),
            },
            quai_consensus::QiOutput {
                address: watched.address(),
                denomination: Denomination::new(2).unwrap(),
            },
        ],
        data: vec![],
    }
    .sign_local(&vec![&key; coins().len()])
    .unwrap();
    store
        .reserve_qi(
            id(3),
            generation,
            U256::from(6),
            &coins().iter().map(|c| c.outpoint).collect::<Vec<_>>(),
        )
        .unwrap();
    store.commit_signed_qi(id(3), &qi).unwrap();
    let generation = store.snapshot().unwrap().generation;
    store
        .import_metadata(generation, &[change_address, watched])
        .unwrap();

    let activity = store.activity(None, 10).unwrap();
    assert_eq!(activity.len(), 3);
    assert_eq!(
        activity[0].status,
        ActivityStatus::Included { block: block(9) }
    );
    assert_eq!(activity[0].transaction, Some(transfer.hash().unwrap()));
    assert!(matches!(
        activity[0].detail,
        ActivityDetail::Account { nonce: n, .. } if n == nonce
    ));
    assert_eq!(activity[1].status, ActivityStatus::Cancelled);
    assert_eq!(activity[1].detail, ActivityDetail::Unsigned);
    assert_eq!(activity[2].status, ActivityStatus::Signed);
    assert_eq!(
        activity[2].detail,
        ActivityDetail::Qi {
            kind: QiActivityKind::Transfer,
            sent: U256::from(
                Denomination::new(1).unwrap().value() + Denomination::new(2).unwrap().value()
            ),
            change: U256::from(Denomination::new(0).unwrap().value()),
            outputs: 3,
        }
    );
    // Paging continues after the last returned ID.
    assert_eq!(store.activity(Some(id(2)), 10).unwrap().len(), 1);
}

#[test]
fn unchanged_coins_advance_only_the_checkpoint_and_keep_the_generation() {
    // An idle wallet's refresh used to rewrite every coin row and bump the
    // generation, which failed concurrent reservations and observers.
    let db = Database::new();
    let mut store = db.open();
    let generation = populate(&mut store);
    let mut other = db.open();
    let same = |checkpoint| Snapshot {
        scope: scope(),
        generation,
        checkpoint: Some(checkpoint),
        coins: coins(),
    };
    let written = store.connection.total_changes();
    assert_eq!(store.replace_snapshot(&same(block(5))).unwrap(), generation);
    assert_eq!(
        store.connection.total_changes(),
        written,
        "no-op writes nothing"
    );
    assert_eq!(other.replace_snapshot(&same(block(9))).unwrap(), generation);
    assert_eq!(other.connection.total_changes(), 1, "only the scope row");
    let snapshot = store.snapshot().unwrap();
    assert_eq!(
        (snapshot.generation, snapshot.checkpoint),
        (generation, Some(block(9)))
    );
    assert_eq!(snapshot.coins, coins());
    // A reservation fenced on the generation it read still succeeds.
    store
        .reserve_qi(id(1), generation, U256::from(10), &[coins()[0].outpoint])
        .unwrap();
    // The label still never rewinds, and any coin difference bumps.
    assert_eq!(
        store.replace_snapshot(&same(block(8))),
        Err(StorageError::StaleSnapshot)
    );
    let mut changed = same(block(10));
    changed.coins[1].unlock_height = U256::from(1);
    assert_eq!(store.replace_snapshot(&changed).unwrap(), generation + 1);
    // After invalidation the coins are unknown, so the full write runs.
    let invalidated = store.invalidate_snapshot(generation + 1).unwrap();
    let mut refill = same(block(11));
    refill.generation = invalidated;
    refill.coins.clear();
    assert_eq!(store.replace_snapshot(&refill).unwrap(), invalidated + 1);
}

fn qi_account() -> AccountPublic {
    HdWallet::from_seed(&[0; 16], CoinType::Qi)
        .unwrap()
        .account_public(0)
        .unwrap()
}
/// A populated store plus `count` fresh change addresses, lowest first.
fn change(store: &mut SqliteStore, count: usize) -> Vec<PublicAddress> {
    populate(store);
    (0..count)
        .map(|_| {
            store
                .allocate_address_compact(&qi_account(), true, 10000, || false)
                .unwrap()
                .address
        })
        .collect()
}

#[test]
fn released_change_comes_back_lowest_first_without_invalidating() {
    let db = Database::new();
    let mut store = db.open();
    let account = qi_account();
    let addresses = change(&mut store, 3);
    let cursor = store.next_derivation_index(&account, true).unwrap();
    let generation = store.snapshot().unwrap().generation;
    store
        .release_change(&account, &[addresses[2].clone(), addresses[0].clone()])
        .unwrap();
    // Releasing twice is harmless.
    store.release_change(&account, &addresses[..1]).unwrap();
    assert_eq!(
        store.take_released_change(&account, 1).unwrap(),
        &addresses[..1]
    );
    assert_eq!(
        store.take_released_change(&account, 9).unwrap(),
        &addresses[2..]
    );
    assert!(store.take_released_change(&account, 9).unwrap().is_empty());
    assert_eq!(store.snapshot().unwrap().generation, generation);
    assert_eq!(store.next_derivation_index(&account, true).unwrap(), cursor);
}

#[test]
fn only_unsigned_change_of_the_bound_account_is_released() {
    let db = Database::new();
    let mut store = db.open();
    let account = qi_account();
    let addresses = change(&mut store, 2);
    // A receive address, an address of another account, too many at once.
    assert_eq!(
        store.release_change(&account, &metadata()[..1]),
        Err(StorageError::Invalid)
    );
    let other = HdWallet::from_seed(&[0; 16], CoinType::Qi)
        .unwrap()
        .account_public(1)
        .unwrap();
    assert_eq!(
        store.release_change(&other, &addresses),
        Err(StorageError::Invalid)
    );
    let foreign = HdWallet::from_seed(&[9; 16], CoinType::Qi)
        .unwrap()
        .account_public(0)
        .unwrap();
    assert_eq!(
        store.release_change(&foreign, &addresses),
        Err(StorageError::Conflict)
    );
    assert_eq!(
        store.release_change(&account, &vec![addresses[0].clone(); 1025]),
        Err(StorageError::Invalid)
    );
    // An address in a stored signed payload is never released, and signing
    // burns one that was.
    let generation = store.snapshot().unwrap().generation;
    let mut snapshot = store.snapshot().unwrap();
    snapshot.checkpoint = Some(block(5));
    snapshot.coins = coins();
    let generation = store
        .replace_snapshot(&Snapshot {
            generation,
            ..snapshot
        })
        .unwrap();
    store.release_change(&account, &addresses[1..]).unwrap();
    let key = signing_key(0);
    let pay = |to: &PublicAddress, coin: &CandidateCoin| {
        quai_consensus::QiTransaction {
            chain_id: scope().chain_id,
            inputs: vec![quai_consensus::QiInput {
                previous_output: coin.outpoint,
                public_key: key.public_key(),
            }],
            outputs: vec![quai_consensus::QiOutput {
                address: to.address(),
                denomination: Denomination::new(1).unwrap(),
            }],
            data: vec![],
        }
        .sign_local(&[&key])
        .unwrap()
    };
    for (n, (to, coin)) in addresses.iter().zip(coins()).enumerate() {
        store
            .reserve_qi(id(n as u8 + 1), generation, U256::from(6), &[coin.outpoint])
            .unwrap();
        store
            .commit_signed_qi(id(n as u8 + 1), &pay(to, &coin))
            .unwrap();
    }
    assert_eq!(
        store.release_change(&account, &addresses[..1]),
        Err(StorageError::Transition)
    );
    assert!(store.take_released_change(&account, 9).unwrap().is_empty());
}

#[test]
fn concurrent_takes_never_share_a_released_address() {
    let db = Database::new();
    let mut store = db.open();
    let addresses = change(&mut store, 24);
    store.release_change(&qi_account(), &addresses).unwrap();
    let barrier = Arc::new(Barrier::new(4));
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let (path, barrier) = (db.0.clone(), barrier.clone());
            thread::spawn(move || {
                let mut store = SqliteStore::open(path, scope()).unwrap();
                barrier.wait();
                let mut taken = vec![];
                loop {
                    let next = store.take_released_change(&qi_account(), 2).unwrap();
                    if next.is_empty() {
                        return taken;
                    }
                    taken.extend(next);
                }
            })
        })
        .collect();
    let mut taken: Vec<_> = workers
        .into_iter()
        .flat_map(|worker| worker.join().unwrap())
        .map(|address| address.address())
        .collect();
    taken.sort();
    let mut expected: Vec<_> = addresses.iter().map(|a| a.address()).collect();
    expected.sort();
    assert_eq!(taken, expected);
}

#[test]
fn a_restore_burns_the_released_set() {
    let db = Database::new();
    let mut store = db.open();
    let addresses = change(&mut store, 2);
    store.release_change(&qi_account(), &addresses).unwrap();
    let state = store.capture_public_state().unwrap();
    store.restore_public_state(&state, &[]).unwrap();
    assert!(
        store
            .take_released_change(&qi_account(), 9)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn schema_five_migrates_to_an_empty_released_set() {
    let db = Database::new();
    let mut store = db.open();
    let addresses = change(&mut store, 1);
    store
        .connection
        .execute_batch("DROP TABLE released_change; PRAGMA user_version=5;")
        .unwrap();
    drop(store);
    let mut store = db.open();
    let version: i64 = store
        .connection
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, 6);
    store.release_change(&qi_account(), &addresses).unwrap();
    assert_eq!(
        store.take_released_change(&qi_account(), 1).unwrap(),
        addresses
    );
}

#[cfg(unix)]
#[test]
fn a_new_store_and_its_wal_files_are_owner_only_and_an_existing_file_keeps_its_mode() {
    use std::os::unix::fs::PermissionsExt;
    let mode = |path: &str| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    let database = Database::new();
    let mut store = database.open();
    // A write guarantees the WAL and shared-memory files exist.
    store.import_metadata(0, &metadata()[..1]).unwrap();
    let path = database.0.display().to_string();
    for suffix in ["", "-wal", "-shm"] {
        assert_eq!(mode(&format!("{path}{suffix}")), 0o600, "{suffix}");
    }
    drop(store);

    // Tightening a file the caller created is not this API's decision.
    let existing = Database::new();
    std::fs::File::create(&existing.0).unwrap();
    std::fs::set_permissions(&existing.0, std::fs::Permissions::from_mode(0o640)).unwrap();
    drop(existing.open());
    assert_eq!(mode(&existing.0.display().to_string()), 0o640);
}

#[test]
fn targeted_address_lookup_returns_held_rows_validated_like_the_full_table() {
    let database = Database::new();
    let mut store = database.open();
    store.import_metadata(0, &metadata()[..]).unwrap();
    let [qi, quai] = metadata().clone();
    let absent = Address::from_bytes([0; 20]);
    let found = store
        .public_addresses([qi.address(), absent, qi.address(), quai.address()])
        .unwrap();
    assert_eq!(found.len(), 2);
    assert_eq!(found[&qi.address()], qi);
    assert_eq!(found[&quai.address()], quai);
    assert!(store.public_addresses([]).unwrap().is_empty());

    // A row whose stored key no longer hashes to its address fails the same
    // way on both reads.
    store
        .connection
        .execute(
            "UPDATE addresses SET public_key=?1 WHERE address=?2",
            params![&quai.public_key()[..], &qi.address().bytes()[..]],
        )
        .unwrap();
    assert_eq!(store.addresses().unwrap_err(), StorageError::Invalid);
    assert_eq!(
        store.public_addresses([qi.address()]).unwrap_err(),
        StorageError::Invalid
    );
    assert_eq!(store.public_addresses([quai.address()]).unwrap().len(), 1);
}
