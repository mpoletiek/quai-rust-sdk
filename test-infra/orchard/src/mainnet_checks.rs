//! Read-only mainnet checks against locked Qi. Every check uses fresh throwaway
//! stores under `checks/`; durable custody stores are only copied, never opened.
use super::*;
use quai_sdk::consensus::{
    ConversionSlippage, Denomination, QiConversionIntent, QiConversionTransaction, QiInput,
    QiOutput, QiWrappingIntent, QiWrappingTransaction,
};
use quai_sdk::provider::QiFeeProfile;
use quai_sdk::qi::{QiChangePool, QiIntent, QiPolicy, QiSession};
use quai_sdk::qi_discovery::{QiScanOptions, qi_balance, scan_and_refresh_qi};
use quai_sdk::wallet::full_backup::{BackupOrigin, WalletBackup};
use quai_sdk::wallet::metadata::KeyOrigin;
use quai_sdk::wallet::storage::{NetworkScope, ReservationId, SqliteStore};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Qits credited by conversions 2, 4 and 6 on 2026-09-14.
const CREDITED_QITS: &str = "3416"; // 689 + 1362 + 1365

fn scope() -> Result<NetworkScope, Box<dyn Error>> {
    Ok(NetworkScope {
        chain_id: U256::from(net().chain_id),
        genesis: net().genesis.parse()?,
        zone: Zone::Cyprus1,
    })
}

fn checks_dir() -> Result<std::path::PathBuf, Box<dyn Error>> {
    let path = dir().join("checks");
    fs::create_dir_all(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    Ok(path)
}

fn stamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn record(name: &str, value: &serde_json::Value) -> Result<(), Box<dyn Error>> {
    let path = checks_dir()?.join(format!("{name}-{}.json", stamp()));
    fs::write(&path, serde_json::to_vec_pretty(value)?)?;
    println!("{value}");
    Ok(())
}

type SeededWallet = (HdWallet, Zeroizing<Vec<u8>>);

fn wallet() -> Result<SeededWallet, Box<dyn Error>> {
    #[derive(Deserialize)]
    struct Saved<'a> {
        #[serde(borrow)]
        mnemonic: &'a str,
        #[serde(borrow)]
        passphrase: &'a str,
    }
    let content = Zeroizing::new(fs::read_to_string(dir().join("qi-wallet.json"))?);
    let saved: Saved = serde_json::from_str(&content).map_err(|_| "invalid Qi wallet JSON")?;
    let mnemonic = Mnemonic::parse(Language::English, saved.mnemonic)?;
    let seed = Zeroizing::new(mnemonic.to_seed(saved.passphrase).expose().to_vec());
    Ok((
        HdWallet::from_mnemonic(&mnemonic, saved.passphrase, CoinType::Qi)?,
        seed,
    ))
}

fn provider() -> Result<Provider<DiagnosticTransport>, Box<dyn Error>> {
    Ok(Provider::new(
        DiagnosticTransport(HttpTransport::new(HttpConfig::default())?),
        Routing::direct(net().endpoint, Zone::Cyprus1.into())?,
        U256::from(net().chain_id),
    ))
}

/// Seed-only default-gap discovery into a new store, repeated on head changes.
async fn recover(
    provider: &Provider<DiagnosticTransport>,
    wallet: &HdWallet,
    path: &Path,
) -> Result<(SqliteStore, serde_json::Value), Box<dyn Error>> {
    if path.exists() {
        return Err("recovery store must be fresh".into());
    }
    let mut store = SqliteStore::open(path, scope()?)?;
    let account = wallet.account_public(0)?;
    let mut last = None;
    for attempt in 1..=4 {
        match scan_and_refresh_qi(
            provider,
            &mut store,
            &account,
            &QiScanOptions::default(),
            || false,
        )
        .await
        {
            Ok(report) => {
                last = Some(report);
                break;
            }
            Err(quai_sdk::qi::QiError::StaleSnapshot) if attempt < 4 => {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
    let report = last.ok_or("no stable recovery scan")?;
    let snapshot = store.snapshot()?;
    let checkpoint = snapshot.checkpoint.ok_or("missing checkpoint")?;
    let balance = qi_balance(&mut store, checkpoint.height + U256::from(1))?;
    let funded: std::collections::BTreeSet<_> = snapshot
        .coins
        .iter()
        .map(|c| c.address.to_string())
        .collect();
    let summary = json!({
        "checkpointHeight":checkpoint.height.to_string(),
        "derivedAddresses":report.addresses.len(),
        "nextRawChild":report.next_index,
        "stopped":format!("{:?}",report.stopped),
        "fundedAddresses":funded,
        "coins":snapshot.coins.iter().map(|c|json!({"address":c.address.to_string(),"hash":c.outpoint.transaction_hash.to_string(),"index":c.outpoint.index,"qits":c.denomination.value(),"unlockHeight":c.unlock_height.to_string()})).collect::<Vec<_>>(),
        "balance":{"total":balance.total.to_string(),"spendable":balance.spendable.to_string(),"locked":balance.locked.to_string(),"reserved":balance.reserved.to_string(),"expired":balance.expired.to_string()},
    });
    Ok((store, summary))
}

fn expected_recipients() -> Result<Vec<String>, Box<dyn Error>> {
    let mut out = Vec::new();
    for n in [2, 4, 6] {
        let value: serde_json::Value = serde_json::from_slice(&fs::read(
            dir().join(format!("conversion-{n}-recipient.json")),
        )?)?;
        out.push(value["address"].as_str().ok_or("recipient")?.to_string());
    }
    Ok(out)
}

fn input(
    wallet: &HdWallet,
    store: &SqliteStore,
    coin: &quai_sdk::wallet::CandidateCoin,
) -> Result<QiInput, Box<dyn Error>> {
    let metadata = store
        .addresses()?
        .into_iter()
        .find(|a| a.address() == coin.address.address())
        .ok_or("coin owner metadata")?;
    let KeyOrigin::Bip44 { change, index, .. } = metadata.origin() else {
        return Err("expected HD owner".into());
    };
    Ok(QiInput {
        previous_output: coin.outpoint,
        public_key: wallet
            .derive_key(0, change, index)?
            .secret_key()?
            .public_key(),
    })
}

pub async fn run(check: &str) -> Result<(), Box<dyn Error>> {
    if !net().mainnet() {
        return Err("mainnet checks require QUAI_QUALIFICATION_NETWORK=mainnet".into());
    }
    let provider = provider()?;
    let (wallet, seed) = wallet()?;
    let t = stamp();
    match check {
        "recovery" => {
            let (_, summary) = recover(
                &provider,
                &wallet,
                &checks_dir()?.join(format!("recovery-{t}.sqlite")),
            )
            .await?;
            let funded: Vec<String> = serde_json::from_value(summary["fundedAddresses"].clone())?;
            let expected = expected_recipients()?;
            let all_found = expected.iter().all(|a| funded.contains(a));
            // The wallet keeps spending after the conversions, so compare the
            // seed-only restore with custody's current coins rather than with
            // the balance recorded when the credit arrived.
            let coin_set = |coins: &serde_json::Value| -> std::collections::BTreeSet<String> {
                coins
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|c| format!("{}:{}:{}", c["hash"], c["index"], c["qits"]))
                    .collect()
            };
            let recovered = coin_set(&summary["coins"]);
            let custody = {
                let mut store = SqliteStore::open(dir().join("state/qi.sqlite"), scope()?)?;
                quai_sdk::qi_discovery::refresh_qi(&provider, &mut store, 1000, || false).await?;
                let coins = store.snapshot()?.coins;
                coin_set(&json!(coins.iter().map(|c| json!({"hash":c.outpoint.transaction_hash.to_string(),"index":c.outpoint.index,"qits":c.denomination.value()})).collect::<Vec<_>>()))
            };
            let credited: u64 = summary["coins"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|c| expected.iter().any(|a| c["address"] == a.as_str()))
                .filter_map(|c| c["qits"].as_u64())
                .sum();
            let matches_custody = recovered == custody;
            let credit_intact = credited.to_string() == CREDITED_QITS;
            record(
                "recovery",
                &json!({"check":"seed-only-gap-50-recovery","expectedRecipients":expected,"allRecipientsFound":all_found,"creditIntact":credit_intact,"matchesCustody":matches_custody,"custodyCoins":custody.len(),"result":summary}),
            )?;
            if !all_found || !credit_intact || !matches_custody {
                return Err(
                    "recovery did not reproduce custody's coins and the conversion credit".into(),
                );
            }
        }
        "locked-spend" => {
            let (mut store, summary) = recover(
                &provider,
                &wallet,
                &checks_dir()?.join(format!("locked-spend-{t}.sqlite")),
            )
            .await?;
            let account = wallet.account_public(0)?;
            let receiver = store.allocate_address_compact(&account, false, 100_000, || false)?;
            let pool = QiChangePool::allocate(&mut store, &account, 8, 6000, || false)?;
            // New ownership metadata invalidates the snapshot; refresh after allocating
            // so any refusal is about coin eligibility, not a missing snapshot.
            let mut refreshed = false;
            for attempt in 1..=4 {
                match quai_sdk::qi_discovery::refresh_qi(&provider, &mut store, 1000, || false)
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
                return Err("no stable refresh before spend attempt".into());
            }
            let id = ReservationId([0x7e; 16]);
            let result = QiSession::new(&provider, &wallet, &mut store)?
                .prepare(
                    id,
                    QiIntent {
                        amount: U256::from(100),
                        destinations: vec![receiver.address.address().try_into()?],
                    },
                    QiPolicy {
                        initial_fee: U256::ZERO,
                        max_fee: U256::from(100),
                        max_inputs: 8,
                        max_outputs: 16,
                        max_fee_rounds: 8,
                        max_snapshot_age: 5,
                    },
                    pool,
                )
                .await;
            let refused = matches!(
                result,
                Err(quai_sdk::qi::QiError::Selection(
                    quai_sdk::wallet::SelectionError::InsufficientFunds
                ))
            );
            let error = result.err().map(|e| format!("{e:?}"));
            let untouched = store.reservation(id)?.is_none() && store.signed_payload(id)?.is_none();
            record(
                "locked-spend",
                &json!({"check":"locked-coins-not-selectable","refused":refused,"error":error,"noReservationOrSignature":untouched,"balance":summary["balance"]}),
            )?;
            if !refused || !untouched {
                return Err("locked Qi was selectable or left custody state".into());
            }
        }
        "special-fee" => {
            let (mut store, _) = recover(
                &provider,
                &wallet,
                &checks_dir()?.join(format!("special-fee-{t}.sqlite")),
            )
            .await?;
            let snapshot = store.snapshot()?;
            let mut ones: Vec<_> = snapshot
                .coins
                .iter()
                .filter(|c| c.denomination.value() == 1000)
                .collect();
            let hundreds: Vec<_> = snapshot
                .coins
                .iter()
                .filter(|c| c.denomination.value() == 100)
                .collect();
            ones.sort_by_key(|c| c.outpoint);
            if ones.len() < 2 || hundreds.len() < 2 {
                return Err("expected two 1 Qi and two 0.1 Qi locked outputs".into());
            }
            let account = wallet.account_public(0)?;
            let change = store.allocate_address_compact(&account, true, 100_000, || false)?;
            let refund = store.allocate_address_compact(&account, false, 100_000, || false)?;
            let content = Zeroizing::new(fs::read_to_string(dir().join("wallets.json"))?);
            let supplied: Supplied = serde_json::from_str(&content).map_err(|_| "wallet JSON")?;
            let a: QuaiAddress = supplied.wallets[0].address.parse()?;
            let b: QuaiAddress = supplied.wallets[1].address.parse()?;
            let change_output = QiOutput {
                denomination: Denomination::new(4)?,
                address: change.address.address(),
            };
            // Planned reversal: 1.2 Qi in, 1 Qi converted, 0.1 Qi change, 0.1 Qi fee.
            let conversion = QiConversionTransaction::new(
                U256::from(net().chain_id),
                vec![
                    input(&wallet, &store, ones[0])?,
                    input(&wallet, &store, hundreds[0])?,
                    input(&wallet, &store, hundreds[1])?,
                ],
                vec![Denomination::new(6)?],
                vec![change_output.clone()],
                QiConversionIntent {
                    destination: b,
                    refund: refund.address.address().try_into()?,
                    slippage: ConversionSlippage::new(2000)?,
                },
            )?;
            // Planned WQI wrap: 1.1 Qi in, 1 Qi backing, 0.1 Qi fee.
            let wrapping = QiWrappingTransaction::new(
                U256::from(net().chain_id),
                vec![
                    input(&wallet, &store, ones[1])?,
                    input(&wallet, &store, hundreds[0])?,
                ],
                vec![Denomination::new(6)?],
                vec![],
                QiWrappingIntent {
                    destination: a,
                    owner_contract: quai_sdk::wrappers::WQI_ADDRESS.parse()?,
                },
            )?;
            let mut quotes = Vec::new();
            for (name, tx) in [
                ("qi-to-quai-1qi", conversion.transaction()),
                ("wqi-wrap-1qi", wrapping.transaction()),
            ] {
                for sample in 0..2 {
                    let quote = provider
                        .estimate_qi_special_fee(tx, QiFeeProfile::V056ShaAnchored)
                        .await?;
                    let recomputed = quai_sdk::provider::qi_special_gas(
                        tx.inputs.len(),
                        tx.outputs.len(),
                        quote.utxo_set_size,
                    )?;
                    let its = quote.gas_price * U256::from(quote.required_gas);
                    let raw = provider
                        .quai_to_qi(
                            Zone::Cyprus1,
                            its,
                            BlockTag::Number(U256::from(quote.block_number)),
                        )
                        .await?;
                    quotes.push(json!({"shape":name,"sample":sample,"inputs":tx.inputs.len(),"outputs":tx.outputs.len(),
                        "blockNumber":quote.block_number,"utxoSetSize":quote.utxo_set_size,"requiredGas":quote.required_gas,
                        "gasRecomputed":recomputed,"gasMatches":recomputed==quote.required_gas,"gasPriceIts":quote.gas_price.to_string(),
                        "feeIts":its.to_string(),"quoteQits":quote.qits.to_string(),"independentQuaiToQiQits":raw.map(|q|q.to_string())}));
                }
            }
            record(
                "special-fee",
                &json!({"check":"automatic-special-fee-estimation","profile":"V056ShaAnchored","note":"unsigned shapes from locked outputs; nothing reserved or signed","quotes":quotes}),
            )?;
        }
        "backup" => {
            let checks = checks_dir()?;
            let state = dir().join("state");
            let mut results = Vec::new();
            // Account custody: copy the real store; capture with its imported key origin.
            let content = Zeroizing::new(fs::read_to_string(dir().join("wallets.json"))?);
            let supplied: Supplied = serde_json::from_str(&content).map_err(|_| "wallet JSON")?;
            for owner in [0usize, 1] {
                let source = state.join(format!("account-{owner}.sqlite"));
                let copy = checks.join(format!("backup-account-{owner}-source-{t}.sqlite"));
                fs::copy(&source, &copy)?;
                let mut original = SqliteStore::open(&copy, scope()?)?;
                let key = load_key(supplied.wallets[owner].private_key)?;
                let backup = WalletBackup::capture(
                    &mut original,
                    vec![BackupOrigin::from_private_key(&key)],
                )?;
                let password = Zeroizing::new(format!("check-{t}-{owner}").into_bytes());
                let encrypted = backup.encrypt(&password, Default::default())?;
                let bytes = encrypted.as_bytes().to_vec();
                let restored_backup =
                    quai_sdk::wallet::full_backup::EncryptedWalletBackup::from_bytes(&bytes)?
                        .decrypt(&password)?;
                let wrong_password_rejected =
                    quai_sdk::wallet::full_backup::EncryptedWalletBackup::from_bytes(&bytes)?
                        .decrypt(b"wrong-password-value")
                        .is_err();
                let mut restored = SqliteStore::open(
                    checks.join(format!("backup-account-{owner}-restored-{t}.sqlite")),
                    scope()?,
                )?;
                let report = restored_backup.restore(&mut restored)?;
                let mut matched = 0;
                let reservations = original.reservations(None, 1000)?;
                for r in &reservations {
                    if restored.reservation(r.id)?.map(|x| format!("{x:?}"))
                        == Some(format!("{r:?}"))
                        && restored.signed_payload(r.id)? == original.signed_payload(r.id)?
                    {
                        matched += 1;
                    }
                }
                results.push(json!({"store":format!("account-{owner}"),"encryptedBytes":bytes.len(),"wrongPasswordRejected":wrong_password_rejected,
                    "reservations":reservations.len(),"reservationsAndPayloadsMatched":matched,
                    "retainedSigned":report.retained_signed_operations,"hashOnly":report.hash_only_operations,"rescanRequired":report.rescan_required,
                    "addressesMatch":restored.addresses()? == original.addresses()?}));
            }
            // Qi discovery state: recover, capture with the seed origin, restore, rescan.
            let (mut qi, summary) = recover(
                &provider,
                &wallet,
                &checks.join(format!("backup-qi-source-{t}.sqlite")),
            )
            .await?;
            let backup = WalletBackup::capture(&mut qi, vec![BackupOrigin::from_seed(&seed)?])?;
            let password = Zeroizing::new(format!("check-{t}-qi").into_bytes());
            let bytes = backup
                .encrypt(&password, Default::default())?
                .as_bytes()
                .to_vec();
            let decoded = quai_sdk::wallet::full_backup::EncryptedWalletBackup::from_bytes(&bytes)?
                .decrypt(&password)?;
            let mut restored = SqliteStore::open(
                checks.join(format!("backup-qi-restored-{t}.sqlite")),
                scope()?,
            )?;
            let report = decoded.restore(&mut restored)?;
            let addresses_match = restored.addresses()? == qi.addresses()?;
            let snapshot_invalidated = restored.snapshot()?.checkpoint.is_none();
            let mut refreshed = None;
            for attempt in 1..=4 {
                match quai_sdk::qi_discovery::refresh_qi(&provider, &mut restored, 1000, || false)
                    .await
                {
                    Ok(c) => {
                        refreshed = Some(c);
                        break;
                    }
                    Err(quai_sdk::qi::QiError::StaleSnapshot) if attempt < 4 => {
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            let checkpoint = refreshed.ok_or("no stable refresh after restore")?;
            let balance = qi_balance(&mut restored, checkpoint.height + U256::from(1))?;
            results.push(json!({"store":"qi-recovered","encryptedBytes":bytes.len(),"addressesMatch":addresses_match,"snapshotInvalidatedOnRestore":snapshot_invalidated,
                "rescanRequired":report.rescan_required,"sourceBalance":summary["balance"],"restoredRefreshBalance":{"total":balance.total.to_string(),"locked":balance.locked.to_string(),"spendable":balance.spendable.to_string()}}));
            record(
                "backup",
                &json!({"check":"authenticated-full-backup-restore","kdf":"Argon2id default (64 MiB, 3 passes, 4 lanes)","results":results}),
            )?;
        }
        "events" => {
            let content = Zeroizing::new(fs::read_to_string(dir().join("wallets.json"))?);
            let supplied: Supplied = serde_json::from_str(&content).map_err(|_| "wallet JSON")?;
            let owner: QuaiAddress = supplied.wallets[0].address.parse()?;
            let wquai: QuaiAddress = net().wquai.parse()?;
            let interface = quai_sdk::abi::AbiInterface::from_human_readable(&[
                "event Deposit(address indexed dst, uint256 wad)",
                "event Withdrawal(address indexed src, uint256 wad)",
            ])?;
            // Deposit/withdraw receipts from 2026-09-14 (0x9a1128 and 0x9a112c).
            let logs = provider
                .logs(&quai_sdk::provider::LogFilter {
                    zone: Zone::Cyprus1,
                    range: quai_sdk::provider::LogRange::Inclusive {
                        from: 0x9a1128,
                        to: 0x9a112c,
                    },
                    addresses: vec![wquai.address()],
                    topics: vec![],
                })
                .await?;
            let mut decoded = Vec::new();
            for log in &logs {
                let event = interface.event_by_topic(log.topics[0])?;
                let values = event.decode_log(&log.topics, log.data.bytes())?;
                decoded.push(json!({"event":event.name(),"block":log.inclusion.block_number,"tx":log.transaction_hash.to_string(),
                    "logIndex":log.log_index,"removed":log.removed,"values":format!("{values:?}")}));
            }
            let ours = logs
                .iter()
                .filter(|l| {
                    l.topics
                        .get(1)
                        .map(|t| t.bytes()[12..] == owner.address().bytes()[..])
                        == Some(true)
                })
                .count();
            record(
                "events",
                &json!({"check":"bounded-log-query-and-event-decoding","contract":net().wquai,"logs":logs.len(),"ownerLogs":ours,"decoded":decoded}),
            )?;
            if ours != 2 {
                return Err("expected exactly one deposit and one withdrawal for account A".into());
            }
        }
        "interchange" => {
            let mut hashes = std::collections::BTreeSet::new();
            for entry in fs::read_dir(dir().join("logs"))? {
                for line in fs::read_to_string(entry?.path())?.lines() {
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(line)
                        && matches!(
                            value["stage"].as_str(),
                            Some("signed-and-persisted" | "replacement-persisted")
                        )
                        && let Some(hash) = value["hash"].as_str()
                    {
                        hashes.insert(hash.to_string());
                    }
                }
            }
            let mut rows = Vec::new();
            let mut failures = 0;
            for hash in hashes {
                let Some(tx) = provider.transaction(Zone::Cyprus1, hash.parse()?).await? else {
                    rows.push(json!({"hash":hash,"mined":false}));
                    continue;
                };
                let signed = tx.verified_quai()?;
                let bytes = signed.signed_bytes()?;
                let document = quai_sdk::consensus::document::TransactionDocument::decode(&bytes)?;
                let json_value = document.to_json()?;
                let back =
                    quai_sdk::consensus::document::TransactionDocument::from_json(&json_value)?
                        .to_bytes()?;
                let proto = quai_sdk::consensus::decode_proto_transaction(&bytes)?;
                let proto_back = quai_sdk::consensus::encode_proto_transaction(&proto)?;
                let ok = back == bytes && proto_back == bytes && signed.hash()?.to_string() == hash;
                failures += usize::from(!ok);
                rows.push(json!({"hash":hash,"mined":true,"bytes":bytes.len(),"jsonRoundTrip":back==bytes,"protoRoundTrip":proto_back==bytes,"hashMatches":signed.hash()?.to_string()==hash}));
            }
            record(
                "interchange",
                &json!({"check":"mined-transaction-json-protobuf-round-trip","transactions":rows,"failures":failures}),
            )?;
            if failures > 0 {
                return Err("interchange round trip mismatch".into());
            }
        }
        _ => {
            return Err(
                "expected recovery, locked-spend, special-fee, backup, events or interchange"
                    .into(),
            );
        }
    }
    Ok(())
}
