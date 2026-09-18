//! Bounded WQI acceptance with isolated public-fixture funds and durable exact signing.
use super::*;
fn fixture_code() -> Result<(RpcData, RpcData), Box<dyn Error>> {
    let bytes = std::fs::read("/tmp/quai-wrapper-source/wqi.json")?;
    if bytes.len() > 65536 {
        return Err("WQI artifact limit".into());
    }
    let artifact: Value = serde_json::from_slice(&bytes)?;
    let creation: RpcData = artifact["creation_bytecode"]
        .as_str()
        .ok_or("creation")?
        .parse()?;
    let expected: RpcData = artifact["deployed_bytecode"]
        .as_str()
        .ok_or("runtime")?
        .parse()?;
    for (code, sha) in [
        (
            &creation,
            "0x8add7fd6b6a88b107b95eb04ede0872388f2a6634a00e58b7741f295a15b7cdc",
        ),
        (
            &expected,
            "0xbc933c0661d3995cdcf53db5bf0424778a2b35b11a8c5a08999f53d33cf08df2",
        ),
    ] {
        assert_eq!(
            quai_sdk::primitives::Hash32::from_bytes(quai_sdk::crypto::sha256(code.bytes()))
                .to_string(),
            sha
        );
    }
    Ok((creation, expected))
}
fn relocated_runtime(
    reference: &RpcData,
    contract: QuaiAddress,
) -> Result<RpcData, Box<dyn Error>> {
    let source: QuaiAddress = "0x002b2596EcF05C93a31ff916E8b456DF6C77c750".parse()?;
    let domain = |address: QuaiAddress, chain: U256| {
        quai_sdk::abi::hash_domain(
            &json!({"name":"Wrapped Qi","version":"1","chainId":chain.to_string(),"verifyingContract":address.to_string()}),
        )
    };
    let old_domain = domain(source, U256::from(9))?;
    assert_eq!(
        old_domain.to_string(),
        "0xd2b1836a50194d1efa6aba84257c050eed7b08e6354c4414ddfda829e3a4fdc0"
    );
    let new_domain = domain(contract, scope().chain_id)?;
    let address_word = |address: QuaiAddress| {
        let mut word = [0; 32];
        word[12..].copy_from_slice(address.bytes());
        word
    };
    let mut bytes = reference.bytes().to_vec();
    for (offset, old, new) in [
        (4188, address_word(source), address_word(contract)),
        (
            4230,
            U256::from(9).to_be_bytes::<32>(),
            scope().chain_id.to_be_bytes::<32>(),
        ),
        (4272, *old_domain.bytes(), *new_domain.bytes()),
    ] {
        assert_eq!(bytes[offset - 1], 0x7f); // Exact PUSH32 cache operands in pinned runtime.
        assert_eq!(&bytes[offset..offset + 32], &old);
        bytes[offset..offset + 32].copy_from_slice(&new);
    }
    Ok(RpcData::new(bytes)?)
}
pub(super) async fn probe(
    provider: &Provider<Recorded>,
    signer: &LocalSigner,
) -> Result<(), Box<dyn Error>> {
    let (creation, expected) = fixture_code()?;
    let sender: QuaiAddress = signer.address().try_into()?;
    let head = provider.latest_header(Zone::Cyprus1).await?.ok_or("head")?;
    let tag = BlockTag::Number(U256::from(head.number));
    let nonce = provider.transaction_count(sender, tag).await?;
    let deployment = prepare_deployment(
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
    let contract = deployment.address();
    let tx = deployment.into_transaction(4_000_000, U256::ZERO)?;
    let mut call = quai_sdk::provider::CallRequest::new(sender, contract);
    call.to = None;
    call.input = RpcData::new(tx.data.clone())?;
    call.nonce = Some(nonce);
    call.gas = Some(4_000_000);
    call.access_list = tx
        .access_list
        .iter()
        .map(|entry| quai_sdk::provider::AccessListItem {
            address: entry.address,
            storage_keys: entry.storage_keys.clone(),
        })
        .collect();
    let runtime = provider.call(&call, tag).await?;
    assert_eq!(runtime, relocated_runtime(&expected, contract)?);
    save(
        "wqi-probe.json",
        json!({"height":head.number,"nonce":nonce,"contract":contract.to_string(),"runtime":runtime.to_hex(),"runtimeMatchesMainnet":runtime==expected,"runtimeBytes":runtime.bytes().len(),"mainnetRuntimeBytes":expected.bytes().len(),"runtimeMatchesExplicitCacheRelocation":true,"submitted":false}),
    )?;
    Ok(())
}

pub(super) async fn deploy(
    mode: &str,
    provider: &Provider<Recorded>,
    signer: &LocalSigner,
    path: &Path,
) -> Result<(), Box<dyn Error>> {
    let (creation, source_runtime) = fixture_code()?;
    let sender: QuaiAddress = signer.address().try_into()?;
    let id = ReservationId([30; 16]);
    let mut store = SqliteStore::open(path, scope())?;
    match mode {
        "wqi-deploy-prepare" => {
            if store.reservation(id)?.is_some() {
                return Err("WQI deployment already exists".into());
            }
            let mut session = AccountSession::new(provider, signer, &mut store)?
                .with_observation_policy(AccountObservationPolicy::PinnedLatest);
            let nonce = session.reserve_deployment_nonce(id).await?;
            let deployment = prepare_deployment(
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
            let expected = relocated_runtime(&source_runtime, deployment.address())?;
            let probe = read("wqi-probe.json")?;
            assert_eq!(probe["nonce"], json!(nonce));
            assert_eq!(probe["contract"], json!(deployment.address().to_string()));
            assert_eq!(probe["runtime"], json!(expected.to_hex()));
            let policy = FeePolicy {
                max_gas: 4_000_000,
                max_gas_price: U256::from(10_000_000_000_000_000u64),
                max_total_fee: U256::from(40_000_000_000_000_000_000_000u128),
                gas_margin_bps: 1000,
            };
            let prepared = session.prepare_deployment(id, deployment, policy).await?;
            let signed = session.sign(&prepared)?;
            let mut record = signed_record(signed.hash()?.to_string(), signed.signed_bytes()?);
            record["contract"] = json!(prepared.created_address().unwrap().to_string());
            record["nonce"] = json!(nonce);
            record["expectedRuntime"] = json!(expected.to_hex());
            save("wqi-deploy-signed.json", record)?;
        }
        "wqi-deploy-broadcast" => {
            let record = read("wqi-deploy-signed.json")?;
            let mut session = AccountSession::new(provider, signer, &mut store)?;
            assert_eq!(
                session.broadcast(id).await?.transaction_hash.to_string(),
                record["hash"].as_str().ok_or("hash")?
            );
        }
        "wqi-deploy-verify" => {
            let record = read("wqi-deploy-signed.json")?;
            let hash: quai_sdk::primitives::Hash32 =
                record["hash"].as_str().ok_or("hash")?.parse()?;
            let contract: QuaiAddress = record["contract"].as_str().ok_or("contract")?.parse()?;
            let expected = relocated_runtime(&source_runtime, contract)?;
            let expected_hash = quai_sdk::primitives::Hash32::from_bytes(
                quai_sdk::crypto::keccak256(expected.bytes()),
            );
            let update = quai_sdk::deployments::track_deployment(
                provider,
                &mut store,
                id,
                hash,
                Some(expected_hash),
            )
            .await?;
            let quai_sdk::provider::DeploymentObservation::Included {
                block,
                outcome,
                code: Some(code),
                ..
            } = update.observation
            else {
                return Err("WQI code missing".into());
            };
            assert_eq!(outcome, quai_sdk::provider::ReceiptOutcome::Succeeded);
            assert_eq!(code.bytes, expected);
            assert_eq!(code.matches_expected, Some(true));
            let wrapper = quai_sdk::wrappers::WrappedQi::new(contract, provider)?;
            let tag = BlockTag::Number(U256::from(block.number));
            assert_eq!(
                wrapper.token()?.balance_of(sender, sender, tag).await?,
                U256::ZERO
            );
            assert_eq!(wrapper.unclaimed(sender, tag).await?, U256::ZERO);
            save(
                "wqi-deploy-verified.json",
                json!({"contract":contract.to_string(),"hash":hash.to_string(),"height":block.number,"block":block.hash.to_string(),"runtimeHash":expected_hash.to_string(),"onlyPermitCacheRelocated":true,"initialTokenAtoms":"0","initialUnclaimedQits":"0","qualification":"isolated patched profile; unverified source; public fixture funds"}),
            )?;
        }
        _ => return Err("WQI deployment mode".into()),
    }
    Ok(())
}

pub(super) async fn workflow(
    mode: &str,
    provider: &Provider<Recorded>,
    signer: &LocalSigner,
    account_path: &Path,
    qi_path: &Path,
    wallet: &HdWallet,
) -> Result<(), Box<dyn Error>> {
    let parts: Vec<_> = mode.split('-').collect();
    if parts.len() != 3 {
        return Err("WQI mode".into());
    }
    let operation = parts[1];
    let stage = parts[2];
    let index = match operation {
        "wrap" => 31,
        "claim" => 32,
        "claim_retry" => 34,
        "claim_access" => 35,
        "unwrap" => 33,
        _ => return Err("WQI operation".into()),
    };
    let id = ReservationId([index; 16]);
    let file = format!("wqi-{operation}-signed.json");
    let contract: QuaiAddress = read("wqi-deploy-verified.json")?["contract"]
        .as_str()
        .ok_or("contract")?
        .parse()?;
    let sender: QuaiAddress = signer.address().try_into()?;
    let wrapper = quai_sdk::wrappers::WrappedQi::new(contract, provider)?;
    let qits = U256::from(1000);
    let mut store = SqliteStore::open(
        if operation == "wrap" {
            qi_path
        } else {
            account_path
        },
        scope(),
    )?;
    match stage {
        "prepare" => {
            if store.reservation(id)?.is_some() {
                return Err("existing WQI operation; use persisted recovery".into());
            }
            let head = provider.latest_header(Zone::Cyprus1).await?.ok_or("head")?;
            let tag = BlockTag::Number(U256::from(head.number));
            let before_backing = wrapper.unclaimed(sender, tag).await?;
            let before_token = wrapper.token()?.balance_of(sender, sender, tag).await?;
            let before_native = provider.balance(sender, tag).await?;
            let mut record = if operation == "wrap" {
                assert_eq!(before_backing, U256::ZERO);
                assert_eq!(before_token, U256::ZERO);
                let account = wallet.account_public(0)?;
                let pool = QiChangePool::allocate(&mut store, &account, 0, 1, || false)?;
                quai_sdk::qi_discovery::refresh_qi(provider, &mut store, 1000, || false).await?;
                let mut session = QiSession::new(provider, wallet, &mut store)?;
                // Public-fixture fee authorized explicitly: consume the remaining 10,000-Qit coin.
                let fee = U256::from(9000);
                let prepared = session
                    .prepare_special(
                        id,
                        qits,
                        quai_sdk::qi::QiSpecialIntent::Wrapping(
                            quai_sdk::consensus::QiWrappingIntent {
                                destination: sender,
                                owner_contract: contract,
                            },
                        ),
                        fee,
                        QiPolicy {
                            initial_fee: fee,
                            max_fee: fee,
                            max_inputs: 32,
                            max_outputs: 32,
                            max_fee_rounds: 1,
                            max_snapshot_age: 0,
                        },
                        pool,
                    )
                    .await?;
                assert_eq!(prepared.transaction().transaction().inputs.len(), 1);
                assert_eq!(prepared.fee(), fee);
                let signed = session.sign_special(&prepared)?;
                let mut record = signed_record(signed.hash()?.to_string(), signed.signed_bytes()?);
                record["explicitFixtureFeeQits"] = json!(fee.to_string());
                record
            } else {
                let mut destination = None;
                let call = if operation.starts_with("claim") {
                    assert_eq!(before_backing, qits);
                    assert_eq!(before_token, U256::ZERO);
                    wrapper.claim_deposit()?
                } else {
                    assert_eq!(before_backing, U256::ZERO);
                    assert_eq!(before_token, quai_sdk::wrappers::qits_to_wqi_atoms(qits)?);
                    let mut qi = SqliteStore::open(qi_path, scope())?;
                    let allocated =
                        qi.allocate_address(&wallet.account_public(0)?, false, 10000, || false)?;
                    let address = QiAddress::try_from(allocated.address.address())?;
                    destination = Some(address);
                    wrapper.unwrap(address, qits, 9000)?
                };
                let policy = FeePolicy {
                    max_gas: 2_000_000,
                    max_gas_price: U256::from(10_000_000_000_000_000u64),
                    max_total_fee: U256::from(20_000_000_000_000_000_000_000u128),
                    gas_margin_bps: 1000,
                };
                let mut session = AccountSession::new(provider, signer, &mut store)?
                    .with_observation_policy(AccountObservationPolicy::PinnedLatest);
                let prepared = session
                    .prepare(id, call.into_account_intent(), policy)
                    .await?;
                let signed = session.sign(&prepared)?;
                let mut record = signed_record(signed.hash()?.to_string(), signed.signed_bytes()?);
                record["destination"] = json!(destination.map(|a| a.to_string()));
                record["gasPrice"] = json!(prepared.transaction().gas_price.to_string());
                record
            };
            record["contract"] = json!(contract.to_string());
            record["qits"] = json!(qits.to_string());
            record["beforeHeight"] = json!(head.number);
            record["beforeNative"] = json!(before_native.to_string());
            record["beforeUnclaimed"] = json!(before_backing.to_string());
            record["beforeTokenAtoms"] = json!(before_token.to_string());
            save(&file, record)?;
        }
        "broadcast" => {
            let record = read(&file)?;
            let hash = if operation == "wrap" {
                QiSession::new(provider, wallet, &mut store)?
                    .broadcast(id)
                    .await?
                    .transaction_hash
            } else {
                AccountSession::new(provider, signer, &mut store)?
                    .broadcast(id)
                    .await?
                    .transaction_hash
            };
            assert_eq!(hash.to_string(), record["hash"].as_str().ok_or("hash")?);
        }
        "verify" => {
            let record = read(&file)?;
            let hash: quai_sdk::primitives::Hash32 =
                record["hash"].as_str().ok_or("hash")?.parse()?;
            let head = provider.latest_header(Zone::Cyprus1).await?.ok_or("head")?;
            let observed =
                quai_sdk::recovery::reconcile_operation(provider, &mut store, id).await?;
            let quai_sdk::recovery::OperationObservation::Included { block, outcome, .. } =
                observed
            else {
                return Err("WQI origin inclusion missing".into());
            };
            let mut reopened = SqliteStore::open(
                if operation == "wrap" {
                    qi_path
                } else {
                    account_path
                },
                scope(),
            )?;
            assert_eq!(
                reopened.reservation(id)?.ok_or("reservation")?.state,
                ReservationState::Confirmed
            );
            let stored_bytes = reopened.signed_payload(id)?.ok_or("persisted bytes")?;
            assert_eq!(RpcData::new(stored_bytes)?.to_hex(), record["signedBytes"]);
            if operation == "wrap" {
                assert_eq!(reopened.reserved_outpoints(id)?.len(), 1);
            } else {
                assert!(reopened.reserved_nonce(id)?.is_some());
                let receipt = provider
                    .receipt(Zone::Cyprus1, hash)
                    .await?
                    .ok_or("receipt")?;
                let signed =
                    SignedQuaiTransaction::decode(&reopened.signed_payload(id)?.ok_or("bytes")?)?;
                let price = receipt.effective_gas_price;
                assert_eq!(price, signed.transaction().gas_price);
                let fee = U256::from(receipt.gas_used)
                    .checked_mul(price)
                    .ok_or("fee overflow")?;
                let before: U256 = record["beforeNative"]
                    .as_str()
                    .ok_or("before balance")?
                    .parse()?;
                let after = provider
                    .balance(sender, BlockTag::Number(block.height))
                    .await?;
                assert_eq!(before.checked_sub(after), Some(fee));
            }
            if (operation == "claim" || operation == "claim_retry")
                && outcome == quai_sdk::provider::ReceiptOutcome::Failed
            {
                let tag = BlockTag::Number(block.height);
                assert_eq!(wrapper.unclaimed(sender, tag).await?, qits);
                assert_eq!(
                    wrapper.token()?.balance_of(sender, sender, tag).await?,
                    U256::ZERO
                );
                save(
                    &format!("wqi-{operation}-failed-verified.json"),
                    json!({"hash":hash.to_string(),"height":block.height.to_string(),"outcome":"failed","backingQits":"1000","tokenAtoms":"0","claimsRetained":true,"reason":"missing lockup access declaration; simulation auto-discovers access while signed execution requires the address"}),
                )?;
                return Ok(());
            }
            assert_eq!(outcome, quai_sdk::provider::ReceiptOutcome::Succeeded);
            let mut result = json!({"hash":hash.to_string(),"contract":contract.to_string(),"originHeight":block.height.to_string(),"height":head.number,"block":head.hash.to_string()});
            if !operation.starts_with("claim") {
                let kind = if operation == "wrap" {
                    quai_sdk::settlement::SettlementKind::QiWrapping
                } else {
                    quai_sdk::settlement::SettlementKind::WqiRedemption {
                        contract,
                        etx_index: 0,
                    }
                };
                let update = quai_sdk::settlement::track_settlement(
                    provider,
                    &mut store,
                    id,
                    hash,
                    kind,
                    quai_sdk::provider::EtxScanRequest::new(
                        Zone::Cyprus1,
                        u64::try_from(block.height)?,
                        head.number,
                        1024,
                        16384,
                    ),
                    100,
                )
                .await?;
                let external = update.external.ok_or("external observation")?;
                let execution = external
                    .scan
                    .and_then(|s| s.execution)
                    .ok_or("WQI destination execution not yet observed")?;
                assert!(matches!(
                    external.outcome,
                    Some(
                        quai_sdk::provider::ReceiptOutcome::Succeeded
                            | quai_sdk::provider::ReceiptOutcome::Locked
                    )
                ));
                result["executionHash"] = json!(execution.transaction.hash.to_string());
                result["destinationOutcome"] = json!(format!("{:?}", external.outcome));
                if operation == "unwrap" {
                    let credit = update.qi_credit.ok_or("Qi destination credit")?;
                    assert_eq!(credit.locked_qits, qits);
                    assert_eq!(credit.unlocked_qits, U256::ZERO);
                    assert_eq!(credit.unobserved_qits, U256::ZERO);
                    assert_eq!(credit.outputs.len(), 1);
                    result["lockedQits"] = json!(credit.locked_qits.to_string());
                    result["unlockedQits"] = json!(credit.unlocked_qits.to_string());
                    result["outputs"] = json!(credit.outputs.iter().map(|o| json!({"txHash":o.outpoint.tx_hash.to_string(),"index":o.outpoint.index,"denomination":o.denomination,"lock":o.lock.to_string()})).collect::<Vec<_>>());
                    let mut qi = SqliteStore::open(qi_path, scope())?;
                    let pool =
                        QiChangePool::allocate(&mut qi, &wallet.account_public(0)?, 0, 1, || {
                            false
                        })?;
                    quai_sdk::qi_discovery::refresh_qi(provider, &mut qi, 1000, || false).await?;
                    let balance =
                        quai_sdk::qi_discovery::qi_balance(&mut qi, U256::from(head.number + 1))?;
                    assert_eq!(balance.total, U256::from(9710));
                    assert_eq!(balance.locked, qits);
                    assert_eq!(balance.spendable, U256::from(8710));
                    let rejected = ReservationId([36; 16]);
                    let mut session = QiSession::new(provider, wallet, &mut qi)?;
                    let rejected_prepare = session
                        .prepare(
                            rejected,
                            QiIntent {
                                amount: balance.total,
                                destinations: vec![
                                    "0x0080000000000000000000000000000000000001".parse()?,
                                ],
                            },
                            QiPolicy {
                                initial_fee: U256::ZERO,
                                max_fee: U256::from(1000),
                                max_inputs: 32,
                                max_outputs: 32,
                                max_fee_rounds: 1,
                                max_snapshot_age: 0,
                            },
                            pool,
                        )
                        .await;
                    assert!(matches!(
                        rejected_prepare,
                        Err(quai_sdk::qi::QiError::Selection(
                            quai_sdk::wallet::SelectionError::InsufficientFunds
                        ))
                    ));
                    assert!(qi.reservation(rejected)?.is_none());
                    result["walletLockedQits"] = json!(balance.locked.to_string());
                    result["otherSpendableQits"] = json!(balance.spendable.to_string());
                    result["prematurePrepareRejectedWithoutClaims"] = json!(true);
                }
            }
            let tag = BlockTag::Number(U256::from(head.number));
            let backing = wrapper.unclaimed(sender, tag).await?;
            let token = wrapper.token()?.balance_of(sender, sender, tag).await?;
            assert_eq!(
                backing,
                if operation == "wrap" {
                    qits
                } else {
                    U256::ZERO
                }
            );
            assert_eq!(
                token,
                if operation.starts_with("claim") {
                    quai_sdk::wrappers::qits_to_wqi_atoms(qits)?
                } else {
                    U256::ZERO
                }
            );
            result["unclaimedQits"] = json!(backing.to_string());
            result["tokenAtoms"] = json!(token.to_string());
            result["qualification"] = json!(
                "isolated patched profile with legacy wrapping semantics; public fixture funds; locked redemption is not a mature spend"
            );
            save(&format!("wqi-{operation}-verified.json"), result)?;
        }
        _ => return Err("WQI stage".into()),
    }
    Ok(())
}
