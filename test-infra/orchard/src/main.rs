mod account;
mod diagnostic;
mod mainnet_checks;
mod mainnet_extra;
mod network;
mod qi;
mod qi_extended;
use network::{dir, net};
use quai_sdk::crypto::SecretKey;
use quai_sdk::payments::{PaymentCode, PrivatePaymentCode};
use quai_sdk::wallet::{CoinType, HdWallet, Language, Mnemonic, Search};
use quai_sdk::{BlockTag, HttpConfig, HttpTransport, Provider, QuaiAddress, Routing, U256, Zone};
use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    fs::{self, OpenOptions},
    os::unix::fs::OpenOptionsExt,
};
use zeroize::Zeroizing;
struct DiagnosticTransport(HttpTransport);
impl quai_sdk::rpc::Transport for DiagnosticTransport {
    async fn request_batch(
        &self,
        endpoint: &quai_sdk::Endpoint,
        requests: Vec<(&str, serde_json::Value)>,
    ) -> Option<quai_sdk::rpc::BatchResult> {
        self.0.request_batch(endpoint, requests).await
    }
    async fn request(
        &self,
        endpoint: &quai_sdk::Endpoint,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, quai_sdk::rpc::RpcError> {
        let result = self.0.request(endpoint, method, params.clone()).await;
        if let Err(error) = &result {
            eprintln!("{} RPC {method} failed: {error}", net().name);
        }
        if let Err(quai_sdk::rpc::RpcError::Remote(error)) = &result
            && matches!(
                method,
                "quai_estimateGas"
                    | "quai_getCode"
                    | "quai_getBalance"
                    | "quai_quaiToQi"
                    | "web3_clientVersion"
            )
        {
            // Only this harness's public test reads/simulations; private material never enters RPC.
            eprintln!(
                "{}",
                serde_json::json!({"diagnosticRpc":method,"params":params,"code":error.code,"message":error.message.chars().take(1000).collect::<String>()})
            );
        }
        // Explicit qualification-only fault: the node accepted the write but its
        // acknowledgement is hidden from the SDK. Never replay the request here.
        if method == "quai_sendRawTransaction"
            && result.is_ok()
            && std::env::args().any(|arg| arg == "broadcast-lost-ack")
        {
            eprintln!("qualification fault: successful submission acknowledgement withheld");
            return Err(quai_sdk::rpc::RpcError::Timeout);
        }
        result
    }
}
#[derive(Serialize)]
struct Generated<'a> {
    network: &'static str,
    expected_chain_id: u64,
    language: &'static str,
    mnemonic: &'a str,
    passphrase: &'static str,
    account: u32,
    payment_code: &'a str,
    hd_receive_address: String,
    hd_receive_path: String,
    hd_receive_index: u32,
    hd_receive_next_index: u32,
}
#[derive(Deserialize)]
struct Supplied<'a> {
    #[serde(borrow)]
    wallets: Vec<Account<'a>>,
}
#[derive(Deserialize)]
struct Account<'a> {
    #[serde(borrow)]
    address: &'a str,
    #[serde(borrow)]
    private_key: &'a str,
}
fn load_key(value: &str) -> Result<SecretKey, Box<dyn Error>> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    if value.len() != 64 || !value.is_ascii() {
        return Err("invalid private key format".into());
    }
    let mut bytes = Zeroizing::new([0u8; 32]);
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16)
            .map_err(|_| "invalid private key hex")?;
    }
    Ok(SecretKey::from_bytes(&bytes).map_err(|_| "invalid private scalar")?)
}
fn generate(name: &str) -> Result<(), Box<dyn Error>> {
    net().require_orchard("wallet generation")?;
    let path = dir().join(name);
    if path.exists() {
        return Err("existing Qi wallet preserved".into());
    }
    let mnemonic = Mnemonic::generate(Language::English, 24)?;
    let seed = mnemonic.to_seed("");
    let payment = PrivatePaymentCode::from_seed(seed.expose(), 0)?;
    let code = payment.public_code().to_base58();
    if PaymentCode::from_base58(&code)? != *payment.public_code() {
        return Err("payment code roundtrip".into());
    }
    let wallet = HdWallet::from_mnemonic(&mnemonic, "", CoinType::Qi)?;
    let receive = wallet.search(
        0,
        false,
        Search {
            zone: Zone::Cyprus1,
            start_index: 0,
            max_attempts: 100_000,
        },
        || false,
    )?;
    let phrase = mnemonic.phrase();
    let stored = Generated {
        network: "orchard",
        expected_chain_id: 15000,
        language: "english",
        mnemonic: phrase.expose(),
        passphrase: "",
        account: 0,
        payment_code: &code,
        hd_receive_address: receive.address.address.to_string(),
        hd_receive_path: receive.address.path(),
        hd_receive_index: receive.address.index,
        hd_receive_next_index: receive
            .address
            .index
            .checked_add(1)
            .ok_or("index overflow")?,
    };
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    serde_json::to_writer_pretty(&mut file, &stored)?;
    file.sync_all()?;
    fs::File::open(dir())?.sync_all()?;
    let reopened = Zeroizing::new(fs::read_to_string(&path)?);
    #[derive(Deserialize)]
    struct Restored<'a> {
        #[serde(borrow)]
        mnemonic: &'a str,
    }
    let restored: Restored =
        serde_json::from_str(&reopened).map_err(|_| "saved wallet JSON invalid")?;
    let recovered = Mnemonic::parse(Language::English, restored.mnemonic)?;
    let recovered_seed = recovered.to_seed("");
    if PrivatePaymentCode::from_seed(recovered_seed.expose(), 0)?
        .public_code()
        .to_base58()
        != code
    {
        return Err("restoration mismatch".into());
    }
    println!(
        "{}",
        serde_json::json!({"network":"orchard","chainId":15000,"zone":"Cyprus1","paymentCode":code,"hdReceiveAddress":stored.hd_receive_address,"hdReceivePath":stored.hd_receive_path,"privateWalletFile":path,"restoreCheck":"passed"})
    );
    Ok(())
}
async fn inspect() -> Result<(), Box<dyn Error>> {
    let content = Zeroizing::new(fs::read_to_string(dir().join("wallets.json"))?);
    if content.len() > 16384 {
        return Err("wallet input too large".into());
    }
    let supplied: Supplied = serde_json::from_str(&content).map_err(|_| "wallet JSON invalid")?;
    if supplied.wallets.len() != 2 {
        return Err("expected two wallet accounts".into());
    }
    let mut addresses = Vec::new();
    for entry in &supplied.wallets {
        let address: QuaiAddress = entry
            .address
            .parse()
            .map_err(|_| "invalid account address")?;
        let derived = QuaiAddress::try_from(load_key(entry.private_key)?.public_key().address())
            .map_err(|_| "private key is not a Quai account")?;
        if derived != address || address.zone() != Zone::Cyprus1 {
            return Err("account ownership or zone mismatch".into());
        }
        addresses.push(address);
    }
    if addresses[0] == addresses[1] {
        return Err("accounts must differ".into());
    }
    println!("Verified two distinct Cyprus-1 Quai private-key/address pairs.");
    let provider = Provider::new(
        HttpTransport::new(HttpConfig::default())?,
        Routing::direct(net().endpoint, Zone::Cyprus1.into())?,
        U256::from(net().chain_id),
    );
    let chain = provider.chain_id(Zone::Cyprus1.into()).await?;
    let genesis = provider.genesis_hash(Zone::Cyprus1).await?;
    if genesis.to_string() != net().genesis {
        return Err("genesis changed; inspect before writes".into());
    }
    let head = provider.block_number(Zone::Cyprus1.into()).await?;
    println!("gas price: {}", provider.gas_price(Zone::Cyprus1).await?);
    for (name, text) in [
        ("WQI", quai_sdk::wrappers::WQI_ADDRESS),
        ("WQUAI", net().wquai),
    ] {
        let address: QuaiAddress = text.parse()?;
        let code = provider.code(address, BlockTag::Latest).await?;
        println!(
            "{}",
            serde_json::json!({"contract":name,"address":text,"codeBytes":code.bytes().len(),"codeSha256":quai_sdk::primitives::Hash32::from_bytes(quai_sdk::crypto::sha256(code.bytes())).to_string()})
        );
    }
    let mut accounts = Vec::new();
    for address in addresses {
        let balance = provider.balance(address, BlockTag::Latest).await?;
        let nonce = provider
            .transaction_count(address, BlockTag::Latest)
            .await?;
        let pending_nonce = provider.transaction_count(address, BlockTag::Pending).await;
        let pending_balance = provider.balance(address, BlockTag::Pending).await;
        println!(
            "{}",
            serde_json::json!({"address":address.to_string(),"pendingNonce":pending_nonce.as_ref().map(ToString::to_string).ok(),"pendingNonceError":pending_nonce.err().map(|e| e.to_string()),"pendingBalance":pending_balance.as_ref().map(ToString::to_string).ok(),"pendingBalanceError":pending_balance.err().map(|e| e.to_string())})
        );
        accounts.push(serde_json::json!({"address":address.to_string(),"balanceBaseUnits":balance.to_string(),"nonce":nonce.to_string()}));
    }
    println!(
        "{}",
        serde_json::json!({"network":net().name,"chainId":chain.to_string(),"genesis":genesis.to_string(),"height":head.to_string(),"accounts":accounts})
    );
    Ok(())
}
#[tokio::main]
async fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let result = match mode.as_str() {
        "generate" => generate("qi-wallet.json"),
        "generate-peer" => generate("qi-peer.json"),
        "generate-redemption" => generate("qi-redemption.json"),
        "qi-extended" if net().mainnet() => {
            Err("qi-extended is not enabled for mainnet qualification".into())
        }
        "qi-extended" => {
            qi_extended::run(
                &std::env::args().nth(2).unwrap_or_default(),
                &std::env::args().nth(3).unwrap_or_default(),
            )
            .await
        }
        "inspect" => inspect().await,
        "mainnet-extra" => mainnet_extra::run(&std::env::args().nth(2).unwrap_or_default()).await,
        "mainnet-check" => mainnet_checks::run(&std::env::args().nth(2).unwrap_or_default()).await,
        "diagnostic" => match net().require_orchard("diagnostic") {
            Ok(()) => diagnostic::run().await,
            Err(error) => Err(error),
        },
        "qi-allocate-conversion" | "qi-prepare" | "qi-broadcast" | "qi-observe" => {
            match net().require_orchard("Qi session stages") {
                Ok(()) => qi::run(&mode).await,
                Err(error) => Err(error),
            }
        }
        "prepare"
        | "broadcast"
        | "observe"
        | "credit"
        | "prepare-replacement"
        | "broadcast-family"
        | "observe-family" => {
            account::run(&mode, &std::env::args().nth(2).unwrap_or_default()).await
        }
        _ => Err("expected generate or inspect".into()),
    };
    if let Err(error) = result {
        eprintln!("{} qualification operation failed: {error}", net().name);
        std::process::exit(1);
    }
}
