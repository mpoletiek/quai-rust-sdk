//! Independent envelope vectors and native-to-worker authenticated backup interoperability.
#![cfg(feature = "backup")]
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
use quai_sdk::wallet::CoinType;
use quai_sdk::wallet::discovery::NetworkScope;
use quai_sdk::wallet::full_backup::{EncryptedWalletBackup, WalletBackup, WalletBackupError};
use quai_sdk::{U256, Zone};
use serde_json::{Value, json};
fn unhex(s: &str) -> Vec<u8> {
    quai_sdk::primitives::get_bytes(&format!("0x{s}")).unwrap()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn inspect(backup: &WalletBackup) -> Value {
    json!(backup.scopes().iter().map(|scope|{
  let state=backup.scope_state(*scope).unwrap();assert_eq!(state.scope(),*scope);
  json!({"chainId":scope.chain_id.to_string(),"genesis":scope.genesis.to_string(),"zone":scope.zone.byte(),
   "addresses":state.addresses().iter().map(|a|hex(&a.export_metadata())).collect::<Vec<_>>(),
   "derivation":state.derivation_cursors().map(|c|json!({"coin":c.coin.number(),"account":c.account,"change":c.change,"xpub":c.account_xpub,"nextIndex":c.next_index})).collect::<Vec<_>>(),
   "nonces":state.nonce_cursors().map(|(a,n)|json!([a.to_string(),n.to_string()])).collect::<Vec<_>>(),
   "operations":state.operations().map(|op|{
    if let Some(payload)=op.signed_payload {
     let hash=if let Ok(signed)=quai_sdk::consensus::SignedQiOperation::decode(payload){signed.hash().unwrap()}else{quai_sdk::consensus::SignedQuaiTransaction::decode(payload).unwrap().hash().unwrap()};assert_eq!(Some(hash),op.reservation.transaction);
    }
    json!({"id":hex(&op.reservation.id.0),"state":op.reservation.state as i64,"hash":op.reservation.transaction.map(|h|h.to_string()),"qiClaims":op.qi_claims.iter().map(|(p,a)|json!([p.transaction_hash.to_string(),p.index,a.to_string()])).collect::<Vec<_>>(),"nonce":op.nonce.map(|(a,n)|json!([a.to_string(),n.to_string()])),"payload":op.signed_payload.map(hex),"replacements":op.replacements.iter().map(|v|json!({"parent":v.parent.to_string(),"payload":hex(&v.payload)})).collect::<Vec<_>>()})
   }).collect::<Vec<_>>()})
 }).collect::<Vec<_>>())
}
fn check_fixture(bytes: &[u8], version: u8) {
    let row: Value = serde_json::from_slice(bytes).unwrap();
    let envelope = unhex(row["envelope"].as_str().unwrap());
    assert_eq!(envelope[8], version);
    let encrypted = EncryptedWalletBackup::from_bytes(&envelope).unwrap();
    assert_eq!(encrypted.as_bytes(), envelope);
    let backup = encrypted
        .decrypt(row["password"].as_str().unwrap().as_bytes())
        .unwrap();
    assert_eq!(format!("{backup:?}"), "WalletBackup([REDACTED])");
    let scope = NetworkScope {
        chain_id: U256::from(15000),
        genesis: quai_sdk::primitives::Hash32::from_bytes([1; 32]),
        zone: Zone::Cyprus1,
    };
    assert_eq!(backup.scopes(), [scope]);
    let mut other = scope;
    other.chain_id += U256::from(1);
    assert!(backup.scope_state(other).is_none());
    if version <= 2 {
        assert_eq!(
            backup.origins()[0].expose_seed().unwrap(),
            unhex(row["seed"].as_str().unwrap())
        );
        let state = backup.scope_state(scope).unwrap();
        assert!(state.addresses().is_empty());
        assert_eq!(state.operations().count(), 0);
    } else {
        assert_eq!(inspect(&backup), row["scopes"]);
        assert_eq!(
            backup.payment_channels().count() as u64,
            row["channels"].as_u64().unwrap()
        );
        assert_eq!(
            backup.payment_exposures().count() as u64,
            row["exposures"].as_u64().unwrap()
        );
    }
    for channel in backup.payment_channels() {
        let owner = backup.origins()[0].payment_code(channel.account()).unwrap();
        assert_eq!(channel.local_code_bytes(), &owner.public_code().to_bytes());
        assert_eq!(
            &channel.network_key()[..32],
            &scope.chain_id.to_be_bytes::<32>()
        );
        assert_eq!(&channel.network_key()[32..], scope.genesis.bytes());
        let public = channel.channel(&owner).unwrap();
        assert_eq!(
            channel.peer_code_bytes(),
            &public.counterparty_code().to_bytes()
        );
        if version == 2 {
            assert_eq!(
                std::str::from_utf8(&public.to_json().unwrap()).unwrap(),
                row["metadata"].as_str().unwrap()
            );
        }
        let wrong = backup.origins()[0].payment_code(channel.account().saturating_add(1));
        if let Ok(wrong) = wrong {
            assert!(matches!(
                channel.channel(&wrong),
                Err(WalletBackupError::Ownership)
            ));
        }
        assert!(channel.generation() <= i64::MAX as u64);
    }
    if version == 3 {
        use quai_sdk::payments::{PaymentCode, PaymentDirection};
        use quai_sdk::wallet::qi_keys::{QiKeyResolver, QiKeyring};
        let mut keys = QiKeyring::new(None).unwrap();
        for exposure in backup.payment_exposures() {
            assert_eq!(exposure.record.direction, PaymentDirection::Receive);
            let owner = backup.origins()[0].payment_code(exposure.account).unwrap();
            assert_eq!(exposure.local_code_bytes, &owner.public_code().to_bytes());
            assert_eq!(
                &exposure.network_key[..32],
                &scope.chain_id.to_be_bytes::<32>()
            );
            let peer = PaymentCode::from_bytes(exposure.peer_code_bytes).unwrap();
            let address = keys
                .import_payment_receive(&owner, &peer, exposure.record.index)
                .unwrap();
            assert_eq!(address.address(), exposure.record.address.address());
            assert_eq!(
                address.public_key(),
                &exposure.record.public_key.to_compressed()
            );
            assert!(
                backup
                    .scope_state(scope)
                    .unwrap()
                    .addresses()
                    .contains(&address)
            );
            assert_eq!(
                keys.resolve(&address).unwrap().public_key(),
                exposure.record.public_key
            );
        }
        assert!(backup.origins()[0].account_public(CoinType::Qi, 7).is_err());
    }
    if version == 5 {
        let state = backup.scope_state(scope).unwrap();
        let cursor = state.derivation_cursors().next().unwrap();
        assert_eq!(cursor.next_index, 10_000);
        assert!(cursor.change);
        let account = backup.origins()[0]
            .account_public(cursor.coin, cursor.account)
            .unwrap();
        assert_eq!(account.export(), cursor.account_xpub);
        let op = state.operations().next().unwrap();
        assert_eq!(op.replacements.len(), 1);
        assert_eq!(op.qi_claims.len(), 1);
        assert!(op.nonce.is_none());
    }
}
macro_rules! fixture_case {
    ($name:ident,$path:literal,$version:literal) => {
        #[cfg_attr(not(target_arch = "wasm32"), test)]
        #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
        fn $name() {
            check_fixture(include_bytes!($path), $version);
        }
    };
}
fixture_case!(
    independent_v1_envelope,
    "fixtures/shared/crates/quai-wallet/tests/full-backup-vector.json",
    1
);
fixture_case!(
    independent_v2_channel_envelope,
    "fixtures/shared/crates/quai-wallet/tests/full-backup-v2-vector.json",
    2
);
fixture_case!(
    native_v3_payment_account_and_exposure,
    "fixtures/shared/crates/quai-wallet/tests/full-backup-v3-portable.json",
    3
);
fixture_case!(
    native_v4_account_candidates_and_nonce,
    "fixtures/shared/crates/quai-wallet/tests/full-backup-v4-portable.json",
    4
);
fixture_case!(
    native_v5_qi_candidates_claims_and_derivation_cursor,
    "fixtures/shared/crates/quai-wallet/tests/full-backup-v5-portable.json",
    5
);
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn hostile_envelopes_are_bounded_before_kdf_and_authentication_rejects_tampering() {
    let row: Value = serde_json::from_slice(include_bytes!(
        "fixtures/shared/crates/quai-wallet/tests/full-backup-vector.json"
    ))
    .unwrap();
    let envelope = unhex(row["envelope"].as_str().unwrap());
    for len in [0, 8, 67, envelope.len() - 1] {
        assert!(EncryptedWalletBackup::from_bytes(&envelope[..len]).is_err());
    }
    for (offset, replacement) in [(8, 6), (9, 2), (11, 1)] {
        let mut bad = envelope.clone();
        bad[offset] = replacement;
        assert!(EncryptedWalletBackup::from_bytes(&bad).is_err());
    }
    for offset in [12, 16, 20] {
        let mut bad = envelope.clone();
        bad[offset..offset + 4].fill(0);
        assert!(EncryptedWalletBackup::from_bytes(&bad).is_err());
    }
    let mut bad = envelope.clone();
    bad[64..68].fill(255);
    assert!(EncryptedWalletBackup::from_bytes(&bad).is_err());
    let mut bad = envelope.clone();
    bad.push(0);
    assert!(EncryptedWalletBackup::from_bytes(&bad).is_err());
    let encrypted = EncryptedWalletBackup::from_bytes(&envelope).unwrap();
    assert!(matches!(
        encrypted.decrypt(b"wrong public password"),
        Err(WalletBackupError::UnlockFailed)
    ));
    let mut bad = envelope;
    let last = bad.len() - 1;
    bad[last] ^= 1;
    assert!(matches!(
        EncryptedWalletBackup::from_bytes(&bad)
            .unwrap()
            .decrypt(row["password"].as_str().unwrap().as_bytes()),
        Err(WalletBackupError::UnlockFailed)
    ));
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn portable_reencryption_uses_fresh_entropy_and_preserves_authenticated_state() {
    let row: Value = serde_json::from_slice(include_bytes!(
        "fixtures/shared/crates/quai-wallet/tests/full-backup-v5-portable.json"
    ))
    .unwrap();
    let password = row["password"].as_str().unwrap().as_bytes();
    let old = EncryptedWalletBackup::from_bytes(&unhex(row["envelope"].as_str().unwrap())).unwrap();
    let backup = old.decrypt(password).unwrap();
    let encrypted = backup
        .encrypt(password, quai_sdk::wallet::BackupKdf::default())
        .unwrap();
    assert_ne!(encrypted.as_bytes(), old.as_bytes());
    assert_ne!(&encrypted.as_bytes()[24..64], &old.as_bytes()[24..64]);
    let restored = encrypted.decrypt(password).unwrap();
    assert_eq!(inspect(&restored), row["scopes"]);
}
