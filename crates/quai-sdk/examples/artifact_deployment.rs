//! Prepare a public-fixture offline deployment from a bounded single-contract artifact.
//! This example neither signs nor submits. Use application-selected sender/nonce in real wallets.
use quai_sdk::{
    U256,
    abi::SolidityArtifact,
    contracts::{DeploymentSearch, prepare_deployment},
};
use serde_json::{Value, json};
use std::io::Read;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let path = args
        .next()
        .ok_or("usage: artifact_deployment ARTIFACT.json [CONSTRUCTOR_ARGUMENTS_JSON]")?;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take((quai_sdk::abi::MAX_DATA_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    let artifact = SolidityArtifact::from_json(&bytes)?;
    let arguments = args
        .next()
        .map(|s| s.into_string().map_err(|_| "arguments must be UTF-8"))
        .transpose()?
        .unwrap_or_else(|| "[]".into());
    if arguments.len() > quai_sdk::abi::MAX_DATA_BYTES {
        return Err("argument JSON exceeds limit".into());
    }
    let arguments: Vec<Value> = serde_json::from_str(&arguments)?;
    let prepared = prepare_deployment(
        artifact.interface(),
        artifact.init_code(),
        &arguments,
        "0x0049CdA3305ccB9cB23E7Ce2528ceF555E9a5B32".parse()?,
        U256::from(1337),
        1,
        U256::ZERO,
        DeploymentSearch::new(0, 10_000),
        || false,
    )?;
    println!(
        "{}",
        json!({"contract":prepared.address().to_string(),"salt":prepared.salt(),"attempts":prepared.attempts(),"init_data_bytes":prepared.init_data().len(),"signed":false,"submitted":false})
    );
    Ok(())
}
