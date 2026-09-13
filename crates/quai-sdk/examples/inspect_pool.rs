//! Explicit read-only node diagnostics and zero-value access-list simulation.
use quai_sdk::provider::CallRequest;
use quai_sdk::{BlockTag, HttpConfig, HttpTransport, Provider, Routing, U256, Zone};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("QUAI_RPC_URL")?;
    let chain = U256::from_str_radix(&std::env::var("QUAI_EXPECTED_CHAIN_ID")?, 10)?;
    let provider = Provider::new(
        HttpTransport::new(HttpConfig::default())?,
        Routing::direct(&url, Zone::Cyprus1.into())?,
        chain,
    );
    let height = provider.block_number(Zone::Cyprus1.into()).await?;
    let expansion = provider.protocol_expansion(Zone::Cyprus1.into()).await?;
    let status = provider.pool_status(Zone::Cyprus1).await?;
    let content = provider.pool_content(Zone::Cyprus1, 512).await?;
    let inspected = provider.pool_inspect(Zone::Cyprus1, 512).await?;
    let pending = provider.pending_header_bytes(Zone::Cyprus1).await?;
    let address = "0x0000000000000000000000000000000000000000".parse()?;
    let mut call = CallRequest::new(address, address);
    call.gas = Some(100_000);
    call.gas_price = Some(U256::ZERO);
    call.value = Some(U256::ZERO);
    let access = provider
        .create_access_list(&call, BlockTag::Number(height))
        .await?;
    println!(
        "{}",
        serde_json::json!({"chain":chain.to_string(),"height":height.to_string(),"protocol_expansion":expansion,"pool":{"pending":status.pending,"queued":status.queued,"qi":status.qi,"decoded_accounts":content.len(),"inspection_entries":inspected.len()},"pending_header_bytes":pending.bytes().len(),"access_list":{"entries":access.access_list.len(),"gas_used":access.gas_used},"submitted":false})
    );
    Ok(())
}
