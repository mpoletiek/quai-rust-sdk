//! Read-only mainnet Cyprus-1 head soak: WebSocket wake-ups plus canonical replay.
//! Logs reorgs, errors and follower rebuilds as JSON lines. Holds no keys.
use quai_sdk::provider::{BlockReference, HeadFollowPolicy, HeadTracker, WsHeadFollower};
use quai_sdk::{
    Endpoint, HttpConfig, HttpTransport, Provider, ProviderError, Routing, U256, WsConfig, Zone,
};
use serde_json::json;
use std::{
    error::Error,
    fs::OpenOptions,
    io::Write,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const GENESIS: &str = "0xac81c28f1a72591b87b5f16c9793cdc0e87c45c6d426d1a364c3b8f6386b5b8b";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

struct Log(std::fs::File);
impl Log {
    fn event(&mut self, mut value: serde_json::Value) {
        value["unixTime"] = json!(now());
        let _ = writeln!(self.0, "{value}");
        let _ = self.0.flush();
    }
}

fn provider() -> Result<Provider<HttpTransport>, Box<dyn Error>> {
    Ok(Provider::new(
        HttpTransport::new(HttpConfig::default())?,
        Routing::direct("https://rpc.quai.network/cyprus1", Zone::Cyprus1.into())?,
        U256::from(9),
    ))
}

async fn anchor(zone: Zone) -> Result<HeadTracker, Box<dyn Error>> {
    let header = provider()?
        .latest_header(zone)
        .await?
        .ok_or("missing latest header")?;
    Ok(HeadTracker::new(
        zone,
        GENESIS.parse()?,
        BlockReference {
            number: header.number,
            hash: header.hash,
        },
        4096,
        64,
    )?)
}

fn follower(tracker: HeadTracker) -> Result<WsHeadFollower<HttpTransport>, Box<dyn Error>> {
    Ok(WsHeadFollower::new(
        provider()?,
        tracker,
        Endpoint::parse("wss://rpc.quai.network/cyprus1")?,
        WsConfig::default(),
        HeadFollowPolicy {
            max_connect_attempts: 8,
            retry_delay: Duration::from_secs(5),
            idle_poll_interval: Duration::from_secs(30),
        },
    )?)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let home = std::env::var_os("HOME").ok_or("HOME required")?;
    let dir = PathBuf::from(home).join(".local/share/quai-sdk-mainnet-qualification/soak");
    std::fs::create_dir_all(&dir)?;
    let mut log = Log(OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("head-soak.jsonl"))?);
    let zone = Zone::Cyprus1;
    let identity = provider()?;
    if identity.chain_id(zone.into()).await? != U256::from(9)
        || identity.genesis_hash(zone).await?.to_string() != GENESIS
    {
        return Err("mainnet identity mismatch".into());
    }
    let mut tracker = anchor(zone).await?;
    let mut active = follower(tracker.clone())?;
    let started = Instant::now();
    let (mut blocks, mut reorgs, mut removed_total, mut max_depth) = (0u64, 0u64, 0u64, 0usize);
    let (mut errors, mut rebuilds, mut reanchors, mut changed) = (0u64, 0u64, 0u64, 0u64);
    let mut heartbeat = Instant::now();
    log.event(json!({"event":"start","checkpoint":tracker.checkpoint().number,"hash":tracker.checkpoint().hash.to_string()}));
    loop {
        match active.next().await {
            Ok(update) => {
                blocks += update.added.len() as u64;
                if !update.removed.is_empty() {
                    reorgs += 1;
                    removed_total += update.removed.len() as u64;
                    max_depth = max_depth.max(update.removed.len());
                    log.event(json!({"event":"reorg","depth":update.removed.len(),
                        "removed":update.removed.iter().map(|r|json!({"number":r.number,"hash":r.hash.to_string()})).collect::<Vec<_>>(),
                        "added":update.added.iter().map(|h|json!({"number":h.number,"hash":h.hash.to_string(),"parent":h.parent_hash.to_string()})).collect::<Vec<_>>(),
                        "checkpoint":update.checkpoint.number}));
                }
                tracker = active.tracker().clone();
            }
            Err(ProviderError::ObservationChanged) => {
                // Tip churn or inconsistent load-balanced reads during a page; the
                // tracker is unchanged and the subscription is kept. Retry in place.
                changed += 1;
                if changed <= 5 || changed.is_power_of_two() {
                    log.event(json!({"event":"observation-changed","count":changed,"checkpoint":tracker.checkpoint().number}));
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Err(error) => {
                errors += 1;
                let deep = matches!(error, ProviderError::ReplayHistoryUnavailable);
                log.event(json!({"event":"error","deepHistory":deep,"error":error.to_string(),"checkpoint":tracker.checkpoint().number}));
                tokio::time::sleep(Duration::from_secs(10)).await;
                if deep {
                    // A reorg beyond retained anchors or unavailable history: record, then re-anchor.
                    match anchor(zone).await {
                        Ok(fresh) => {
                            reanchors += 1;
                            tracker = fresh;
                        }
                        Err(e) => {
                            log.event(json!({"event":"reanchor-failed","error":e.to_string()}))
                        }
                    }
                }
                match follower(tracker.clone()) {
                    Ok(f) => {
                        rebuilds += 1;
                        active = f;
                    }
                    Err(e) => log.event(json!({"event":"rebuild-failed","error":e.to_string()})),
                }
            }
        }
        if heartbeat.elapsed() >= Duration::from_secs(300) {
            heartbeat = Instant::now();
            let http_head = identity
                .block_number(zone.into())
                .await
                .map(|n| n.to_string())
                .unwrap_or_else(|e| format!("error: {e}"));
            log.event(json!({"event":"heartbeat","uptimeSeconds":started.elapsed().as_secs(),"checkpoint":tracker.checkpoint().number,
                "httpHead":http_head,"blocks":blocks,"reorgs":reorgs,"removedBlocks":removed_total,"maxDepth":max_depth,
                "errors":errors,"rebuilds":rebuilds,"reanchors":reanchors,"observationChanged":changed}));
        }
    }
}
