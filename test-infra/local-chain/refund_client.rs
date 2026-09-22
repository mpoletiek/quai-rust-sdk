//! Public synthetic refund discovery/custody/spend qualification, never Orchard.
use quai_sdk::consensus::{ConversionSlippage, Denomination, SignedQiOperation};
use quai_sdk::qi::{QiChangePool, QiIntent, QiPolicy, QiSession};
use quai_sdk::wallet::qi_keys::QiKeyring;
use quai_sdk::wallet::storage::{NetworkScope, ReservationId, SqliteStore};
use quai_sdk::wallet::{CoinType, HdWallet};
use quai_sdk::{BlockTag, HttpConfig, HttpTransport, Provider, QiAddress, Routing, U256, Zone};
use serde_json::json;
fn key(value: u64) -> quai_sdk::crypto::SecretKey {
    let mut bytes = [0; 32];
    bytes[24..].copy_from_slice(&value.to_be_bytes());
    quai_sdk::crypto::SecretKey::from_bytes(&bytes).unwrap()
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mode = std::env::args()
        .nth(1)
        .ok_or("credit | prepare | broadcast | observe")?;
    let scope = NetworkScope {
        chain_id: U256::from(1337),
        genesis: "0xff38a93744ee5aae738addc88da4f6b171528244e81d34aa4b25579fa3f44ed2".parse()?,
        zone: Zone::Cyprus1,
    };
    let provider = Provider::new(
        HttpTransport::new(HttpConfig::default())?,
        Routing::direct("http://127.0.0.1:19200", scope.zone.into())?,
        scope.chain_id,
    );
    if provider.genesis_hash(scope.zone).await? != scope.genesis {
        return Err("wrong isolated genesis".into());
    }
    let wallet = HdWallet::from_seed(&[7; 32], CoinType::Qi)?;
    let mut keys = QiKeyring::new(Some(&wallet))?;
    let refund = keys.import(key(2285))?;
    let mut store = SqliteStore::open("/tmp/quai-sdk-refund-qualification/refund.sqlite", scope)?;
    let id = ReservationId([1; 16]);
    match mode.as_str() {
        "credit" => {
            let origin = std::env::args().nth(2).ok_or("origin hash")?.parse()?;
            let signed = provider
                .transaction(scope.zone, origin)
                .await?
                .ok_or("origin absent")?
                .verified_qi()?;
            let SignedQiOperation::Conversion(signed) = signed else {
                return Err("not conversion".into());
            };
            if signed.transaction().intent().slippage != ConversionSlippage::new(30)? {
                return Err("expected explicit strict slippage".into());
            }
            let reference =
                quai_sdk::provider::ConversionReference::from_qi(scope.genesis, &signed)?;
            let head = provider
                .latest_header(scope.zone)
                .await?
                .ok_or("head absent")?;
            let (observation, credit) = provider
                .observe_conversion_qi_credit(
                    &reference,
                    quai_sdk::provider::EtxScanRequest::new(
                        scope.zone,
                        8,
                        head.number.min(39),
                        4096,
                        65536,
                    ),
                    64,
                )
                .await?;
            let credit = credit.ok_or("no Qi refund credit")?;
            println!(
                "{}",
                json!({"stage":"refund-credit","origin":origin.to_string(),"effect":format!("{:?}",observation.effect),"creatingHash":credit.creating_hash.to_string(),"beneficiary":credit.beneficiary.to_string(),"lockedQits":credit.locked_qits.to_string(),"unlockedQits":credit.unlocked_qits.to_string(),"unobservedQits":credit.unobserved_qits.to_string(),"head":credit.head.number,"outputs":credit.outputs.iter().map(|c|json!({"hash":c.outpoint.tx_hash.to_string(),"index":c.outpoint.index,"denomination":c.denomination,"lock":c.lock.to_string()})).collect::<Vec<_>>()})
            );
        }
        "prepare" => {
            if store.reservation(id)?.is_some() {
                return Err("existing custody; recover instead".into());
            }
            if store.addresses()?.is_empty() {
                store.import_metadata(0, &[refund])?;
            }
            let pool =
                QiChangePool::allocate(&mut store, &wallet.account_public(0)?, 16, 6000, || false)?;
            quai_sdk::qi_discovery::refresh_qi(&provider, &mut store, 100, || false).await?;
            let snapshot = store.snapshot()?;
            if snapshot.coins.len() != 1
                || snapshot.coins[0].denomination != Denomination::new(7)?
                || snapshot.coins[0].outpoint.transaction_hash.bytes()[3] & 0x80 != 0
            {
                return Err("expected one 5 Qi refund with Quai hash bit".into());
            }
            let mut session = QiSession::with_keys(&provider, &keys, &mut store);
            let prepared = session
                .prepare(
                    id,
                    QiIntent::new(
                        U256::from(1000),
                        vec![QiAddress::try_from(key(300).public_key().address())?],
                    ),
                    QiPolicy::new(U256::from(500), 8, 32, 5),
                    pool,
                )
                .await?;
            let fee = prepared.fee();
            let signed = session.sign(&prepared)?;
            println!(
                "{}",
                json!({"stage":"refund-spend-signed","hash":signed.hash()?.to_string(),"inputHash":signed.transaction().inputs[0].previous_output.transaction_hash.to_string(),"feeQits":fee.to_string(),"amountQits":"1000","outputQits":signed.transaction().outputs.iter().map(|o|o.denomination.value()).sum::<u64>(),"outputs":signed.transaction().outputs.len()})
            );
        }
        "broadcast" => {
            let ack = QiSession::with_keys(&provider, &keys, &mut store)
                .broadcast(id)
                .await?;
            println!(
                "{}",
                json!({"stage":"refund-spend-acknowledged","hash":ack.transaction_hash.to_string()})
            );
        }
        "observe" => {
            let bytes = store.signed_payload(id)?.ok_or("no signed payload")?;
            let signed = SignedQiOperation::decode(&bytes)?;
            let hash = signed.hash()?;
            let observation =
                quai_sdk::recovery::reconcile_operation(&provider, &mut store, id).await?;
            if !matches!(
                observation,
                quai_sdk::recovery::OperationObservation::Included {
                    outcome: quai_sdk::provider::ReceiptOutcome::Succeeded,
                    ..
                }
            ) {
                return Err("spend not successful".into());
            }
            let mined = provider
                .transaction(scope.zone, hash)
                .await?
                .ok_or("missing mined transaction")?
                .verified_qi()?;
            if mined.signed_bytes()? != bytes {
                return Err("mined bytes differ".into());
            }
            let mut matched = 0u64;
            for (index, output) in signed.transaction().outputs.iter().enumerate() {
                let points = provider.outpoints(output.address.try_into()?).await?;
                if !points.iter().any(|p| {
                    p.outpoint.tx_hash == hash
                        && usize::from(p.outpoint.index) == index
                        && p.denomination == output.denomination.index()
                }) {
                    return Err("output missing or mismatched".into());
                }
                matched += output.denomination.value();
            }
            println!(
                "{}",
                json!({"stage":"refund-spend-verified","hash":hash.to_string(),"observation":format!("{observation:?}"),"durableBytesMatch":true,"matchedQits":matched,"heldClaims":store.reserved_outpoints(id)?.len(),"refundOutpointsRemaining":provider.outpoints(key(2285).public_key().address().try_into()?).await?.len(),"destinationQuaiBalance":provider.balance(key(805).public_key().address().try_into()?,BlockTag::Latest).await?.to_string()})
            );
        }
        _ => return Err("unknown stage".into()),
    }
    Ok(())
}
