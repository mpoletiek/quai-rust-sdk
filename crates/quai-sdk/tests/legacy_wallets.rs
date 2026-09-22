//! Published public-toy whole-wallet migration vectors, native and worker.
#![cfg(feature = "backup")]
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
use quai_sdk::wallet::allocation::AddressAllocationBook;
use quai_sdk::wallet::discovery::NetworkScope;
use quai_sdk::wallet::full_backup::legacy::{
    LegacyWalletIdentity, MAX_LEGACY_JSON_BYTES, export_quais_json, import_quais_json,
};
use quai_sdk::wallet::full_backup::{
    BackupOrigin, PortableWalletCapture, WalletBackup, WalletBackupError,
};
use quai_sdk::wallet::{CoinType, ExtendedPublicKey, HdWallet, Language, Mnemonic};
use quai_sdk::{U256, Zone};
use serde_json::{Value, json};
fn vectors() -> Value {
    serde_json::from_str(include_str!(
        "fixtures/shared/compatibility/fixtures/legacy-wallets.json"
    ))
    .unwrap()
}
fn scope() -> NetworkScope {
    NetworkScope {
        chain_id: U256::from(9),
        genesis: quai_sdk::primitives::Hash32::from_bytes([1; 32]),
        zone: Zone::Cyprus1,
    }
}
fn mnemonic(v: &Value) -> Mnemonic {
    Mnemonic::parse(
        if v["language"] == "fr" {
            Language::French
        } else {
            Language::English
        },
        v["document"]["phrase"].as_str().unwrap(),
    )
    .unwrap()
}
fn coin(v: &Value) -> CoinType {
    if v["document"]["coinType"] == 969 {
        CoinType::Qi
    } else {
        CoinType::Quai
    }
}
fn import(v: &Value, doc: &[u8]) -> Result<WalletBackup, WalletBackupError> {
    let m = mnemonic(v);
    let root = ExtendedPublicKey::import(v["expectedRoot"].as_str().unwrap()).unwrap();
    import_quais_json(
        doc,
        scope(),
        LegacyWalletIdentity {
            language: m.language(),
            passphrase: v["passphrase"].as_str().unwrap(),
            expected_root: &root,
        },
    )
}
fn sorted(mut v: Value) -> Value {
    fn rows(v: &mut Value) {
        v.as_array_mut()
            .unwrap()
            .sort_by_key(|r| r["address"].as_str().unwrap().to_lowercase());
    }
    rows(&mut v["addresses"]);
    if let Some(map) = v["senderPaymentCodeInfo"].as_object_mut() {
        for value in map.values_mut() {
            rows(value);
        }
    }
    v
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn published_legacy_documents_preserve_identity_origins_and_safe_export_roundtrips() {
    for v in vectors()["vectors"].as_array().unwrap() {
        let bytes = serde_json::to_vec(&v["document"]).unwrap();
        let backup = import(v, &bytes).unwrap();
        let m = mnemonic(v);
        let count: usize = backup
            .scopes()
            .iter()
            .map(|s| backup.scope_state(*s).unwrap().addresses().len())
            .sum();
        assert_eq!(count, v["document"]["addresses"].as_array().unwrap().len());
        let wallet =
            HdWallet::from_mnemonic(&m, v["passphrase"].as_str().unwrap(), coin(v)).unwrap();
        assert_eq!(
            backup.origins()[0]
                .account_public(coin(v), 0)
                .unwrap()
                .export(),
            wallet.account_public(0).unwrap().export()
        );
        for s in backup.scopes() {
            assert_eq!(backup.scope_state(s).unwrap().operations().count(), 0);
        }
        if coin(v) == CoinType::Qi {
            assert_eq!(backup.origins().len(), 2);
            assert_eq!(backup.payment_channels().count(), 2);
            assert_eq!(backup.payment_exposures().count(), 4);
        }
        if v["canExport"] == true {
            let output = export_quais_json(&backup, scope(), coin(v), &m).unwrap();
            assert_eq!(format!("{output:?}"), "SecretString([REDACTED])");
            let doc: Value = serde_json::from_str(output.expose()).unwrap();
            assert_eq!(sorted(doc), sorted(v["canonicalExport"].clone()));
            let again = import(v, output.expose().as_bytes()).unwrap();
            assert_eq!(again.scopes(), backup.scopes());
            assert_eq!(
                again.payment_exposures().count(),
                backup.payment_exposures().count()
            );
        } else {
            assert!(export_quais_json(&backup, scope(), coin(v), &m).is_err());
        }
    }
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn forged_metadata_private_keys_and_payment_exposures_reject_without_partial_recovery() {
    let all = vectors();
    let v = &all["vectors"][1];
    let original = &v["document"];
    for which in 0..15 {
        let mut doc = original.clone();
        match which {
            0 => doc["version"] = json!(2),
            1 => doc["coinType"] = json!(994),
            2 => doc["phrase"] = json!("wrong phrase"),
            3 => doc["addresses"][0]["account"] = json!(2147483648u64),
            4 => doc["addresses"][0]["zone"] = json!("0x01"),
            5 => doc["addresses"][0]["pubKey"] = json!("0x00"),
            6 => doc["addresses"][0]["change"] = json!(true),
            7 => doc["addresses"][0]["status"] = json!("FORGED"),
            8 => doc["addresses"][0]["lastSyncedBlock"]["number"] = json!(9007199254740992u64),
            9 => {
                let first = doc["addresses"][0].clone();
                doc["addresses"].as_array_mut().unwrap().push(first);
            }
            10 => {
                for row in doc["addresses"].as_array_mut().unwrap() {
                    if row["index"] == -1 {
                        row["derivationPath"] = json!(format!("0x{:064x}", 131));
                    }
                }
            }
            11 => {
                let peer = doc["senderPaymentCodeInfo"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .next()
                    .unwrap()
                    .clone();
                let sent = doc["senderPaymentCodeInfo"][&peer][0].clone();
                for row in doc["addresses"].as_array_mut().unwrap() {
                    if row["derivationPath"] == peer && row["account"] == 0 {
                        for f in ["address", "pubKey", "zone"] {
                            row[f] = sent[f].clone();
                        }
                    }
                }
            }
            12 => {
                let peer = doc["senderPaymentCodeInfo"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .next()
                    .unwrap()
                    .clone();
                doc["senderPaymentCodeInfo"][&peer][0]["index"] = json!(0);
            }
            13 => doc["unexpected"] = json!(true),
            _ => doc["addresses"][0]["lastSyncedBlock"]["unexpected"] = json!(true),
        }
        assert!(
            import(v, &serde_json::to_vec(&doc).unwrap()).is_err(),
            "mutation {which}"
        );
    }
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn duplicate_fields_channel_keys_bounds_and_wrong_empty_wallet_identity_reject() {
    let all = vectors();
    let v = &all["vectors"][1];
    let raw = serde_json::to_string(&v["document"]).unwrap();
    assert!(import(v, format!("{{\"version\":1,{}", &raw[1..]).as_bytes()).is_err());
    let mut duplicate = v["document"].clone();
    let peer = duplicate["senderPaymentCodeInfo"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    duplicate["senderPaymentCodeInfo"] = json!({});
    let raw = serde_json::to_string(&duplicate).unwrap();
    let replacement = format!("\"senderPaymentCodeInfo\":{{\"{peer}\":[],\"{peer}\":[]}}");
    assert!(
        import(
            v,
            raw.replace("\"senderPaymentCodeInfo\":{}", &replacement)
                .as_bytes()
        )
        .is_err()
    );
    assert!(import(v, &vec![b' '; MAX_LEGACY_JSON_BYTES + 1]).is_err());
    let mut too_many = v["document"].clone();
    too_many["addresses"] = json!(vec![v["document"]["addresses"][0].clone(); 1025]);
    assert!(import(v, &serde_json::to_vec(&too_many).unwrap()).is_err());
    let v = &all["vectors"][2];
    let m = mnemonic(v);
    let root = ExtendedPublicKey::import(v["expectedRoot"].as_str().unwrap()).unwrap();
    assert!(matches!(
        import_quais_json(
            &serde_json::to_vec(&v["document"]).unwrap(),
            scope(),
            LegacyWalletIdentity {
                language: m.language(),
                passphrase: "",
                expected_root: &root
            }
        ),
        Err(WalletBackupError::Ownership)
    ));
    let mut foreign = scope();
    foreign.genesis = quai_sdk::primitives::Hash32::ZERO;
    assert!(
        import_quais_json(
            &serde_json::to_vec(&v["document"]).unwrap(),
            foreign,
            LegacyWalletIdentity {
                language: m.language(),
                passphrase: v["passphrase"].as_str().unwrap(),
                expected_root: &root
            }
        )
        .is_err()
    );
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn recovery_merge_keeps_newer_burned_floors_and_legacy_export_rejects_unrepresentable_tails() {
    let all = vectors();
    let v = &all["vectors"][1];
    let backup = import(v, &serde_json::to_vec(&v["document"]).unwrap()).unwrap();
    let m = mnemonic(v);
    let wallet = HdWallet::from_mnemonic(&m, "", CoinType::Qi).unwrap();
    let account = wallet.account_public(0).unwrap();
    let restored = AddressAllocationBook::from_backup(&backup, scope(), account.clone()).unwrap();
    let receive = restored.next_index(false);
    let change = restored.next_index(true);
    assert!(receive > 0 && change > 0);
    let mut live =
        AddressAllocationBook::new(scope(), account, receive + 100, change + 100).unwrap();
    live.merge_backup(&backup).unwrap();
    assert_eq!(live.next_index(false), receive + 100);
    assert_eq!(live.next_index(true), change + 100);
    let captured = WalletBackup::capture_portable(
        PortableWalletCapture::new().with_allocations(&[&live]),
        vec![BackupOrigin::from_mnemonic(&m, "").unwrap()],
    )
    .unwrap();
    assert!(matches!(
        export_quais_json(&captured, scope(), CoinType::Qi, &m),
        Err(WalletBackupError::Unsupported)
    ));
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn legacy_export_cannot_discard_account_claims_nonce_floors_or_payment_burned_ranges() {
    use quai_sdk::payments::{PaymentCode, PaymentDirection, PrivatePaymentCode};
    use quai_sdk::wallet::account_custody::AccountOperationBook;
    use quai_sdk::wallet::full_backup::ReservationId;
    use quai_sdk::wallet::metadata::PublicAddress;
    use quai_sdk::wallet::payment_allocation::PaymentAllocationBook;
    let all = vectors();
    let v = &all["vectors"][0];
    let m = mnemonic(v);
    let wallet = HdWallet::from_mnemonic(&m, "", CoinType::Quai).unwrap();
    let first = &v["document"]["addresses"][0];
    let account = wallet
        .account_public(first["account"].as_u64().unwrap() as u32)
        .unwrap();
    let public =
        PublicAddress::derive(&account, false, first["index"].as_u64().unwrap() as u32).unwrap();
    let owner = quai_sdk::crypto::PublicKey::from_sec1_bytes(public.public_key()).unwrap();
    for first_nonce in [0, 10] {
        let mut book = AccountOperationBook::new(scope(), owner, first_nonce).unwrap();
        if first_nonce == 0 {
            book.reserve_nonce(ReservationId([1; 16]), 0).unwrap();
        }
        let backup = WalletBackup::capture_account_custody(
            &book,
            public.clone(),
            vec![BackupOrigin::from_mnemonic(&m, "").unwrap()],
        )
        .unwrap();
        assert!(matches!(
            export_quais_json(&backup, scope(), CoinType::Quai, &m),
            Err(WalletBackupError::Unsupported)
        ));
    }
    let peer = all["vectors"][1]["document"]["senderPaymentCodeInfo"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap();
    let owner = PrivatePaymentCode::from_seed(m.to_seed("").expose(), 0).unwrap();
    let payment = PaymentAllocationBook::new(
        scope(),
        &owner,
        PaymentCode::from_base58(peer).unwrap(),
        PaymentDirection::Send,
        100,
    )
    .unwrap();
    let backup = WalletBackup::capture_portable(
        PortableWalletCapture::new().with_payments(&[&payment]),
        vec![BackupOrigin::from_mnemonic(&m, "").unwrap()],
    )
    .unwrap();
    assert!(matches!(
        export_quais_json(&backup, scope(), CoinType::Qi, &m),
        Err(WalletBackupError::Unsupported)
    ));
}
