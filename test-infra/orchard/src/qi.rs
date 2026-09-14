use super::*;
use quai_sdk::provider::{ReceiptOutcome, WaitConfig};
use quai_sdk::qi::{QiChangePool, QiIntent, QiPolicy, QiSession};
use quai_sdk::wallet::storage::{NetworkScope, PublicAddress, ReservationId, SqliteStore};
pub async fn run(stage: &str) -> Result<(), Box<dyn Error>> {
    #[derive(Deserialize)]
    struct Saved<'a> {
        #[serde(borrow)]
        mnemonic: &'a str,
        #[serde(borrow)]
        passphrase: &'a str,
        hd_receive_index: u32,
    }
    let content = Zeroizing::new(fs::read_to_string(dir().join("qi-wallet.json"))?);
    let saved: Saved = serde_json::from_str(&content).map_err(|_| "invalid Qi wallet JSON")?;
    let mnemonic = Mnemonic::parse(Language::English, saved.mnemonic)?;
    let wallet = HdWallet::from_mnemonic(&mnemonic, saved.passphrase, CoinType::Qi)?;
    let account = wallet.account_public(0)?;
    let scope = NetworkScope {
        chain_id: U256::from(15000),
        genesis: "0x663a73416275109a01aad3a4c29ea9e310aded63c5eea491243b7312ad8cd16b".parse()?,
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
    if provider.genesis_hash(scope.zone).await? != scope.genesis {
        return Err("Orchard identity mismatch".into());
    }
    let mut store = SqliteStore::open(dir().join("state/qi.sqlite"), scope)?;
    let owner = PublicAddress::derive(&account, false, saved.hd_receive_index)?;
    if store.addresses()?.is_empty() {
        store.import_metadata(0, &[owner])?;
    }
    let id = ReservationId([1; 16]);
    match stage {
        "qi-allocate-conversion" => {
            let path = dir().join("conversion-v2-recipient.json");
            if path.exists() {
                return Err("conversion recipient already allocated".into());
            }
            let allocation = store.allocate_address(&account, false, 100_000, || false)?;
            let index = match allocation.address.origin() {
                quai_sdk::wallet::metadata::KeyOrigin::Bip44 { index, .. } => index,
                _ => return Err("expected HD allocation".into()),
            };
            let metadata = PublicAddress::derive(&account, false, index)?;
            if metadata.address() != allocation.address.address() {
                return Err("allocation index mismatch".into());
            }
            let value = serde_json::json!({"address":metadata.address().to_string(),"index":index});
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)?;
            serde_json::to_writer(&mut file, &value)?;
            file.sync_all()?;
            fs::File::open(dir())?.sync_all()?;
            println!("{value}");
        }
        "qi-prepare" => {
            if store.reservation(id)?.is_some() {
                return Err("Qi operation already reserved; explicit recovery required".into());
            }
            let receiver = store
                .allocate_address(&account, false, 100_000, || false)?
                .address;
            let pool = QiChangePool::allocate(&mut store, &account, 16, 6000, || false)?;
            let mut checkpoint = None;
            for attempt in 1..=3 {
                match quai_sdk::qi_discovery::refresh_qi(&provider, &mut store, 1000, || false)
                    .await
                {
                    Ok(value) => {
                        checkpoint = Some(value);
                        break;
                    }
                    Err(quai_sdk::qi::QiError::StaleSnapshot) if attempt < 3 => {
                        eprintln!(
                            "Qi refresh crossed a block boundary; repeating public reads ({attempt}/3)"
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            let checkpoint = checkpoint.ok_or("no stable Qi refresh")?;
            let balance =
                quai_sdk::qi_discovery::qi_balance(&mut store, checkpoint.height + U256::from(1))?;
            println!(
                "{}",
                serde_json::json!({"stage":"Qi-refreshed","head":checkpoint.height.to_string(),"spendableQits":balance.spendable.to_string(),"lockedQits":balance.locked.to_string()})
            );
            let mut session = QiSession::new(&provider, &wallet, &mut store)?;
            let prepared = session
                .prepare(
                    id,
                    QiIntent {
                        amount: U256::from(1000),
                        destinations: vec![receiver.address().try_into()?],
                    },
                    QiPolicy {
                        initial_fee: U256::ZERO,
                        max_fee: U256::from(100),
                        max_inputs: 8,
                        max_outputs: 32,
                        max_fee_rounds: 8,
                        max_snapshot_age: 5,
                    },
                    pool,
                )
                .await?;
            println!(
                "{}",
                serde_json::json!({"stage":"Qi-prepared","feeQits":prepared.fee().to_string(),"inputs":prepared.transaction().inputs.iter().map(|i|serde_json::json!({"hash":i.previous_output.transaction_hash.to_string(),"index":i.previous_output.index})).collect::<Vec<_>>(),"outputs":prepared.transaction().outputs.iter().map(|o|serde_json::json!({"address":o.address.to_string(),"denomination":o.denomination.index()})).collect::<Vec<_>>()})
            );
            let signed = session.sign(&prepared)?;
            println!(
                "{}",
                serde_json::json!({"stage":"Qi-signed-persisted","hash":signed.hash()?.to_string()})
            );
        }
        "qi-broadcast" => {
            let bytes = store
                .signed_payload(id)?
                .ok_or("missing Qi signed payload")?;
            let signed = quai_sdk::consensus::SignedQiTransaction::decode(&bytes)?;
            if provider
                .receipt(scope.zone, signed.hash()?)
                .await?
                .is_some()
            {
                return Err("Qi receipt already exists; observe instead".into());
            }
            let ack = QiSession::new(&provider, &wallet, &mut store)?
                .broadcast(id)
                .await?;
            println!(
                "{}",
                serde_json::json!({"stage":"Qi-acknowledged","hash":ack.transaction_hash.to_string()})
            );
        }
        "qi-observe" => {
            let bytes = store
                .signed_payload(id)?
                .ok_or("missing Qi signed payload")?;
            let signed = quai_sdk::consensus::SignedQiTransaction::decode(&bytes)?;
            let result = provider
                .wait_for_receipt(
                    scope.zone,
                    signed.hash()?,
                    WaitConfig {
                        confirmations: 2,
                        timeout: std::time::Duration::from_secs(60),
                        poll_interval: std::time::Duration::from_secs(3),
                    },
                )
                .await?;
            let mut receipt = result.receipt.to_rpc_json()?;
            receipt
                .as_object_mut()
                .ok_or("receipt shape")?
                .remove("logsBloom");
            println!(
                "{}",
                serde_json::json!({"stage":"Qi-observed","confirmations":result.confirmations,"receipt":receipt})
            );
            if result.receipt.outcome != ReceiptOutcome::Succeeded {
                return Err("Qi execution failed".into());
            }
            for output in &signed.transaction().outputs {
                let points = provider.outpoints(output.address.try_into()?).await?;
                if !points.iter().any(|p| {
                    p.outpoint.tx_hash == signed.hash().unwrap()
                        && p.denomination == output.denomination.index()
                }) {
                    return Err("Qi output not currently indexed".into());
                }
            }
            println!("Every signed Qi output is indexed after execution.");
        }
        _ => return Err("unsupported Qi stage".into()),
    }
    Ok(())
}
