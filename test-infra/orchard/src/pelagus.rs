//! Pelagus interoperability. Pelagus (quais 1.0.0-alpha.57) announces payment
//! channels through a mailbox contract rather than BIP47 notification transactions:
//! senders call `notify(sender, receiver)` and receivers read `getNotifications`.
use super::*;
use quai_sdk::payment_channels::{PaymentScanOptions, scan_payment_channel};
use quai_sdk::payments::PaymentChannel;
use quai_sdk::wallet::storage::{NetworkScope, SqliteStore};
use serde_json::{Value, json};

/// Pelagus `MAILBOX_CONTRACT_ADDRESS` (pelagus-extension 8c8a044, `.env.defaults`).
const MAILBOX: &str = "0x004C82298b3ED69a949008d7037918B13A4260c5";

pub async fn run(stage: &str) -> Result<(), Box<dyn Error>> {
    if !net().mainnet() {
        return Err("pelagus stages require QUAI_QUALIFICATION_NETWORK=mainnet".into());
    }
    let scope = NetworkScope {
        chain_id: U256::from(net().chain_id),
        genesis: net().genesis.parse()?,
        zone: Zone::Cyprus1,
    };
    let provider = Provider::new(
        DiagnosticTransport(HttpTransport::new(HttpConfig::default())?),
        Routing::direct(net().endpoint, Zone::Cyprus1.into())?,
        scope.chain_id,
    );
    if provider.genesis_hash(scope.zone).await? != scope.genesis {
        return Err("genesis mismatch".into());
    }
    let (_, owner) = super::qi_extended::load_named_wallet("qi-wallet.json")?;
    let our_code = owner.public_code().to_base58();
    let senders_path = dir().join("pelagus-senders.json");
    match stage {
        "discover" => {
            let interface = quai_sdk::abi::AbiInterface::from_human_readable(&[
                "function getNotifications(string receiverPaymentCode) view returns (string[])",
            ])?;
            let mailbox: QuaiAddress = MAILBOX.parse()?;
            let code = provider.code(mailbox, BlockTag::Latest).await?;
            let caller: QuaiAddress = {
                let content = Zeroizing::new(fs::read_to_string(dir().join("wallets.json"))?);
                let supplied: Supplied =
                    serde_json::from_str(&content).map_err(|_| "wallet JSON")?;
                supplied.wallets[0].address.parse()?
            };
            let contract = quai_sdk::contracts::Contract::new(mailbox, interface, &provider);
            let result = contract
                .call(
                    caller,
                    "getNotifications",
                    &[Value::String(our_code.clone())],
                    BlockTag::Latest,
                )
                .await?;
            let senders: Vec<String> = result
                .first()
                .and_then(Value::as_array)
                .ok_or("unexpected mailbox result")?
                .iter()
                .map(|v| v.as_str().map(str::to_string).ok_or("non-string sender"))
                .collect::<Result<_, _>>()?;
            // Announcements are unauthenticated contract data: validate every code.
            let mut valid = Vec::new();
            for sender in &senders {
                let parsed = quai_sdk::payments::PaymentCode::from_base58(sender);
                valid.push(json!({"code":sender,"valid":parsed.is_ok()}));
            }
            let record = json!({"stage":"mailbox-discovery","mailbox":MAILBOX,"mailboxCodeBytes":code.bytes().len(),
                "receiver":our_code,"senders":valid});
            fs::write(&senders_path, serde_json::to_vec_pretty(&record)?)?;
            println!("{record}");
        }
        "scan" => {
            let record: Value = serde_json::from_slice(&fs::read(&senders_path)?)?;
            let index: usize = std::env::args()
                .nth(3)
                .map_or(Ok(0), |v| v.parse())
                .map_err(|_| "sender index")?;
            let sender = record["senders"][index]["code"]
                .as_str()
                .ok_or("no discovered sender at that index")?;
            let peer = quai_sdk::payments::PaymentCode::from_base58(sender)?;
            let mut store = SqliteStore::open(dir().join("state/qi.sqlite"), scope)?;
            if store.payment_channel(&owner, &peer)?.is_none() {
                store.import_payment_channel(
                    &owner,
                    &PaymentChannel::new(&owner, peer.clone()),
                    None,
                )?;
            }
            let mut report = None;
            for attempt in 1..=4 {
                match scan_payment_channel(
                    &provider,
                    &mut store,
                    &owner,
                    &peer,
                    &PaymentScanOptions::default(),
                    || false,
                )
                .await
                {
                    Ok(r) => {
                        report = Some(r);
                        break;
                    }
                    Err(quai_sdk::qi::QiError::StaleSnapshot) if attempt < 4 => {
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            let report = report.ok_or("no stable scan")?;
            let receive: std::collections::BTreeSet<_> = store
                .payment_addresses(&owner, &peer, quai_sdk::payments::PaymentDirection::Receive)?
                .into_iter()
                .map(|p| p.address.to_string())
                .collect();
            let snapshot = store.snapshot()?;
            let checkpoint = snapshot.checkpoint.ok_or("missing checkpoint")?;
            let channel_coins: Vec<_> = snapshot
                .coins
                .iter()
                .filter(|c| receive.contains(&c.address.to_string()))
                .map(|c| json!({"address":c.address.to_string(),"hash":c.outpoint.transaction_hash.to_string(),"index":c.outpoint.index,"qits":c.denomination.value(),"unlockHeight":c.unlock_height.to_string()}))
                .collect();
            let channel_qits: u64 = channel_coins
                .iter()
                .filter_map(|c| c["qits"].as_u64())
                .sum();
            let balance =
                quai_sdk::qi_discovery::qi_balance(&mut store, checkpoint.height + U256::from(1))?;
            println!(
                "{}",
                json!({"stage":"pelagus-channel-scan","sender":sender,"indexes":report.indexes,"nextIndex":report.next_index,
                    "stop":format!("{:?}",report.stopped),"channelCoins":channel_coins,"channelQits":channel_qits,
                    "walletBalance":{"total":balance.total.to_string(),"spendable":balance.spendable.to_string(),"locked":balance.locked.to_string()}})
            );
        }
        _ => return Err("expected pelagus discover or scan".into()),
    }
    Ok(())
}
