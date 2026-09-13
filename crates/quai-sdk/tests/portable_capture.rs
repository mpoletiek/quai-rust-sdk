//! Recovery capture across HD, account, Qi and payment-code journals.
#![cfg(feature = "backup")]
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
use quai_sdk::consensus::{
    Denomination, OutPoint, QiInput, QiOutput, QiTransaction, QuaiTransaction, SignedQiOperation,
};
use quai_sdk::crypto::PublicKey;
use quai_sdk::payments::{PaymentDirection, PaymentSearch, PrivatePaymentCode};
use quai_sdk::primitives::Hash32;
use quai_sdk::wallet::account_custody::AccountOperationBook;
use quai_sdk::wallet::allocation::{AddressAllocationBook, AddressAllocationId};
use quai_sdk::wallet::discovery::{Checkpoint, NetworkScope};
use quai_sdk::wallet::full_backup::{
    AccountCustodyCapture, BackupOrigin, MAX_PORTABLE_CAPTURE_JOURNALS, PortableWalletCapture,
    WalletBackup,
};
use quai_sdk::wallet::metadata::PublicAddress;
use quai_sdk::wallet::payment_allocation::{PaymentAllocationBook, PaymentAllocationId};
use quai_sdk::wallet::qi_custody::{QiOperationBook, ReservationId};
use quai_sdk::wallet::qi_keys::QiKeyResolver;
use quai_sdk::wallet::{CandidateCoin, CoinType, HdWallet, Search};
use quai_sdk::{U256, Zone};
fn scope() -> NetworkScope {
    NetworkScope {
        chain_id: U256::from(15000),
        genesis: Hash32::from_bytes([1; 32]),
        zone: Zone::Cyprus1,
    }
}
fn id(n: u8) -> ReservationId {
    let mut id = [0; 16];
    id[15] = n;
    ReservationId(id)
}
struct Fixture {
    hd: [AddressAllocationBook; 2],
    account: AccountOperationBook,
    account_address: PublicAddress,
    qi: QiOperationBook,
    payments: [PaymentAllocationBook; 2],
    owner: PrivatePaymentCode,
}
impl Fixture {
    fn new() -> Self {
        let mut hd = Vec::new();
        let mut addresses = Vec::new();
        let mut keys = Vec::new();
        for coin in [CoinType::Quai, CoinType::Qi] {
            let wallet = HdWallet::from_seed(&[1; 16], coin).unwrap();
            let public = wallet.account_public(0).unwrap();
            let found = public
                .search(
                    false,
                    Search {
                        zone: scope().zone,
                        start_index: 0,
                        max_attempts: 100_000,
                    },
                    || false,
                )
                .unwrap();
            let mut b = AddressAllocationBook::new(scope(), public, 0, 0).unwrap();
            b.reserve(AddressAllocationId(id(1).0), false, found.address.index + 1)
                .unwrap();
            let address = b
                .complete(AddressAllocationId(id(1).0), found.address.index)
                .unwrap();
            b.reserve(AddressAllocationId(id(2).0), true, 10).unwrap(); // Pending range must survive recovery as burned.
            b.reserve(AddressAllocationId(id(3).0), false, 10).unwrap();
            b.abandon(AddressAllocationId(id(3).0)).unwrap();
            let key = if coin == CoinType::Qi {
                wallet.resolve(&address).unwrap()
            } else {
                wallet
                    .derive_key(0, false, found.address.index)
                    .unwrap()
                    .secret_key()
                    .unwrap()
            };
            keys.push(key);
            addresses.push(address);
            hd.push(b);
        }
        let mut account = AccountOperationBook::new(scope(), keys[0].public_key(), 8).unwrap();
        account.reserve_nonce(id(100), 0).unwrap();
        account
            .commit_signed(
                id(100),
                &QuaiTransaction {
                    chain_id: scope().chain_id,
                    nonce: 8,
                    to: None,
                    value: U256::ZERO,
                    gas_limit: 21000,
                    gas_price: U256::from(1),
                    data: vec![],
                    access_list: vec![],
                }
                .sign(&keys[0])
                .unwrap(),
            )
            .unwrap();
        let owner = PrivatePaymentCode::from_seed(&[1; 16], 0).unwrap();
        let peer = PrivatePaymentCode::from_seed(&[2; 16], 0).unwrap();
        let mut payments = Vec::new();
        let mut receive = None;
        for direction in [PaymentDirection::Send, PaymentDirection::Receive] {
            let found = owner
                .search(
                    peer.public_code(),
                    direction,
                    PaymentSearch {
                        zone: scope().zone,
                        start_index: 0,
                        max_attempts: 100_000,
                    },
                    || false,
                )
                .unwrap();
            let mut b = PaymentAllocationBook::new(
                scope(),
                &owner,
                peer.public_code().clone(),
                direction,
                0,
            )
            .unwrap();
            b.reserve(PaymentAllocationId(id(1).0), found.index + 1)
                .unwrap();
            let record = b
                .complete(&owner, PaymentAllocationId(id(1).0), found.index)
                .unwrap();
            b.reserve(PaymentAllocationId(id(2).0), 10).unwrap();
            b.reserve(PaymentAllocationId(id(3).0), 10).unwrap();
            b.abandon(PaymentAllocationId(id(3).0)).unwrap();
            if direction == PaymentDirection::Receive {
                receive = Some(record);
            }
            payments.push(b);
        }
        let receive = receive.unwrap();
        let payment_key = owner
            .receive_key(peer.public_code(), receive.index)
            .unwrap();
        let public = [
            addresses[1].clone(),
            PublicAddress::imported(&payment_key.public_key()).unwrap(),
        ];
        let inputs: Vec<_> = public
            .iter()
            .enumerate()
            .map(|(n, p)| {
                let mut hash = [0; 32];
                hash[3] = 0x80;
                hash[31] = n as u8 + 1;
                QiInput {
                    previous_output: OutPoint {
                        transaction_hash: Hash32::from_bytes(hash),
                        index: 0,
                    },
                    public_key: PublicKey::from_sec1_bytes(p.public_key()).unwrap(),
                }
            })
            .collect();
        let coins: Vec<_> = inputs
            .iter()
            .map(|i| CandidateCoin {
                outpoint: i.previous_output,
                address: i.public_key.address().try_into().unwrap(),
                denomination: Denomination::new(3).unwrap(),
                unlock_height: U256::ZERO,
                expires_at: None,
                reserved: false,
            })
            .collect();
        let transaction = QiTransaction {
            chain_id: scope().chain_id,
            inputs,
            outputs: vec![QiOutput {
                address: "0x0080000000000000000000000000000000000001"
                    .parse()
                    .unwrap(),
                denomination: Denomination::new(0).unwrap(),
            }],
            data: vec![],
        };
        let mut qi = QiOperationBook::new(scope(), Hash32::from_bytes([7; 32])).unwrap();
        qi.reserve(
            id(101),
            Checkpoint {
                hash: Hash32::from_bytes([2; 32]),
                height: U256::from(1),
            },
            U256::from(1),
            &coins,
            &public,
        )
        .unwrap();
        qi.commit_signed(
            id(101),
            &SignedQiOperation::Transfer(
                transaction.sign_local(&[&keys[1], &payment_key]).unwrap(),
            ),
        )
        .unwrap();
        Self {
            hd: hd.try_into().unwrap(),
            account,
            account_address: addresses.remove(0),
            qi,
            payments: payments.try_into().unwrap(),
            owner,
        }
    }
    fn capture(&self) -> WalletBackup {
        WalletBackup::capture_portable(
            PortableWalletCapture {
                allocations: &[&self.hd[0], &self.hd[1]],
                accounts: &[AccountCustodyCapture {
                    book: &self.account,
                    address: &self.account_address,
                }],
                qi: &[&self.qi],
                payments: &[&self.payments[0], &self.payments[1]],
                ..Default::default()
            },
            vec![BackupOrigin::from_seed(&[1; 16]).unwrap()],
        )
        .unwrap()
    }
    fn verify(&self, backup: &WalletBackup) {
        let state = backup.scope_state(scope()).unwrap();
        assert_eq!(state.addresses().len(), 3); // Two HD owners plus payment receive; send is not owned.
        assert_eq!(state.derivation_cursors().count(), 4);
        assert_eq!(state.operations().count(), 2);
        assert_eq!(backup.payment_channels().count(), 1);
        assert_eq!(backup.payment_exposures().count(), 2);
        assert_eq!(
            AccountOperationBook::from_backup(backup, scope(), self.account.owner())
                .unwrap()
                .operation(id(100))
                .unwrap()
                .payload,
            self.account.operation(id(100)).unwrap().payload
        );
        assert_eq!(
            QiOperationBook::from_backup(backup, scope(), self.qi.identity())
                .unwrap()
                .operation(id(101))
                .unwrap()
                .payload,
            self.qi.operation(id(101)).unwrap().payload
        );
        for b in &self.hd {
            let restored =
                AddressAllocationBook::from_backup(backup, scope(), b.account().clone()).unwrap();
            for change in [false, true] {
                assert_eq!(restored.next_index(change), b.next_index(change));
            }
            assert_eq!(restored.allocations().count(), 0); // Recovery format starts new request IDs above every burned range.
        }
        for b in &self.payments {
            let restored = PaymentAllocationBook::from_backup(
                backup,
                scope(),
                &self.owner,
                b.peer().clone(),
                b.direction(),
            )
            .unwrap();
            assert_eq!(restored.next_index(), b.next_index());
            assert_eq!(restored.allocations().count(), 0);
        }
    }
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn unified_recovery_proves_payment_inputs_and_preserves_all_burned_cursors() {
    let f = Fixture::new();
    let backup = f.capture();
    f.verify(&backup);
    assert!(
        WalletBackup::capture_qi_custody(&f.qi, vec![BackupOrigin::from_seed(&[1; 16]).unwrap()])
            .is_err()
    ); // Payment exposure is the missing derivation proof.
    assert_eq!(f.hd[0].allocations().count(), 3); // Capture does not mutate live journals.
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn unified_capture_rejects_duplicate_journals_unowned_inventory_and_cross_ledger_id_collisions() {
    let f = Fixture::new();
    let origins = || vec![BackupOrigin::from_seed(&[1; 16]).unwrap()];
    assert!(
        WalletBackup::capture_portable(
            PortableWalletCapture {
                allocations: &[&f.hd[0], &f.hd[0]],
                ..Default::default()
            },
            origins()
        )
        .is_err()
    );
    assert!(
        WalletBackup::capture_portable(
            PortableWalletCapture {
                qi: &vec![&f.qi; MAX_PORTABLE_CAPTURE_JOURNALS + 1],
                ..Default::default()
            },
            origins()
        )
        .is_err()
    );
    assert!(
        WalletBackup::capture_portable(
            PortableWalletCapture {
                allocations: &[&f.hd[0]],
                ..Default::default()
            },
            vec![BackupOrigin::from_seed(&[2; 16]).unwrap()]
        )
        .is_err()
    );
    let mut collision = AccountOperationBook::new(scope(), f.account.owner(), 0).unwrap();
    collision.reserve_nonce(id(101), 0).unwrap();
    assert!(
        WalletBackup::capture_portable(
            PortableWalletCapture {
                accounts: &[AccountCustodyCapture {
                    book: &collision,
                    address: &f.account_address
                }],
                qi: &[&f.qi],
                payments: &[&f.payments[1]],
                ..Default::default()
            },
            origins()
        )
        .is_err()
    );
    let mut wrong = scope();
    wrong.genesis = Hash32::ZERO;
    assert!(
        WalletBackup::capture_portable(
            PortableWalletCapture {
                additional_addresses: &[(wrong, f.account_address.clone())],
                ..Default::default()
            },
            origins()
        )
        .is_err()
    );
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn unified_capture_encrypts_and_restores_owned_channels_and_both_ledger_claims() {
    let f = Fixture::new();
    let encrypted = f
        .capture()
        .encrypt(
            b"public toy backup password",
            quai_sdk::wallet::BackupKdf::default(),
        )
        .unwrap();
    let restored = encrypted.decrypt(b"public toy backup password").unwrap();
    f.verify(&restored);
    #[cfg(all(not(target_arch = "wasm32"), feature = "sqlite"))]
    {
        let mut random = [0; 16];
        quai_sdk::crypto::fill_random(&mut random).unwrap();
        let directory = std::env::temp_dir().join(format!(
            "quai-portable-capture-{:x}",
            u128::from_be_bytes(random)
        ));
        std::fs::create_dir(&directory).unwrap();
        let mut store =
            quai_sdk::wallet::storage::SqliteStore::open(directory.join("wallet.sqlite"), scope())
                .unwrap();
        assert_eq!(
            restored
                .restore(&mut store)
                .unwrap()
                .retained_signed_operations,
            2
        );
        assert_eq!(
            store.signed_payload(id(100)).unwrap(),
            f.account.operation(id(100)).unwrap().payload
        );
        assert_eq!(
            store.signed_payload(id(101)).unwrap(),
            f.qi.operation(id(101)).unwrap().payload
        );
        let recaptured =
            WalletBackup::capture(&mut store, vec![BackupOrigin::from_seed(&[1; 16]).unwrap()])
                .unwrap();
        f.verify(&recaptured);
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn portable_capture_retains_exhausted_cursors_across_zones_and_networks() {
    let wallet = HdWallet::from_seed(&[1; 16], CoinType::Qi).unwrap();
    let account = wallet.account_public(0).unwrap();
    let owner = PrivatePaymentCode::from_seed(&[1; 16], 0).unwrap();
    let peer = PrivatePaymentCode::from_seed(&[2; 16], 0).unwrap();
    let scopes = [
        scope(),
        NetworkScope {
            zone: Zone::from_byte(0x11).unwrap(),
            ..scope()
        },
        NetworkScope {
            chain_id: U256::from(9),
            ..scope()
        },
    ];
    let hd: Vec<_> = scopes
        .iter()
        .map(|s| AddressAllocationBook::new(*s, account.clone(), 1 << 31, 17).unwrap())
        .collect();
    let payment: Vec<_> = scopes
        .iter()
        .map(|s| {
            PaymentAllocationBook::new(
                *s,
                &owner,
                peer.public_code().clone(),
                PaymentDirection::Send,
                1 << 31,
            )
            .unwrap()
        })
        .collect();
    let backup = WalletBackup::capture_portable(
        PortableWalletCapture {
            allocations: &hd.iter().collect::<Vec<_>>(),
            payments: &payment.iter().collect::<Vec<_>>(),
            ..Default::default()
        },
        vec![BackupOrigin::from_seed(&[1; 16]).unwrap()],
    )
    .unwrap();
    assert_eq!(backup.scopes().len(), 3);
    assert_eq!(backup.payment_channels().count(), 2);
    for scope in scopes {
        let recovered =
            AddressAllocationBook::from_backup(&backup, scope, account.clone()).unwrap();
        assert_eq!(recovered.next_index(false), 1 << 31);
        assert_eq!(recovered.next_index(true), 17);
        let recovered = PaymentAllocationBook::from_backup(
            &backup,
            scope,
            &owner,
            peer.public_code().clone(),
            PaymentDirection::Send,
        )
        .unwrap();
        assert_eq!(recovered.next_index(), 1 << 31);
    }
    let many: Vec<_> = (1..=65)
        .map(|n| {
            QiOperationBook::new(
                NetworkScope {
                    chain_id: U256::from(n),
                    ..scope()
                },
                Hash32::from_bytes([7; 32]),
            )
            .unwrap()
        })
        .collect();
    assert!(
        WalletBackup::capture_portable(
            PortableWalletCapture {
                qi: &many.iter().collect::<Vec<_>>(),
                ..Default::default()
            },
            vec![BackupOrigin::from_seed(&[1; 16]).unwrap()]
        )
        .is_err()
    );
}

#[cfg(all(target_arch = "wasm32", feature = "browser"))]
#[wasm_bindgen_test::wasm_bindgen_test]
async fn combined_backup_initializes_all_browser_journals_without_losing_custody() {
    use quai_sdk::browser_accounts::BrowserAccountBook;
    use quai_sdk::browser_addresses::BrowserAddressBook;
    use quai_sdk::browser_payments::BrowserPaymentBook;
    use quai_sdk::browser_qi::BrowserQiBook;
    let f = Fixture::new();
    let backup = f.capture();
    let mut random = [0; 16];
    quai_sdk::crypto::fill_random(&mut random).unwrap();
    let name = format!("quai-combined-capture-{:x}", u128::from_be_bytes(random));
    for b in &f.hd {
        let store = BrowserAddressBook::open(&name, b.scope(), b.account().clone())
            .await
            .unwrap();
        store.initialize_from_backup(&backup).await.unwrap();
        let restored = store.snapshot().await.unwrap();
        for change in [false, true] {
            assert_eq!(restored.book.next_index(change), b.next_index(change));
        }
        assert!(store.initialize_from_backup(&backup).await.is_err());
    }
    for b in &f.payments {
        let store =
            BrowserPaymentBook::open(&name, b.scope(), &f.owner, b.peer().clone(), b.direction())
                .await
                .unwrap();
        store
            .initialize_from_backup(&f.owner, &backup)
            .await
            .unwrap();
        assert_eq!(
            store.snapshot(&f.owner).await.unwrap().book.next_index(),
            b.next_index()
        );
    }
    let account = BrowserAccountBook::open(&name, scope(), f.account.owner())
        .await
        .unwrap();
    account.initialize_from_backup(&backup).await.unwrap();
    assert_eq!(
        account
            .snapshot()
            .await
            .unwrap()
            .book
            .operation(id(100))
            .unwrap()
            .payload,
        f.account.operation(id(100)).unwrap().payload
    );
    let qi = BrowserQiBook::open(&name, scope(), f.qi.identity())
        .await
        .unwrap();
    qi.initialize_from_backup(&backup).await.unwrap();
    assert_eq!(
        qi.snapshot()
            .await
            .unwrap()
            .book
            .operation(id(101))
            .unwrap()
            .payload,
        f.qi.operation(id(101)).unwrap().payload
    );
    assert!(qi.release_unsigned(id(101)).await.is_err());
    assert!(account.release_unsigned(id(100)).await.is_err());
}
