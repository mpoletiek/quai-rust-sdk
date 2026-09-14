use super::*;
use quai_sdk::accounts::{AccountIntent, AccountObservationPolicy, AccountSession, FeePolicy};
use quai_sdk::consensus::SignedQuaiTransaction;
use quai_sdk::provider::{ReceiptOutcome, RpcData, WaitConfig};
use quai_sdk::signer::{LocalSigner, Signer};
use quai_sdk::wallet::storage::{NetworkScope, PublicAddress, ReservationId, SqliteStore};
use std::{os::unix::fs::PermissionsExt, time::Duration};
const GENESIS: &str = "0x663a73416275109a01aad3a4c29ea9e310aded63c5eea491243b7312ad8cd16b";
pub async fn run(stage: &str, operation: &str) -> Result<(), Box<dyn Error>> {
    let content = Zeroizing::new(fs::read_to_string(Path::new(DIR).join("wallets.json"))?);
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
        | "wquai-withdraw" => 0,
        "transfer-b-a" => 1,
        _ => return Err("unsupported operation".into()),
    };
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
        U256::from(15000),
    )?;
    let scope = NetworkScope {
        chain_id: U256::from(15000),
        genesis: GENESIS.parse()?,
        zone: Zone::Cyprus1,
    };
    let provider = Provider::new(
        DiagnosticTransport(HttpTransport::new(HttpConfig::default())?),
        Routing::direct(
            "https://orchard.rpc.quai.network/cyprus1",
            Zone::Cyprus1.into(),
        )?,
        scope.chain_id,
    );
    if provider.genesis_hash(Zone::Cyprus1).await? != scope.genesis {
        return Err("Orchard genesis mismatch".into());
    }
    let state = Path::new(DIR).join("state");
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
    let wrapping = operation.starts_with("wquai-");
    let withdrawal = operation == "wquai-withdraw";
    let id = ReservationId(
        [if withdrawal {
            5
        } else if wrapping {
            4
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
            let value = U256::from(if conversion {
                quai_sdk::consensus::MIN_QUAI_CONVERSION_VALUE
            } else {
                10_000_000_000_000_000u64
            });
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
                    Zeroizing::new(fs::read_to_string(Path::new(DIR).join("qi-wallet.json"))?);
                let saved: Saved =
                    serde_json::from_str(&saved_content).map_err(|_| "invalid Qi wallet JSON")?;
                let mnemonic = Mnemonic::parse(Language::English, saved.mnemonic)?;
                let wallet = HdWallet::from_mnemonic(&mnemonic, saved.passphrase, CoinType::Qi)?;
                let (index, address) = if v2 {
                    let public: serde_json::Value = serde_json::from_slice(&fs::read(
                        Path::new(DIR).join("conversion-v2-recipient.json"),
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
                    serde_json::json!({"operation":operation,"valueBaseUnits":value.to_string(),"quotedQits":quoted.to_string(),"destination":address.to_string(),"slippageBasisPoints":100})
                );
                Some(address)
            } else {
                None
            };
            let fee = FeePolicy {
                max_gas: 500_000,
                max_gas_price: U256::from(100_000_000_000u64),
                max_total_fee: U256::from(10_000_000_000_000_000u64),
                gas_margin_bps: 1000,
            };
            let wrapper_intent = if wrapping {
                let (wrapper, _) = quai_sdk::wrappers::WrappedQuai::new_verified(
                    quai_sdk::wrappers::WQUAI_ORCHARD_ADDRESS.parse()?,
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
                        quai_sdk::consensus::ConversionSlippage::new(100)?,
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
            let review = serde_json::json!({"operation":operation,"stage":"prepared","chainId":15000,"from":addresses[owner].to_string(),"to":prepared.transaction().to.map(|a| a.to_string()),"valueBaseUnits":prepared.transaction().value.to_string(),"nonce":prepared.transaction().nonce,"gasLimit":prepared.transaction().gas_limit,"gasPrice":prepared.transaction().gas_price.to_string(),"maximumFeeBaseUnits":prepared.maximum_fee().to_string(),"signingDigest":prepared.signing_digest().to_string()});
            println!("{review}");
            let signed = session.sign(&prepared)?;
            let hash = signed.hash()?;
            println!(
                "{}",
                serde_json::json!({"operation":operation,"stage":"signed-and-persisted","hash":hash.to_string()})
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
            let cursor_path = state.join(if v2 {
                "conversion-v2-cursor.json"
            } else {
                "conversion-cursor.json"
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
                let wrapper = quai_sdk::wrappers::WrappedQuai::new(
                    quai_sdk::wrappers::WQUAI_ORCHARD_ADDRESS.parse()?,
                    &provider,
                )?;
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
        }
        _ => return Err("expected prepare, broadcast or observe".into()),
    }
    Ok(())
}
