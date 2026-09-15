//! Pelagus interoperability. Pelagus (quais 1.0.0-alpha.57) announces payment
//! channels through a mailbox contract rather than BIP47 notification transactions:
//! senders call `notify(sender, receiver)` and receivers read `getNotifications`.
use super::*;
use quai_sdk::payment_channels::{PaymentScanOptions, scan_payment_channel};
use quai_sdk::payment_mailbox::{PELAGUS_MAILBOX_ADDRESS, PaymentMailbox};
use quai_sdk::payments::PaymentChannel;
use quai_sdk::wallet::storage::{NetworkScope, SqliteStore};
use serde_json::{Value, json};

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
            let ctx = super::mainnet_extra::Ctx::load().await?;
            let mailbox: QuaiAddress = PELAGUS_MAILBOX_ADDRESS.parse()?;
            let code = provider.code(mailbox, BlockTag::Latest).await?;
            let found = PaymentMailbox::new(mailbox, &provider)?
                .notifications(ctx.addresses[0], owner.public_code(), BlockTag::Latest)
                .await?;
            let record = json!({"stage":"mailbox-discovery","mailbox":PELAGUS_MAILBOX_ADDRESS,"mailboxCodeBytes":code.bytes().len(),
                "receiver":our_code,"senders":found.senders.iter().map(|c|json!({"code":c.to_base58(),"valid":true})).collect::<Vec<_>>(),
                "invalid":found.invalid,"duplicates":found.duplicates});
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
        "mailbox-scan" => {
            // The SDK's combined discovery: read announcements, register bounded channels, scan.
            let ctx = super::mainnet_extra::Ctx::load().await?;
            let mailbox = PaymentMailbox::new(PELAGUS_MAILBOX_ADDRESS.parse()?, &provider)?;
            let mut store = SqliteStore::open(dir().join("state/qi.sqlite"), scope)?;
            let mut result = None;
            for attempt in 1..=4 {
                match quai_sdk::payment_channels::discover_mailbox_channels(
                    &provider,
                    &mut store,
                    &owner,
                    &mailbox,
                    ctx.addresses[0],
                    0,
                    8,
                    &PaymentScanOptions::default(),
                    || false,
                )
                .await
                {
                    Ok(r) => {
                        result = Some(r);
                        break;
                    }
                    Err(quai_sdk::qi::QiError::StaleSnapshot) if attempt < 4 => {
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            let report = result.ok_or("no stable mailbox scan")?;
            let snapshot = store.snapshot()?;
            let checkpoint = snapshot.checkpoint.ok_or("missing checkpoint")?;
            let balance =
                quai_sdk::qi_discovery::qi_balance(&mut store, checkpoint.height + U256::from(1))?;
            println!(
                "{}",
                json!({"stage":"sdk-mailbox-discovery","scanned":report.scanned.iter().map(|c|json!({"sender":c.sender.to_base58(),"newlyRegistered":c.newly_registered,"indexes":c.report.indexes.len(),"firstIndexes":c.report.indexes.iter().take(3).collect::<Vec<_>>(),"stop":format!("{:?}",c.report.stopped)})).collect::<Vec<_>>(),
                    "deferred":report.deferred.len(),"invalid":report.invalid,"duplicates":report.duplicates,
                    "walletBalance":{"total":balance.total.to_string(),"spendable":balance.spendable.to_string(),"locked":balance.locked.to_string()}})
            );
        }
        "return-notify" | "return-prepare" | "return-broadcast" | "return-observe" => {
            let record: Value = serde_json::from_slice(&fs::read(&senders_path)?)?;
            let peer = quai_sdk::payments::PaymentCode::from_base58(
                record["senders"][0]["code"]
                    .as_str()
                    .ok_or("no discovered sender")?,
            )?;
            let mut store = SqliteStore::open(dir().join("state/qi.sqlite"), scope)?;
            if store.payment_channel(&owner, &peer)?.is_none() {
                return Err("scan the Pelagus channel before returning funds".into());
            }
            let id = quai_sdk::wallet::storage::ReservationId([7; 16]);
            let (wallet, _) = super::qi_extended::load_named_wallet("qi-wallet.json")?;
            match stage {
                "return-notify" => {
                    // Pelagus receivers only open channels announced in the mailbox.
                    let ctx = super::mainnet_extra::Ctx::load().await?;
                    let caller = ctx.addresses[0];
                    let mailbox =
                        PaymentMailbox::new(PELAGUS_MAILBOX_ADDRESS.parse()?, &ctx.provider)?;
                    let before = mailbox
                        .is_notified(caller, owner.public_code(), &peer, BlockTag::Latest)
                        .await?;
                    let sent = if before {
                        None
                    } else {
                        let call = mailbox.notify(owner.public_code(), &peer)?;
                        Some(
                            ctx.send(0, 27, "pelagus-notify", call.into_account_intent())
                                .await?,
                        )
                    };
                    let after = mailbox
                        .is_notified(caller, owner.public_code(), &peer, BlockTag::Latest)
                        .await?;
                    println!(
                        "{}",
                        json!({"stage":"pelagus-notify","sender":our_code,"receiver":peer.to_base58(),"alreadyNotified":before,"notifyTransaction":sent,"notifiedAfter":after})
                    );
                    if !after {
                        return Err("mailbox does not list our code after notify".into());
                    }
                }
                "return-prepare" => {
                    if store.reservation(id)?.is_some() {
                        return Err("return already reserved; recover instead".into());
                    }
                    let intent = quai_sdk::payment_channels::payment_intent(
                        &mut store,
                        &owner,
                        &peer,
                        U256::from(1000),
                        1,
                        6000,
                        || false,
                    )?;
                    let pool = quai_sdk::qi::QiChangePool::allocate(
                        &mut store,
                        &wallet.account_public(0)?,
                        16,
                        6000,
                        || false,
                    )?;
                    let mut refreshed = false;
                    for attempt in 1..=4 {
                        match quai_sdk::qi_discovery::refresh_qi(
                            &provider,
                            &mut store,
                            1000,
                            || false,
                        )
                        .await
                        {
                            Ok(_) => {
                                refreshed = true;
                                break;
                            }
                            Err(quai_sdk::qi::QiError::StaleSnapshot) if attempt < 4 => {
                                tokio::time::sleep(std::time::Duration::from_secs(2)).await
                            }
                            Err(error) => return Err(error.into()),
                        }
                    }
                    if !refreshed {
                        return Err("no stable refresh".into());
                    }
                    let mut keys = quai_sdk::wallet::qi_keys::QiKeyring::new(Some(&wallet))?;
                    keys.load_payment_channel(&store, &owner, &peer)?;
                    let mut session =
                        quai_sdk::qi::QiSession::with_keys(&provider, &keys, &mut store);
                    let prepared = session
                        .prepare(
                            id,
                            intent,
                            quai_sdk::qi::QiPolicy {
                                initial_fee: U256::ZERO,
                                max_fee: U256::from(200),
                                max_inputs: 8,
                                max_outputs: 32,
                                max_fee_rounds: 8,
                                max_snapshot_age: 5,
                            },
                            pool,
                        )
                        .await?;
                    let signed = session.sign(&prepared)?;
                    let tx = signed.transaction();
                    println!(
                        "{}",
                        json!({"stage":"pelagus-return-signed","hash":signed.hash()?.to_string(),"feeQits":prepared.fee().to_string(),
                            "inputs":tx.inputs.iter().map(|i|json!({"hash":i.previous_output.transaction_hash.to_string(),"index":i.previous_output.index})).collect::<Vec<_>>(),
                            "outputs":tx.outputs.iter().map(|o|json!({"address":o.address.to_string(),"qits":o.denomination.value()})).collect::<Vec<_>>()})
                    );
                }
                "return-broadcast" => {
                    let bytes = store.signed_payload(id)?.ok_or("no signed return")?;
                    let signed = quai_sdk::consensus::SignedQiTransaction::decode(&bytes)?;
                    if provider
                        .receipt(scope.zone, signed.hash()?)
                        .await?
                        .is_some()
                    {
                        return Err("receipt exists; observe instead".into());
                    }
                    let mut keys = quai_sdk::wallet::qi_keys::QiKeyring::new(Some(&wallet))?;
                    keys.load_payment_channel(&store, &owner, &peer)?;
                    let ack = quai_sdk::qi::QiSession::with_keys(&provider, &keys, &mut store)
                        .broadcast(id)
                        .await?;
                    println!(
                        "{}",
                        json!({"stage":"pelagus-return-acknowledged","hash":ack.transaction_hash.to_string()})
                    );
                }
                _ => {
                    let bytes = store.signed_payload(id)?.ok_or("no signed return")?;
                    let signed = quai_sdk::consensus::SignedQiTransaction::decode(&bytes)?;
                    let hash = signed.hash()?;
                    let observed = provider
                        .wait_for_receipt(
                            scope.zone,
                            hash,
                            quai_sdk::provider::WaitConfig {
                                confirmations: 2,
                                timeout: std::time::Duration::from_secs(300),
                                poll_interval: std::time::Duration::from_secs(3),
                            },
                        )
                        .await?;
                    let mined = provider
                        .transaction(scope.zone, hash)
                        .await?
                        .ok_or("mined transaction absent")?
                        .verified_qi()?;
                    let sends: std::collections::BTreeSet<_> = store
                        .payment_addresses(
                            &owner,
                            &peer,
                            quai_sdk::payments::PaymentDirection::Send,
                        )?
                        .into_iter()
                        .map(|p| p.address.to_string())
                        .collect();
                    let mut to_pelagus = Vec::new();
                    for (index, output) in signed.transaction().outputs.iter().enumerate() {
                        let address = quai_sdk::QiAddress::try_from(output.address)?;
                        if sends.contains(&address.to_string()) {
                            let indexed = provider.outpoints(address).await?.iter().any(|p| {
                                p.outpoint.tx_hash == hash && p.outpoint.index == index as u16
                            });
                            to_pelagus.push(json!({"address":address.to_string(),"qits":output.denomination.value(),"indexed":indexed}));
                        }
                    }
                    println!(
                        "{}",
                        json!({"stage":"pelagus-return-observed","hash":hash.to_string(),"block":observed.receipt.inclusion.block_number,
                            "outcome":format!("{:?}",observed.receipt.outcome),"minedBytesMatchCustody":mined.signed_bytes()? == bytes,"toPelagus":to_pelagus})
                    );
                }
            }
        }
        _ => return Err("expected pelagus discover, scan or return-*".into()),
    }
    Ok(())
}
