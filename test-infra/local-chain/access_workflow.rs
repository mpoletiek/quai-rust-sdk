//! Ordinary contract access discovery using the isolated public-fixture wallet.
use super::*;
pub(super) async fn run(
    mode: &str,
    provider: &Provider<Recorded>,
    signer: &LocalSigner,
    path: &Path,
) -> Result<(), Box<dyn Error>> {
    let id = ReservationId([37; 16]);
    let mut store = SqliteStore::open(path, scope())?;
    let contract: QuaiAddress = read("wquai-deploy-verified.json")?["contract"]
        .as_str()
        .ok_or("contract")?
        .parse()?;
    let sender = signer.address().try_into()?;
    let spender = "0x0011223344556677889900112233445566778899".parse()?;
    let token = quai_sdk::wrappers::WrappedQuai::new(contract, provider)?.token()?;
    match mode {
        "access-prepare" => {
            if store.reservation(id)?.is_some() {
                return Err("existing access operation".into());
            }
            let head = provider.latest_header(Zone::Cyprus1).await?.ok_or("head")?;
            let tag = BlockTag::Number(U256::from(head.number));
            assert_eq!(
                token.allowance(sender, sender, spender, tag).await?,
                U256::from(1000)
            );
            let before = provider.balance(sender, tag).await?;
            let call = token.approve(spender, U256::ZERO)?;
            assert!(call.access_list().is_empty());
            let mut session = AccountSession::new(provider, signer, &mut store)?
                .with_observation_policy(AccountObservationPolicy::PinnedLatest)
                .with_access_list_policy(quai_sdk::accounts::AccountAccessListPolicy::Discover);
            let prepared = session
                .prepare(
                    id,
                    call.into_account_intent(),
                    FeePolicy::new(
                        200000,
                        U256::from(10_000_000_000_000_000u64),
                        U256::from(2_000_000_000_000_000_000_000u128),
                    )
                    .with_gas_margin_bps(1000),
                )
                .await?;
            assert!(
                prepared
                    .transaction()
                    .access_list
                    .iter()
                    .any(|a| a.address == contract.address() && !a.storage_keys.is_empty())
            );
            let signed = session.sign(&prepared)?;
            let mut record = signed_record(signed.hash()?.to_string(), signed.signed_bytes()?);
            record["beforeHeight"] = json!(head.number);
            record["beforeNative"] = json!(before.to_string());
            record["contract"] = json!(contract.to_string());
            record["nonce"] = json!(prepared.transaction().nonce);
            record["accessList"]=json!(prepared.transaction().access_list.iter().map(|a|json!({"address":a.address.to_string(),"storageKeys":a.storage_keys.iter().map(ToString::to_string).collect::<Vec<_>>()})).collect::<Vec<_>>());
            save("access-signed.json", record)?;
        }
        "access-broadcast" => {
            let expected = read("access-signed.json")?;
            let mut session = AccountSession::new(provider, signer, &mut store)?
                .with_access_list_policy(quai_sdk::accounts::AccountAccessListPolicy::Discover);
            assert_eq!(
                session.broadcast(id).await?.transaction_hash.to_string(),
                expected["hash"]
            );
        }
        "access-verify" => {
            let record = read("access-signed.json")?;
            let quai_sdk::recovery::OperationObservation::Included { block, outcome, .. } =
                quai_sdk::recovery::reconcile_operation(provider, &mut store, id).await?
            else {
                return Err("inclusion".into());
            };
            assert_eq!(outcome, quai_sdk::provider::ReceiptOutcome::Succeeded);
            let hash = record["hash"].as_str().ok_or("hash")?.parse()?;
            let receipt = provider
                .receipt(Zone::Cyprus1, hash)
                .await?
                .ok_or("receipt")?;
            let tag = BlockTag::Number(block.height);
            assert_eq!(
                token.allowance(sender, sender, spender, tag).await?,
                U256::ZERO
            );
            let before: U256 = record["beforeNative"].as_str().ok_or("before")?.parse()?;
            let after = provider.balance(sender, tag).await?;
            let fee = U256::from(receipt.gas_used)
                .checked_mul(receipt.effective_gas_price)
                .ok_or("fee")?;
            assert_eq!(before.checked_sub(after), Some(fee));
            let mut reopened = SqliteStore::open(path, scope())?;
            assert_eq!(
                reopened.reservation(id)?.ok_or("reservation")?.state,
                ReservationState::Confirmed
            );
            assert_eq!(
                RpcData::new(reopened.signed_payload(id)?.ok_or("bytes")?)?.to_hex(),
                record["signedBytes"]
            );
            save(
                "access-verified.json",
                json!({"hash":hash.to_string(),"height":block.height.to_string(),"block":block.hash.to_string(),"contract":contract.to_string(),"allowance":"0","fee":fee.to_string(),"generatedAccessListSignedAndPersisted":true,"qualification":"isolated patched go-quai profile; public fixture funds"}),
            )?;
        }
        _ => return Err("access mode".into()),
    }
    Ok(())
}
