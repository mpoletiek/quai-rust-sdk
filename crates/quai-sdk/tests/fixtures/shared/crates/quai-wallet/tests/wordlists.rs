//! Full published dictionaries, custom mnemonic identities and hostile inputs.
use quai_wallet::wordlist::{
    CustomMnemonic, MAX_PHRASE_BYTES, MAX_WORDLIST_INPUT_BYTES, Wordlist, WordlistError,
    WordlistStyle,
};
use quai_wallet::{CoinType, HdWallet, Language, Mnemonic};
use serde_json::Value;
fn fixture() -> Value {
    serde_json::from_slice(include_bytes!(
        "fixtures/shared/compatibility/fixtures/wordlists.json"
    ))
    .unwrap()
}
fn language(locale: &str) -> Language {
    match locale {
        "en" => Language::English,
        "es" => Language::Spanish,
        "fr" => Language::French,
        "it" => Language::Italian,
        "cz" => Language::Czech,
        "pt" => Language::Portuguese,
        "ko" => Language::Korean,
        "ja" => Language::Japanese,
        "zh_cn" => Language::SimplifiedChinese,
        "zh_tw" => Language::TraditionalChinese,
        _ => panic!("unexpected fixture locale"),
    }
}
fn bytes(v: &Value) -> Vec<u8> {
    quai_primitives::get_bytes(v.as_str().unwrap()).unwrap()
}
fn words() -> Vec<String> {
    (0..2048).map(|i| format!("word{i:04}")).collect()
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn all_published_dictionaries_and_compressed_overlays_preserve_exact_indexes() {
    let f = fixture();
    assert_eq!(f["languages"].as_array().unwrap().len(), 10);
    let mut compressed = 0;
    for row in f["languages"].as_array().unwrap() {
        let locale = row["locale"].as_str().unwrap();
        let list = Wordlist::builtin(language(locale));
        let expected: Vec<_> = row["words"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w.as_str().unwrap())
            .collect();
        assert_eq!(list.words().collect::<Vec<_>>(), expected, "{locale}");
        assert_eq!(list.len(), 2048);
        assert!(!list.is_empty());
        assert_eq!(list.locale(), locale);
        assert_eq!(list.checksum().to_string(), row["checksum"]);
        for index in [0, 1, 1023, 2047] {
            assert_eq!(list.get_word_index(expected[index]), Some(index as u16));
            assert_eq!(list.get_word(index), Some(expected[index]));
        }
        assert_eq!(list.get_word(2048), None);
        assert_eq!(list.get_word_index("not-a-word"), None);
        if let Some(owl) = row["owl"].as_str() {
            compressed += 1;
            let accents = row["accents"].as_str();
            let decoded = Wordlist::from_owl(
                locale,
                owl,
                accents,
                row["checksum"].as_str().unwrap().parse().unwrap(),
            )
            .unwrap_or_else(|e| panic!("{locale}: {e:?}"));
            assert_eq!(decoded.words().collect::<Vec<_>>(), expected, "{locale}");
            assert_eq!(decoded.owl_data(), Some(owl));
            assert_eq!(decoded.accent_data(), accents);
            assert_eq!(decoded.clone().checksum(), list.checksum());
            assert_eq!(
                Wordlist::from_owl(
                    locale,
                    owl,
                    accents,
                    quai_primitives::Hash32::from_bytes([0; 32])
                )
                .unwrap_err(),
                WordlistError::Checksum
            );
        }
    }
    assert!(compressed >= 6);
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn phrase_conventions_match_and_unseparated_chinese_recovers_same_identity() {
    for row in fixture()["languages"].as_array().unwrap() {
        let lang = language(row["locale"].as_str().unwrap());
        let list = Wordlist::builtin(lang);
        for split in row["splits"].as_array().unwrap() {
            let actual = list.split(split["phrase"].as_str().unwrap()).unwrap();
            let expected: Vec<_> = split["words"]
                .as_array()
                .unwrap()
                .iter()
                .map(|w| w.as_str().unwrap())
                .collect();
            assert_eq!(
                actual.expose(),
                expected,
                "{} {:?}",
                list.locale(),
                split["phrase"]
            );
            assert_eq!(format!("{actual:?}"), "SecretWords([REDACTED])");
        }
        let sample: Vec<_> = list.words().take(3).collect();
        assert_eq!(list.join(&sample).unwrap().expose(), row["join"]);
        if matches!(
            lang,
            Language::SimplifiedChinese | Language::TraditionalChinese
        ) {
            let joined = row["phrase"].as_str().unwrap().replace(' ', "");
            let parsed = Mnemonic::parse(lang, &joined).unwrap();
            assert_eq!(parsed.entropy().expose(), bytes(&row["entropy"]));
            assert_eq!(
                parsed
                    .to_seed(row["password"].as_str().unwrap())
                    .expose()
                    .as_slice(),
                bytes(&row["seed"])
            );
            assert_eq!(
                CustomMnemonic::parse(&list, &joined)
                    .unwrap()
                    .entropy()
                    .expose(),
                parsed.entropy().expose()
            );
        }
    }
    let general = Wordlist::builtin(Language::English);
    assert_eq!(
        general
            .split("\u{feff}A\u{00a0}B\u{0085}C\u{feff}")
            .unwrap()
            .expose(),
        ["", "a", "b\u{0085}c", ""]
    );
    assert_eq!(general.split("").unwrap().expose(), [""]);
    assert!(
        Wordlist::builtin(Language::SimplifiedChinese)
            .split("")
            .unwrap()
            .expose()
            .is_empty()
    );
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn dictionary_mnemonic_entropy_seeds_and_hd_roots_match_published_identities() {
    let f = fixture();
    for row in f["languages"].as_array().unwrap() {
        let lang = language(row["locale"].as_str().unwrap());
        let list = Wordlist::builtin(lang);
        let entropy = bytes(&row["entropy"]);
        let mnemonic = CustomMnemonic::from_entropy(&list, &entropy).unwrap();
        let phrase = mnemonic.phrase().unwrap();
        assert_eq!(phrase.expose(), row["phrase"]);
        let parsed = CustomMnemonic::parse(&list, phrase.expose()).unwrap();
        assert_eq!(parsed.entropy().expose(), entropy);
        let seed = parsed.to_seed(row["password"].as_str().unwrap()).unwrap();
        assert_eq!(seed.expose().as_slice(), bytes(&row["seed"]));
        assert_eq!(
            seed.expose(),
            Mnemonic::from_entropy(lang, &entropy)
                .unwrap()
                .to_seed(row["password"].as_str().unwrap())
                .expose()
        );
        assert_eq!(mnemonic.wordlist().locale(), list.locale());
        assert_eq!(format!("{mnemonic:?}"), "CustomMnemonic([REDACTED])");
    }
    let list = Wordlist::from_words("custom", words(), WordlistStyle::General).unwrap();
    for row in f["custom"].as_array().unwrap() {
        let m = CustomMnemonic::from_entropy(&list, &bytes(&row["entropy"])).unwrap();
        assert_eq!(m.phrase().unwrap().expose(), row["phrase"]);
        let parsed = CustomMnemonic::parse(&list, row["phrase"].as_str().unwrap()).unwrap();
        assert_eq!(parsed.entropy().expose(), bytes(&row["entropy"]));
        let seed = parsed.to_seed(row["password"].as_str().unwrap()).unwrap();
        let expected = bytes(&row["seed"]);
        assert_eq!(seed.expose().as_slice(), expected);
        assert_eq!(
            HdWallet::from_seed(seed.expose(), CoinType::Qi)
                .unwrap()
                .root_public_key()
                .export(),
            HdWallet::from_seed(&expected, CoinType::Qi)
                .unwrap()
                .root_public_key()
                .export()
        );
    }
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn malformed_dictionaries_expansions_accents_and_phrase_budgets_reject() {
    let checksum = quai_primitives::Hash32::from_bytes([0; 32]);
    assert_eq!(
        Wordlist::from_words("en", vec![], WordlistStyle::General).unwrap_err(),
        WordlistError::Words
    );
    for list in [
        vec!["same".into(), "same".into()],
        vec!["é".into()],
        vec!["Upper".into()],
        vec!["a b".into()],
        vec![String::new()],
    ] {
        assert!(Wordlist::from_words("custom", list, WordlistStyle::General).is_err());
    }
    assert!(
        Wordlist::from_words("bad locale", vec!["word".into()], WordlistStyle::General).is_err()
    );
    assert!(Wordlist::from_words("zh", vec!["two".into()], WordlistStyle::Chinese).is_err());
    assert!(
        Wordlist::from_words("custom", vec!["x".into(); 2049], WordlistStyle::General).is_err()
    );
    assert_eq!(
        Wordlist::from_owl(
            "en",
            &"x".repeat(MAX_WORDLIST_INPUT_BYTES + 1),
            None,
            checksum
        )
        .unwrap_err(),
        WordlistError::Limit
    );
    for data in ["", "1", "0", "0日本語"] {
        assert!(Wordlist::from_owl("en", data, None, checksum).is_err());
    }
    let subs = b" !#$%&'()*+,-./<=>?@[]^_`{|}~";
    let simple = format!("0{}A", "aa".repeat(subs.len()));
    let hash = quai_crypto::keccak256(b"aaa\n").into();
    let small = Wordlist::from_owl("small", &simple, None, hash).unwrap();
    assert_eq!(small.get_word(0), Some("aaa"));
    assert!(CustomMnemonic::from_entropy(&small, &[0; 16]).is_err());
    for overlay in [
        "",
        "a7690A",
        "a7695=",
        "a0A",
        "a7695_",
        "a700005A",
        "a7695A,a7695A",
    ] {
        assert!(
            Wordlist::from_owl("small", &simple, Some(overlay), hash).is_err(),
            "{overlay}"
        );
    }
    let mut bomb = String::from("0");
    for i in 0..subs.len() {
        let b = if i == 0 { b'A' } else { subs[i - 1] };
        bomb.push(char::from(b));
        bomb.push(char::from(b));
    }
    bomb.push(char::from(*subs.last().unwrap()));
    assert_eq!(
        Wordlist::from_owl("small", &bomb, None, checksum).unwrap_err(),
        WordlistError::Limit
    );
    let list = Wordlist::builtin(Language::English);
    assert_eq!(
        list.split(&"a".repeat(MAX_PHRASE_BYTES + 1)).unwrap_err(),
        WordlistError::Limit
    );
    assert!(list.join(&[&"a".repeat(MAX_PHRASE_BYTES + 1)]).is_err());
    assert!(CustomMnemonic::from_entropy(&list, &[0; 15]).is_err());
    assert!(CustomMnemonic::parse(&list, "abandon abandon").is_err());
    assert!(CustomMnemonic::parse(&list, &["abandon"; 12].join(" ")).is_err());
    let m = CustomMnemonic::from_entropy(&list, &[0; 16]).unwrap();
    assert!(m.to_seed(&"x".repeat(MAX_PHRASE_BYTES + 1)).is_err());
    assert!(CustomMnemonic::parse(&list, &format!(" {}", m.phrase().unwrap().expose())).is_err());
}

#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn entropy_export_uses_validated_indexes_for_every_language_size_and_pattern() {
    for &lang in Language::ALL {
        for length in [16, 20, 24, 28, 32] {
            for pattern in [0u8, 0xa5, 0xff] {
                let entropy = vec![pattern; length];
                let m = Mnemonic::from_entropy(lang, &entropy).unwrap();
                assert_eq!(m.entropy().expose(), entropy);
                let phrase = m.phrase();
                let parsed = Mnemonic::parse(lang, phrase.expose()).unwrap();
                assert_eq!(parsed.language(), lang);
                assert_eq!(parsed.entropy().expose(), entropy);
                if matches!(
                    lang,
                    Language::SimplifiedChinese | Language::TraditionalChinese
                ) {
                    let joined = phrase.expose().replace(' ', "");
                    let restored = Mnemonic::parse(lang, &joined).unwrap();
                    assert_eq!(restored.language(), lang);
                    assert_eq!(restored.entropy().expose(), entropy);
                }
            }
        }
    }
}

#[test]
fn general_splitting_keeps_the_unicode_final_sigma_rule() {
    // `str::to_lowercase` implements Unicode Final_Sigma; folding character by
    // character does not, and would produce "οδοσ" where the reference's
    // `toLowerCase` produces "οδος". A custom wordlist holding a final-sigma
    // word would then stop matching an uppercase phrase, so a wallet that
    // restored under an earlier release would fail to restore here.
    let mut words: Vec<String> = (0..2048).map(|i| format!("w{i}")).collect();
    words[0] = "οδος".to_string();
    let list = Wordlist::from_words("custom", words, WordlistStyle::General).unwrap();

    let split = list.split("ΟΔΟΣ").unwrap();
    assert_eq!(split.expose(), &["οδος".to_string()]);
    assert_eq!(
        list.get_word_index("οδος"),
        Some(0),
        "the folded word must resolve to its dictionary entry"
    );

    // The same phrase already in lower case is unaffected either way.
    assert_eq!(list.split("οδος").unwrap().expose(), &["οδος".to_string()]);
}
