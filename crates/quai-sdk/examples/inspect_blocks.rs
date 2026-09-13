//! Read an exact block through full, hash-only and header RPC forms; never submits.
use quai_sdk::provider::MinedBlock;
use quai_sdk::{HttpConfig, HttpTransport, Provider, Routing, U256, Zone};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("QUAI_RPC_URL")?;
    let chain = U256::from_str_radix(&std::env::var("QUAI_EXPECTED_CHAIN_ID")?, 10)?;
    let provider = Provider::new(
        HttpTransport::new(HttpConfig::default())?,
        Routing::direct(&url, Zone::Cyprus1.into())?,
        chain,
    );
    let head = provider
        .latest_header(Zone::Cyprus1)
        .await?
        .ok_or("head unavailable")?;
    let block = provider
        .mined_block(Zone::Cyprus1, MinedBlock::Hash(head.hash), 4096)
        .await?
        .ok_or("block unavailable")?;
    let hashes = provider
        .block_hashes(Zone::Cyprus1, MinedBlock::Hash(head.hash), 4096)
        .await?
        .ok_or("block unavailable")?;
    let header = provider
        .header_by_hash(Zone::Cyprus1, head.hash)
        .await?
        .ok_or("header unavailable")?;
    if block.block != hashes.block
        || block.block.number != header.number
        || block.parent_hash != header.parent_hash
        || hashes.transactions
            != block
                .transactions
                .iter()
                .map(|t| t.hash)
                .collect::<Vec<_>>()
    {
        return Err("source block representations disagree".into());
    }
    let canonical = provider
        .header_at(Zone::Cyprus1, head.number)
        .await?
        .is_some_and(|h| h.hash == head.hash);
    println!(
        "{}",
        serde_json::json!({"chain":chain.to_string(),"height":head.number,"hash":head.hash.to_string(),"executed_transactions":block.transactions.len(),"representations_agree":true,"canonical_at_recheck":canonical,"submitted":false})
    );
    Ok(())
}
