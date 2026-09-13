//! Explicit isolated WQUAI acceptance using bytecode observed on mainnet, not verified source.
use super::*;
use quai_sdk::wrappers::WrappedQuai;
const SOURCE_RUNTIME_SHA: &str =
    "0xb55a87a8bbabbb5ffd44c6e17f7273c9c22fea6a47a03d58a49b45160bcbc3f6";
const SOURCE_CREATION_SHA: &str =
    "0xe670f177b98bb8bd4653a78f295308e31fe70e9ec30b396fd4bbb7d1be1518a8";
fn fixture_code() -> Result<(RpcData, RpcData), Box<dyn Error>> {
    let bytes = std::fs::read("/tmp/quai-wrapper-source/wquai.json")?;
    if bytes.len() > 65536 {
        return Err("wrapper artifact size".into());
    }
    let v: Value = serde_json::from_slice(&bytes)?;
    let creation: RpcData = v["creation_bytecode"].as_str().ok_or("creation")?.parse()?;
    let runtime: RpcData = v["deployed_bytecode"].as_str().ok_or("runtime")?.parse()?;
    for (code, expected) in [
        (&creation, SOURCE_CREATION_SHA),
        (&runtime, SOURCE_RUNTIME_SHA),
    ] {
        assert_eq!(
            quai_sdk::primitives::Hash32::from_bytes(quai_sdk::crypto::sha256(code.bytes()))
                .to_string(),
            expected
        );
    }
    Ok((creation, runtime))
}
pub(super) async fn run(
    mode: &str,
    provider: &Provider<Recorded>,
    signer: &LocalSigner,
    path: &Path,
) -> Result<(), Box<dyn Error>> {
    if !path.is_file() {
        return Err("existing funded fixture wallet required".into());
    }
    let parts: Vec<_> = mode.split('-').collect();
    if parts.len() != 3 {
        return Err("wrapper mode".into());
    }
    let operation = parts[1];
    let stage = parts[2];
    let index = match operation {
        "deploy" => 20,
        "deposit" => 21,
        "approve" => 22,
        "transfer" => 23,
        "withdraw" => 24,
        _ => return Err("wrapper operation".into()),
    };
    let id = ReservationId([index; 16]);
    let file = format!("wquai-{operation}-signed.json");
    let sender: QuaiAddress = signer.address().try_into()?;
    let recipient: QuaiAddress = "0x0011223344556677889900112233445566778899".parse()?;
    let deposit = U256::from(123456789);
    let transfer = U256::from(23456);
    let approval = U256::from(1000);
    let (creation, runtime) = fixture_code()?;
    let deployed = if operation == "deploy" {
        None
    } else {
        Some(
            read("wquai-deploy-signed.json")?["contract"]
                .as_str()
                .ok_or("contract")?
                .parse::<QuaiAddress>()?,
        )
    };
    let mut store = SqliteStore::open(path, scope())?;
    match stage {
        "prepare" => {
            if store.reservation(id)?.is_some() {
                return Err(
                    "wrapper operation already exists; use exact broadcast/recovery".into(),
                );
            }
            let head = provider.latest_header(Zone::Cyprus1).await?.ok_or("head")?;
            let before_native = provider
                .balance(sender, BlockTag::Number(U256::from(head.number)))
                .await?;
            let policy = FeePolicy {
                max_gas: 2_000_000,
                max_gas_price: U256::from(10_000_000_000_000_000u64),
                max_total_fee: U256::from(20_000_000_000_000_000_000_000u128),
                gas_margin_bps: 1000,
            };
            let mut session = AccountSession::new(provider, signer, &mut store)?
                .with_observation_policy(AccountObservationPolicy::PinnedLatest);
            let prepared = if operation == "deploy" {
                let nonce = session.reserve_deployment_nonce(id).await?;
                let intent = prepare_deployment(
                    &AbiInterface::from_json(b"[]")?,
                    creation.bytes(),
                    &[],
                    sender,
                    scope().chain_id,
                    nonce,
                    U256::ZERO,
                    DeploymentSearch {
                        start_salt: 0,
                        max_attempts: 10000,
                    },
                    || false,
                )?;
                session.prepare_deployment(id, intent, policy).await?
            } else {
                let wrapper = WrappedQuai::new(deployed.ok_or("deployment")?, provider)?;
                let call = match operation {
                    "deposit" => wrapper.deposit(deposit)?,
                    "approve" => wrapper.token()?.approve(recipient, approval)?,
                    "transfer" => wrapper.token()?.transfer(recipient, transfer)?,
                    "withdraw" => wrapper.withdraw(deposit - transfer)?,
                    _ => return Err("wrapper call".into()),
                };
                session
                    .prepare(id, call.into_account_intent(), policy)
                    .await?
            };
            let signed = session.sign(&prepared)?;
            let mut record = signed_record(signed.hash()?.to_string(), signed.signed_bytes()?);
            record["contract"] = json!(
                prepared
                    .created_address()
                    .or(deployed)
                    .ok_or("contract")?
                    .to_string()
            );
            record["nonce"] = json!(prepared.transaction().nonce);
            record["gasPrice"] = json!(prepared.transaction().gas_price.to_string());
            record["gasLimit"] = json!(prepared.transaction().gas_limit);
            record["beforeNative"] = json!(before_native.to_string());
            record["beforeHeight"] = json!(head.number);
            record["runtimeSha256"] = json!(SOURCE_RUNTIME_SHA);
            save(&file, record)?;
        }
        "broadcast" => {
            let record = read(&file)?;
            let expected = record["hash"].as_str().ok_or("hash")?.parse()?;
            let mut session = AccountSession::new(provider, signer, &mut store)?;
            assert_eq!(session.broadcast(id).await?.transaction_hash, expected);
        }
        "verify" => {
            let record = read(&file)?;
            let hash = record["hash"].as_str().ok_or("hash")?.parse()?;
            let contract: QuaiAddress = record["contract"].as_str().ok_or("contract")?.parse()?;
            let observed =
                quai_sdk::recovery::reconcile_operation(provider, &mut store, id).await?;
            let quai_sdk::recovery::OperationObservation::Included {
                block,
                outcome,
                confirmations,
            } = observed
            else {
                return Err("wrapper inclusion missing".into());
            };
            assert_eq!(outcome, quai_sdk::provider::ReceiptOutcome::Succeeded);
            let tag = BlockTag::Number(block.height);
            assert_eq!(provider.code(contract, tag).await?.bytes(), runtime.bytes());
            let token = WrappedQuai::new(contract, provider)?.token()?;
            let balance = token.balance_of(sender, sender, tag).await?;
            let other_balance = token.balance_of(sender, recipient, tag).await?;
            let allowance = token.allowance(sender, sender, recipient, tag).await?;
            let native_backing = provider.balance(contract, tag).await?;
            let expected_owner = match operation {
                "deploy" | "withdraw" => U256::ZERO,
                "transfer" => deposit - transfer,
                _ => deposit,
            };
            assert_eq!(balance, expected_owner);
            let transferred = matches!(operation, "transfer" | "withdraw");
            assert_eq!(
                other_balance,
                if transferred { transfer } else { U256::ZERO }
            );
            assert_eq!(
                allowance,
                if matches!(operation, "approve" | "transfer" | "withdraw") {
                    approval
                } else {
                    U256::ZERO
                }
            );
            assert_eq!(native_backing, balance + other_balance);
            let receipt = provider
                .receipt(Zone::Cyprus1, hash)
                .await?
                .ok_or("receipt")?;
            let price = U256::from_str_radix(record["gasPrice"].as_str().ok_or("price")?, 10)?;
            assert_eq!(receipt.effective_gas_price, price);
            let fee = U256::from(receipt.gas_used)
                .checked_mul(price)
                .ok_or("fee overflow")?;
            let before =
                U256::from_str_radix(record["beforeNative"].as_str().ok_or("balance")?, 10)?;
            let expected_native = if operation == "deposit" {
                before - fee - deposit
            } else if operation == "withdraw" {
                before - fee + deposit - transfer
            } else {
                before - fee
            };
            let after = provider.balance(sender, tag).await?;
            assert_eq!(after, expected_native);
            // Read-only overdraw simulation; no extra nonce claim or signed payload.
            let wrapper = WrappedQuai::new(contract, provider)?;
            let overdraw = wrapper.withdraw(balance + U256::from(1))?;
            let mut call = quai_sdk::provider::CallRequest::new(sender, contract);
            call.input = overdraw.data().clone();
            call.gas = Some(1_000_000);
            let error = provider
                .call(&call, tag)
                .await
                .expect_err("overdraw must revert");
            assert!(
                matches!(error,quai_sdk::provider::ProviderError::Rpc(quai_sdk::rpc::RpcError::Remote(error)) if error.code == 3)
            );
            drop(store);
            let reopened = SqliteStore::open(path, scope())?;
            assert_eq!(
                reopened.reservation(id)?.ok_or("reservation")?.state,
                ReservationState::Confirmed
            );
            save(
                &format!("wquai-{operation}-verified.json"),
                json!({"operation":operation,"hash":hash.to_string(),"contract":contract.to_string(),"height":block.height.to_string(),"block":block.hash.to_string(),"confirmations":confirmations,"gasUsed":receipt.gas_used,"fee":fee.to_string(),"ownerTokenBalance":balance.to_string(),"recipientTokenBalance":other_balance.to_string(),"allowance":allowance.to_string(),"nativeBacking":native_backing.to_string(),"nativeBalanceDeltaChecked":true,"overdrawSimulationReverted":true,"durableConfirmedAfterReopen":true,"runtimeMatchesMainnetObservedBytes":true,"sourceVerified":false,"qualification":"isolated patched go-quai development profile; public toy-key funds; no mainnet transaction"}),
            )?;
        }
        _ => return Err("wrapper stage".into()),
    }
    Ok(())
}
