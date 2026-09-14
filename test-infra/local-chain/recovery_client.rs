//! Read-only real-node reconciliation against copies of public fixture custody.
use quai_sdk::recovery::{OperationObservation, reconcile_operation};
use quai_sdk::wallet::storage::{NetworkScope, ReservationId, SqliteStore};
use quai_sdk::{HttpConfig, HttpTransport, Provider, Routing, U256, Zone};
use serde_json::json;
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let stage = std::env::args()
        .nth(1)
        .ok_or("expected included or removed")?;
    if !matches!(stage.as_str(), "included" | "removed") {
        return Err("invalid stage".into());
    }
    let scope = NetworkScope {
        chain_id: U256::from(1337),
        genesis: "0x654e7a894d57de62ec19b9c161cb1c647466278e0565d3e1ba5d806ae6af0aee".parse()?,
        zone: Zone::Cyprus1,
    };
    let provider = Provider::new(
        HttpTransport::new(HttpConfig::default())?,
        Routing::direct("http://127.0.0.1:19200", scope.zone.into())?,
        scope.chain_id,
    );
    let mut store = SqliteStore::open("/tmp/quai-sdk-reorg-qualification/qi.sqlite", scope)?;
    let id = ReservationId([3; 16]);
    let original = store
        .signed_payload(id)?
        .ok_or("no original signed payload")?;
    let claims = store.reserved_outpoints(id)?;
    if claims.is_empty() {
        return Err("no held inputs".into());
    }
    let before = store.reservation(id)?.ok_or("no operation")?;
    let observation = reconcile_operation(&provider, &mut store, id).await?;
    match (&stage[..], &observation) {
        (
            "included",
            OperationObservation::Included {
                outcome: quai_sdk::provider::ReceiptOutcome::Succeeded,
                ..
            },
        ) => (),
        ("removed", OperationObservation::Reorganized) => (),
        _ => return Err(format!("unexpected observation {observation:?}").into()),
    }
    if store.signed_payload(id)?.as_ref() != Some(&original)
        || store.reserved_outpoints(id)? != claims
    {
        return Err("custody changed".into());
    }
    let after = store.reservation(id)?.ok_or("operation disappeared")?;
    if stage == "removed" && after.inclusion.is_some() {
        return Err("stale inclusion retained".into());
    }
    println!(
        "{}",
        json!({"stage":stage,"observation":format!("{observation:?}"),"before":format!("{:?}",before.state),"after":format!("{:?}",after.state),"heldInputs":claims.len(),"signedBytesUnchanged":true,"claimsUnchanged":true,"hash":before.transaction.map(|h|h.to_string())})
    );
    if stage == "removed" {
        drop(store);
        let mut reopened = SqliteStore::open("/tmp/quai-sdk-reorg-qualification/qi.sqlite", scope)?;
        let again = reconcile_operation(&provider, &mut reopened, id).await?;
        if !matches!(again, OperationObservation::NotObserved)
            || reopened.reserved_outpoints(id)? != claims
            || reopened.signed_payload(id)?.as_ref() != Some(&original)
        {
            return Err("restart recovery mismatch".into());
        }
        println!(
            "{}",
            json!({"stage":"removed-after-reopen","observation":format!("{again:?}"),"signedBytesUnchanged":true,"claimsUnchanged":true})
        );
    }
    Ok(())
}
