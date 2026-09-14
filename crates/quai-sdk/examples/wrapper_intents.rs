//! Offline wrapper intents. Prints call data; never contacts or submits to a node.
use quai_sdk::wrappers::{WQI_ADDRESS, WQUAI_ORCHARD_ADDRESS, WrappedQi, WrappedQuai};
use quai_sdk::{HttpConfig, HttpTransport, Provider, Routing, U256, Zone};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let provider = Provider::new(
        HttpTransport::new(HttpConfig::default())?,
        Routing::direct("http://127.0.0.1:9200", Zone::Cyprus1.into())?,
        U256::from(15000),
    );
    let wquai = WrappedQuai::new(WQUAI_ORCHARD_ADDRESS.parse()?, &provider)?;
    let wqi = WrappedQi::new(WQI_ADDRESS.parse()?, &provider)?;
    let its = U256::from(1_000_000_000_000_000_000u64);
    let beneficiary = "0x0080000000000000000000000000000000000001".parse()?;
    for (name, call) in [
        ("WQUAI deposit", wquai.deposit(its)?),
        ("WQUAI withdrawal", wquai.withdraw(its)?),
        ("WQI claim", wqi.claim_deposit()?),
        (
            "WQI redemption",
            wqi.unwrap(beneficiary, U256::from(1000), 9000)?,
        ),
    ] {
        println!(
            "{name}: value={}, data={}, access_list={:?}",
            call.value(),
            call.data().to_hex(),
            call.access_list()
        );
        // With sqlite enabled: call.into_account_intent() feeds AccountSession::prepare.
    }
    Ok(())
}
