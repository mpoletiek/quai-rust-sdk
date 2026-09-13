//! Offline public-fixture compatibility, secret-boundary and cancellation tests.
use quai_primitives::Zone;
use quai_wallet::{
    AccountPublic, CoinType, ExtendedPrivateKey, ExtendedPublicKey, HdWallet, Language, Mnemonic,
    Search, WalletError,
};
use serde_json::Value;

const PHRASE: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

fn fixture() -> Value {
    serde_json::from_str(include_str!("reference.json")).unwrap()
}
fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap()
}
fn number(value: &Value, key: &str) -> u32 {
    value[key].as_u64().unwrap().try_into().unwrap()
}
fn unhex(value: &str) -> Vec<u8> {
    value
        .trim_start_matches("0x")
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).unwrap();
            u8::from_str_radix(pair, 16).unwrap()
        })
        .collect()
}
fn hex(bytes: &[u8]) -> String {
    let mut text = String::from("0x");
    for byte in bytes {
        use std::fmt::Write;
        write!(text, "{byte:02x}").unwrap();
    }
    text
}
fn language(value: &str) -> Language {
    match value {
        "cz" => Language::Czech,
        "en" => Language::English,
        "es" => Language::Spanish,
        "fr" => Language::French,
        "it" => Language::Italian,
        "pt" => Language::Portuguese,
        "ja" => Language::Japanese,
        "ko" => Language::Korean,
        "zh_cn" => Language::SimplifiedChinese,
        "zh_tw" => Language::TraditionalChinese,
        _ => panic!("unknown fixture language"),
    }
}

#[test]
fn all_ten_complete_wordlists_match_pinned_quais() {
    let fixtures = fixture();
    assert_eq!(fixtures["reference"], "quais@1.0.0-alpha.57");
    assert_eq!(Language::ALL.len(), 10);
    for list in fixtures["wordlists"].as_array().unwrap() {
        let actual = language(text(list, "language")).word_list();
        let expected = list["words"].as_array().unwrap();
        assert_eq!(expected.len(), 2048);
        for (index, word) in actual.iter().enumerate() {
            assert_eq!(*word, expected[index].as_str().unwrap());
        }
    }
}

#[test]
fn all_languages_entropy_lengths_and_nfkd_seeds_match_quais() {
    for vector in fixture()["mnemonics"].as_array().unwrap() {
        let language = language(text(vector, "language"));
        let mnemonic = Mnemonic::from_entropy(language, &unhex(text(vector, "entropy"))).unwrap();
        assert_eq!(mnemonic.phrase().expose(), text(vector, "phrase"));
        assert_eq!(
            hex(mnemonic.to_seed(text(vector, "passphrase")).expose()),
            text(vector, "seed")
        );
        let imported = Mnemonic::parse(language, text(vector, "phrase")).unwrap();
        for export in [mnemonic.entropy(), imported.entropy()] {
            assert_eq!(hex(export.expose()), text(vector, "entropy"));
            assert_eq!(format!("{export:?}"), "MnemonicEntropy([REDACTED])");
            assert_eq!(
                Mnemonic::from_entropy(language, export.expose())
                    .unwrap()
                    .phrase()
                    .expose(),
                text(vector, "phrase")
            );
        }
        assert_eq!(
            hex(imported.to_seed(text(vector, "passphrase")).expose()),
            text(vector, "seed")
        );
    }
}

#[test]
fn extended_keys_and_variable_seed_lengths_match_quais() {
    for vector in fixture()["extended"].as_array().unwrap() {
        let master = ExtendedPrivateKey::from_seed(&unhex(text(vector, "seed"))).unwrap();
        let node = master.derive_path(text(vector, "path")).unwrap();
        assert_eq!(node.export().unwrap().expose(), text(vector, "xprv"));
        assert_eq!(node.public_key().export(), text(vector, "xpub"));
        let public = node.public_key().public_key().unwrap();
        assert_eq!(hex(&public.to_compressed()), text(vector, "publicKey"));
        assert_eq!(public.address().to_string(), text(vector, "address"));
        assert_eq!(
            ExtendedPrivateKey::import(text(vector, "xprv"))
                .unwrap()
                .export()
                .unwrap()
                .expose(),
            text(vector, "xprv")
        );
        assert_eq!(
            ExtendedPublicKey::import(text(vector, "xpub"))
                .unwrap()
                .export(),
            text(vector, "xpub")
        );
        assert_eq!(
            node.secret_key().unwrap().public_key().address(),
            public.address()
        );
    }
}

#[test]
fn account_watch_only_and_bounded_grinding_match_quais() {
    for vector in fixture()["grinding"].as_array().unwrap() {
        let coin = if number(vector, "coin") == 994 {
            CoinType::Quai
        } else {
            CoinType::Qi
        };
        let mnemonic = Mnemonic::parse(Language::English, text(vector, "phrase")).unwrap();
        let wallet = HdWallet::from_mnemonic(&mnemonic, "", coin).unwrap();
        assert_eq!(wallet.root_public_key().export(), text(vector, "rootXpub"));
        let account = number(vector, "account");
        let change = vector["change"].as_bool().unwrap();
        let public = wallet.account_public(account).unwrap();
        assert_eq!(public.export(), text(vector, "accountXpub"));
        let watch = AccountPublic::import(&public.export(), coin, account).unwrap();
        let expected_index = number(vector, "index");
        let start_index = number(vector, "startIndex");
        let search = Search {
            zone: text(vector, "zone").parse().unwrap(),
            start_index,
            max_attempts: expected_index - start_index + 1,
        };
        let found = watch.search(change, search, || false).unwrap();
        assert_eq!(found.address.address.to_string(), text(vector, "address"));
        assert_eq!(hex(&found.address.public_key), text(vector, "publicKey"));
        assert_eq!(found.address.path(), text(vector, "path"));
        assert_eq!(found.address.index, expected_index);
        assert_eq!(found.attempts, search.max_attempts);
        assert_eq!(found.next_index, Some(expected_index + 1));
        assert_eq!(
            watch.derive_address(change, expected_index).unwrap(),
            found.address
        );
        let metadata =
            quai_wallet::metadata::PublicAddress::derive(&public, change, expected_index).unwrap();
        assert_eq!(metadata.address().to_string(), text(vector, "address"));
        assert_eq!(hex(metadata.public_key()), text(vector, "publicKey"));
        let mut expected = b"QADDR001".to_vec();
        expected.extend(unhex(text(vector, "publicKey")));
        expected.extend([1, u8::from(coin == CoinType::Qi), u8::from(change)]);
        expected.extend(account.to_be_bytes());
        expected.extend(expected_index.to_be_bytes());
        assert_eq!(metadata.export_metadata(), expected);
        assert_eq!(
            quai_wallet::metadata::PublicAddress::from_metadata(&expected).unwrap(),
            metadata
        );
        let private = wallet.derive_key(account, change, expected_index).unwrap();
        assert_eq!(
            private.public_key().public_key().unwrap().address(),
            found.address.address
        );
    }
}

#[test]
fn passphrase_changes_identity_and_composed_unicode_is_equivalent() {
    let mnemonic = Mnemonic::parse(Language::English, PHRASE).unwrap();
    assert_eq!(
        mnemonic.to_seed("é").expose(),
        mnemonic.to_seed("e\u{301}").expose()
    );
    let original = HdWallet::from_mnemonic(&mnemonic, "public-passphrase", CoinType::Quai).unwrap();
    let restored = HdWallet::from_mnemonic(&mnemonic, "public-passphrase", CoinType::Quai).unwrap();
    let different = HdWallet::from_mnemonic(&mnemonic, "", CoinType::Quai).unwrap();
    assert_eq!(
        original.root_public_key().export(),
        restored.root_public_key().export()
    );
    assert_ne!(
        original.root_public_key().export(),
        different.root_public_key().export()
    );
    assert!(Mnemonic::parse(Language::English, &PHRASE.to_uppercase()).is_ok());
}

#[test]
fn invalid_mnemonics_entropy_seeds_and_paths_are_rejected_without_echo() {
    for phrase in [
        "",
        "secret-secret",
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon",
    ] {
        let error = Mnemonic::parse(Language::English, phrase).unwrap_err();
        assert_eq!(error, WalletError::InvalidMnemonic);
        assert!(!format!("{error:?}").contains("secret-secret"));
    }
    for length in [0, 15, 17, 31, 33] {
        assert!(Mnemonic::from_entropy(Language::English, &vec![0; length]).is_err());
    }
    for length in [0, 15, 65] {
        assert!(ExtendedPrivateKey::from_seed(&vec![0; length]).is_err());
    }
    let node = ExtendedPrivateKey::from_seed(&[0; 16]).unwrap();
    assert!(Mnemonic::parse(Language::English, &"x".repeat(4097)).is_err());
    assert!(node.derive_path(&format!("m{}", "/0".repeat(256))).is_err());
    assert!(node.derive_path(&"m".repeat(4097)).is_err());
    assert_eq!(
        node.public_key().derive_path("m/0/1/2").unwrap().export(),
        node.derive_path("m/0/1/2").unwrap().public_key().export()
    );
    assert!(node.public_key().derive_path("m/0'").is_err());
    for path in ["", "0/1", "m//1", "m/-1", "m/2147483648", "m/1x"] {
        assert!(node.derive_path(path).is_err(), "{path}");
    }
    assert!(node.derive_child(1 << 31, false).is_err());
    assert!(
        node.derive_child(0, false)
            .unwrap()
            .derive_path("m/1")
            .is_err()
    );
    assert_eq!(
        node.public_key().derive_child(0, true).unwrap_err(),
        WalletError::HardenedPublicChild
    );
}

#[test]
fn secret_debug_is_redacted_and_watch_only_rejects_private_input() {
    let mnemonic = Mnemonic::parse(Language::English, PHRASE).unwrap();
    let seed = mnemonic.to_seed("test");
    let private = ExtendedPrivateKey::from_seed(seed.expose()).unwrap();
    let exported = private.export().unwrap();
    for debug in [
        format!("{mnemonic:?}"),
        format!("{seed:?}"),
        format!("{private:?}"),
        format!("{exported:?}"),
    ] {
        assert!(debug.contains("REDACTED"));
        assert!(!debug.contains("abandon"));
        assert!(!debug.contains("xprv"));
    }
    assert!(ExtendedPublicKey::import(exported.expose()).is_err());
    assert!(ExtendedPrivateKey::import(&private.public_key().export()).is_err());
    let account = private.derive_path("m/44'/994'/0'").unwrap().public_key();
    assert!(AccountPublic::import(&account.export(), CoinType::Quai, 1).is_err());
    assert!(AccountPublic::import(&private.public_key().export(), CoinType::Quai, 0).is_err());
}

#[test]
fn malformed_extended_key_metadata_and_material_are_rejected() {
    let key = ExtendedPrivateKey::from_seed(&[0; 16]).unwrap();
    let mut public_bytes = bs58::decode(key.public_key().export())
        .with_check(None)
        .into_vec()
        .unwrap();
    public_bytes[45..78].fill(0);
    assert!(
        ExtendedPublicKey::import(&bs58::encode(public_bytes).with_check().into_string()).is_err()
    );
    let encoded = key.export().unwrap();
    let raw = bs58::decode(encoded.expose())
        .with_check(None)
        .into_vec()
        .unwrap();
    let mut maximum_depth = raw.clone();
    maximum_depth[4] = 255;
    let depth_key =
        ExtendedPrivateKey::import(&bs58::encode(maximum_depth).with_check().into_string())
            .unwrap();
    assert_eq!(depth_key.depth(), 255);
    assert_eq!(
        depth_key.derive_child(0, false).unwrap_err(),
        WalletError::Derivation
    );
    assert_eq!(
        depth_key.public_key().derive_child(0, false).unwrap_err(),
        WalletError::Derivation
    );
    for offset in [5, 9] {
        let mut malformed = raw.clone();
        malformed[offset] = 1;
        assert!(
            ExtendedPrivateKey::import(&bs58::encode(malformed).with_check().into_string())
                .is_err()
        );
    }
    let mut zero_scalar = raw.clone();
    zero_scalar[46..78].fill(0);
    assert!(
        ExtendedPrivateKey::import(&bs58::encode(zero_scalar).with_check().into_string()).is_err()
    );
    let mut invalid_prefix = raw;
    invalid_prefix[0] ^= 1;
    assert!(
        ExtendedPrivateKey::import(&bs58::encode(invalid_prefix).with_check().into_string())
            .is_err()
    );
    let mut checksum = encoded.expose().to_owned();
    checksum.pop();
    checksum.push('0');
    assert!(ExtendedPrivateKey::import(&checksum).is_err());
}

#[test]
fn search_cancels_and_resumes_without_overflow_or_silent_index_changes() {
    let mnemonic = Mnemonic::parse(Language::English, PHRASE).unwrap();
    let wallet = HdWallet::from_mnemonic(&mnemonic, "", CoinType::Qi).unwrap();
    let account = wallet.account_public(0).unwrap();
    let search = Search {
        zone: Zone::Cyprus1,
        start_index: 0,
        max_attempts: 1000,
    };
    assert_eq!(
        account.search(false, search, || true).unwrap_err(),
        WalletError::Cancelled {
            attempts: 0,
            next_index: 0
        }
    );
    let mut calls = 0;
    assert_eq!(
        account
            .search(false, search, || {
                calls += 1;
                calls > 3
            })
            .unwrap_err(),
        WalletError::Cancelled {
            attempts: 3,
            next_index: 3
        }
    );
    assert_eq!(
        account
            .search(
                false,
                Search {
                    max_attempts: 3,
                    ..search
                },
                || false
            )
            .unwrap_err(),
        WalletError::SearchExhausted {
            attempts: 3,
            next_index: Some(3)
        }
    );
    for max_attempts in [0, 10_000_001] {
        assert_eq!(
            account
                .search(
                    false,
                    Search {
                        max_attempts,
                        ..search
                    },
                    || false
                )
                .unwrap_err(),
            WalletError::InvalidSearchLimit
        );
    }
    assert_eq!(
        account
            .search(
                false,
                Search {
                    start_index: 1 << 31,
                    ..search
                },
                || false
            )
            .unwrap_err(),
        WalletError::InvalidIndex
    );
    let last = (1 << 31) - 1;
    let address = wallet
        .derive_key(0, false, last)
        .unwrap()
        .public_key()
        .public_key()
        .unwrap()
        .address();
    let zone = if address.zone() == Ok(Zone::Cyprus1) {
        Zone::Cyprus2
    } else {
        Zone::Cyprus1
    };
    assert_eq!(
        account
            .search(
                false,
                Search {
                    zone,
                    start_index: last,
                    max_attempts: 2
                },
                || false
            )
            .unwrap_err(),
        WalletError::SearchExhausted {
            attempts: 1,
            next_index: None
        }
    );
    let resumed = wallet
        .search(
            0,
            false,
            Search {
                start_index: 3,
                max_attempts: 667,
                ..search
            },
            || false,
        )
        .unwrap();
    assert_eq!(resumed.address.index, 669);
    assert_eq!(
        resumed.address.address.to_string(),
        "0x00AA16b5AF6Bc831D200917005898b9645e717Bd"
    );
}

#[test]
fn extended_metadata_matches_quais_and_survives_import() {
    let fixtures: Value = serde_json::from_str(include_str!(
        "fixtures/shared/compatibility/fixtures/key-metadata.json"
    ))
    .unwrap();
    for v in fixtures["vectors"].as_array().unwrap() {
        let node = ExtendedPrivateKey::from_seed(&unhex(text(v, "seed")))
            .unwrap()
            .derive_path(text(v, "path"))
            .unwrap();
        let metadata = node.metadata();
        assert_eq!(metadata, node.public_key().metadata());
        assert_eq!(
            metadata,
            ExtendedPublicKey::import(text(v, "xpub"))
                .unwrap()
                .metadata()
        );
        assert_eq!(
            metadata,
            ExtendedPrivateKey::import(node.export().unwrap().expose())
                .unwrap()
                .metadata()
        );
        assert_eq!(u32::from(metadata.depth), number(v, "depth"));
        assert_eq!(metadata.child_number, number(v, "index"));
        assert_eq!(hex(&metadata.fingerprint), text(v, "fingerprint"));
        assert_eq!(
            hex(&metadata.parent_fingerprint),
            text(v, "parentFingerprint")
        );
        assert_eq!(hex(&metadata.chain_code), text(v, "chainCode"));
        assert_eq!(metadata.is_hardened(), text(v, "path").ends_with("'"));
        assert_eq!(metadata.child_index(), metadata.child_number & 0x7fff_ffff);
        assert_eq!(format!("{metadata:?}"), "ExtendedKeyMetadata([REDACTED])");
        let child = node.derive_child(7, false).unwrap();
        assert_eq!(child.metadata().parent_fingerprint, metadata.fingerprint);
        assert_eq!(
            child.metadata(),
            node.public_key().derive_child(7, false).unwrap().metadata()
        );
    }
}

#[test]
fn relative_paths_match_reference_subtrees_and_check_depth_before_deriving() {
    for v in fixture()["extended"].as_array().unwrap() {
        if text(v, "path") != "m/44'/994'/0'/0/0" {
            continue;
        }
        let master = ExtendedPrivateKey::from_seed(&unhex(text(v, "seed"))).unwrap();
        let account = master.derive_relative_path("44'/994'/0'").unwrap();
        assert_eq!(
            account
                .derive_relative_path("0/0")
                .unwrap()
                .public_key()
                .export(),
            text(v, "xpub")
        );
        assert_eq!(
            account
                .public_key()
                .derive_relative_path("0/0")
                .unwrap()
                .export(),
            text(v, "xpub")
        );
        assert_eq!(
            account
                .public_key()
                .derive_relative_path("0/0'")
                .unwrap_err(),
            WalletError::HardenedPublicChild
        );
        for path in ["", "m", "m/0", "/0", "0//1", "-1", "2147483648", "0/", " 0"] {
            assert!(account.derive_relative_path(path).is_err(), "{path}");
            assert!(
                account.public_key().derive_relative_path(path).is_err(),
                "{path}"
            );
        }
    }
    let master = ExtendedPrivateKey::from_seed(&[1; 16]).unwrap();
    let path = vec!["0"; 255].join("/");
    let deepest = master.derive_relative_path(&path).unwrap();
    assert_eq!(deepest.depth(), 255);
    assert_eq!(
        deepest.derive_relative_path("0").unwrap_err(),
        WalletError::InvalidPath
    );
    assert_eq!(
        deepest.public_key().derive_relative_path("0").unwrap_err(),
        WalletError::InvalidPath
    );
    assert_eq!(
        master.derive_relative_path(&(path + "/0")).unwrap_err(),
        WalletError::InvalidPath
    );
    assert_eq!(
        master.derive_relative_path(&"0/".repeat(2048)).unwrap_err(),
        WalletError::InvalidPath
    );
}
