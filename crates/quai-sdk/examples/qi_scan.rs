//! Public-only current Qi scan. Does not load a seed, sign, or submit transactions.
use quai_sdk::qi_discovery::{QiScanOptions, scan_and_refresh_qi};
use quai_sdk::wallet::storage::{NetworkScope, SqliteStore};
use quai_sdk::wallet::{AccountPublic, CoinType};
use quai_sdk::{HttpConfig, HttpTransport, Provider, Routing, U256, Zone, parse_use_pathing};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 7 {
        return Err("usage: qi_scan URL true|false DECIMAL_CHAIN_ID GENESIS_HASH DATABASE ACCOUNT_XPUB ACCOUNT_INDEX (Cyprus-1)".into());
    }
    let scope = NetworkScope {
        chain_id: U256::from_str_radix(&args[2], 10)?,
        genesis: args[3].parse()?,
        zone: Zone::Cyprus1,
    };
    let account = AccountPublic::import(&args[5], CoinType::Qi, args[6].parse()?)?;
    let provider = Provider::new(
        HttpTransport::new(HttpConfig::default())?,
        Routing::with_pathing(&args[0], scope.zone.into(), parse_use_pathing(&args[1])?)?,
        scope.chain_id,
    );
    let mut store = SqliteStore::open(&args[4], scope)?;
    let report = scan_and_refresh_qi(
        &provider,
        &mut store,
        &account,
        &QiScanOptions::default(),
        || false,
    )
    .await?;
    let snapshot = store.snapshot()?;
    println!(
        "Current gap scan: {:?}; next raw children {:?}; {} current outpoints",
        report.stopped,
        report.next_index,
        snapshot.coins.len()
    );
    println!(
        "This latest-state scan cannot establish fully spent address history. Use explicit ranges with gap_limit=None for deeper recovery."
    );
    Ok(())
}
