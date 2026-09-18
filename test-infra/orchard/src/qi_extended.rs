//! Fixed funded Cyprus-1 WQI and two-wallet payment-channel qualification.
use super::*;
use quai_sdk::consensus::{QiWrappingIntent, SignedQiOperation};
use quai_sdk::payments::{PaymentChannel, PaymentDirection};
use quai_sdk::provider::{ReceiptOutcome, WaitConfig};
use quai_sdk::qi::{QiChangePool, QiPolicy, QiSession, QiSpecialIntent};
use quai_sdk::wallet::qi_keys::QiKeyring;
use quai_sdk::wallet::storage::{NetworkScope, ReservationId, SqliteStore};
use quai_sdk::wrappers::{WQI_ADDRESS, WrappedQi};
use serde_json::json;

pub(super) fn load_wallet(peer: bool) -> Result<(HdWallet, PrivatePaymentCode), Box<dyn Error>> {
    load_named_wallet(if peer {
        "qi-peer.json"
    } else {
        "qi-wallet.json"
    })
}

pub(super) fn load_named_wallet(
    name: &str,
) -> Result<(HdWallet, PrivatePaymentCode), Box<dyn Error>> {
    #[derive(Deserialize)]
    struct Saved<'a> {
        #[serde(borrow)]
        mnemonic: &'a str,
        #[serde(borrow)]
        passphrase: &'a str,
    }
    let bytes = Zeroizing::new(fs::read_to_string(dir().join(name))?);
    let saved: Saved = serde_json::from_str(&bytes).map_err(|_| "invalid private wallet file")?;
    let mnemonic = Mnemonic::parse(Language::English, saved.mnemonic)?;
    let wallet = HdWallet::from_mnemonic(&mnemonic, saved.passphrase, CoinType::Qi)?;
    let seed = mnemonic.to_seed(saved.passphrase);
    Ok((wallet, PrivatePaymentCode::from_seed(seed.expose(), 0)?))
}

pub(super) fn scope() -> Result<NetworkScope, Box<dyn Error>> {
    Ok(NetworkScope {
        chain_id: U256::from(net().chain_id),
        genesis: net().genesis.parse()?,
        zone: Zone::Cyprus1,
    })
}

pub(super) fn save(name: &str, value: &serde_json::Value) -> Result<(), Box<dyn Error>> {
    let path = dir().join(name);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.sync_all()?;
    fs::File::open(dir())?.sync_all()?;
    Ok(())
}

async fn refresh(
    provider: &Provider<DiagnosticTransport>,
    store: &mut SqliteStore,
) -> Result<(), Box<dyn Error>> {
    for attempt in 0..3 {
        match quai_sdk::qi_discovery::refresh_qi(provider, store, 1000, || false).await {
            Ok(head) => {
                let b = quai_sdk::qi_discovery::qi_balance(store, head.height + U256::from(1))?;
                println!(
                    "{}",
                    json!({"stage":"refresh","head":head.height.to_string(),"unreservedQits":b.spendable.to_string(),"lockedQits":b.locked.to_string()})
                );
                return Ok(());
            }
            Err(quai_sdk::qi::QiError::StaleSnapshot) if attempt < 2 => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err("no stable refresh".into())
}

pub async fn run(operation: &str, stage: &str) -> Result<(), Box<dyn Error>> {
    let (index, is_peer) = match operation {
        "wrap" => (2, false),
        "payment-send" => (3, false),
        "payment-return" => (1, true),
        "redemption-spend" => (1, false),
        "convert-qi-to-quai" => (4, false),
        "aggregate" => (5, false),
        "sweep" => (2, true),
        "self-transfer" if net().mainnet() => (6, false),
        _ => return Err("unsupported Qi extended operation".into()),
    };
    let mainnet = net().mainnet();
    // Mainnet amounts are scaled for 1 Qi ≈ 146 QUAI; Orchard keeps its recorded values.
    let payment_qits = |orchard: u64, mainnet_value: u64| {
        U256::from(if mainnet { mainnet_value } else { orchard })
    };
    let scope = scope()?;
    let provider = Provider::new(
        DiagnosticTransport(HttpTransport::new(HttpConfig::default())?),
        Routing::direct(net().endpoint, scope.zone.into())?,
        scope.chain_id,
    );
    if provider.genesis_hash(scope.zone).await? != scope.genesis
        || provider.chain_id(scope.zone.into()).await? != scope.chain_id
    {
        return Err("network identity mismatch".into());
    }
    let accounts: [QuaiAddress; 2] = {
        let content = Zeroizing::new(fs::read_to_string(dir().join("wallets.json"))?);
        let supplied: Supplied = serde_json::from_str(&content).map_err(|_| "wallet JSON")?;
        [
            supplied.wallets[0].address.parse()?,
            supplied.wallets[1].address.parse()?,
        ]
    };
    let conversion = operation == "convert-qi-to-quai";
    let sweeping = matches!(operation, "aggregate" | "sweep");
    let redemption = operation == "redemption-spend";
    let (wallet, payment) = if redemption {
        load_named_wallet("qi-redemption.json")?
    } else {
        load_wallet(is_peer)?
    };
    let (_, peer) = load_wallet(if redemption { false } else { !is_peer })?;
    let mut store = SqliteStore::open(
        dir().join(if redemption {
            "state/qi-redemption.sqlite"
        } else if is_peer {
            "state/qi-peer.sqlite"
        } else {
            "state/qi.sqlite"
        }),
        scope,
    )?;
    if store
        .payment_channel(&payment, peer.public_code())?
        .is_none()
    {
        store.import_payment_channel(
            &payment,
            &PaymentChannel::new(&payment, peer.public_code().clone()),
            None,
        )?;
    }
    let id = ReservationId([index; 16]);
    let beneficiary = accounts[0];
    let wrapper = WrappedQi::new(WQI_ADDRESS.parse()?, &provider)?;
    match stage {
        "probe" => {
            let (_, code) = WrappedQi::new_verified(
                WQI_ADDRESS.parse()?,
                &provider,
                scope.genesis,
                None,
                BlockTag::Latest,
            )
            .await?;
            println!(
                "{}",
                json!({"stage":"WQI-probe","code":format!("{code:?}"),"unclaimedQits":wrapper.unclaimed(beneficiary,BlockTag::Latest).await?.to_string(),"tokenAtoms":wrapper.token()?.balance_of(beneficiary,beneficiary,BlockTag::Latest).await?.to_string()})
            );
        }
        "scan" => {
            // Recover using only this wallet's seed and the exchanged public peer code.
            let mut options = quai_sdk::payment_channels::PaymentScanOptions::default();
            if let Some(start) = std::env::args().nth(4) {
                options.range.start = start
                    .parse()
                    .map_err(|_| "invalid scan continuation index")?;
            }
            let report = quai_sdk::payment_channels::scan_payment_channel(
                &provider,
                &mut store,
                &payment,
                peer.public_code(),
                &options,
                || false,
            )
            .await?;
            let snapshot = store.snapshot()?;
            println!(
                "{}",
                json!({"stage":"payment-scan","ownerCode":payment.public_code().to_base58(),"peerCode":peer.public_code().to_base58(),"indexes":report.indexes,"nextIndex":report.next_index,"stop":format!("{:?}",report.stopped),"coins":snapshot.coins.iter().map(|c|json!({"hash":c.outpoint.transaction_hash.to_string(),"index":c.outpoint.index,"address":c.address.to_string(),"qits":c.denomination.value()})).collect::<Vec<_>>()})
            );
        }
        "inventory" => {
            refresh(&provider, &mut store).await?;
            let snapshot = store.snapshot()?;
            println!(
                "{}",
                json!({"stage":"inventory","operation":operation,"coins":snapshot.coins.iter().map(|c|json!({"hash":c.outpoint.transaction_hash.to_string(),"index":c.outpoint.index,"address":c.address.to_string(),"qits":c.denomination.value(),"reserved":c.reserved,"origin":store.addresses().ok().and_then(|rows|rows.into_iter().find(|a|a.address()==c.address.address())).map(|a|format!("{:?}",a.origin()))})).collect::<Vec<_>>() })
            );
        }
        "reconcile" => {
            let result = quai_sdk::recovery::reconcile_operation(&provider, &mut store, id).await?;
            println!(
                "{}",
                json!({"stage":"reconcile","operation":operation,"observation":format!("{result:?}")})
            );
        }
        "prepare" => {
            if store.reservation(id)?.is_some() {
                return Err("existing operation; recover instead".into());
            }
            if operation == "wrap" {
                WrappedQi::new_verified(
                    WQI_ADDRESS.parse()?,
                    &provider,
                    scope.genesis,
                    None,
                    BlockTag::Latest,
                )
                .await?;
                if wrapper.unclaimed(beneficiary, BlockTag::Latest).await? != U256::ZERO
                    || wrapper
                        .token()?
                        .balance_of(beneficiary, beneficiary, BlockTag::Latest)
                        .await?
                        != U256::ZERO
                {
                    return Err("expected zero WQI backing and tokens before wrap".into());
                }
            }
            let conversion_intent = if conversion {
                let refund = store.allocate_address_compact(
                    &wallet.account_public(0)?,
                    false,
                    6000,
                    || false,
                )?;
                let destination = accounts[1];
                // Mainnet discounts whole prime-block batches; see mainnet-2026-09-14.json.
                let slippage = if mainnet { 2000 } else { 100 };
                let intent = quai_sdk::consensus::QiConversionIntent {
                    destination,
                    refund: refund.address.address().try_into()?,
                    slippage: quai_sdk::consensus::ConversionSlippage::new(slippage)?,
                };
                println!(
                    "{}",
                    json!({"stage":"conversion-preflight","destination":destination.to_string(),"refund":intent.refund.to_string(),"amountQits":"1000","slippageBasisPoints":slippage,"quotedIts":provider.qi_to_quai(scope.zone,U256::from(1000),BlockTag::Latest).await?.map(|q|q.to_string()),"balanceBeforeIts":provider.balance(destination,BlockTag::Latest).await?.to_string(),"lockedBeforeIts":provider.locked_quai_balance(destination).await?.balance.to_string()})
                );
                Some(intent)
            } else {
                None
            };
            let (count, attempts) = if sweeping {
                (32, 3000)
            } else if operation == "payment-send" {
                (24, 4000)
            } else {
                (16, 6000)
            };
            let pool = QiChangePool::allocate(
                &mut store,
                &wallet.account_public(0)?,
                count,
                attempts,
                || false,
            )?;
            let intent = if operation == "self-transfer" {
                let receiver = store.allocate_address_compact(
                    &wallet.account_public(0)?,
                    false,
                    100_000,
                    || false,
                )?;
                Some(quai_sdk::qi::QiIntent {
                    amount: U256::from(100),
                    destinations: vec![receiver.address.address().try_into()?],
                })
            } else if operation != "wrap" && !conversion && !sweeping {
                // Each payment uses one recipient address, so mainnet amounts must be
                // a single Qi denomination (250 Qits would need three outputs).
                let amount = if redemption {
                    payment_qits(500, 500)
                } else if is_peer {
                    payment_qits(1000, 100)
                } else {
                    payment_qits(5000, 500)
                };
                Some(quai_sdk::payment_channels::payment_intent(
                    &mut store,
                    &payment,
                    peer.public_code(),
                    amount,
                    1,
                    6000,
                    || false,
                )?)
            } else {
                None
            };
            refresh(&provider, &mut store).await?;
            let mut keys = QiKeyring::new(Some(&wallet))?;
            keys.load_payment_channels(&store, &payment)?;
            let mut session = QiSession::with_keys(&provider, &keys, &mut store);
            let policy = QiPolicy {
                initial_fee: U256::ZERO,
                max_fee: U256::from(match (mainnet, sweeping) {
                    (true, true) => 1000,
                    (true, false) => 200,
                    (false, true) => 500,
                    (false, false) => 100,
                }),
                max_inputs: if sweeping { 128 } else { 8 },
                max_outputs: 32,
                max_fee_rounds: 8,
                max_snapshot_age: 5,
            };
            let (signed, fee) = if sweeping {
                let mode = if operation == "aggregate" {
                    quai_sdk::wallet::SweepMode::AggregateThreshold(
                        quai_sdk::wallet::AggregationPolicy::default(),
                    )
                } else {
                    quai_sdk::wallet::SweepMode::PreserveDenominations
                };
                let prepared = session.prepare_sweep(id, mode, policy, pool).await?;
                let fee = prepared.fee();
                (SignedQiOperation::Transfer(session.sign(&prepared)?), fee)
            } else if let Some(intent) = conversion_intent {
                let prepared = if mainnet {
                    session
                        .prepare_special_estimated(
                            id,
                            U256::from(1000),
                            QiSpecialIntent::Conversion(intent),
                            quai_sdk::provider::QiFeeProfile::V056ShaAnchored,
                            policy,
                            pool,
                        )
                        .await?
                } else {
                    session
                        .prepare_special(
                            id,
                            U256::from(1000),
                            QiSpecialIntent::Conversion(intent),
                            U256::from(100),
                            policy,
                            pool,
                        )
                        .await?
                };
                let fee = prepared.fee();
                (session.sign_special(&prepared)?, fee)
            } else if let Some(intent) = intent {
                let prepared = session.prepare(id, intent, policy, pool).await?;
                let fee = prepared.fee();
                (SignedQiOperation::Transfer(session.sign(&prepared)?), fee)
            } else {
                let wrap = QiSpecialIntent::Wrapping(QiWrappingIntent {
                    destination: beneficiary,
                    owner_contract: WQI_ADDRESS.parse()?,
                });
                let prepared = if mainnet {
                    // Mainnet satisfies the pinned activation profile (checked 2026-09-14).
                    session
                        .prepare_special_estimated(
                            id,
                            U256::from(1000),
                            wrap,
                            quai_sdk::provider::QiFeeProfile::V056ShaAnchored,
                            policy,
                            pool,
                        )
                        .await?
                } else {
                    // Orchard does not satisfy the estimator's pinned activation profile.
                    // Explicit 0.1 Qi qualification fee, bounded by the same policy cap.
                    session
                        .prepare_special(id, U256::from(1000), wrap, U256::from(100), policy, pool)
                        .await?
                };
                let fee = prepared.fee();
                (session.sign_special(&prepared)?, fee)
            };
            let tx = signed.transaction();
            let record = json!({"stage":"signed-persisted","operation":operation,"hash":signed.hash()?.to_string(),"feeQits":fee.to_string(),"dataLength":tx.data.len(),"inputs":tx.inputs.iter().map(|i|json!({"hash":i.previous_output.transaction_hash.to_string(),"index":i.previous_output.index})).collect::<Vec<_>>(),"outputs":tx.outputs.iter().map(|o|json!({"address":o.address.to_string(),"denomination":o.denomination.index()})).collect::<Vec<_>>()});
            save(&format!("{operation}-signed-review.json"), &record)?;
            println!("{record}");
        }
        "broadcast" | "broadcast-lost-ack" | "observe" => {
            let bytes = store.signed_payload(id)?.ok_or("no signed operation")?;
            let signed = SignedQiOperation::decode(&bytes)?;
            let hash = signed.hash()?;
            if stage.starts_with("broadcast") {
                if provider.receipt(scope.zone, hash).await?.is_some() {
                    return Err("receipt exists; observe instead".into());
                }
                let mut keys = QiKeyring::new(Some(&wallet))?;
                keys.load_payment_channels(&store, &payment)?;
                let ack = QiSession::with_keys(&provider, &keys, &mut store)
                    .broadcast(id)
                    .await?;
                println!(
                    "{}",
                    json!({"stage":"acknowledged","operation":operation,"hash":ack.transaction_hash.to_string()})
                );
            } else {
                let observed = provider
                    .wait_for_receipt(
                        scope.zone,
                        hash,
                        WaitConfig::new(
                            2,
                            std::time::Duration::from_secs(if mainnet { 300 } else { 55 }),
                            std::time::Duration::from_secs(2),
                        ),
                    )
                    .await?;
                let mut receipt = observed.receipt.to_rpc_json()?;
                receipt
                    .as_object_mut()
                    .ok_or("receipt shape")?
                    .remove("logsBloom");
                println!(
                    "{}",
                    json!({"stage":"observed","operation":operation,"confirmations":observed.confirmations,"receipt":receipt})
                );
                if observed.receipt.outcome != ReceiptOutcome::Succeeded {
                    return Err("origin failed".into());
                }
                let mined = provider
                    .transaction(scope.zone, hash)
                    .await?
                    .ok_or("missing mined transaction")?
                    .verified_qi()?;
                if mined.signed_bytes()? != bytes {
                    return Err("mined bytes differ from custody".into());
                }
                let block = provider
                    .block_with_transactions(
                        scope.zone,
                        observed.receipt.inclusion.block_number,
                        4096,
                    )
                    .await?
                    .ok_or("missing block")?;
                let position = block
                    .transactions
                    .iter()
                    .position(|t| t.hash == hash)
                    .ok_or("transaction absent from block")?;
                let qi_before = block.transactions[..position]
                    .iter()
                    .filter(|t| t.kind() == quai_sdk::provider::TransactionKind::Qi)
                    .count();
                if operation == "aggregate" && qi_before != 0 {
                    println!(
                        "{}",
                        json!({"stage":"aggregation-position","operation":operation,"block":observed.receipt.inclusion.block_number,"qiTransactionsBefore":qi_before})
                    );
                    return Err("aggregation not first Qi in block".into());
                }
                let destinations = signed
                    .transaction()
                    .outputs
                    .iter()
                    .filter_map(|o| quai_sdk::QiAddress::try_from(o.address).ok())
                    .collect::<Vec<_>>();
                let reads = provider.outpoints_many(&destinations).await?;
                let mut observed_qits = U256::ZERO;
                let mut matched = 0;
                for (index, output) in signed.transaction().outputs.iter().enumerate() {
                    if let Ok(address) = quai_sdk::QiAddress::try_from(output.address) {
                        let outpoints = reads.get(&address).ok_or("missing output response")?;
                        if let Some(coin) = outpoints.iter().find(|c| {
                            c.outpoint.tx_hash == hash && c.outpoint.index == index as u16
                        }) {
                            if coin.denomination != output.denomination.index() {
                                return Err("output denomination mismatch".into());
                            }
                            observed_qits += U256::from(output.denomination.value());
                            matched += 1;
                        }
                    }
                }
                println!(
                    "{}",
                    json!({"stage":"verified","operation":operation,"durableSignedBytesMatchMinedTransaction":true,"inputs":signed.transaction().inputs.len(),"heldInputClaims":store.reserved_outpoints(id)?.len(),"qiTransactionsBefore":qi_before,"matchedCurrentOutputs":matched,"matchedQits":observed_qits.to_string()})
                );
            }
        }
        "credit" if conversion => {
            let bytes = store.signed_payload(id)?.ok_or("no signed conversion")?;
            let hash = SignedQiOperation::decode(&bytes)?.hash()?;
            let receipt = provider
                .receipt(scope.zone, hash)
                .await?
                .ok_or("no origin receipt")?;
            let head = provider.latest_header(scope.zone).await?.ok_or("head")?;
            let from = receipt.inclusion.block_number;
            let update = quai_sdk::settlement::track_settlement(
                &provider,
                &mut store,
                id,
                hash,
                quai_sdk::settlement::SettlementKind::Conversion,
                quai_sdk::provider::EtxScanRequest::new(
                    scope.zone,
                    from,
                    head.number.min(from + 31),
                    4096,
                    65536,
                )
                .with_preceding_block(None),
                100,
            )
            .await?;
            let observation = update.conversion.ok_or("no conversion observation")?;
            let destination = accounts[1];
            println!(
                "{}",
                json!({"stage":"conversion-credit","operation":operation,"effect":format!("{:?}",observation.effect),"execution":observation.scan.as_ref().and_then(|s|s.execution.as_ref()).map(|e|json!({"hash":e.transaction.hash.to_string(),"block":e.transaction.inclusion.map(|i|i.block_number),"transaction":e.transaction.to_rpc_json().ok(),"outcome":e.receipt.as_ref().map(|r|format!("{:?}",r.outcome))})),"head":head.number,"balanceIts":provider.balance(destination,BlockTag::Latest).await?.to_string(),"lockedIts":provider.locked_quai_balance(destination).await?.balance.to_string()})
            );
        }
        "credit" if operation == "wrap" => {
            let bytes = store.signed_payload(id)?.ok_or("no signed wrap")?;
            let hash = SignedQiOperation::decode(&bytes)?.hash()?;
            let receipt = provider
                .receipt(scope.zone, hash)
                .await?
                .ok_or("missing wrap receipt")?;
            let head = provider.latest_header(scope.zone).await?.ok_or("head")?;
            let from = receipt.inclusion.block_number;
            let update = quai_sdk::settlement::track_settlement(
                &provider,
                &mut store,
                id,
                hash,
                quai_sdk::settlement::SettlementKind::QiWrapping,
                quai_sdk::provider::EtxScanRequest::new(
                    scope.zone,
                    from,
                    head.number.min(from + 31),
                    4096,
                    65536,
                )
                .with_preceding_block(None),
                100,
            )
            .await?;
            let external = update.external.ok_or("no external observation")?;
            let execution = external
                .scan
                .and_then(|s| s.execution)
                .ok_or("wrap destination not found in bounded range")?;
            let unclaimed = wrapper.unclaimed(beneficiary, BlockTag::Latest).await?;
            println!(
                "{}",
                json!({"stage":"backing-observed","executionHash":execution.transaction.hash.to_string(),"outcome":format!("{:?}",external.outcome),"unclaimedQits":unclaimed.to_string()})
            );
            if !matches!(
                external.outcome,
                Some(ReceiptOutcome::Succeeded | ReceiptOutcome::Locked)
            ) || unclaimed != U256::from(1000)
            {
                return Err("wrap backing not settled".into());
            }
        }
        "channels" => {
            println!(
                "{}",
                json!({"ownerCode":payment.public_code().to_base58(),"peerCode":peer.public_code().to_base58(),"send":store.payment_addresses(&payment,peer.public_code(),PaymentDirection::Send)?.iter().map(|p|json!({"address":p.address.to_string(),"index":p.index,"publicKey":quai_sdk::provider::RpcData::new(p.public_key.to_compressed().to_vec()).expect("fixed public key size").to_hex(),"burnedStart":p.burned.start,"burnedEnd":p.burned.end})).collect::<Vec<_>>()})
            );
        }
        _ => return Err("unsupported Qi extended stage".into()),
    }
    Ok(())
}
