//! Captured mainnet proofs verify against their header's `evmRoot`, and any
//! change to a proof node, key or root fails.
use quai_primitives::{Hash32, QuaiAddress};
use quai_provider::state_proof::{
    EMPTY_CODE_HASH, EMPTY_TRIE_ROOT, ProofError, verify_account_proof, verify_storage_proof,
};
use quai_rpc::U256;
use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "fixtures/shared/test-infra/fixtures/state-proofs-mainnet.json"
    ))
    .unwrap()
}
fn hex(value: &Value) -> Vec<u8> {
    let text = value.as_str().unwrap().trim_start_matches("0x");
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}
fn nodes(value: &Value) -> Vec<Vec<u8>> {
    value.as_array().unwrap().iter().map(hex).collect()
}
fn hash(value: &Value) -> Hash32 {
    value.as_str().unwrap().parse().unwrap()
}
fn quantity(value: &Value) -> U256 {
    let text = value.as_str().unwrap().trim_start_matches("0x");
    U256::from_str_radix(if text.is_empty() { "0" } else { text }, 16).unwrap()
}
fn state_root(fixture: &Value) -> Hash32 {
    hash(&fixture["header"]["evmRoot"])
}

#[test]
fn every_captured_account_and_slot_verifies_and_matches_what_the_node_reported() {
    let fixture = fixture();
    let root = state_root(&fixture);
    let mut shapes = Vec::new();
    for entry in fixture["proofs"].as_array().unwrap() {
        let result = &entry["result"];
        let address: QuaiAddress = result["address"].as_str().unwrap().parse().unwrap();
        let account = verify_account_proof(root, address, &nodes(&result["accountProof"])).unwrap();
        match account {
            Some(account) => {
                assert_eq!(account.balance, quantity(&result["balance"]));
                assert_eq!(account.nonce, quantity(&result["nonce"]).to::<u64>());
                assert_eq!(account.code_hash, hash(&result["codeHash"]));
                assert_eq!(account.storage_root, hash(&result["storageHash"]));
            }
            None => {
                // go-quai reports an absent account with these values.
                assert_eq!(quantity(&result["balance"]), U256::ZERO);
                assert_eq!(hash(&result["codeHash"]), EMPTY_CODE_HASH);
                assert_eq!(hash(&result["storageHash"]), EMPTY_TRIE_ROOT);
            }
        }
        let storage_root = account.map_or(EMPTY_TRIE_ROOT, |a| a.storage_root);
        for slot in result["storageProof"].as_array().unwrap() {
            let value =
                verify_storage_proof(storage_root, hash(&slot["key"]), &nodes(&slot["proof"]))
                    .unwrap();
            assert_eq!(value, quantity(&slot["value"]), "slot {}", slot["key"]);
        }
        shapes.push((
            account.is_some(),
            account.is_some_and(|a| a.code_hash != EMPTY_CODE_HASH),
        ));
    }
    // A contract, an absent account and a funded account without code.
    assert_eq!(shapes, [(true, true), (false, false), (true, false)]);
}

#[test]
fn the_contract_proves_its_name_symbol_and_decimals() {
    let fixture = fixture();
    let result = &fixture["proofs"][0]["result"];
    let address = result["address"].as_str().unwrap().parse().unwrap();
    let account = verify_account_proof(
        state_root(&fixture),
        address,
        &nodes(&result["accountProof"]),
    )
    .unwrap()
    .unwrap();
    let slots = result["storageProof"].as_array().unwrap();
    let value = |i: usize| {
        verify_storage_proof(
            account.storage_root,
            hash(&slots[i]["key"]),
            &nodes(&slots[i]["proof"]),
        )
        .unwrap()
    };
    // Solidity stores a short string left-aligned, with twice its length in the last byte.
    let short_string = |word: U256| {
        let bytes = word.to_be_bytes::<32>();
        String::from_utf8(bytes[..usize::from(bytes[31] / 2)].to_vec()).unwrap()
    };
    assert_eq!(short_string(value(0)), "Wrapped Quai");
    assert_eq!(short_string(value(1)), "WQUAI");
    assert_eq!(value(2), U256::from(18));
    // A key the contract never wrote proves zero by absence.
    assert_eq!(value(4), U256::ZERO);
}

#[test]
fn any_changed_byte_key_or_root_fails() {
    let fixture = fixture();
    let root = state_root(&fixture);
    let result = &fixture["proofs"][0]["result"];
    let address: QuaiAddress = result["address"].as_str().unwrap().parse().unwrap();
    let proof = nodes(&result["accountProof"]);
    for node in 0..proof.len() {
        for at in [0, proof[node].len() / 2, proof[node].len() - 1] {
            let mut changed = proof.clone();
            changed[node][at] ^= 0x01;
            assert!(
                verify_account_proof(root, address, &changed).is_err(),
                "node {node} byte {at}"
            );
        }
    }
    let mut other = *address.bytes();
    other[19] ^= 1;
    if let Ok(other) = QuaiAddress::try_from(quai_primitives::Address::from_bytes(other)) {
        // Another key walks a different path; this proof cannot show its state.
        assert!(verify_account_proof(root, other, &proof).map_or(true, |a| a.is_none()));
    }
    let mut wrong_root = *root.bytes();
    wrong_root[0] ^= 1;
    assert_eq!(
        verify_account_proof(Hash32::from_bytes(wrong_root), address, &proof),
        Err(ProofError::HashMismatch)
    );
    assert_eq!(
        verify_account_proof(root, address, &proof[..proof.len() - 1]),
        Err(ProofError::Incomplete)
    );
    let mut extra = proof.clone();
    extra.push(proof[0].clone());
    assert_eq!(
        verify_account_proof(root, address, &extra),
        Err(ProofError::UnusedNodes)
    );
    let mut oversized = proof.clone();
    oversized[0] = vec![0; 1025];
    assert_eq!(
        verify_account_proof(root, address, &oversized),
        Err(ProofError::TooLarge)
    );
}

mod provider {
    use super::*;
    use quai_primitives::{ErrorClass, Zone};
    use quai_provider::state_proof::{EMPTY_CODE_HASH, verify_account_proof};
    use quai_provider::{BlockTag, MAX_PROVEN_ACCOUNTS, MAX_PROVEN_SLOTS, Provider, ProviderError};
    use quai_rpc::{BatchResult, Endpoint, Routing, RpcError, Transport};
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Copy, PartialEq)]
    enum Mode {
        Honest,
        /// The header's canonical hash at its height differs in round two.
        Reorg,
        /// The node reports a balance its proof does not show.
        InflatedBalance,
        /// One proof node is altered.
        AlteredNode,
        /// A storage value is reported that its proof does not show.
        AlteredSlotValue,
        /// The header carries no `evmRoot`.
        NoStateRoot,
    }
    /// Each batch as sent: its methods and parameters.
    type Sent = Vec<Vec<(String, Value)>>;
    #[derive(Clone)]
    struct Mock(Arc<Mutex<(Mode, Sent)>>, Arc<Value>);
    impl Mock {
        fn new(mode: Mode) -> Self {
            Self(
                Arc::new(Mutex::new((mode, Vec::new()))),
                Arc::new(fixture()),
            )
        }
        fn provider(&self) -> Provider<Self> {
            Provider::new(
                self.clone(),
                Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into()).unwrap(),
                U256::from(9),
            )
        }
        fn batches(&self) -> Sent {
            self.0.lock().unwrap().1.clone()
        }
    }
    impl Transport for Mock {
        async fn request(&self, _: &Endpoint, method: &str, _: Value) -> Result<Value, RpcError> {
            panic!("a batching transport received a single {method} request")
        }
        async fn request_batch(
            &self,
            _: &Endpoint,
            requests: Vec<(&str, Value)>,
        ) -> Option<BatchResult> {
            let fixture = &self.1;
            let mut state = self.0.lock().unwrap();
            let (mode, round) = (state.0, state.1.len());
            state.1.push(
                requests
                    .iter()
                    .map(|(m, p)| ((*m).to_owned(), p.clone()))
                    .collect(),
            );
            let header_hash = fixture["header"]["woHeader"]["hash"].clone();
            Some(Ok(requests
                .into_iter()
                .map(|(method, params)| match method {
                    "quai_chainId" => Ok(json!("0x9")),
                    "quai_getHeaderByNumber" if params[0] == "0x0" => {
                        Ok(fixture["genesis"].clone())
                    }
                    "quai_getHeaderByNumber" => {
                        let mut header = fixture["header"].clone();
                        if mode == Mode::Reorg && round == 1 {
                            header["woHeader"]["hash"] = json!(format!("0x{}", "99".repeat(32)));
                        }
                        if mode == Mode::NoStateRoot {
                            header.as_object_mut().unwrap().remove("evmRoot");
                        }
                        Ok(header)
                    }
                    "quai_getProof" => {
                        assert_eq!(params[2], json!({"blockHash": header_hash}));
                        let entry = fixture["proofs"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|e| {
                                e["request"]["params"][0]
                                    .as_str()
                                    .unwrap()
                                    .eq_ignore_ascii_case(params[0].as_str().unwrap())
                            })
                            .expect("a captured address");
                        assert_eq!(params[1], entry["request"]["params"][1]);
                        let mut result = entry["result"].clone();
                        match mode {
                            Mode::InflatedBalance => result["balance"] = json!("0x1"),
                            Mode::AlteredNode => {
                                let node = result["accountProof"][2].as_str().unwrap().to_owned();
                                let flipped = if node.ends_with('0') { '1' } else { '0' };
                                result["accountProof"][2] =
                                    json!(format!("{}{flipped}", &node[..node.len() - 1]));
                            }
                            Mode::AlteredSlotValue if !result["storageProof"][0].is_null() => {
                                result["storageProof"][0]["value"] = json!("0x2a")
                            }
                            _ => (),
                        }
                        Ok(result)
                    }
                    _ => panic!("unexpected RPC method {method}"),
                })
                .collect()))
        }
    }

    fn genesis() -> Hash32 {
        hash(&fixture()["genesis"]["woHeader"]["hash"])
    }
    /// The captured requests: each address and the slots asked for it.
    fn requests() -> Vec<(QuaiAddress, Vec<Hash32>)> {
        fixture()["proofs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                let params = &e["request"]["params"];
                (
                    params[0].as_str().unwrap().parse().unwrap(),
                    params[1].as_array().unwrap().iter().map(hash).collect(),
                )
            })
            .collect()
    }
    async fn prove(mock: &Mock) -> Result<Vec<quai_provider::ProvenAccount>, ProviderError> {
        let requests = requests();
        let targets = requests
            .iter()
            .map(|(address, slots)| (*address, slots.as_slice()))
            .collect::<Vec<_>>();
        mock.provider()
            .prove_accounts(genesis(), &targets, BlockTag::Latest)
            .await
    }

    #[tokio::test]
    async fn accounts_are_proven_in_two_rounds_and_the_first_names_no_address() {
        let mock = Mock::new(Mode::Honest);
        let proven = prove(&mock).await.unwrap();
        let fixture = fixture();
        let header = &fixture["header"];
        assert_eq!(proven.len(), 3);
        for account in &proven {
            assert_eq!(account.genesis, genesis());
            assert_eq!(account.block.hash, hash(&header["woHeader"]["hash"]));
            assert_eq!(account.state_root, hash(&header["evmRoot"]));
            // A stored proof verifies again without the node.
            assert_eq!(
                verify_account_proof(account.state_root, account.address, &account.account_proof)
                    .unwrap(),
                account.account
            );
        }
        let (contract, absent, funded) = (&proven[0], &proven[1], &proven[2]);
        assert!(contract.has_code());
        assert_eq!(
            contract.storage_value(requests()[0].1[2]),
            Some(U256::from(18))
        );
        assert_eq!(absent.account, None);
        assert_eq!(
            (absent.balance(), absent.nonce(), absent.code_hash()),
            (U256::ZERO, 0, EMPTY_CODE_HASH)
        );
        assert_eq!(absent.storage[0].value, U256::ZERO);
        assert!(!funded.has_code());
        assert!(funded.balance() > U256::ZERO && funded.nonce() > 0);

        let batches = mock.batches();
        assert_eq!(batches.len(), 2);
        let first = serde_json::to_string(&batches[0]).unwrap().to_lowercase();
        for (address, _) in requests() {
            assert!(!first.contains(&address.to_string().to_lowercase()[4..]));
        }
        let methods = batches[1]
            .iter()
            .map(|(m, _)| m.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            methods,
            [
                "quai_chainId",
                "quai_getProof",
                "quai_getProof",
                "quai_getProof",
                "quai_getHeaderByNumber",
                "quai_getHeaderByNumber"
            ]
        );
    }

    #[tokio::test]
    async fn a_wrong_network_is_refused_before_any_address_is_sent() {
        let mock = Mock::new(Mode::Honest);
        let address = requests()[0].0;
        let result = mock
            .provider()
            .prove_accounts(
                Hash32::from_bytes([7; 32]),
                &[(address, &[])],
                BlockTag::Latest,
            )
            .await;
        assert!(matches!(result, Err(ProviderError::GenesisMismatch)));
        assert_eq!(mock.batches().len(), 1);
    }

    #[tokio::test]
    async fn a_lying_node_yields_an_error_never_a_value() {
        for (mode, expected) in [
            (Mode::InflatedBalance, ProofError::Contradicted),
            (Mode::AlteredNode, ProofError::HashMismatch),
            (Mode::AlteredSlotValue, ProofError::Contradicted),
        ] {
            let error = prove(&Mock::new(mode)).await.unwrap_err();
            assert!(
                matches!(&error, ProviderError::Proof(e) if *e == expected),
                "{error:?}"
            );
            assert_eq!(error.class(), ErrorClass::Invalid);
        }
        assert!(matches!(
            prove(&Mock::new(Mode::Reorg)).await,
            Err(ProviderError::ObservationChanged)
        ));
        assert!(matches!(
            prove(&Mock::new(Mode::NoStateRoot)).await,
            Err(ProviderError::InvalidResult(_))
        ));
    }

    #[tokio::test]
    async fn invalid_requests_send_nothing() {
        let mock = Mock::new(Mode::Honest);
        let provider = mock.provider();
        let address = requests()[0].0;
        let cyprus2: QuaiAddress = "0x0100000000000000000000000000000000000001"
            .parse()
            .unwrap();
        let slots = vec![Hash32::ZERO; MAX_PROVEN_SLOTS + 1];
        let many = vec![(address, &[][..]); MAX_PROVEN_ACCOUNTS + 1];
        for (genesis, targets, block) in [
            (Hash32::ZERO, vec![(address, &[][..])], BlockTag::Latest),
            (genesis(), vec![], BlockTag::Latest),
            (genesis(), many, BlockTag::Latest),
            (genesis(), vec![(address, &slots[..])], BlockTag::Latest),
            (
                genesis(),
                vec![(address, &[][..]), (cyprus2, &[][..])],
                BlockTag::Latest,
            ),
            (genesis(), vec![(address, &[][..])], BlockTag::Pending),
        ] {
            assert!(matches!(
                provider.prove_accounts(genesis, &targets, block).await,
                Err(ProviderError::InvalidRequest(_))
            ));
        }
        assert!(mock.batches().is_empty());
    }
}
