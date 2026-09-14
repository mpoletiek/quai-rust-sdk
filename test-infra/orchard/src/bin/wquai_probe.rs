use quai_sdk::abi::AbiInterface;
use quai_sdk::contracts::Contract;
use quai_sdk::{BlockTag, HttpConfig, HttpTransport, Provider, QuaiAddress, Routing, U256, Zone};
use serde_json::json;
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let provider = Provider::new(
        HttpTransport::new(HttpConfig::default())?,
        Routing::direct(
            "https://orchard.rpc.quai.network/cyprus1",
            Zone::Cyprus1.into(),
        )?,
        U256::from(15000),
    );
    let genesis = "0x663a73416275109a01aad3a4c29ea9e310aded63c5eea491243b7312ad8cd16b".parse()?;
    if provider.chain_id(Zone::Cyprus1.into()).await? != U256::from(15000)
        || provider.genesis_hash(Zone::Cyprus1).await? != genesis
    {
        return Err("Orchard identity mismatch".into());
    }
    let header = provider
        .latest_header(Zone::Cyprus1)
        .await?
        .ok_or("missing head")?;
    let block = BlockTag::Number(U256::from(header.number));
    let address: QuaiAddress = quai_sdk::wrappers::WQUAI_ORCHARD_ADDRESS.parse()?;
    let from: QuaiAddress = "0x0006506bDE7140b85DED58a40D7444F84cde4821".parse()?;
    let code = provider.code(address, block).await?;
    let (wrapper, _) =
        quai_sdk::wrappers::WrappedQuai::new_verified(address, &provider, genesis, None, block)
            .await?;
    let abi=AbiInterface::from_json(br#"[
 {"type":"function","name":"name","stateMutability":"view","inputs":[],"outputs":[{"type":"string"}]},
 {"type":"function","name":"symbol","stateMutability":"view","inputs":[],"outputs":[{"type":"string"}]},
 {"type":"function","name":"decimals","stateMutability":"view","inputs":[],"outputs":[{"type":"uint8"}]}
 ]"#)?;
    let contract = Contract::new(address, abi, &provider);
    let mut metadata = serde_json::Map::new();
    for name in ["name", "symbol", "decimals"] {
        metadata.insert(
            name.into(),
            json!(contract.call(from, name, &[], block).await?),
        );
    }
    let token_balance = wrapper.token()?.balance_of(from, from, block).await?;
    println!(
        "{}",
        json!({"network":"orchard","chainId":15000,"genesis":genesis.to_string(),"blockNumber":header.number,"blockHash":header.hash.to_string(),"address":address.to_string(),"codeBytes":code.bytes().len(),"codeSha256":quai_sdk::primitives::Hash32::from_bytes(quai_sdk::crypto::sha256(code.bytes())).to_string(),"metadata":metadata,"account":from.to_string(),"tokenBalance":token_balance.to_string()})
    );
    Ok(())
}
