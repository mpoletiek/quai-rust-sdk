//! Contract-code observation over a batching transport: two rounds, the first
//! address-free, with code pinned to the header's hash and rechecked anchors.
use quai_primitives::{Hash32, QuaiAddress, Zone};
use quai_provider::{BlockTag, MAX_CONTRACT_CODE_TARGETS, Provider, ProviderError};
use quai_rpc::{BatchResult, Endpoint, Routing, RpcError, Transport, U256};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

const A: &str = "0x0000000000000000000000000000000000000001";
const B: &str = "0x0000000000000000000000000000000000000002";
const C: &str = "0x0000000000000000000000000000000000000003";
const CYPRUS2: &str = "0x0100000000000000000000000000000000000001";

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Normal,
    /// Round one reports another network's genesis.
    WrongNetwork,
    /// The genesis read in round two differs from round one.
    GenesisMoves,
    /// The height's canonical header differs in round two.
    Reorg,
    /// The latest header is unavailable.
    NoHeader,
    /// Every code read fails while the anchors hold.
    CodeFails,
    /// Code reads fail and the height was also reorganized.
    CodeFailsInReorg,
}

/// Each batch as sent: its methods and parameters.
type Sent = Vec<Vec<(String, Value)>>;
#[derive(Clone)]
struct Mock(Arc<Mutex<(Mode, Sent)>>);
impl Mock {
    fn new(mode: Mode) -> Self {
        Self(Arc::new(Mutex::new((mode, Vec::new()))))
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
fn hash(n: u8) -> Hash32 {
    Hash32::from_bytes([n; 32])
}
fn genesis(n: u8) -> Value {
    json!({"woHeader":{"hash":hash(n).to_string(),"number":"0x0","location":"0x","parentHash":Hash32::ZERO.to_string()}})
}
fn header(n: u8) -> Value {
    json!({"gasLimit":"0x10000","stateLimit":"0x10000","woHeader":{"hash":hash(n).to_string(),"parentHash":hash(0).to_string(),"number":"0x10","primeTerminusNumber":"0x4","location":"0x0000"}})
}
fn code(address: &str) -> String {
    format!("0x60{}", &address[40..])
}
fn keccak(hex: &str) -> Hash32 {
    let bytes = (2..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect::<Vec<_>>();
    Hash32::from_bytes(quai_crypto::keccak256(&bytes))
}
fn address(text: &str) -> QuaiAddress {
    text.parse().unwrap()
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
        let mut state = self.0.lock().unwrap();
        let mode = state.0;
        let round = state.1.len();
        state.1.push(
            requests
                .iter()
                .map(|(method, params)| ((*method).to_owned(), params.clone()))
                .collect(),
        );
        let moved = round == 1;
        Some(Ok(requests
            .into_iter()
            .map(|(method, params)| match method {
                "quai_chainId" => Ok(json!("0x9")),
                "quai_getHeaderByNumber" if params[0] == "0x0" => Ok(genesis(match mode {
                    Mode::WrongNetwork => 9,
                    Mode::GenesisMoves if moved => 9,
                    _ => 1,
                })),
                "quai_getHeaderByNumber" => {
                    assert!(params[0] == "latest" || params[0] == "0x10");
                    Ok(match mode {
                        Mode::NoHeader => Value::Null,
                        Mode::Reorg | Mode::CodeFailsInReorg if moved => header(9),
                        _ => header(2),
                    })
                }
                "quai_getCode" => {
                    assert_eq!(params[1], json!({"blockHash": hash(2).to_string()}));
                    match mode {
                        Mode::CodeFails | Mode::CodeFailsInReorg => Err(RpcError::Transport),
                        _ => Ok(json!(code(params[0].as_str().unwrap()))),
                    }
                }
                _ => panic!("unexpected RPC method {method}"),
            })
            .collect()))
    }
}

#[tokio::test]
async fn several_contracts_take_two_rounds_and_the_first_names_no_address() {
    let mock = Mock::new(Mode::Normal);
    let targets = [
        (address(A), Some(keccak(&code(A)))),
        (address(B), None),
        (address(C), Some(hash(9))),
    ];
    let observations = mock
        .provider()
        .observe_contract_codes(hash(1), &targets, BlockTag::Latest)
        .await
        .unwrap();
    assert_eq!(observations.len(), 3);
    for (observation, (target, _)) in observations.iter().zip(&targets) {
        assert_eq!(observation.address, *target);
        assert_eq!(observation.genesis, hash(1));
        assert_eq!(observation.chain_id, U256::from(9));
        assert_eq!(
            (observation.block.number, observation.block.hash),
            (16, hash(2))
        );
        assert_eq!(
            observation.code.bytes.to_hex(),
            code(&target.to_string().to_lowercase())
        );
    }
    let matches = observations
        .iter()
        .map(|o| o.code.matches_expected)
        .collect::<Vec<_>>();
    assert_eq!(matches, [Some(true), None, Some(false)]);

    let batches = mock.batches();
    assert_eq!(batches.len(), 2);
    let methods = |batch: &[(String, Value)]| {
        batch
            .iter()
            .map(|(method, params)| format!("{method} {}", params[0]))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        methods(&batches[0]),
        [
            "quai_chainId null",
            "quai_getHeaderByNumber \"0x0\"",
            "quai_getHeaderByNumber \"latest\"",
        ]
    );
    let first_round = serde_json::to_string(&batches[0]).unwrap();
    for (target, _) in &targets {
        assert!(!first_round.contains(&target.to_string()[4..]));
    }
    assert_eq!(
        methods(&batches[1]),
        [
            "quai_chainId null".to_owned(),
            format!("quai_getCode \"{A}\""),
            format!("quai_getCode \"{B}\""),
            format!("quai_getCode \"{C}\""),
            "quai_getHeaderByNumber \"0x10\"".to_owned(),
            "quai_getHeaderByNumber \"0x0\"".to_owned(),
        ]
    );
}

#[tokio::test]
async fn a_single_observation_also_takes_two_rounds() {
    let mock = Mock::new(Mode::Normal);
    let observation = mock
        .provider()
        .observe_contract_code(address(A), BlockTag::Number(U256::from(16)), None)
        .await
        .unwrap();
    assert_eq!(observation.block.hash, hash(2));
    assert_eq!(observation.code.bytes.to_hex(), code(A));
    let batches = mock.batches();
    assert_eq!(batches.len(), 2);
    assert_eq!(batches[0][2].1, json!(["0x10"]));
}

#[tokio::test]
async fn a_wrong_network_is_refused_before_any_address_is_sent() {
    let mock = Mock::new(Mode::WrongNetwork);
    let result = mock
        .provider()
        .observe_contract_codes(hash(1), &[(address(A), None)], BlockTag::Latest)
        .await;
    assert!(matches!(result, Err(ProviderError::GenesisMismatch)));
    assert_eq!(mock.batches().len(), 1);

    // Without a trusted genesis there is nothing to refuse; the caller compares.
    let mock = Mock::new(Mode::WrongNetwork);
    let observation = mock
        .provider()
        .observe_contract_code(address(A), BlockTag::Latest, None)
        .await
        .unwrap();
    assert_eq!(observation.genesis, hash(9));
}

#[tokio::test]
async fn changed_anchors_outrank_code_failures_and_never_become_observations() {
    for (mode, rounds) in [
        (Mode::Reorg, 2),
        (Mode::GenesisMoves, 2),
        (Mode::NoHeader, 1),
        (Mode::CodeFailsInReorg, 2),
    ] {
        let mock = Mock::new(mode);
        let result = mock
            .provider()
            .observe_contract_codes(hash(1), &[(address(A), None)], BlockTag::Latest)
            .await;
        assert!(matches!(result, Err(ProviderError::ObservationChanged)));
        assert_eq!(mock.batches().len(), rounds);
    }
    let mock = Mock::new(Mode::CodeFails);
    let result = mock
        .provider()
        .observe_contract_codes(hash(1), &[(address(A), None)], BlockTag::Latest)
        .await;
    assert!(matches!(
        result,
        Err(ProviderError::Rpc(RpcError::Transport))
    ));
}

#[tokio::test]
async fn invalid_requests_send_nothing() {
    let mock = Mock::new(Mode::Normal);
    let provider = mock.provider();
    let one = [(address(A), None)];
    let too_many = vec![(address(A), None); MAX_CONTRACT_CODE_TARGETS + 1];
    let mixed = [(address(A), None), (address(CYPRUS2), None)];
    for (genesis, targets, block) in [
        (Hash32::ZERO, &one[..], BlockTag::Latest),
        (hash(1), &[][..], BlockTag::Latest),
        (hash(1), &too_many[..], BlockTag::Latest),
        (hash(1), &mixed[..], BlockTag::Latest),
        (hash(1), &one[..], BlockTag::Pending),
        (hash(1), &one[..], BlockTag::Number(U256::ZERO)),
    ] {
        assert!(matches!(
            provider
                .observe_contract_codes(genesis, targets, block)
                .await,
            Err(ProviderError::InvalidRequest(_))
        ));
    }
    assert!(mock.batches().is_empty());
    let most = vec![(address(A), None); MAX_CONTRACT_CODE_TARGETS];
    assert_eq!(
        provider
            .observe_contract_codes(hash(1), &most, BlockTag::Latest)
            .await
            .unwrap()
            .len(),
        MAX_CONTRACT_CODE_TARGETS
    );
    assert_eq!(
        mock.batches()[1].len(),
        quai_rpc::MAX_BATCH_CALLS,
        "the largest request fills one batch exactly"
    );
}
