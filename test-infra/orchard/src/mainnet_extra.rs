//! Cheap QUAI-only mainnet qualification: WQUAI token operations, a withheld
//! submission acknowledgement, an unregistered nonce competitor and a trivial
//! deployment. Each reservation resumes from durable state and is never re-signed.
use super::*;
use quai_sdk::accounts::{AccountIntent, AccountObservationPolicy, AccountSession};
use quai_sdk::consensus::SignedQuaiTransaction;
use quai_sdk::primitives::Hash32;
use quai_sdk::provider::{ReceiptOutcome, RpcData, WaitConfig};
use quai_sdk::signer::LocalSigner;
use quai_sdk::wallet::storage::{NetworkScope, PublicAddress, ReservationId, SqliteStore};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const CENTI: u64 = 10_000_000_000_000_000; // 0.01 QUAI / WQUAI

/// Forwards one successful raw submission, then hides its acknowledgement.
struct LostAck(HttpTransport, AtomicBool);
impl quai_sdk::rpc::Transport for LostAck {
    async fn request_batch(
        &self,
        endpoint: &quai_sdk::Endpoint,
        requests: Vec<(&str, Value)>,
    ) -> Option<quai_sdk::rpc::BatchResult> {
        self.0.request_batch(endpoint, requests).await
    }
    async fn request(
        &self,
        endpoint: &quai_sdk::Endpoint,
        method: &str,
        params: Value,
    ) -> Result<Value, quai_sdk::rpc::RpcError> {
        let result = self.0.request(endpoint, method, params).await;
        if method == "quai_sendRawTransaction"
            && result.is_ok()
            && !self.1.swap(true, Ordering::SeqCst)
        {
            eprintln!("qualification fault: accepted submission acknowledgement withheld");
            return Err(quai_sdk::rpc::RpcError::Timeout);
        }
        result
    }
}

pub(super) struct Ctx {
    pub(super) provider: Provider<DiagnosticTransport>,
    key_hex: Vec<Zeroizing<String>>,
    pub(super) addresses: [QuaiAddress; 2],
    scope: NetworkScope,
}

impl Ctx {
    pub(super) async fn load() -> Result<Self, Box<dyn Error>> {
        if !net().mainnet() {
            return Err("mainnet-extra requires QUAI_QUALIFICATION_NETWORK=mainnet".into());
        }
        let content = Zeroizing::new(fs::read_to_string(dir().join("wallets.json"))?);
        let supplied: Supplied = serde_json::from_str(&content).map_err(|_| "wallet JSON")?;
        let mut key_hex = Vec::new();
        let mut addresses = Vec::new();
        for w in &supplied.wallets {
            let address: QuaiAddress = w.address.parse()?;
            if load_key(w.private_key)?.public_key().address() != address.address() {
                return Err("key/address mismatch".into());
            }
            key_hex.push(Zeroizing::new(w.private_key.to_string()));
            addresses.push(address);
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
        if provider.genesis_hash(Zone::Cyprus1).await? != scope.genesis {
            return Err("genesis mismatch".into());
        }
        Ok(Self {
            provider,
            key_hex,
            addresses: [addresses[0], addresses[1]],
            scope,
        })
    }

    fn key(&self, owner: usize) -> Result<quai_sdk::crypto::SecretKey, Box<dyn Error>> {
        load_key(&self.key_hex[owner])
    }

    fn store(&self, owner: usize) -> Result<SqliteStore, Box<dyn Error>> {
        let store = SqliteStore::open(
            dir().join("state").join(format!("account-{owner}.sqlite")),
            self.scope,
        )?;
        let registered = store.addresses()?;
        if registered.len() != 1
            || registered[0] != PublicAddress::imported(&self.key(owner)?.public_key())?
        {
            return Err("store owner mismatch".into());
        }
        Ok(store)
    }

    fn signer(&self, owner: usize) -> Result<LocalSigner, Box<dyn Error>> {
        Ok(LocalSigner::new(self.key(owner)?, self.scope.chain_id)?)
    }

    async fn wait(&self, hash: Hash32) -> Result<Value, Box<dyn Error>> {
        let observed = self
            .provider
            .wait_for_receipt(
                Zone::Cyprus1,
                hash,
                WaitConfig {
                    confirmations: 2,
                    timeout: Duration::from_secs(300),
                    poll_interval: Duration::from_secs(3),
                },
            )
            .await?;
        if observed.receipt.outcome != ReceiptOutcome::Succeeded {
            return Err("receipt did not succeed".into());
        }
        let mut receipt = observed.receipt.to_rpc_json()?;
        receipt
            .as_object_mut()
            .ok_or("receipt shape")?
            .remove("logsBloom");
        Ok(
            json!({"confirmations":observed.confirmations,"block":observed.receipt.inclusion.block_number,
            "gasUsed":observed.receipt.gas_used,"feeIts":observed.receipt.fee()?.to_string(),"receipt":receipt}),
        )
    }

    /// Prepare (retrying head changes), sign, broadcast and confirm one intent;
    /// resumes from a saved signature or an existing receipt without re-signing.
    pub(super) async fn send(
        &self,
        owner: usize,
        id: u8,
        label: &str,
        intent: AccountIntent,
    ) -> Result<Value, Box<dyn Error>> {
        let id = ReservationId([id; 16]);
        let signer = self.signer(owner)?;
        let mut store = self.store(owner)?;
        let bytes = match store.signed_payload(id)? {
            Some(bytes) => bytes,
            None => {
                if store.reservation(id)?.is_some() {
                    return Err(
                        format!("{label}: unsigned reservation needs explicit recovery").into(),
                    );
                }
                let mut attempt = 0;
                let signed = loop {
                    attempt += 1;
                    let mut session = AccountSession::new(&self.provider, &signer, &mut store)?
                        .with_observation_policy(AccountObservationPolicy::PinnedLatest);
                    match session.prepare(id, intent.clone(), net().account_fee).await {
                        Ok(prepared) => {
                            println!(
                                "{}",
                                json!({"op":label,"stage":"prepared","nonce":prepared.transaction().nonce,"gasLimit":prepared.transaction().gas_limit,"maximumFeeIts":prepared.maximum_fee().to_string()})
                            );
                            break session.sign(&prepared)?;
                        }
                        Err(quai_sdk::accounts::AccountError::ObservationChanged)
                            if attempt < 5 =>
                        {
                            tokio::time::sleep(Duration::from_secs(3)).await;
                        }
                        Err(error) => return Err(error.into()),
                    }
                };
                signed.signed_bytes()?
            }
        };
        let signed = SignedQuaiTransaction::decode(&bytes)?;
        let hash = signed.hash()?;
        if self.provider.receipt(Zone::Cyprus1, hash).await?.is_none() {
            let ack = AccountSession::new(&self.provider, &signer, &mut store)?
                .broadcast(id)
                .await?;
            println!(
                "{}",
                json!({"op":label,"stage":"acknowledged","hash":ack.transaction_hash.to_string()})
            );
        }
        let observed = self.wait(hash).await?;
        let mined = self
            .provider
            .transaction(Zone::Cyprus1, hash)
            .await?
            .ok_or("mined transaction absent")?
            .verified_quai()?;
        if mined.signed_bytes()? != bytes {
            return Err("mined bytes differ from custody".into());
        }
        Ok(
            json!({"op":label,"hash":hash.to_string(),"nonce":signed.transaction().nonce,"minedBytesMatchCustody":true,"observed":observed}),
        )
    }
}

fn save_record(name: &str, value: &Value) -> Result<(), Box<dyn Error>> {
    let path = dir().join("checks");
    fs::create_dir_all(&path)?;
    fs::write(
        path.join(format!("{name}.json")),
        serde_json::to_vec_pretty(value)?,
    )?;
    println!("{value}");
    Ok(())
}

pub async fn run(op: &str) -> Result<(), Box<dyn Error>> {
    let ctx = Ctx::load().await?;
    let [a, b] = ctx.addresses;
    let centi = U256::from(CENTI);
    match op {
        "token-roundtrip" => {
            let wquai = quai_sdk::wrappers::WrappedQuai::new(net().wquai.parse()?, &ctx.provider)?;
            let token = wquai.token()?;
            // Token state at an exact block: each step is checked at its own receipt height,
            // so resuming a completed sequence re-verifies history instead of latest state.
            let state = |label: &'static str, block: Option<u64>| {
                let token = &token;
                async move {
                    let tag = block.map_or(BlockTag::Latest, |n| BlockTag::Number(U256::from(n)));
                    Ok::<Value, Box<dyn Error>>(json!({"after":label,
                        "aBalance":token.balance_of(a, a, tag).await?.to_string(),
                        "bBalance":token.balance_of(a, b, tag).await?.to_string(),
                        "allowanceAtoB":token.allowance(a, a, b, tag).await?.to_string()}))
                }
            };
            let resuming = ctx
                .store(0)?
                .signed_payload(ReservationId([21; 16]))?
                .is_some();
            let start = state("start", None).await?;
            if !resuming
                && (start["aBalance"] != "0"
                    || start["bBalance"] != "0"
                    || start["allowanceAtoB"] != "0")
            {
                return Err(format!("unexpected starting token state {start}").into());
            }
            let mut steps = vec![json!({"resuming":resuming,"latestAtStart":start})];
            let plan: Vec<(usize, u8, &str, AccountIntent, [&str; 3])> = vec![
                (
                    0,
                    21,
                    "a-deposit-0.02",
                    wquai.deposit(U256::from(2 * CENTI))?.into_account_intent(),
                    ["20000000000000000", "0", "0"],
                ),
                (
                    0,
                    22,
                    "a-approve-b-0.01",
                    token.approve(b, centi)?.into_account_intent(),
                    ["20000000000000000", "0", "10000000000000000"],
                ),
                (
                    1,
                    31,
                    "b-transferFrom-a-0.01",
                    token
                        .contract()
                        .prepare(
                            "transferFrom",
                            &[
                                Value::String(a.to_string()),
                                Value::String(b.to_string()),
                                Value::String(centi.to_string()),
                            ],
                            U256::ZERO,
                        )?
                        .into_account_intent(),
                    ["10000000000000000", "10000000000000000", "0"],
                ),
                (
                    0,
                    23,
                    "a-transfer-b-0.01",
                    token.transfer(b, centi)?.into_account_intent(),
                    ["0", "20000000000000000", "0"],
                ),
                (
                    1,
                    32,
                    "b-withdraw-0.02",
                    wquai.withdraw(U256::from(2 * CENTI))?.into_account_intent(),
                    ["0", "0", "0"],
                ),
            ];
            for (owner, id, label, intent, expected) in plan {
                let result = ctx.send(owner, id, label, intent).await?;
                let block = result["observed"]["block"]
                    .as_u64()
                    .ok_or("receipt block")?;
                let after = state("receipt-block", Some(block)).await?;
                let ok = after["aBalance"] == expected[0]
                    && after["bBalance"] == expected[1]
                    && after["allowanceAtoB"] == expected[2];
                steps.push(
                    json!({"result":result,"tokenState":after,"expected":expected,"matches":ok}),
                );
                if !ok {
                    save_record("token-roundtrip", &json!({"steps":steps,"failed":label}))?;
                    return Err(format!("{label}: unexpected token state").into());
                }
            }
            save_record(
                "token-roundtrip",
                &json!({"check":"wquai-approve-transferFrom-transfer-withdraw","steps":steps}),
            )?;
        }
        "lost-ack" => {
            let id = ReservationId([24; 16]);
            let signer = ctx.signer(0)?;
            let mut store = ctx.store(0)?;
            if store.signed_payload(id)?.is_none() {
                let mut attempt = 0;
                loop {
                    attempt += 1;
                    let mut session = AccountSession::new(&ctx.provider, &signer, &mut store)?
                        .with_observation_policy(AccountObservationPolicy::PinnedLatest);
                    match session
                        .prepare(
                            id,
                            AccountIntent {
                                to: b,
                                value: centi,
                                data: RpcData::new(vec![])?,
                                access_list: vec![],
                            },
                            net().account_fee,
                        )
                        .await
                    {
                        Ok(prepared) => {
                            session.sign(&prepared)?;
                            break;
                        }
                        Err(quai_sdk::accounts::AccountError::ObservationChanged)
                            if attempt < 5 =>
                        {
                            tokio::time::sleep(Duration::from_secs(3)).await
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
            }
            let bytes = store.signed_payload(id)?.ok_or("signed payload")?;
            let signed = SignedQuaiTransaction::decode(&bytes)?;
            let hash = signed.hash()?;
            let mut fault = None;
            if ctx.provider.receipt(Zone::Cyprus1, hash).await?.is_none() {
                let lossy = Provider::new(
                    LostAck(
                        HttpTransport::new(HttpConfig::default())?,
                        AtomicBool::new(false),
                    ),
                    Routing::direct(net().endpoint, Zone::Cyprus1.into())?,
                    ctx.scope.chain_id,
                );
                let outcome = AccountSession::new(&lossy, &signer, &mut store)?
                    .broadcast(id)
                    .await;
                fault = Some(format!(
                    "{:?}",
                    outcome.as_ref().map(|r| r.transaction_hash)
                ));
                if outcome.is_ok() {
                    return Err("withheld acknowledgement still reported success".into());
                }
            }
            let state_after_fault = store.reservation(id)?.map(|r| format!("{:?}", r.state));
            // A fresh process view: reopen and reconcile the exact saved hash without resubmitting.
            drop(store);
            let observed = ctx.wait(hash).await?;
            let mut reopened = ctx.store(0)?;
            let family = AccountSession::new(&ctx.provider, &signer, &mut reopened)?
                .observe_candidates(id)
                .await?;
            let mined = ctx
                .provider
                .transaction(Zone::Cyprus1, hash)
                .await?
                .ok_or("mined absent")?
                .verified_quai()?;
            let nonce_now = ctx.provider.transaction_count(a, BlockTag::Latest).await?;
            save_record(
                "lost-ack",
                &json!({"check":"account-broadcast-acknowledgement-lost","hash":hash.to_string(),"nonce":signed.transaction().nonce,
                "broadcastResultUnderFault":fault,"reservationStateAfterFault":state_after_fault,"observed":observed,
                "familyObservation":format!("{family:?}"),"canonical":family.canonical.map(|h|h.to_string()),
                "minedBytesMatchCustody":mined.signed_bytes()? == bytes,"latestNonceAfter":nonce_now.to_string()}),
            )?;
            if family.canonical != Some(hash) || mined.signed_bytes()? != bytes {
                return Err(
                    "lost acknowledgement not reconciled to the exact saved transaction".into(),
                );
            }
        }
        "unknown-replacement" => {
            // The wallet signs and durably holds an original but never sends it. A
            // separate signer (another device/restored copy) consumes the nonce.
            let id = ReservationId([25; 16]);
            let signer = ctx.signer(0)?;
            let mut store = ctx.store(0)?;
            if store.signed_payload(id)?.is_none() {
                let mut attempt = 0;
                loop {
                    attempt += 1;
                    let mut session = AccountSession::new(&ctx.provider, &signer, &mut store)?
                        .with_observation_policy(AccountObservationPolicy::PinnedLatest);
                    match session
                        .prepare(
                            id,
                            AccountIntent {
                                to: b,
                                value: centi,
                                data: RpcData::new(vec![])?,
                                access_list: vec![],
                            },
                            net().account_fee,
                        )
                        .await
                    {
                        Ok(prepared) => {
                            session.sign(&prepared)?;
                            break;
                        }
                        Err(quai_sdk::accounts::AccountError::ObservationChanged)
                            if attempt < 5 =>
                        {
                            tokio::time::sleep(Duration::from_secs(3)).await
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
            }
            let original =
                SignedQuaiTransaction::decode(&store.signed_payload(id)?.ok_or("original")?)?;
            let start = ctx.provider.block_number(Zone::Cyprus1.into()).await?;
            let start = u64::try_from(start).map_err(|_| "height")?;
            let competitor_path = dir()
                .join("checks")
                .join("unknown-replacement-competitor.hex");
            let competitor = if competitor_path.exists() {
                let text = fs::read_to_string(&competitor_path)?;
                let raw = (0..text.trim().len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&text.trim()[i..i + 2], 16))
                    .collect::<Result<Vec<_>, _>>()?;
                SignedQuaiTransaction::decode(&raw)?
            } else {
                let mut tx = original.transaction().clone();
                tx.to = Some(a.address()); // Cancellation: zero-value, empty-data transfer to self.
                tx.value = U256::ZERO;
                tx.data = vec![];
                tx.gas_price *= U256::from(2);
                let signed = tx.sign(&ctx.key(0)?)?;
                fs::create_dir_all(dir().join("checks"))?;
                fs::write(
                    &competitor_path,
                    signed
                        .signed_bytes()?
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>(),
                )?;
                signed
            };
            if ctx
                .provider
                .receipt(Zone::Cyprus1, competitor.hash()?)
                .await?
                .is_none()
            {
                let sent = ctx.provider.broadcast(&competitor).await;
                println!(
                    "{}",
                    json!({"stage":"unregistered-competitor-broadcast","hash":competitor.hash()?.to_string(),"result":format!("{:?}",sent.map(|r|r.transaction_hash))})
                );
            }
            let found = ctx
                .provider
                .wait_for_account_transaction(
                    &original,
                    ctx.scope.genesis,
                    start.saturating_sub(4).max(1),
                    WaitConfig {
                        confirmations: 2,
                        timeout: Duration::from_secs(300),
                        poll_interval: Duration::from_secs(3),
                    },
                )
                .await?;
            let family = AccountSession::new(&ctx.provider, &signer, &mut store)?
                .observe_candidates(id)
                .await?;
            let reservation = store.reservation(id)?.map(|r| format!("{:?}", r.state));
            save_record(
                "unknown-replacement",
                &json!({"check":"unregistered-nonce-competitor-discovery",
                "originalHash":original.hash()?.to_string(),"originalBroadcast":false,"nonce":original.transaction().nonce,
                "competitorHash":competitor.hash()?.to_string(),"foundHash":found.transaction.hash()?.to_string(),
                "reason":format!("{:?}",found.reason),"confirmations":found.confirmations,"block":found.inclusion.block_number,
                "receiptOutcome":found.receipt.as_ref().map(|r|format!("{:?}",r.outcome)),
                "registeredFamilyObservation":format!("{family:?}"),"reservationStillHeld":reservation}),
            )?;
            if found.transaction.hash()? != competitor.hash()?
                || found.reason != Some(quai_sdk::provider::ReplacementReason::Cancelled)
            {
                return Err("waiter did not report the unregistered cancellation".into());
            }
        }
        "deploy" => {
            let id = ReservationId([26; 16]);
            let signer = ctx.signer(0)?;
            let mut store = ctx.store(0)?;
            // Init code copies ten runtime bytes that return 42; no storage or owner.
            let init: RpcData = "0x600a600c600039600a6000f3602a60005260206000f3".parse()?;
            let runtime =
                quai_sdk::crypto::keccak256("0x602a60005260206000f3".parse::<RpcData>()?.bytes());
            if store.signed_payload(id)?.is_none() {
                let nonce = match store.reserved_nonce(id)? {
                    Some((_, nonce)) => nonce,
                    None => {
                        AccountSession::new(&ctx.provider, &signer, &mut store)?
                            .with_observation_policy(AccountObservationPolicy::PinnedLatest)
                            .reserve_deployment_nonce(id)
                            .await?
                    }
                };
                let mut attempt = 0;
                loop {
                    attempt += 1;
                    let deployment = quai_sdk::contracts::prepare_deployment(
                        &quai_sdk::abi::AbiInterface::from_json(b"[]")?,
                        init.bytes(),
                        &[],
                        a,
                        ctx.scope.chain_id,
                        nonce,
                        U256::ZERO,
                        quai_sdk::contracts::DeploymentSearch {
                            start_salt: 0,
                            max_attempts: 10_000,
                        },
                        || false,
                    )?;
                    println!(
                        "{}",
                        json!({"stage":"deployment-ground","nonce":nonce,"salt":deployment.salt(),"predicted":deployment.address().to_string()})
                    );
                    let mut session = AccountSession::new(&ctx.provider, &signer, &mut store)?
                        .with_observation_policy(AccountObservationPolicy::PinnedLatest);
                    match session
                        .prepare_deployment(id, deployment, net().account_fee)
                        .await
                    {
                        Ok(prepared) => {
                            println!(
                                "{}",
                                json!({"stage":"deployment-prepared","gasLimit":prepared.transaction().gas_limit,"maximumFeeIts":prepared.maximum_fee().to_string()})
                            );
                            session.sign(&prepared)?;
                            break;
                        }
                        Err(quai_sdk::accounts::AccountError::ObservationChanged)
                            if attempt < 5 =>
                        {
                            tokio::time::sleep(Duration::from_secs(3)).await
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
            }
            let bytes = store.signed_payload(id)?.ok_or("signed deployment")?;
            let signed = SignedQuaiTransaction::decode(&bytes)?;
            let hash = signed.hash()?;
            let predicted = QuaiAddress::try_from(quai_sdk::primitives::contract_address(
                a.address(),
                signed.transaction().nonce,
                &signed.transaction().data,
            ))?;
            if ctx.provider.receipt(Zone::Cyprus1, hash).await?.is_none() {
                AccountSession::new(&ctx.provider, &signer, &mut store)?
                    .broadcast(id)
                    .await?;
            }
            let observed = ctx.wait(hash).await?;
            let receipt = ctx
                .provider
                .receipt(Zone::Cyprus1, hash)
                .await?
                .ok_or("receipt")?;
            let code = ctx
                .provider
                .wait_for_contract_code(
                    quai_sdk::provider::ContractCodeTarget {
                        address: predicted,
                        genesis: ctx.scope.genesis,
                        expected_runtime: Some(Hash32::from_bytes(runtime)),
                    },
                    quai_sdk::provider::CodeWaitConfig {
                        timeout_ms: 120_000,
                        poll_interval_ms: 3_000,
                        max_polls: 60,
                    },
                )
                .await?;
            let call = ctx
                .provider
                .call(
                    &quai_sdk::provider::CallRequest {
                        from: a,
                        to: Some(predicted),
                        gas: Some(100_000),
                        gas_price: None,
                        value: None,
                        nonce: None,
                        input: RpcData::new(vec![])?,
                        access_list: vec![],
                    },
                    BlockTag::Latest,
                )
                .await?;
            save_record(
                "deploy",
                &json!({"check":"durable-grinded-deployment-and-code-wait","hash":hash.to_string(),"nonce":signed.transaction().nonce,
                "predicted":predicted.to_string(),"receiptContractAddress":receipt.contract_address.map(|c|c.to_string()),
                "observed":observed,"codeBlock":code.block.number,"runtimeHashMatched":true,"callReturn":format!("0x{}",call.bytes().iter().map(|b|format!("{b:02x}")).collect::<String>())}),
            )?;
            if receipt.contract_address != Some(predicted) {
                return Err("receipt contract address differs from prediction".into());
            }
        }
        "nonce-outcome" => {
            // Re-check the held nonce-13 reservation through the combined SDK API.
            let id = ReservationId([25; 16]);
            let signer = ctx.signer(0)?;
            let mut store = ctx.store(0)?;
            let head = u64::try_from(ctx.provider.block_number(Zone::Cyprus1.into()).await?)
                .map_err(|_| "height")?;
            let from = 10_097_890;
            let observation = AccountSession::new(&ctx.provider, &signer, &mut store)?
                .observe_nonce(
                    id,
                    quai_sdk::provider::AccountReplacementScanRequest {
                        from_block: from,
                        to_block: head.min(from + 255),
                        max_transactions_per_block: 4096,
                        max_total_transactions: 65_536,
                        preceding_block: None,
                    },
                )
                .await?;
            let summary = match &observation.outcome {
                quai_sdk::accounts::AccountNonceOutcome::Registered(hash) => {
                    json!({"outcome":"registered","hash":hash.to_string()})
                }
                quai_sdk::accounts::AccountNonceOutcome::Unregistered(c) => {
                    json!({"outcome":"unregistered","hash":c.transaction.hash()?.to_string(),"reason":format!("{:?}",c.reason),"block":c.inclusion.block_number,"confirmations":c.confirmations})
                }
                quai_sdk::accounts::AccountNonceOutcome::Unresolved {
                    scanned_through,
                    missing_block,
                } => {
                    json!({"outcome":"unresolved","scannedThrough":scanned_through.map(|b|b.number),"missingBlock":missing_block})
                }
            };
            save_record(
                "nonce-outcome",
                &json!({"check":"observe-nonce-combined-reconciliation","family":format!("{:?}",observation.family),"result":summary,"reservation":store.reservation(id)?.map(|r|format!("{:?}",r.state))}),
            )?;
            if summary["outcome"] != "unregistered"
                || summary["hash"]
                    != "0x007a003c4f13ab9de078f3d5a3315d4fb61c5720d83d26e03a901a01c78c6adc"
            {
                return Err("observe_nonce did not report the unregistered cancellation".into());
            }
        }
        _ => {
            return Err("expected token-roundtrip, lost-ack, unknown-replacement or deploy".into());
        }
    }
    Ok(())
}
