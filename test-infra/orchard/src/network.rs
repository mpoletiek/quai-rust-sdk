//! Explicit qualification network selection. Orchard remains the default; mainnet
//! requires `QUAI_QUALIFICATION_NETWORK=mainnet` and a persistent private directory.
use quai_sdk::U256;
use quai_sdk::accounts::FeePolicy;
use std::{
    error::Error,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::OnceLock,
};

pub struct Network {
    pub name: &'static str,
    pub dir: PathBuf,
    pub endpoint: &'static str,
    pub chain_id: u64,
    pub genesis: &'static str,
    pub wquai: &'static str,
    /// Account transfer value from A to B. Mainnet funds B for its own later fees.
    pub transfer_a_b_its: U256,
    /// Quai-to-Qi value. Mainnet uses a small amount below the steep controller discount.
    pub conversion_its: U256,
    /// Conversion slippage bound. Mainnet discounts the whole prime-block batch.
    /// On 2026-09-14 a recurring ~270 QUAI batch cost ~5% at 200 QUAI; a bound
    /// of 800 accepts that but refunds rarer ~18% batches for only the origin fee.
    pub conversion_slippage_bps: u16,
    pub account_fee: FeePolicy,
}

impl Network {
    pub fn mainnet(&self) -> bool {
        self.name == "mainnet"
    }

    /// Stages whose amounts, stores or fee profiles were only reviewed on Orchard.
    pub fn require_orchard(&self, stage: &str) -> Result<(), Box<dyn Error>> {
        if self.mainnet() {
            return Err(format!("{stage} is not enabled for mainnet qualification").into());
        }
        Ok(())
    }
}

const QUAI: u128 = 1_000_000_000_000_000_000;

fn select() -> Result<Network, Box<dyn Error>> {
    match std::env::var("QUAI_QUALIFICATION_NETWORK").as_deref() {
        Err(std::env::VarError::NotPresent) | Ok("orchard") => Ok(Network {
            name: "orchard",
            dir: PathBuf::from("/tmp/quai-sdk-orchard-secrets"),
            endpoint: "https://orchard.rpc.quai.network/cyprus1",
            chain_id: 15000,
            genesis: "0x663a73416275109a01aad3a4c29ea9e310aded63c5eea491243b7312ad8cd16b",
            wquai: quai_sdk::wrappers::WQUAI_ORCHARD_ADDRESS,
            transfer_a_b_its: U256::from(QUAI / 100),
            conversion_its: U256::from(quai_sdk::consensus::MIN_QUAI_CONVERSION_VALUE),
            conversion_slippage_bps: 100,
            account_fee: FeePolicy {
                max_gas: 500_000,
                max_gas_price: U256::from(100_000_000_000u64),
                max_total_fee: U256::from(QUAI / 100),
                gas_margin_bps: 1000,
            },
        }),
        Ok("mainnet") => {
            // Real funds: never keep custody in a directory that may be cleaned.
            let home = std::env::var_os("HOME").ok_or("HOME is required for mainnet")?;
            let dir = Path::new(&home).join(".local/share/quai-sdk-mainnet-qualification");
            let mode = std::fs::metadata(&dir)
                .map_err(|_| "create the private mainnet qualification directory first")?
                .permissions()
                .mode();
            if mode & 0o077 != 0 {
                return Err("mainnet qualification directory must be mode 0700".into());
            }
            Ok(Network {
                name: "mainnet",
                dir,
                endpoint: "https://rpc.quai.network/cyprus1",
                chain_id: 9,
                genesis: "0xac81c28f1a72591b87b5f16c9793cdc0e87c45c6d426d1a364c3b8f6386b5b8b",
                wquai: quai_sdk::wrappers::WQUAI_MAINNET_ADDRESS,
                transfer_a_b_its: U256::from(10 * QUAI),
                conversion_its: U256::from(200 * QUAI),
                conversion_slippage_bps: 800,
                account_fee: FeePolicy {
                    max_gas: 500_000,
                    // Observed mainnet prices were about 41,000 gwei on 2026-09-14.
                    max_gas_price: U256::from(100_000_000_000_000u64),
                    max_total_fee: U256::from(25 * QUAI),
                    gas_margin_bps: 1000,
                },
            })
        }
        _ => Err("QUAI_QUALIFICATION_NETWORK must be orchard or mainnet".into()),
    }
}

/// The selected network; exits on invalid configuration before any key is read.
pub fn net() -> &'static Network {
    static NETWORK: OnceLock<Network> = OnceLock::new();
    NETWORK.get_or_init(|| {
        select().unwrap_or_else(|error| {
            eprintln!("qualification network configuration failed: {error}");
            std::process::exit(1)
        })
    })
}

pub fn dir() -> &'static Path {
    &net().dir
}
