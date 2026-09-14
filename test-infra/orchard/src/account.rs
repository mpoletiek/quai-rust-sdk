use super::*;
use quai_sdk::accounts::{AccountIntent, AccountObservationPolicy, AccountSession};
use quai_sdk::consensus::SignedQuaiTransaction;
use quai_sdk::provider::{ReceiptOutcome, RpcData, WaitConfig};
use quai_sdk::signer::{LocalSigner, Signer};
use quai_sdk::wallet::storage::{NetworkScope, PublicAddress, ReservationId, SqliteStore};
use std::{os::unix::fs::PermissionsExt, time::Duration};
pub async fn run(stage: &str, operation: &str) -> Result<(), Box<dyn Error>> {
    let content = Zeroizing::new(fs::read_to_string(dir().join("wallets.json"))?);
    if content.len() > 16384 {
        return Err("oversized wallet input".into());
    }
    let supplied: Supplied = serde_json::from_str(&content).map_err(|_| "invalid wallet JSON")?;
    if supplied.wallets.len() != 2 {
        return Err("expected two wallets".into());
    }
    let owner = match operation {
        "transfer-a-b"
        | "convert-quai-to-qi"
        | "convert-quai-to-qi-v2"
        | "wquai-deposit"
        | "wquai-withdraw"
        | "wqi-claim"
        | "wqi-unwrap" => 0,
        "transfer-b-a" | "conversion-credit-spend" => 1,
        op if conversion_number(op).is_some() => 0,
        _ => return Err("unsupported operation".into()),
    };
    // Mainnet phase one: amounts and verification reviewed for these operations only.
    if net().mainnet()
        && !matches!(
            operation,
            "transfer-a-b"
                | "transfer-b-a"
                | "convert-quai-to-qi"
                | "wquai-deposit"
                | "wquai-withdraw"
                | "conversion-credit-spend"
        )
        && conversion_number(operation).is_none()
    {
        return Err("operation is not enabled for mainnet qualification".into());
    }
    let keys = [
        load_key(supplied.wallets[0].private_key)?,
        load_key(supplied.wallets[1].private_key)?,
    ];
    let mut addresses = Vec::new();
    for (i, key) in keys.iter().enumerate() {
        let a: QuaiAddress = supplied.wallets[i]
            .address
            .parse()
            .map_err(|_| "invalid account address")?;
        if key.public_key().address() != a.address() || a.zone() != Zone::Cyprus1 {
            return Err("key/address/zone mismatch".into());
        }
        addresses.push(a);
    }
    if addresses[0] == addresses[1] {
        return Err("duplicate accounts".into());
    }
    let signer = LocalSigner::new(
        load_key(supplied.wallets[owner].private_key)?,
        U256::from(net().chain_id),
    )?;
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
    if provider.genesis_hash(Zone::Cyprus1).await? != scope.genesis {
        return Err("network genesis mismatch".into());
    }
    let state = dir().join("state");
    fs::create_dir_all(&state)?;
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700))?;
    let mut store = SqliteStore::open(state.join(format!("account-{owner}.sqlite")), scope)?;
    if store.addresses()?.is_empty() {
        store.import_metadata(0, &[PublicAddress::imported(&keys[owner].public_key())?])?;
    }
    let registered = store.addresses()?;
    if registered.len() != 1 || registered[0].address() != signer.address() {
        return Err("store owner mismatch".into());
    }
    let conversion = operation.starts_with("convert-quai-to-qi");
    let v2 = operation == "convert-quai-to-qi-v2";
    let numbered = conversion_number(operation);
    let wrapping = operation.starts_with("wquai-");
    let withdrawal = operation == "wquai-withdraw";
    let wqi = operation.starts_with("wqi-");
    let id = ReservationId(
        [if operation == "conversion-credit-spend" {
            2
        } else if operation == "wqi-claim" {
            6
        } else if operation == "wqi-unwrap" {
            7
        } else if withdrawal {
            5
        } else if wrapping {
            4
        } else if let Some(n) = numbered {
            10 + n
        } else if v2 {
            3
        } else if conversion {
            2
        } else {
            1
        }; 16],
    );
    match stage {
        "prepare" => {
            if store.reservation(id)?.is_some() {
                return Err("operation already reserved; use explicit recovery".into());
            }
            let latest = provider
                .transaction_count(addresses[owner], BlockTag::Latest)
                .await?;
            let pending = provider
                .transaction_count(addresses[owner], BlockTag::Pending)
                .await?;
            if latest != pending {
                return Err("pending account transactions; wait before preparing".into());
            }
            let value = if conversion {
                net().conversion_its
            } else if operation == "transfer-a-b" {
                net().transfer_a_b_its
            } else {
                U256::from(10_000_000_000_000_000u64)
            };
            let destination = if conversion {
                #[derive(Deserialize)]
                struct Saved<'a> {
                    #[serde(borrow)]
                    mnemonic: &'a str,
                    #[serde(borrow)]
                    passphrase: &'a str,
                    #[serde(borrow)]
                    hd_receive_address: &'a str,
                    hd_receive_index: u32,
                }
                let saved_content =
                    Zeroizing::new(fs::read_to_string(dir().join("qi-wallet.json"))?);
                let saved: Saved =
                    serde_json::from_str(&saved_content).map_err(|_| "invalid Qi wallet JSON")?;
                let mnemonic = Mnemonic::parse(Language::English, saved.mnemonic)?;
                let wallet = HdWallet::from_mnemonic(&mnemonic, saved.passphrase, CoinType::Qi)?;
                let (index, address) = if let Some(n) = numbered {
                    numbered_recipient(n, &wallet, saved.hd_receive_index, scope)?
                } else if v2 {
                    let public: serde_json::Value = serde_json::from_slice(&fs::read(
                        dir().join("conversion-v2-recipient.json"),
                    )?)?;
                    (
                        u32::try_from(public["index"].as_u64().ok_or("recipient index")?)?,
                        public["address"]
                            .as_str()
                            .ok_or("recipient address")?
                            .parse::<quai_sdk::QiAddress>()?,
                    )
                } else {
                    (
                        saved.hd_receive_index,
                        saved.hd_receive_address.parse::<quai_sdk::QiAddress>()?,
                    )
                };
                let key = wallet.derive_key(0, false, index)?.secret_key()?;
                if key.public_key().address() != address.address()
                    || address.zone() != Zone::Cyprus1
                {
                    return Err("Qi destination ownership mismatch".into());
                }
                let quoted = provider
                    .quai_to_qi(Zone::Cyprus1, value, BlockTag::Latest)
                    .await?
                    .ok_or("conversion rate unavailable")?;
                if quoted == U256::ZERO {
                    return Err("conversion quote is zero".into());
                }
                println!(
                    "{}",
                    serde_json::json!({"operation":operation,"valueBaseUnits":value.to_string(),"quotedQits":quoted.to_string(),"destination":address.to_string(),"slippageBasisPoints":net().conversion_slippage_bps})
                );
                Some(address)
            } else {
                None
            };
            let fee = net().account_fee;
            let wrapper_intent = if wqi {
                let (wrapper, _) = quai_sdk::wrappers::WrappedQi::new_verified(
                    quai_sdk::wrappers::WQI_ADDRESS.parse()?,
                    &provider,
                    scope.genesis,
                    None,
                    BlockTag::Latest,
                )
                .await?;
                let backing = wrapper
                    .unclaimed(addresses[owner], BlockTag::Latest)
                    .await?;
                let tokens = wrapper
                    .token()?
                    .balance_of(addresses[owner], addresses[owner], BlockTag::Latest)
                    .await?;
                let claiming = operation == "wqi-claim";
                if backing
                    != if claiming {
                        U256::from(1000)
                    } else {
                        U256::ZERO
                    }
                    || tokens
                        != if claiming {
                            U256::ZERO
                        } else {
                            quai_sdk::wrappers::qits_to_wqi_atoms(U256::from(1000))?
                        }
                {
                    return Err("unexpected WQI backing/token balance".into());
                }
                println!(
                    "{}",
                    serde_json::json!({"operation":operation,"stage":"WQI-preflight","unclaimedQits":backing.to_string(),"tokenAtoms":tokens.to_string()})
                );
                let call = if claiming {
                    wrapper.claim_deposit()?
                } else {
                    let (wallet, _) = super::qi_extended::load_named_wallet("qi-redemption.json")?;
                    let mut qi = SqliteStore::open(state.join("qi-redemption.sqlite"), scope)?;
                    let allocation =
                        qi.allocate_address(&wallet.account_public(0)?, false, 6000, || false)?;
                    let address: quai_sdk::QiAddress = allocation.address.address().try_into()?;
                    let metadata = serde_json::json!({"address":address.to_string(),"origin":format!("{:?}",allocation.address.origin())});
                    super::qi_extended::save("wqi-redemption-recipient.json", &metadata)?;
                    println!(
                        "{}",
                        serde_json::json!({"operation":operation,"stage":"redemption-recipient","metadata":metadata})
                    );
                    wrapper.unwrap(address, U256::from(1000), 30_000)?
                };
                Some(call.into_account_intent())
            } else if wrapping {
                let (wrapper, _) = quai_sdk::wrappers::WrappedQuai::new_verified(
                    net().wquai.parse()?,
                    &provider,
                    scope.genesis,
                    None,
                    BlockTag::Latest,
                )
                .await?;
                let balance = wrapper
                    .token()?
                    .balance_of(addresses[owner], addresses[owner], BlockTag::Latest)
                    .await?;
                let expected = if withdrawal { value } else { U256::ZERO };
                if balance != expected {
                    return Err("unexpected WQUAI balance for round-trip stage".into());
                }
                println!(
                    "{}",
                    serde_json::json!({"operation":operation,"stage":"wrapper-preflight","tokenAtoms":value.to_string(),"tokenBalanceBefore":balance.to_string(),"nativeBalanceBefore":provider.balance(addresses[owner],BlockTag::Latest).await?.to_string()})
                );
                Some(
                    if withdrawal {
                        wrapper.withdraw(value)?
                    } else {
                        wrapper.deposit(value)?
                    }
                    .into_account_intent(),
                )
            } else {
                None
            };
            let mut session = AccountSession::new(&provider, &signer, &mut store)?
                .with_observation_policy(AccountObservationPolicy::PinnedLatest);
            let prepared = if let Some(intent) = wrapper_intent {
                session.prepare(id, intent, fee).await?
            } else if let Some(destination) = destination {
                session
                    .prepare_conversion(
                        id,
                        destination,
                        value,
                        quai_sdk::consensus::ConversionSlippage::new(
                            net().conversion_slippage_bps,
                        )?,
                        fee,
                    )
                    .await?
            } else {
                session
                    .prepare(
                        id,
                        AccountIntent {
                            to: addresses[1 - owner],
                            value,
                            data: RpcData::new(vec![])?,
                            access_list: vec![],
                        },
                        fee,
                    )
                    .await?
            };
            let review = serde_json::json!({"operation":operation,"stage":"prepared","network":net().name,"chainId":net().chain_id,"from":addresses[owner].to_string(),"to":prepared.transaction().to.map(|a| a.to_string()),"valueBaseUnits":prepared.transaction().value.to_string(),"nonce":prepared.transaction().nonce,"gasLimit":prepared.transaction().gas_limit,"gasPrice":prepared.transaction().gas_price.to_string(),"maximumFeeBaseUnits":prepared.maximum_fee().to_string(),"signingDigest":prepared.signing_digest().to_string()});
            println!("{review}");
            let signed = session.sign(&prepared)?;
            let hash = signed.hash()?;
            println!(
                "{}",
                serde_json::json!({"operation":operation,"stage":"signed-and-persisted","hash":hash.to_string()})
            );
        }
        "prepare-replacement" if operation == "conversion-credit-spend" => {
            let mut session = AccountSession::new(&provider, &signer, &mut store)?
                .with_observation_policy(AccountObservationPolicy::PinnedLatest);
            let candidates = session.signed_candidates(id)?;
            if candidates.len() != 1 {
                return Err("replacement already prepared".into());
            }
            let prepared = session
                .prepare_replacement(
                    id,
                    candidates[0].hash()?,
                    quai_sdk::accounts::ReplacementPolicy {
                        minimum_price_bump_percent: 100,
                        fees: net().account_fee,
                    },
                )
                .await?;
            let signed = session.sign_replacement(&prepared)?;
            println!(
                "{}",
                serde_json::json!({"stage":"replacement-persisted","operation":operation,"parent":prepared.parent_hash().to_string(),"hash":signed.hash()?.to_string(),"nonce":signed.transaction().nonce,"gasPrice":signed.transaction().gas_price.to_string(),"valueIts":signed.transaction().value.to_string()})
            );
        }
        "broadcast-family" if operation == "conversion-credit-spend" => {
            let mut session = AccountSession::new(&provider, &signer, &mut store)?;
            let candidates = session.signed_candidates(id)?;
            if candidates.len() != 2 {
                return Err("expected original and one replacement".into());
            }
            for candidate in &candidates {
                if provider
                    .receipt(scope.zone, candidate.hash()?)
                    .await?
                    .is_some()
                {
                    return Err("family already mined; observe instead".into());
                }
            }
            for candidate in candidates {
                let ack = session.broadcast_candidate(id, candidate.hash()?).await?;
                println!(
                    "{}",
                    serde_json::json!({"stage":"candidate-acknowledged","operation":operation,"hash":ack.transaction_hash.to_string()})
                );
            }
        }
        "observe-family" if operation == "conversion-credit-spend" => {
            let mut session = AccountSession::new(&provider, &signer, &mut store)?;
            let observation = session.observe_candidates(id).await?;
            let hash = observation
                .canonical
                .ok_or("no canonical family member yet")?;
            let signed = session
                .signed_candidates(id)?
                .into_iter()
                .find(|c| c.hash().ok() == Some(hash))
                .ok_or("candidate absent")?;
            let mined = provider
                .transaction(scope.zone, hash)
                .await?
                .ok_or("mined transaction absent")?
                .verified_quai()?;
            if mined.signed_bytes()? != signed.signed_bytes()? {
                return Err("mined family bytes differ".into());
            }
            let receipt = provider
                .receipt(scope.zone, hash)
                .await?
                .ok_or("receipt absent")?;
            if receipt.outcome != ReceiptOutcome::Succeeded {
                return Err("spend failed".into());
            }
            println!(
                "{}",
                serde_json::json!({"stage":"family-verified","operation":operation,"canonical":hash.to_string(),"observation":format!("{observation:?}"),"receipt":receipt.to_rpc_json()?,"durableBytesMatch":true,"senderBalanceIts":provider.balance(addresses[owner],BlockTag::Latest).await?.to_string(),"recipientBalanceIts":provider.balance(addresses[1-owner],BlockTag::Latest).await?.to_string()})
            );
        }
        "broadcast" => {
            let stored = store.signed_payload(id)?.ok_or("no durable payload")?;
            let signed = SignedQuaiTransaction::decode(&stored)?;
            if signed.from().address() != signer.address() {
                return Err("payload owner mismatch".into());
            }
            if provider
                .receipt(Zone::Cyprus1, signed.hash()?)
                .await?
                .is_some()
            {
                return Err("receipt already exists; observe instead of resubmitting".into());
            }
            let mut session = AccountSession::new(&provider, &signer, &mut store)?;
            let ack = session.broadcast(id).await?;
            println!(
                "{}",
                serde_json::json!({"operation":operation,"stage":"acknowledged","hash":ack.transaction_hash.to_string()})
            );
        }
        "credit" => {
            if operation == "wqi-unwrap" {
                let stored = store.signed_payload(id)?.ok_or("no signed redemption")?;
                let hash = SignedQuaiTransaction::decode(&stored)?.hash()?;
                let receipt = provider
                    .receipt(scope.zone, hash)
                    .await?
                    .ok_or("no origin receipt")?;
                let from = receipt.inclusion.block_number;
                let head = provider.latest_header(scope.zone).await?.ok_or("head")?;
                let update = quai_sdk::settlement::track_settlement(
                    &provider,
                    &mut store,
                    id,
                    hash,
                    quai_sdk::settlement::SettlementKind::WqiRedemption {
                        contract: quai_sdk::wrappers::WQI_ADDRESS.parse()?,
                        etx_index: 0,
                    },
                    quai_sdk::provider::EtxScanRequest {
                        zone: scope.zone,
                        from,
                        to: head.number.min(from + 31),
                        max_transactions_per_block: 4096,
                        max_total_transactions: 65536,
                        preceding_block: None,
                    },
                    100,
                )
                .await?;
                let external = update.external.ok_or("no external observation")?;
                let execution = external
                    .scan
                    .and_then(|s| s.execution)
                    .ok_or("redemption destination not found in bounded range")?;
                let credit = update.qi_credit.ok_or("no Qi credit")?;
                println!(
                    "{}",
                    serde_json::json!({"stage":"redemption-credit","executionHash":execution.transaction.hash.to_string(),"outcome":format!("{:?}",external.outcome),"observedHead":credit.head.number,"lockedQits":credit.locked_qits.to_string(),"unlockedQits":credit.unlocked_qits.to_string(),"unobservedQits":credit.unobserved_qits.to_string(),"outputs":credit.outputs.iter().map(|o|serde_json::json!({"hash":o.outpoint.tx_hash.to_string(),"index":o.outpoint.index,"denomination":o.denomination,"lock":o.lock.to_string()})).collect::<Vec<_>>() })
                );
                if credit.unobserved_qits != U256::ZERO
                    || credit.locked_qits + credit.unlocked_qits != U256::from(1000)
                {
                    return Err("incomplete WQI redemption".into());
                }
                return Ok(());
            }
            if !conversion {
                return Err("credit observation requires a conversion".into());
            }
            let stored = store.signed_payload(id)?.ok_or("no durable conversion")?;
            let signed = SignedQuaiTransaction::decode(&stored)?;
            let reference =
                quai_sdk::provider::ConversionReference::from_quai(scope.genesis, &signed)?;
            let hash = signed.hash()?;
            let receipt = provider
                .receipt(Zone::Cyprus1, hash)
                .await?
                .ok_or("origin receipt absent")?;
            let head = u64::try_from(provider.block_number(Zone::Cyprus1.into()).await?)
                .map_err(|_| "height range")?;
            let cursor_path = state.join(match numbered {
                Some(n) => format!("conversion-{n}-cursor.json"),
                None if v2 => "conversion-v2-cursor.json".into(),
                None => "conversion-cursor.json".into(),
            });
            let (from, preceding) = if cursor_path.exists() {
                let c: serde_json::Value = serde_json::from_slice(&fs::read(&cursor_path)?)?;
                if c["originHash"] != hash.to_string() {
                    return Err("conversion cursor identity mismatch".into());
                }
                let n = c["lastNumber"].as_u64().ok_or("cursor number")?;
                let h = c["lastHash"].as_str().ok_or("cursor hash")?.parse()?;
                (
                    n.checked_add(1).ok_or("cursor overflow")?,
                    Some(quai_sdk::provider::BlockReference { number: n, hash: h }),
                )
            } else {
                (receipt.inclusion.block_number, None)
            };
            if from > head {
                println!(
                    "Conversion scan is caught up; destination execution remains subject to later observation."
                );
                return Ok(());
            }
            let to = head.min(from.checked_add(15).ok_or("range overflow")?);
            let (observation, credit) = provider
                .observe_conversion_qi_credit(
                    &reference,
                    quai_sdk::provider::EtxScanRequest {
                        zone: Zone::Cyprus1,
                        from,
                        to,
                        max_transactions_per_block: 4096,
                        max_total_transactions: 65_536,
                        preceding_block: preceding,
                    },
                    4096,
                )
                .await?;
            let origin = match &observation.origin {
                quai_sdk::provider::ConversionOriginObservation::Emitted { .. } => "emitted",
                quai_sdk::provider::ConversionOriginObservation::Failed { .. } => "failed",
                quai_sdk::provider::ConversionOriginObservation::Unavailable => "unavailable",
                quai_sdk::provider::ConversionOriginObservation::Noncanonical { .. } => {
                    "noncanonical"
                }
                quai_sdk::provider::ConversionOriginObservation::EmissionNotObserved { .. } => {
                    "emission-not-observed"
                }
            };
            let current_credit=credit.as_ref().map(|c|serde_json::json!({"beneficiary":c.beneficiary.to_string(),"executionHash":c.transaction_hash.to_string(),"creatingHash":c.creating_hash.to_string(),"executionBlock":c.execution.number,"observedHead":c.head.number,"lockedQits":c.locked_qits.to_string(),"unlockedQits":c.unlocked_qits.to_string(),"unobservedQits":c.unobserved_qits.to_string(),"outputs":c.outputs.iter().map(|p|serde_json::json!({"hash":p.outpoint.tx_hash.to_string(),"index":p.outpoint.index,"denomination":p.denomination,"lock":p.lock.to_string()})).collect::<Vec<_>>()}));
            let result = serde_json::json!({"operation":operation,"originHash":hash.to_string(),"from":from,"to":to,"origin":origin,"effect":format!("{:?}",observation.effect),"coverage":observation.scan.as_ref().map(|s|format!("{:?}",s.coverage)),"examined":observation.scan.as_ref().map(|s|s.transactions_examined),"executionHash":observation.scan.as_ref().and_then(|s|s.execution.as_ref()).map(|e|e.transaction.hash.to_string()),"qiCredit":current_credit});
            println!("{result}");
            let output = state.join(format!("{operation}-scan-{from}-{to}.json"));
            fs::write(output, serde_json::to_vec_pretty(&result)?)?;
            if let Some(scan) = &observation.scan
                && scan.execution.is_none()
                && let Some(last) = scan.last_block
            {
                let value = serde_json::json!({"originHash":hash.to_string(),"lastNumber":last.number,"lastHash":last.hash.to_string()});
                let tmp = state.join(format!("{operation}-cursor.pending"));
                let mut f = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&tmp)?;
                serde_json::to_writer(&mut f, &value)?;
                f.sync_all()?;
                fs::rename(&tmp, &cursor_path)?;
                fs::File::open(&state)?.sync_all()?;
            }
        }
        "observe" => {
            let stored = store.signed_payload(id)?.ok_or("no durable payload")?;
            let signed = SignedQuaiTransaction::decode(&stored)?;
            let hash = signed.hash()?;
            let observed = provider
                .wait_for_receipt(
                    Zone::Cyprus1,
                    hash,
                    WaitConfig {
                        confirmations: 2,
                        timeout: Duration::from_secs(90),
                        poll_interval: Duration::from_secs(2),
                    },
                )
                .await?;
            println!(
                "{}",
                serde_json::json!({"operation":operation,"stage":"observed","confirmations":observed.confirmations,"receipt":observed.receipt.to_rpc_json()?})
            );
            if observed.receipt.outcome != ReceiptOutcome::Succeeded {
                return Err("receipt did not report execution success".into());
            }
            let transaction = provider
                .transaction(Zone::Cyprus1, hash)
                .await?
                .ok_or("mined transaction absent")?;
            let verified = transaction.verified_quai()?;
            if verified.signed_bytes()? != stored {
                return Err("mined signed bytes differ from custody".into());
            }
            println!(
                "Signed transaction bytes independently reconstructed from the mined RPC response match durable custody."
            );
            if wrapping {
                let wrapper =
                    quai_sdk::wrappers::WrappedQuai::new(net().wquai.parse()?, &provider)?;
                let block = BlockTag::Number(U256::from(observed.receipt.inclusion.block_number));
                let balance = wrapper
                    .token()?
                    .balance_of(addresses[owner], addresses[owner], block)
                    .await?;
                let expected = if withdrawal {
                    U256::ZERO
                } else {
                    U256::from(10_000_000_000_000_000u64)
                };
                println!(
                    "{}",
                    serde_json::json!({"operation":operation,"stage":"wrapper-observed","tokenBalanceAfter":balance.to_string(),"nativeBalanceAfter":provider.balance(addresses[owner],block).await?.to_string(),"feeBaseUnits":observed.receipt.fee()?.to_string()})
                );
                if balance != expected {
                    return Err("unexpected WQUAI balance after execution".into());
                }
            }
            if wqi {
                let wrapper = quai_sdk::wrappers::WrappedQi::new(
                    quai_sdk::wrappers::WQI_ADDRESS.parse()?,
                    &provider,
                )?;
                let block = BlockTag::Number(U256::from(observed.receipt.inclusion.block_number));
                let tokens = wrapper
                    .token()?
                    .balance_of(addresses[owner], addresses[owner], block)
                    .await?;
                let backing = wrapper.unclaimed(addresses[owner], block).await?;
                let expected = if operation == "wqi-claim" {
                    quai_sdk::wrappers::qits_to_wqi_atoms(U256::from(1000))?
                } else {
                    U256::ZERO
                };
                println!(
                    "{}",
                    serde_json::json!({"operation":operation,"stage":"WQI-observed","tokenAtoms":tokens.to_string(),"unclaimedQits":backing.to_string(),"feeBaseUnits":observed.receipt.fee()?.to_string()})
                );
                if tokens != expected || backing != U256::ZERO {
                    return Err("WQI balance mismatch after execution".into());
                }
            }
        }
        _ => return Err("expected prepare, broadcast or observe".into()),
    }
    Ok(())
}

/// Numbered mainnet follow-up conversions: `convert-quai-to-qi-2` through `-9`.
fn conversion_number(operation: &str) -> Option<u8> {
    let n: u8 = operation
        .strip_prefix("convert-quai-to-qi-")?
        .parse()
        .ok()?;
    (2..=9).contains(&n).then_some(n)
}

/// A fresh compact receive address per numbered conversion. The public recipient
/// file is written once; a preparation that never reserved may reuse it safely.
fn numbered_recipient(
    n: u8,
    wallet: &HdWallet,
    base_index: u32,
    scope: NetworkScope,
) -> Result<(u32, quai_sdk::QiAddress), Box<dyn Error>> {
    let path = dir().join(format!("conversion-{n}-recipient.json"));
    if !path.exists() {
        let account = wallet.account_public(0)?;
        let mut qi = SqliteStore::open(dir().join("state/qi.sqlite"), scope)?;
        if qi.addresses()?.is_empty() {
            qi.import_metadata(0, &[PublicAddress::derive(&account, false, base_index)?])?;
        }
        let allocation = qi.allocate_address_compact(&account, false, 100_000, || false)?;
        let quai_sdk::wallet::metadata::KeyOrigin::Bip44 { index, .. } =
            allocation.address.origin()
        else {
            return Err("expected HD allocation".into());
        };
        let value =
            serde_json::json!({"address":allocation.address.address().to_string(),"index":index});
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        serde_json::to_writer(&mut file, &value)?;
        file.sync_all()?;
        fs::File::open(dir())?.sync_all()?;
    }
    let public: serde_json::Value = serde_json::from_slice(&fs::read(&path)?)?;
    Ok((
        u32::try_from(public["index"].as_u64().ok_or("recipient index")?)?,
        public["address"]
            .as_str()
            .ok_or("recipient address")?
            .parse()?,
    ))
}
