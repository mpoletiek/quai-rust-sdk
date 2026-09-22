//! Isolated public-fixture wallet-session acceptance. Never use this source as production discovery.
use quai_sdk::{
    BlockTag, Endpoint, HttpConfig, HttpTransport, Provider, QiAddress, QuaiAddress, Routing, U256,
    Zone,
    abi::AbiInterface,
    accounts::{
        AccountIntent, AccountObservationPolicy, AccountSession, FeePolicy, ReplacementPolicy,
    },
    consensus::{
        Denomination, OutPoint, QiInput, QiOutput, QiTransaction, SignedQiTransaction,
        SignedQuaiTransaction,
    },
    contracts::{DeploymentSearch, prepare_deployment},
    crypto::SecretKey,
    provider::RpcData,
    qi::{QiChangePool, QiIntent, QiPolicy, QiSession},
    rpc::{RpcError, Transport},
    signer::{LocalSigner, Signer},
    wallet::discovery::{
        AddressObservation, Checkpoint, DiscoveryError, DiscoveryRequest, HistoryCapability,
        IndexRange, ObservationSource, ScopedCheckpoint, discover,
    },
    wallet::storage::{NetworkScope, PublicAddress, ReservationId, ReservationState, SqliteStore},
    wallet::{CandidateCoin, CoinType, DerivedAddress, HdWallet, Search},
};
use serde_json::{Value, json};
use std::{
    error::Error,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
mod access_workflow;
mod qi_wrapper_workflow;
mod wrapper_workflow;
const GENESIS: &str = "0x654e7a894d57de62ec19b9c161cb1c647466278e0565d3e1ba5d806ae6af0aee";
const ROOT: &str = "/tmp/quai-sdk-highlevel-wallets";
fn key(n: u64) -> SecretKey {
    let mut b = [0; 32];
    b[24..].copy_from_slice(&n.to_be_bytes());
    SecretKey::from_bytes(&b).unwrap()
}
fn scope() -> NetworkScope {
    NetworkScope {
        chain_id: U256::from(1337),
        genesis: GENESIS.parse().unwrap(),
        zone: Zone::Cyprus1,
    }
}
fn save(name: &str, value: Value) -> Result<(), Box<dyn Error>> {
    std::fs::write(
        Path::new(ROOT).join(name),
        serde_json::to_vec_pretty(&value)?,
    )?;
    Ok(())
}
fn read(name: &str) -> Result<Value, Box<dyn Error>> {
    Ok(serde_json::from_slice(&std::fs::read(
        Path::new(ROOT).join(name),
    )?)?)
}
#[derive(Clone)]
struct Recorded {
    inner: HttpTransport,
    calls: Arc<Mutex<Vec<Value>>>,
}
impl Transport for Recorded {
    async fn request(&self, e: &Endpoint, m: &str, p: Value) -> Result<Value, RpcError> {
        let result = self.inner.request(e, m, p.clone()).await;
        self.calls
            .lock()
            .unwrap()
            .push(json!({"method":m,"params":p,"result":result.as_ref().ok(),"error":match result.as_ref().err(){Some(RpcError::Remote(e))=>Some(json!({"code":e.code,"message":e.message.chars().take(500).collect::<String>()})),_=>None}}));
        result
    }
}
// This source is qualified ONLY because the disposable node is quiescent: no miner
// or other writer runs during collection. Every latest-only read checks the same
// head before/after. This does not implement a production block-pinned Qi RPC.
struct Quiescent<'a> {
    provider: &'a Provider<Recorded>,
    checkpoint: Checkpoint,
}
impl ObservationSource for Quiescent<'_> {
    fn history_capability(&self, _: NetworkScope, _: CoinType) -> HistoryCapability {
        HistoryCapability::CurrentStateOnly
    }
    async fn tip(&self, s: NetworkScope) -> Result<ScopedCheckpoint, DiscoveryError> {
        let h = self
            .provider
            .latest_header(s.zone)
            .await
            .map_err(|_| DiscoveryError::SourceUnavailable)?
            .ok_or(DiscoveryError::SourceUnavailable)?;
        if h.hash != self.checkpoint.hash {
            return Err(DiscoveryError::InvalidObservation);
        }
        Ok(ScopedCheckpoint {
            scope: s,
            checkpoint: self.checkpoint,
        })
    }
    async fn canonical(
        &self,
        s: NetworkScope,
        height: U256,
    ) -> Result<Option<ScopedCheckpoint>, DiscoveryError> {
        let h = self
            .provider
            .header_at(
                s.zone,
                u64::try_from(height).map_err(|_| DiscoveryError::InvalidRequest)?,
            )
            .await
            .map_err(|_| DiscoveryError::SourceUnavailable)?;
        Ok(h.map(|h| ScopedCheckpoint {
            scope: s,
            checkpoint: Checkpoint {
                hash: h.hash,
                height: U256::from(h.number),
            },
        }))
    }
    async fn observe(
        &self,
        s: NetworkScope,
        a: &DerivedAddress,
        c: Checkpoint,
    ) -> Result<AddressObservation, DiscoveryError> {
        if c != self.checkpoint || a.coin != CoinType::Qi {
            return Err(DiscoveryError::InvalidObservation);
        }
        self.tip(s).await?;
        let address =
            QiAddress::try_from(a.address).map_err(|_| DiscoveryError::InvalidObservation)?;
        let points = self
            .provider
            .outpoints(address)
            .await
            .map_err(|_| DiscoveryError::SourceUnavailable)?;
        self.tip(s).await?;
        let coins = points
            .into_iter()
            .map(|p| {
                let denomination = Denomination::new(p.denomination)
                    .map_err(|_| DiscoveryError::InvalidObservation)?;
                let outpoint = OutPoint {
                    transaction_hash: p.outpoint.tx_hash,
                    index: p.outpoint.index,
                };
                Ok(CandidateCoin::new(outpoint, address, denomination).with_unlock_height(p.lock))
            })
            .collect::<Result<Vec<_>, DiscoveryError>>()?;
        let mut observation = AddressObservation::new(s, c, a.address);
        observation.coins = coins;
        Ok(observation)
    }
}
fn signed_record(hash: String, bytes: Vec<u8>) -> Value {
    json!({"hash":hash,"signedBytes":RpcData::new(bytes).unwrap().to_hex()})
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mode = std::env::args().nth(1).ok_or(
        "init-fund | account-prepare | account-broadcast | qi-prepare | qi-broadcast | verify",
    )?;
    let transport = Recorded {
        inner: HttpTransport::new(HttpConfig::default())?,
        calls: Arc::new(Mutex::new(vec![])),
    };
    let provider = Provider::new(
        transport.clone(),
        Routing::direct("http://127.0.0.1:19200", Zone::Cyprus1.into())?,
        scope().chain_id,
    );
    if provider.genesis_hash(Zone::Cyprus1).await? != scope().genesis {
        return Err("refusing unknown ordinary genesis".into());
    }
    let root = PathBuf::from(ROOT);
    let account_path = root.join("account.sqlite");
    let qi_path = root.join("qi.sqlite");
    let wallet = HdWallet::from_seed(&[7; 32], CoinType::Qi)?;
    let account = wallet.account_public(0)?;
    let signer = LocalSigner::new(key(805), scope().chain_id)?;
    let sender = QuaiAddress::try_from(signer.address())?;
    let account_id = ReservationId([1; 16]);
    let deploy_id = ReservationId([2; 16]);
    let qi_id = ReservationId([3; 16]);
    let operation: Result<(), Box<dyn Error>> = async {
    match mode.as_str() {
        "init-fund" => {
            if root.exists() {
                return Err("refusing existing wallet fixture directory".into());
            }
            std::fs::create_dir(&root)?;
            let mut store = SqliteStore::open(&account_path, scope())?;
            store.import_metadata(0, &[PublicAddress::imported(&key(805).public_key())?])?;
            drop(store);
            let mut store = SqliteStore::open(&qi_path, scope())?;
            let mut addresses = vec![];
            for _ in 0..2 {
                addresses.push(
                    store
                        .allocate_address(&account, false, 10000, || false)?
                        .address
                        .address(),
                );
            }
            let bootstrap = key(130);
            let tx = QiTransaction {
                chain_id: scope().chain_id,
                inputs: vec![QiInput {
                    previous_output: OutPoint {
                        transaction_hash:
                            "0x0080008011111111111111111111111111111111111111111111111111111111"
                                .parse()?,
                        index: 0,
                    },
                    public_key: bootstrap.public_key(),
                }],
                outputs: addresses
                    .iter()
                    .map(|a| QiOutput {
                        address: *a,
                        denomination: Denomination::new(8).unwrap(),
                    })
                    .collect(),
                data: vec![],
            };
            let signed = tx.sign_single(&bootstrap)?;
            let sent = provider.broadcast_qi(&signed).await?;
            save(
                "funding.json",
                json!({"publicSeedHex":"07".repeat(32),"addresses":addresses.iter().map(ToString::to_string).collect::<Vec<_>>(),"hash":sent.transaction_hash.to_string(),"fundedQitsPerAddress":10000,"fundingFeeIsSyntheticAndNotWalletFeeQualification":true}),
            )?;
        }
        "account-prepare" => {
            let mut store = SqliteStore::open(&account_path, scope())?;
            let policy = FeePolicy::new(1_000_000, U256::from(10_000_000_000_000_000u64), U256::from(10_000_000_000_000_000_000_000u128)).with_gas_margin_bps(1000);
            let mut session = AccountSession::new(&provider, &signer, &mut store)?
                .with_observation_policy(AccountObservationPolicy::PinnedLatest);
            let prepared = session
                .prepare(
                    account_id,
                    AccountIntent::new("0x0011223344556677889900112233445566778899".parse()?, U256::from(12345)).with_data(RpcData::new(vec![])?),
                    policy,
                )
                .await?;
            let signed = session.sign(&prepared)?;
            let mut transfer = signed_record(signed.hash()?.to_string(), signed.signed_bytes()?);
            transfer["nonce"] = json!(prepared.transaction().nonce);
            transfer["gas"] = json!(prepared.transaction().gas_limit);
            transfer["maximumFee"] = json!(prepared.maximum_fee().to_string());
            save("account-signed.json", transfer)?;
            let nonce = session.reserve_deployment_nonce(deploy_id).await?;
            let init: RpcData = "0x600a600c600039600a6000f3602a60005260206000f3".parse()?;
            let deployment = prepare_deployment(
                &AbiInterface::from_json(b"[]")?,
                init.bytes(),
                &[],
                sender,
                scope().chain_id,
                nonce,
                U256::ZERO,
                DeploymentSearch::new(0, 10000),
                || false,
            )?;
            let counter = deployment.salt();
            let prepared = session
                .prepare_deployment(deploy_id, deployment, policy)
                .await?;
            let signed = session.sign(&prepared)?;
            let mut record = signed_record(signed.hash()?.to_string(), signed.signed_bytes()?);
            record["nonce"] = json!(nonce);
            record["counter"] = json!(counter);
            record["contract"] = json!(prepared.created_address().unwrap().to_string());
            record["gas"] = json!(prepared.transaction().gas_limit);
            record["maximumFee"] = json!(prepared.maximum_fee().to_string());
            save("deployment-signed.json", record)?;
            assert_eq!(
                store.reservation(account_id)?.unwrap().state,
                ReservationState::Signed
            );
            assert_eq!(
                store.reservation(deploy_id)?.unwrap().state,
                ReservationState::Signed
            );
            assert!(store.release_unsigned(account_id).is_err());
        }
        "account-broadcast" => {
            let mut store = SqliteStore::open(&account_path, scope())?;
            for (id, file) in [
                (account_id, "account-signed.json"),
                (deploy_id, "deployment-signed.json"),
            ] {
                let expected = read(file)?;
                let bytes = store
                    .signed_payload(id)?
                    .ok_or("missing durable account bytes")?;
                assert_eq!(
                    RpcData::new(bytes.clone())?.to_hex(),
                    expected["signedBytes"]
                );
                assert_eq!(
                    SignedQuaiTransaction::decode(&bytes)?.hash()?.to_string(),
                    expected["hash"]
                );
                let sent = AccountSession::new(&provider, &signer, &mut store)?
                    .broadcast(id)
                    .await?;
                assert_eq!(sent.transaction_hash.to_string(), expected["hash"]);
                assert_eq!(
                    store.reservation(id)?.unwrap().state,
                    ReservationState::Submitted
                );
            }
        }
        "qi-prepare" => {
            let mut store = SqliteStore::open(&qi_path, scope())?;
            let pool = QiChangePool::allocate(&mut store, &account, 24, 4000, || false)?;
            let head = provider
                .latest_header(Zone::Cyprus1)
                .await?
                .ok_or("no head")?;
            let checkpoint = Checkpoint {
                hash: head.hash,
                height: U256::from(head.number),
            };
            let source = Quiescent {
                provider: &provider,
                checkpoint,
            };
            let generation=store.snapshot()?.generation;
            let mut reports=vec![];
            for metadata in store.addresses()? {
                let quai_sdk::wallet::storage::KeyOrigin::Bip44{change,index,..}=metadata.origin() else {return Err("unexpected imported Qi key".into())};
                let empty=IndexRange{start:0,end:0};let exact=IndexRange{start:index,end:index+1};
                reports.push(discover(&source,&account,&DiscoveryRequest::new(scope(),if change{empty}else{exact},if change{exact}else{empty}).with_gap_limit(None).with_require_history(false).with_max_addresses(1).with_max_coins(100),||false).await?);
            }
            let count=reports.len();store.commit_discovery(generation,&reports)?;
            let recipient = HdWallet::from_seed(&[8; 32], CoinType::Qi)?.account_public(0)?;
            let mut next = 0;
            let mut destinations = vec![];
            for _ in 0..10 {
                let found = recipient
                    .search(
                        false,
                        Search {
                            zone: Zone::Cyprus1,
                            start_index: next,
                            max_attempts: 10000,
                        },
                        || false,
                    )?
                    .address;
                next = found.index + 1;
                destinations.push(QiAddress::try_from(found.address)?);
            }
            let mut session = QiSession::new(&provider, &wallet, &mut store)?;
            let prepared = session
                .prepare(
                    qi_id,
                    QiIntent::new(U256::from(1234), destinations),
                    QiPolicy::new(U256::from(1000), 4, 64, 0),
                    pool,
                )
                .await?;
            let signed = session.sign(&prepared)?;
            let mut record = signed_record(signed.hash()?.to_string(), signed.signed_bytes()?);
            record["feeQits"] = json!(prepared.fee().to_string());
            record["recipientOutputs"] = json!(prepared.recipient_outputs());
            record["checkpointHeight"] = json!(head.number);
            record["checkpointHash"] = json!(head.hash.to_string());
            record["observedAddresses"] = json!(count);
            record["outputs"]=json!(prepared.transaction().outputs.iter().map(|o|json!({"address":o.address.to_string(),"denomination":o.denomination.index(),"qits":o.denomination.value()})).collect::<Vec<_>>());
            record["inputCount"] = json!(prepared.transaction().inputs.len());
            save("qi-signed.json", record)?;
            assert_eq!(
                store.reservation(qi_id)?.unwrap().state,
                ReservationState::Signed
            );
            assert!(store.release_unsigned(qi_id).is_err());
        }
        "qi-broadcast" => {
            let mut store = SqliteStore::open(&qi_path, scope())?;
            let expected = read("qi-signed.json")?;
            let bytes = store
                .signed_payload(qi_id)?
                .ok_or("missing durable Qi bytes")?;
            assert_eq!(
                RpcData::new(bytes.clone())?.to_hex(),
                expected["signedBytes"]
            );
            assert_eq!(
                SignedQiTransaction::decode(&bytes)?.hash()?.to_string(),
                expected["hash"]
            );
            assert!(!store.reserved_outpoints(qi_id)?.is_empty());
            let result = QiSession::new(&provider, &wallet, &mut store)?
                .broadcast(qi_id)
                .await?;
            assert_eq!(result.transaction_hash.to_string(), expected["hash"]);
            assert_eq!(
                store.reservation(qi_id)?.unwrap().state,
                ReservationState::Submitted
            );
        }
        "replacement-prepare" => {
            let id=ReservationId([4;16]);
            let mut store=SqliteStore::open(&account_path,scope())?;
            let policy=FeePolicy::new(1_000_000, U256::from(10_000_000_000_000_000u64), U256::from(10_000_000_000_000_000_000_000u128)).with_gas_margin_bps(1000);
            let mut session=AccountSession::new(&provider,&signer,&mut store)?.with_observation_policy(AccountObservationPolicy::PinnedLatest);
            let prepared=session.prepare(id,AccountIntent::new(sender, U256::from(1)).with_data(RpcData::new(vec![])?),policy).await?;
            let original=session.sign(&prepared)?;
            let prepared=session.prepare_replacement(id,original.hash()?,ReplacementPolicy::new(5, policy)).await?;
            let replacement=session.sign_replacement(&prepared)?;
            save("replacement-signed.json",json!({"original":original.hash()?.to_string(),"replacement":replacement.hash()?.to_string(),"nonce":replacement.transaction().nonce,"oldPrice":original.transaction().gas_price.to_string(),"newPrice":replacement.transaction().gas_price.to_string(),"signedBytes":RpcData::new(replacement.signed_bytes()?)?.to_hex()}))?;
        }
        "replacement-broadcast" => {
            let record=read("replacement-signed.json")?;
            let mut store=SqliteStore::open(&account_path,scope())?;
            let mut session=AccountSession::new(&provider,&signer,&mut store)?;
            for field in ["original","replacement"] {
                let hash=record[field].as_str().ok_or("candidate hash")?.parse()?;
                assert_eq!(session.broadcast_candidate(ReservationId([4;16]),hash).await?.transaction_hash,hash);
            }
        }
        "verify-replacement" => {
            let record=read("replacement-signed.json")?;
            let mut store=SqliteStore::open(&account_path,scope())?;
            let update=quai_sdk::recovery::track_family(&provider,&mut store,ReservationId([4;16])).await?;
            assert_eq!(update.canonical,Some(record["replacement"].as_str().ok_or("candidate hash")?.parse()?));
            assert_eq!(update.candidates.len(),2);
            assert!(store.release_unsigned(ReservationId([4;16])).is_err());
            let root=update.candidates[0].0;
            drop(store);
            let mut reopened=SqliteStore::open(&account_path,scope())?;
            let cache=reopened.observation_cache(ReservationId([4;16]),root,u16::MAX)?.ok_or("family cache missing")?;
            assert_eq!(cache.revision,update.revision);
            let summary:Value=serde_json::from_slice(cache.payload.as_deref().ok_or("family cache empty")?)?;
            assert_eq!(summary["canonical"],record["replacement"]);
            save("replacement-verified.json",json!({"canonical":update.canonical.map(|h|h.to_string()),"observations":format!("{:?}",update.candidates),"nonceClaimRetained":true,"signedCandidates":reopened.quai_replacements(ReservationId([4;16]))?.len()+1,"cacheRevision":cache.revision,"cacheSurvivesReopen":true,"signerRequiredForObservation":false,"submitted":false,"qualification":"isolated documented development profile; public fixture funds"}))?;
        }
        "verify-deployment" => {
            let record=read("deployment-signed.json")?;
            let hash=record["hash"].as_str().ok_or("deployment hash")?.parse()?;
            let runtime:RpcData="0x602a60005260206000f3".parse()?;
            let expected_runtime=quai_sdk::primitives::Hash32::from_bytes(quai_sdk::crypto::keccak256(runtime.bytes()));
            let mut store=SqliteStore::open(&account_path,scope())?;
            let update=quai_sdk::deployments::track_deployment(&provider,&mut store,deploy_id,hash,Some(expected_runtime)).await?;
            let quai_sdk::provider::DeploymentObservation::Included{block,outcome,confirmations,code:Some(code)}=update.observation else{return Err("deployment not observed with code".into());};
            assert_eq!(outcome,quai_sdk::provider::ReceiptOutcome::Succeeded);
            assert_eq!(code.bytes,runtime);assert_eq!(code.matches_expected,Some(true));
            assert!(store.release_unsigned(deploy_id).is_err());drop(store);
            let mut reopened=SqliteStore::open(&account_path,scope())?;
            let cache=reopened.observation_cache(deploy_id,hash,0)?.ok_or("deployment cache missing")?;
            assert_eq!(cache.revision,update.revision);
            let summary:Value=serde_json::from_slice(cache.payload.as_deref().ok_or("deployment cache empty")?)?;
            assert_eq!(summary["address"],record["contract"]);
            save("deployment-verified.json",json!({"candidate":hash.to_string(),"contract":record["contract"],"height":block.number,"block":block.hash.to_string(),"confirmations":confirmations,"runtime":code.bytes.to_hex(),"runtimeHash":code.hash.to_string(),"runtimeMatchesExpected":code.matches_expected,"cacheRevision":cache.revision,"cacheSurvivesReopen":true,"nonceClaimRetained":true,"submitted":false,"qualification":"isolated documented development profile; public fixture funds"}))?;
        }
        "verify" | "verify-qi" => {
            let mut records = vec![];
            for (file, path, id) in [
                ("account-signed.json", &account_path, account_id),
                ("deployment-signed.json", &account_path, deploy_id),
                ("qi-signed.json", &qi_path, qi_id),
            ] {
                if mode == "verify-qi" && file != "qi-signed.json" { continue; }
                let expected = read(file)?;
                let hash = expected["hash"].as_str().ok_or("hash")?.parse()?;
                let receipt = provider
                    .receipt(Zone::Cyprus1, hash)
                    .await?
                    .ok_or("receipt missing")?;
                assert!(matches!(receipt.outcome, quai_sdk::provider::ReceiptOutcome::Succeeded));
                let header = provider
                    .header_at(Zone::Cyprus1, receipt.inclusion.block_number)
                    .await?
                    .ok_or("header")?;
                assert_eq!(header.hash, receipt.inclusion.block_hash);
                let mut store = SqliteStore::open(path, scope())?;
                store.observe_inclusion(
                    id,
                    hash,
                    Checkpoint {
                        hash: header.hash,
                        height: U256::from(header.number),
                    },
                )?;
                drop(store);
                let reopened = SqliteStore::open(path, scope())?;
                assert_eq!(
                    reopened.reservation(id)?.unwrap().state,
                    ReservationState::Confirmed
                );
                records.push(json!({"file":file,"hash":hash.to_string(),"height":header.number,"gasUsed":receipt.gas_used,"outcome":format!("{:?}",receipt.outcome),"durableState":"Confirmed"}));
            }
            let runtime = if mode == "verify" {
                let deployment=read("deployment-signed.json")?;
                let address:QuaiAddress=deployment["contract"].as_str().ok_or("contract")?.parse()?;
                let code=provider.code(address,BlockTag::Latest).await?;
                assert_eq!(code.to_hex(),"0x602a60005260206000f3");Some(code.to_hex())
            } else {None};
            let plan = read("qi-signed.json")?;
            let mut actual = vec![];
            for (index, output) in plan["outputs"]
                .as_array()
                .ok_or("outputs")?
                .iter()
                .enumerate()
            {
                let address: QiAddress = output["address"].as_str().unwrap().parse()?;
                let points = provider.outpoints(address).await?;
                let found = points
                    .iter()
                    .find(|p| {
                        p.outpoint.tx_hash.to_string() == plan["hash"]
                            && usize::from(p.outpoint.index) == index
                    })
                    .ok_or("output absent")?;
                assert_eq!(
                    u64::from(found.denomination),
                    output["denomination"].as_u64().unwrap()
                );
                actual.push(json!({"address":address.to_string(),"index":index,"denomination":found.denomination}));
            }
            save(
                "verified.json",
                json!({"receipts":records,"runtime":runtime,"qiOutputAssertions":actual,"qualification":"isolated quiescent node only; no production pinned Qi source"}),
            )?;
        }
        value if value.starts_with("access-") => access_workflow::run(value, &provider, &signer, &account_path).await?,
        "wqi-probe" => qi_wrapper_workflow::probe(&provider, &signer).await?,
        value if value.starts_with("wqi-deploy-") => qi_wrapper_workflow::deploy(value, &provider, &signer, &account_path).await?,
        value if value.starts_with("wqi-") => qi_wrapper_workflow::workflow(value, &provider, &signer, &account_path, &qi_path, &wallet).await?,
        value if value.starts_with("wquai-") => wrapper_workflow::run(value, &provider, &signer, &account_path).await?,
        _ => return Err("unknown high-level mode".into()),
    }
    Ok(())
    }.await;
    save(
        &format!("{mode}-rpc.json"),
        json!(transport.calls.lock().unwrap().clone()),
    )?;
    operation?;
    println!("{mode} complete; public evidence in {ROOT}");
    Ok(())
}
