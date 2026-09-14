//! Public fixtures only; fixed isolated endpoint and distinct conversion genesis.
use quai_sdk::{
    BlockTag, HttpConfig, HttpTransport, Provider, QiAddress, QuaiAddress, Routing, U256, Zone,
    consensus::{
        ConversionSlippage, Denomination, OutPoint, QiConversionIntent, QiConversionTransaction,
        QiInput, QiOutput, QiTransaction, QuaiToQiTransaction, QuaiTransaction,
    },
    crypto::SecretKey,
};
use std::error::Error;
fn key(value: u64) -> SecretKey {
    let mut bytes = [0; 32];
    bytes[24..].copy_from_slice(&value.to_be_bytes());
    SecretKey::from_bytes(&bytes).unwrap()
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mode = std::env::args()
        .nth(1)
        .ok_or("pass quai-to-qi or qi-to-quai")?;
    let provider = Provider::new(
        HttpTransport::new(HttpConfig::default())?,
        Routing::direct("http://127.0.0.1:19200", Zone::Cyprus1.into())?,
        U256::from(1337),
    );
    if provider.genesis_hash(Zone::Cyprus1).await?.to_string()
        != "0xff38a93744ee5aae738addc88da4f6b171528244e81d34aa4b25579fa3f44ed2"
    {
        return Err("refusing unknown conversion genesis".into());
    }
    let account = key(805);
    let sender = QuaiAddress::try_from(account.public_key().address())?;
    if mode == "qi-spend-converted" {
        let qi = key(300);
        let mut points = provider
            .outpoints(QiAddress::try_from(qi.public_key().address())?)
            .await?;
        points.sort_by_key(|point| point.outpoint);
        if points.is_empty() || points.len() > 64 {
            return Err("expected 1..=64 conversion outpoints".into());
        }
        let total: u64 = points
            .iter()
            .map(|p| Denomination::new(p.denomination).unwrap().value())
            .sum();
        if total < 4 {
            return Err("insufficient converted fixture value".into());
        }
        let select = |value: u64| {
            (0..15)
                .rev()
                .map(|index| Denomination::new(index).unwrap())
                .find(|d| d.value() <= value)
                .unwrap()
        };
        let recipient = select(total / 2);
        let change = select((total - recipient.value()) / 2);
        let inputs = points
            .iter()
            .map(|p| QiInput {
                previous_output: OutPoint {
                    transaction_hash: p.outpoint.tx_hash,
                    index: p.outpoint.index,
                },
                public_key: qi.public_key(),
            })
            .collect();
        let tx = QiTransaction {
            chain_id: U256::from(1337),
            inputs,
            outputs: vec![
                QiOutput {
                    address: key(1332).public_key().address(),
                    denomination: recipient,
                },
                QiOutput {
                    address: key(2209).public_key().address(),
                    denomination: change,
                },
            ],
            data: vec![],
        };
        let signed = tx.sign_local(&vec![&qi; points.len()])?;
        println!(
            "converted-spend-hash={} locks={:?}",
            signed.hash()?,
            points.iter().map(|p| p.lock).collect::<Vec<_>>()
        );
        println!("converted-spend={:?}", provider.broadcast_qi(&signed).await);
        return Ok(());
    }
    if mode == "quai-to-qi" || mode == "quai-to-qi-large" {
        let tx = QuaiTransaction {
            chain_id: U256::from(1337),
            nonce: provider.transaction_count(sender, BlockTag::Latest).await?,
            to: Some(key(300).public_key().address()),
            value: U256::from(if mode == "quai-to-qi-large" {
                10_000_000_000_000_000_000_000u128
            } else {
                10_000_000_000_000_000_000u128
            }),
            gas_limit: 1_000_000,
            gas_price: provider.gas_price(Zone::Cyprus1).await?,
            data: vec![0x23, 0x28],
            access_list: vec![],
        };
        let tx = QuaiToQiTransaction::new(tx)?;
        println!(
            "quai-to-qi={:?}",
            provider.broadcast(&tx.sign(&account)?).await?
        );
    } else if matches!(
        mode.as_str(),
        "qi-to-quai" | "qi-to-quai-strict" | "qi-to-quai-strict-whole" | "qi-to-quai-strict-large"
    ) {
        let qi = key(130);
        let tx = QiConversionTransaction::new(
            U256::from(1337),
            vec![QiInput {
                previous_output: OutPoint {
                    transaction_hash:
                        "0x0080008011111111111111111111111111111111111111111111111111111111"
                            .parse()?,
                    index: 0,
                },
                public_key: qi.public_key(),
            }],
            vec![Denomination::new(if mode == "qi-to-quai-strict-large" {
                7
            } else if mode == "qi-to-quai-strict-whole" {
                6
            } else {
                4
            })?],
            vec![],
            QiConversionIntent {
                destination: sender,
                refund: QiAddress::try_from(key(2285).public_key().address())?,
                slippage: ConversionSlippage::new(if mode.starts_with("qi-to-quai-strict") {
                    30
                } else {
                    9000
                })?,
            },
        )?;
        println!(
            "qi-to-quai={:?}",
            provider
                .broadcast_qi_conversion(&tx.sign_single(&qi)?)
                .await?
        );
    } else {
        return Err("unknown conversion mode".into());
    }
    Ok(())
}
