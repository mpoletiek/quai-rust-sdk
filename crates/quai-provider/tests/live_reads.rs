//! Opt-in read-only smoke test. Never enabled by ordinary test runs.
#![cfg(all(feature = "http", not(target_arch = "wasm32")))]
use quai_primitives::{Hash32, QiAddress, QuaiAddress, Zone};
use quai_provider::{BlockTag, CallRequest, LogFilter, LogRange, Provider};
use quai_rpc::{HttpConfig, HttpTransport, Routing, U256};

#[tokio::test]
#[ignore = "requires QUAI_RPC_URL and QUAI_EXPECTED_CHAIN_ID; read-only public-node access"]
async fn explicit_endpoint_wallet_reads() {
    let url = std::env::var("QUAI_RPC_URL").expect("set explicit public endpoint");
    let chain = std::env::var("QUAI_EXPECTED_CHAIN_ID").expect("set expected chain ID");
    let chain = U256::from_str_radix(&chain, 10).expect("decimal chain ID");
    let provider = Provider::new(
        HttpTransport::new(HttpConfig::default()).unwrap(),
        Routing::direct(&url, Zone::Cyprus1.into()).unwrap(),
        chain,
    );
    let address: QuaiAddress = "0x0000000000000000000000000000000000000000"
        .parse()
        .unwrap();
    let qi: QiAddress = "0x0080000000000000000000000000000000000000"
        .parse()
        .unwrap();
    let block = BlockTag::Number(provider.block_number(Zone::Cyprus1.into()).await.unwrap());
    assert_ne!(
        provider.genesis_hash(Zone::Cyprus1).await.unwrap(),
        Hash32::ZERO
    );
    provider.balance(address, block).await.unwrap();
    provider
        .transaction_count(address, BlockTag::Pending)
        .await
        .unwrap();
    provider.gas_price(Zone::Cyprus1).await.unwrap();
    provider.code(address, block).await.unwrap();
    provider
        .storage_at(address, U256::ZERO, block)
        .await
        .unwrap();
    provider.outpoints(qi).await.unwrap();
    // A nonexistent all-zero transaction preserves nullable lookup semantics.
    assert!(
        provider
            .transaction(Zone::Cyprus1, Hash32::ZERO)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        provider
            .receipt(Zone::Cyprus1, Hash32::ZERO)
            .await
            .unwrap()
            .is_none()
    );
    let mut request = CallRequest::new(address, address);
    request.gas = Some(50_000);
    request.gas_price = Some(U256::ZERO);
    provider.call(&request, block).await.unwrap();
    provider.estimate_gas(&request, block).await.unwrap();
    let BlockTag::Number(height) = block else {
        unreachable!()
    };
    let height = u64::try_from(height).unwrap();
    provider
        .logs(&LogFilter {
            zone: Zone::Cyprus1,
            range: LogRange::Inclusive {
                from: height,
                to: height,
            },
            addresses: vec![address],
            topics: vec![],
        })
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires explicit endpoint/chain; advisory fee estimate only, no send"]
async fn explicit_endpoint_ordinary_qi_fee() {
    let url = std::env::var("QUAI_RPC_URL").expect("set explicit public endpoint");
    let chain = U256::from_str_radix(
        &std::env::var("QUAI_EXPECTED_CHAIN_ID").expect("set decimal chain"),
        10,
    )
    .unwrap();
    let fixtures: serde_json::Value = serde_json::from_str(include_str!(
        "../../../compatibility/fixtures/transactions.json"
    ))
    .unwrap();
    let v = fixtures["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == "qi-multi-0")
        .unwrap();
    let raw: quai_provider::RpcData = v["unsigned"].as_str().unwrap().parse().unwrap();
    let mut tx = quai_consensus::QiTransaction::decode_unsigned(raw.bytes()).unwrap();
    tx.chain_id = chain;
    let provider = Provider::new(
        HttpTransport::new(HttpConfig::default()).unwrap(),
        Routing::direct(&url, Zone::Cyprus1.into()).unwrap(),
        chain,
    );
    let quote = provider.estimate_qi_fee(&tx).await.unwrap();
    assert!(quote > U256::ZERO);
    println!(
        "read-only ordinary Qi quote: {quote} Qits on chain {chain}; fixture inputs are not spendability evidence"
    );
}
