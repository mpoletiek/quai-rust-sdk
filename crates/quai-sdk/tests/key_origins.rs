//! Portable public origins and HD/imported/BIP47 key ownership, including real workers.
#![cfg(feature = "wallet")]
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
use quai_sdk::Zone;
use quai_sdk::crypto::{PublicKey, SecretKey};
use quai_sdk::wallet::metadata::{KeyOrigin, PublicAddress, StorageError};
use quai_sdk::wallet::qi_keys::{QiKeyResolver, QiKeyring};
use quai_sdk::wallet::{CoinType, HdWallet, Search};
fn imported_key() -> SecretKey {
    let mut scalar = [0; 32];
    scalar[31] = 130;
    SecretKey::from_bytes(&scalar).unwrap()
}
fn hd_address(wallet: &HdWallet) -> PublicAddress {
    let account = wallet.account_public(0).unwrap();
    let found = account
        .search(
            false,
            Search {
                zone: Zone::Cyprus1,
                start_index: 0,
                max_attempts: 100_000,
            },
            || false,
        )
        .unwrap();
    PublicAddress::derive(&account, false, found.address.index).unwrap()
}
fn proves_message(ring: &QiKeyring<'_>, address: &PublicAddress) {
    let key = ring.resolve(address).unwrap();
    let message = "Qi origin 日本語 🍊".as_bytes();
    let signature = quai_sdk::signer::sign_qi_message(&key, message).unwrap();
    quai_sdk::signer::verify_qi_message(
        address.address().try_into().unwrap(),
        &PublicKey::from_sec1_bytes(address.public_key()).unwrap(),
        message,
        &signature,
    )
    .unwrap();
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn metadata_roundtrips_both_ledgers_and_rejects_malformed_or_forged_ancestry() {
    for coin in [CoinType::Qi, CoinType::Quai] {
        let wallet = HdWallet::from_seed(&[7; 32], coin).unwrap();
        let address = hd_address(&wallet);
        let bytes = address.export_metadata();
        assert_eq!(bytes.len(), 52);
        assert_eq!(PublicAddress::from_metadata(&bytes).unwrap(), address);
        for len in 0..bytes.len() {
            assert!(PublicAddress::from_metadata(&bytes[..len]).is_err());
        }
        let mut invalid = bytes.clone();
        invalid.push(0);
        assert!(PublicAddress::from_metadata(&invalid).is_err());
        for (index, value) in [
            (0, b'X'),
            (8, 4),
            (41, 2),
            (42, 2),
            (43, 2),
            (44, 128),
            (48, 128),
        ] {
            let mut invalid = bytes.clone();
            invalid[index] = value;
            assert!(
                PublicAddress::from_metadata(&invalid).is_err(),
                "byte {index}"
            );
        }
        let mut wrong_coin = bytes.clone();
        wrong_coin[42] = u8::from(coin == CoinType::Quai);
        assert!(PublicAddress::from_metadata(&wrong_coin).is_err());
        if coin == CoinType::Qi {
            let mut false_account = bytes.clone();
            false_account[47] = 1;
            let forged = PublicAddress::from_metadata(&false_account).unwrap();
            assert!(wallet.resolve(&forged).is_err());
            let wrong_wallet = HdWallet::from_seed(&[8; 32], CoinType::Qi).unwrap();
            assert!(wrong_wallet.resolve(&address).is_err());
        }
    }
    let imported = PublicAddress::imported(&imported_key().public_key()).unwrap();
    let bytes = imported.export_metadata();
    assert_eq!(bytes.len(), 42);
    assert_eq!(PublicAddress::from_metadata(&bytes).unwrap(), imported);
    assert_eq!(imported.origin(), KeyOrigin::ImportedPublic);
    assert_eq!(
        PublicAddress::from_metadata(&vec![0; 1024 * 1024]),
        Err(StorageError::Invalid)
    );
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn portable_keyring_proves_hd_and_imported_ownership_and_never_infers_secrets() {
    let wallet = HdWallet::from_seed(&[7; 32], CoinType::Qi).unwrap();
    let hd = hd_address(&wallet);
    let mut ring = QiKeyring::new(Some(&wallet)).unwrap();
    let imported = ring.import(imported_key()).unwrap();
    proves_message(&ring, &hd);
    proves_message(&ring, &imported);
    assert!(ring.import(imported_key()).is_err());
    let empty = QiKeyring::new(None).unwrap();
    assert!(empty.resolve(&imported).is_err());
    assert!(empty.resolve(&hd).is_err());
    let wrong = HdWallet::from_seed(&[7; 32], CoinType::Quai).unwrap();
    assert!(QiKeyring::new(Some(&wrong)).is_err());
    let mut scalar = [0; 32];
    scalar[30] = 3;
    scalar[31] = 37; // Public Quai toy scalar 805.
    assert!(
        ring.import(SecretKey::from_bytes(&scalar).unwrap())
            .is_err()
    );
    let restored = PublicAddress::from_metadata(&imported.export_metadata()).unwrap();
    proves_message(&ring, &restored);
}
#[cfg(feature = "payments")]
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn portable_keyring_resolves_and_signs_ordered_hd_imported_payment_inputs() {
    use quai_sdk::consensus::{
        Denomination, OutPoint, QiInput, QiOutput, QiTransaction, SignedQiTransaction, U256,
    };
    use quai_sdk::payments::{PaymentDirection, PaymentSearch, PrivatePaymentCode};
    let wallet = HdWallet::from_seed(&[7; 32], CoinType::Qi).unwrap();
    let hd = hd_address(&wallet);
    let mut ring = QiKeyring::new(Some(&wallet)).unwrap();
    let imported = ring.import(imported_key()).unwrap();
    let owner = PrivatePaymentCode::from_seed(&[1; 32], 0).unwrap();
    let peer = PrivatePaymentCode::from_seed(&[2; 32], 0).unwrap();
    let found = owner
        .search(
            peer.public_code(),
            PaymentDirection::Receive,
            PaymentSearch {
                zone: Zone::Cyprus1,
                start_index: 0,
                max_attempts: 100_000,
            },
            || false,
        )
        .unwrap();
    let payment = ring
        .import_payment_receive(&owner, peer.public_code(), found.index)
        .unwrap();
    assert_eq!(payment.address(), found.address.address());
    assert_eq!(
        payment.public_key(),
        &peer
            .send_public_key(owner.public_code(), found.index)
            .unwrap()
            .to_compressed()
    );
    proves_message(&ring, &payment);
    assert!(
        ring.import_payment_receive(&owner, peer.public_code(), found.index)
            .is_err()
    );
    assert!(
        ring.import_payment_receive(&owner, peer.public_code(), 1 << 31)
            .is_err()
    );
    let addresses = [hd, imported, payment];
    let keys: Vec<_> = addresses
        .iter()
        .map(|address| ring.resolve(address).unwrap())
        .collect();
    let inputs = addresses
        .iter()
        .enumerate()
        .map(|(i, address)| {
            let mut hash = [0; 32];
            hash[3] = 0x80;
            hash[31] = i as u8 + 1;
            QiInput {
                previous_output: OutPoint {
                    transaction_hash: hash.into(),
                    index: 0,
                },
                public_key: PublicKey::from_sec1_bytes(address.public_key()).unwrap(),
            }
        })
        .collect();
    let transaction = QiTransaction {
        chain_id: U256::from(15000),
        inputs,
        outputs: vec![QiOutput {
            address: "0x0080000000000000000000000000000000000001"
                .parse()
                .unwrap(),
            denomination: Denomination::new(0).unwrap(),
        }],
        data: vec![],
    };
    let refs: Vec<_> = keys.iter().collect();
    let signed = transaction.sign_local(&refs).unwrap();
    let decoded = SignedQiTransaction::decode(&signed.signed_bytes().unwrap()).unwrap();
    assert_eq!(decoded.hash().unwrap(), signed.hash().unwrap());
    let mut reversed = refs;
    reversed.reverse();
    assert!(transaction.sign_local(&reversed).is_err());
}

#[cfg(all(target_arch = "wasm32", feature = "browser"))]
#[wasm_bindgen_test::wasm_bindgen_test]
async fn browser_metadata_reopens_through_indexeddb_without_granting_key_authority() {
    use quai_sdk::U256;
    use quai_sdk::browser::{BrowserSnapshotStore, BrowserStorageScope};
    use quai_sdk::primitives::Hash32;
    let scope = BrowserStorageScope {
        chain_id: U256::from(15000),
        genesis: Hash32::from_bytes([71; 32]),
        zone: Zone::Cyprus1,
        wallet: Hash32::from_bytes([93; 32]),
    };
    let wallet = HdWallet::from_seed(&[7; 32], CoinType::Qi).unwrap();
    let address = hd_address(&wallet);
    let store = BrowserSnapshotStore::open("quai-sdk-key-origin-fixture", scope, 52)
        .await
        .unwrap();
    let old = store
        .read()
        .await
        .unwrap()
        .map(|snapshot| snapshot.revision);
    let saved = store
        .compare_exchange(old, Some(&address.export_metadata()))
        .await
        .unwrap();
    drop(store);
    let reopened = BrowserSnapshotStore::open("quai-sdk-key-origin-fixture", scope, 52)
        .await
        .unwrap();
    let record = reopened.read().await.unwrap().unwrap();
    assert_eq!(record.revision, saved);
    let restored = PublicAddress::from_metadata(&record.bytes.unwrap()).unwrap();
    assert_eq!(restored, address);
    assert!(QiKeyring::new(None).unwrap().resolve(&restored).is_err());
    proves_message(&QiKeyring::new(Some(&wallet)).unwrap(), &restored);
    assert!(
        reopened
            .compare_exchange(old, Some(&restored.export_metadata()))
            .await
            .is_err()
    );
    reopened.compare_exchange(Some(saved), None).await.unwrap();
}

#[path = "fixtures/shared/crates/quai-crypto/tests/metadata.rs"]
mod crypto_metadata;
