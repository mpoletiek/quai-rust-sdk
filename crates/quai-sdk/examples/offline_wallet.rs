//! Deterministic public-fixture wallet derivation and signing. Never fund these keys.
use quai_sdk::consensus::{QuaiTransaction, SignedQuaiTransaction};
use quai_sdk::signer::{LocalSigner, Signer, WatchOnlySigner};
use quai_sdk::wallet::{CoinType, HdWallet, Language, Mnemonic, Search};
use quai_sdk::{U256, Zone};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Official BIP39 public test vector. This example performs no network operations.
    let mnemonic = Mnemonic::parse(
        Language::English,
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
    )?;
    let wallet = HdWallet::from_mnemonic(&mnemonic, "", CoinType::Quai)?;
    let receive = wallet.search(
        0,
        false,
        Search {
            zone: Zone::Cyprus1,
            start_index: 0,
            max_attempts: 100000,
        },
        || false,
    )?;
    let public = wallet.account_public(0)?;
    let watch_address = public.derive_address(false, receive.address.index)?;
    assert_eq!(receive.address.address, watch_address.address);
    let signer = LocalSigner::new(
        wallet
            .derive_key(0, false, receive.address.index)?
            .secret_key()?,
        U256::from(15000),
    )?;
    let watch = WatchOnlySigner::new(signer.address(), signer.chain_id())?;
    let transaction = QuaiTransaction {
        chain_id: signer.chain_id(),
        nonce: 0,
        to: Some("0x0011223344556677889900112233445566778899".parse()?),
        value: U256::from(1),
        gas_limit: 21000,
        gas_price: U256::from(1000000000),
        data: vec![],
        access_list: vec![],
    };
    let signed = signer.sign_quai(&transaction)?;
    let decoded = SignedQuaiTransaction::decode(&signed.signed_bytes()?)?;
    assert_eq!(decoded.from().address(), watch.address());
    println!("PUBLIC TEST FIXTURE — never fund this address");
    println!("Quai address: {}", receive.address.address);
    println!("Derivation path: {}", receive.address.path());
    println!("Offline transaction ID: {}", signed.hash()?);
    let qi = HdWallet::from_mnemonic(&mnemonic, "", CoinType::Qi)?;
    let qi_receive = qi.search(
        0,
        false,
        Search {
            zone: Zone::Cyprus1,
            start_index: 0,
            max_attempts: 100000,
        },
        || false,
    )?;
    println!("Qi address: {}", qi_receive.address.address);
    println!("Qi derivation path: {}", qi_receive.address.path());
    Ok(())
}
