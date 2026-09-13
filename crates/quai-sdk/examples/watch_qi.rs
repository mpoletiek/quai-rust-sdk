//! Read-only account-xpub Qi discovery. Never unlocks a key or submits a transaction.
use quai_sdk::discovery::{QiDiscoveryOptions, discover_qi};
use quai_sdk::wallet::discovery::NetworkScope;
use quai_sdk::wallet::{AccountPublic, CoinType};
use quai_sdk::{HttpConfig, HttpTransport, Provider, Routing, U256, Zone, parse_quantity};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 7 {
        return Err(
            "usage: watch_qi RPC_URL CHAIN_ID_HEX GENESIS_HASH ZONE ACCOUNT_INDEX ACCOUNT_XPUB"
                .into(),
        );
    }
    let zone: Zone = args[4].parse().map_err(|_| "invalid zone")?;
    let scope = NetworkScope {
        chain_id: parse_quantity(&args[2])?,
        genesis: args[3].parse().map_err(|_| "invalid genesis hash")?,
        zone,
    };
    let account = AccountPublic::import(
        &args[6],
        CoinType::Qi,
        args[5].parse().map_err(|_| "invalid account index")?,
    )?;
    let provider = Provider::new(
        HttpTransport::new(HttpConfig::default())?,
        Routing::direct(&args[1], zone.into())?,
        scope.chain_id,
    );
    let report = discover_qi(
        &provider,
        scope,
        &account,
        &QiDiscoveryOptions::default(),
        || false,
    )
    .await?;
    let candidate_height = report
        .checkpoint
        .height
        .checked_add(U256::from(1))
        .ok_or("height overflow")?;
    let balance = report.balance_at(candidate_height)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "head":report.checkpoint.height.to_string(),"block_hash":report.checkpoint.hash.to_string(),"chain_id":scope.chain_id.to_string(),"genesis":scope.genesis.to_string(),"candidate_height":candidate_height.to_string(),
            "addresses":report.addresses.len(),"outputs":report.addresses.iter().map(|a|a.outputs.len()).sum::<usize>(),
            "total_qits":balance.total.to_string(),"locked_qits":balance.locked.to_string(),"unlocked_qits":balance.unlocked.to_string(),
            "next_index":report.next_index,"stopped":report.stopped.map(|s|format!("{s:?}")),
            "history":"current outpoints only; empty addresses can have spent history","submitted":false
        }))?
    );
    Ok(())
}
