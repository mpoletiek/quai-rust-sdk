//! Prove an account's balance, nonce and code hash, and optionally one ERC-20
//! balance, against a block's state root; read-only, never submits.
//!
//! QUAI_RPC_URL, QUAI_EXPECTED_CHAIN_ID (decimal) and QUAI_EXPECTED_GENESIS are
//! required. QUAI_ACCOUNT defaults to WQUAI. For a token balance, set
//! QUAI_TOKEN, QUAI_TOKEN_HOLDER and QUAI_BALANCES_SLOT: the slot where that
//! contract declares its `balanceOf` mapping (3 for WQUAI, a WETH9 layout).
use quai_sdk::primitives::{Hash32, QuaiAddress};
use quai_sdk::provider::state_proof::{
    address_word, solidity_mapping_slot, verify_account_proof, verify_storage_proof,
};
use quai_sdk::{BlockTag, HttpConfig, HttpTransport, Provider, Routing, U256, Zone};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let var = |name| std::env::var(name);
    let url = var("QUAI_RPC_URL")?;
    let chain = U256::from_str_radix(&var("QUAI_EXPECTED_CHAIN_ID")?, 10)?;
    let genesis: Hash32 = var("QUAI_EXPECTED_GENESIS")?.parse()?;
    let account: QuaiAddress = var("QUAI_ACCOUNT")
        // WQUAI on mainnet; `quai_sdk::wrappers` names it with the `abi` feature.
        .unwrap_or_else(|_| "0x006C3e2AaAE5DB1bCd11A1a097cE572312EADdBB".into())
        .parse()?;
    let provider = Provider::new(
        HttpTransport::new(HttpConfig::default())?,
        Routing::direct(&url, Zone::Cyprus1.into())?,
        chain,
    );

    // Accounts to prove at one block, each with the storage slots to prove.
    let mut targets: Vec<(QuaiAddress, Vec<Hash32>)> = vec![(account, vec![])];
    let token = match (
        var("QUAI_TOKEN"),
        var("QUAI_TOKEN_HOLDER"),
        var("QUAI_BALANCES_SLOT"),
    ) {
        (Ok(token), Ok(holder), Ok(base)) => {
            let holder: QuaiAddress = holder.parse()?;
            let slot =
                solidity_mapping_slot(address_word(holder), U256::from_str_radix(&base, 10)?);
            targets.push((token.parse()?, vec![slot]));
            Some((holder, slot))
        }
        _ => None,
    };
    let borrowed = targets
        .iter()
        .map(|(address, slots)| (*address, slots.as_slice()))
        .collect::<Vec<_>>();
    let proven = provider
        .prove_accounts(genesis, &borrowed, BlockTag::Latest)
        .await?;

    // What a wallet can store: the header identity and the proof. Checking it
    // again later needs no node.
    let first = &proven[0];
    let offline = verify_account_proof(first.state_root, first.address, &first.account_proof)?;
    assert_eq!(offline, first.account);
    let token_balance = token.map(|(holder, slot)| {
        let proven_token = &proven[1];
        let slot_proof = &proven_token.storage[0];
        let storage_root = proven_token.account.map(|a| a.storage_root);
        let again = storage_root
            .map(|root| verify_storage_proof(root, slot, &slot_proof.proof))
            .transpose();
        serde_json::json!({
            "token": proven_token.address.to_string(),
            "holder": holder.to_string(),
            "balance": slot_proof.value.to_string(),
            "reverified": again.is_ok(),
        })
    });
    println!(
        "{}",
        serde_json::json!({
            "block": first.block.number,
            "blockHash": first.block.hash.to_string(),
            "stateRoot": first.state_root.to_string(),
            "account": first.address.to_string(),
            "exists": first.account.is_some(),
            "balanceIts": first.balance().to_string(),
            "nonce": first.nonce(),
            "codeHash": first.code_hash().to_string(),
            "hasCode": first.has_code(),
            "accountProofBytes": first.account_proof.iter().map(|n| n.bytes().len()).sum::<usize>(),
            "token": token_balance,
            "provenAgainst": "the node-reported header above; not an independent header check",
            "submitted": false,
        })
    );
    Ok(())
}
