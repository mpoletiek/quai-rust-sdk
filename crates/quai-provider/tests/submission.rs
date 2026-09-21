//! Offline signed broadcast regressions; no actual transaction is submitted.
use quai_consensus::SignedQuaiTransaction;
use quai_primitives::{Hash32, Zone};
use quai_provider::{BroadcastError, Provider, RpcData};
use quai_rpc::{Endpoint, Routing, RpcError, Transport, U256};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

const URL: &str = "http://127.0.0.1:9200/exact?test=public";
type Call = (String, Value, Result<Value, RpcError>);
#[derive(Clone, Default)]
struct Mock(Arc<Mutex<VecDeque<Call>>>);
impl Mock {
    fn provider(&self, chain: u64) -> Provider<Self> {
        Provider::new(
            self.clone(),
            Routing::direct(URL, Zone::Cyprus1.into()).unwrap(),
            U256::from(chain),
        )
    }
    fn scripted(signed: &SignedQuaiTransaction, result: Result<Value, RpcError>) -> Self {
        Self(Arc::new(Mutex::new(VecDeque::from([
            ("quai_chainId".into(), json!([]), Ok(json!("0x9"))),
            (
                "quai_sendRawTransaction".into(),
                json!([RpcData::new(signed.signed_bytes().unwrap())
                    .unwrap()
                    .to_hex()]),
                result,
            ),
        ]))))
    }
    fn drained(&self) {
        assert!(self.0.lock().unwrap().is_empty());
    }
}
impl Transport for Mock {
    async fn request(
        &self,
        endpoint: &Endpoint,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        assert_eq!(endpoint.as_str(), URL);
        let (expected, args, result) = self
            .0
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected request or unsafe retry");
        assert_eq!(method, expected);
        assert_eq!(params, args);
        result
    }
}
fn signed() -> SignedQuaiTransaction {
    let vectors: Value = serde_json::from_str(include_str!(
        "fixtures/shared/compatibility/fixtures/transactions.json"
    ))
    .unwrap();
    let vector = vectors["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["kind"] == "quai" && v["input"]["chainId"] == "9")
        .unwrap();
    let bytes: RpcData = vector["signed"].as_str().unwrap().parse().unwrap();
    SignedQuaiTransaction::decode(bytes.bytes()).unwrap()
}

#[tokio::test]
async fn signed_chain_mismatch_fails_before_any_rpc() {
    let signed = signed();
    let mock = Mock::default();
    let error = mock.provider(15000).broadcast(&signed).await.unwrap_err();
    assert!(matches!(error, BroadcastError::Preflight(_)));
    assert!(!error.acceptance_is_ambiguous());
    mock.drained();
}
#[tokio::test]
async fn successful_submit_sends_exact_protobuf_once_and_matches_local_id() {
    let signed = signed();
    let hash = signed.hash().unwrap();
    let mock = Mock::scripted(&signed, Ok(json!(hash.to_string())));
    let result = mock.provider(9).broadcast(&signed).await.unwrap();
    assert_eq!(result.transaction_hash, hash);
    assert_eq!(result.zone, signed.from().zone());
    mock.drained();
}
#[tokio::test]
async fn node_chain_mismatch_does_not_submit() {
    let signed = signed();
    let mock = Mock(Arc::new(Mutex::new(VecDeque::from([(
        "quai_chainId".into(),
        json!([]),
        Ok(json!("0x3a98")),
    )]))));
    assert!(matches!(
        mock.provider(9).broadcast(&signed).await,
        Err(BroadcastError::Preflight(_))
    ));
    mock.drained();
}
#[tokio::test]
async fn send_errors_retain_expected_id_and_never_retry() {
    for error in [
        RpcError::Timeout,
        RpcError::Transport,
        RpcError::HttpStatus(503),
        RpcError::InvalidResponse("wrong ID"),
    ] {
        let signed = signed();
        let hash = signed.hash().unwrap();
        let mock = Mock::scripted(&signed, Err(error));
        let error = mock.provider(9).broadcast(&signed).await.unwrap_err();
        assert!(error.acceptance_is_ambiguous());
        assert_eq!(error.transaction_hash(), Some(hash));
        mock.drained();
    }
}
#[tokio::test]
async fn malformed_or_conflicting_acknowledgements_remain_ambiguous() {
    for response in [Value::Null, json!("0x01"), json!(Hash32::ZERO.to_string())] {
        let signed = signed();
        let expected = signed.hash().unwrap();
        let mock = Mock::scripted(&signed, Ok(response));
        let error = mock.provider(9).broadcast(&signed).await.unwrap_err();
        assert!(matches!(
            error,
            BroadcastError::InvalidAcknowledgement { .. }
        ));
        assert!(error.acceptance_is_ambiguous());
        assert_eq!(error.transaction_hash(), Some(expected));
        mock.drained();
    }
}

#[tokio::test]
async fn an_oversized_quai_transaction_is_refused_before_the_submit() {
    use quai_consensus::QuaiTransaction;
    use quai_crypto::SecretKey;

    // Signing binds the sender's zone and ledger, and only about one key in 512
    // yields a Cyprus-1 Quai address, so the fixture key is ground rather than
    // picked.
    let key = (1u64..)
        .find_map(|n| {
            let mut scalar = [0u8; 32];
            scalar[24..].copy_from_slice(&n.to_be_bytes());
            let key = SecretKey::from_bytes(&scalar).ok()?;
            let address = key.public_key().address();
            (address.zone().ok() == Some(Zone::Cyprus1)
                && address.ledger() == quai_primitives::Ledger::Quai)
                .then_some(key)
        })
        .expect("a Cyprus-1 Quai key exists");
    let base = |data: Vec<u8>| QuaiTransaction {
        chain_id: U256::from(9),
        nonce: 0,
        to: Some(key.public_key().address()),
        value: U256::ZERO,
        gas_limit: 21_000,
        gas_price: U256::from(1),
        data,
        access_list: vec![],
    };

    // The node's pool rejects a Quai transaction over `txMaxSize` with
    // ErrOversizedData before anything else, so this must fail locally and
    // unambiguously rather than after a round trip.
    let oversized = base(vec![0x11; quai_consensus::MAX_POOL_TRANSACTION_BYTES])
        .sign(&key)
        .unwrap();
    assert!(
        oversized.signed_bytes().unwrap().len() > quai_consensus::MAX_POOL_TRANSACTION_BYTES,
        "fixture must exceed the pool bound"
    );
    let mock = Mock::default();
    let error = mock.provider(9).broadcast(&oversized).await.unwrap_err();
    assert!(matches!(error, BroadcastError::Preflight(_)), "{error:?}");
    assert!(
        !error.acceptance_is_ambiguous(),
        "nothing was sent, so the outcome is not ambiguous"
    );
    mock.drained();

    // A transaction just inside the bound still goes out, so the check is a
    // bound and not a blanket refusal of large payloads.
    let accepted = base(vec![0x11; 1024]).sign(&key).unwrap();
    assert!(accepted.signed_bytes().unwrap().len() <= quai_consensus::MAX_POOL_TRANSACTION_BYTES);
    let hash = accepted.hash().unwrap();
    let mock = Mock::scripted(&accepted, Ok(json!(hash.to_string())));
    assert_eq!(
        mock.provider(9)
            .broadcast(&accepted)
            .await
            .unwrap()
            .transaction_hash,
        hash
    );
    mock.drained();
}
