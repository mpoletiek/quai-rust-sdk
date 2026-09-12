//! Read-only network check. No wallet or transaction submission.
use quai_sdk::{
    HttpConfig, HttpTransport, Provider, Routing, Shard, U256, Zone, parse_quantity,
    parse_use_pathing,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() && args.len() != 3 {
        return Err("usage: read_network [URL true|false EXPECTED_CHAIN_ID]; chain ID may be decimal or canonical hex".into());
    }
    let url = args
        .first()
        .map(String::as_str)
        .unwrap_or("https://orchard.rpc.quai.network");
    let pathing = parse_use_pathing(args.get(1).map(String::as_str).unwrap_or("true"))?;
    let chain = args.get(2).map(String::as_str).unwrap_or("15000");
    let expected = if chain.starts_with("0x") {
        parse_quantity(chain)?
    } else {
        U256::from_str_radix(chain, 10).map_err(|_| "invalid decimal chain ID")?
    };
    let shard = Shard::Zone(Zone::Cyprus1);
    let provider = Provider::new(
        HttpTransport::new(HttpConfig::default())?,
        Routing::with_pathing(url, shard, pathing)?,
        expected,
    );
    println!("chain ID: {}", provider.chain_id(shard).await?);
    println!(
        "{} block number: {}",
        shard,
        provider.block_number(shard).await?
    );
    Ok(())
}
