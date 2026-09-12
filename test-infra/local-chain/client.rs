//! Public-fixture acceptance client; refuses any endpoint other than the isolated loopback profile.
use quai_sdk::{
    BlockTag, HttpConfig, HttpTransport, Provider, QiAddress, QuaiAddress, Routing, U256, Zone,
    consensus::{
        AccessTuple, Denomination, OutPoint, QiInput, QiOutput, QiTransaction, QuaiTransaction,
    },
    crypto::SecretKey,
    primitives::{Hash32, contract_address},
};
use std::error::Error;
const GENESIS: &str = "0x654e7a894d57de62ec19b9c161cb1c647466278e0565d3e1ba5d806ae6af0aee";
fn key(n: u64) -> SecretKey {
    let mut bytes = [0; 32];
    bytes[24..].copy_from_slice(&n.to_be_bytes());
    SecretKey::from_bytes(&bytes).unwrap()
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mode = std::env::args()
        .nth(1)
        .ok_or("pass inventory, transfer, deploy, qi or receipt HASH")?;
    let provider = Provider::new(
        HttpTransport::new(HttpConfig::default())?,
        Routing::direct("http://127.0.0.1:19200", Zone::Cyprus1.into())?,
        U256::from(1337),
    );
    if provider.genesis_hash(Zone::Cyprus1).await?.to_string() != GENESIS {
        return Err("refusing unknown genesis".into());
    }
    let secret = key(805);
    let address = QuaiAddress::try_from(secret.public_key().address())?;
    if mode == "inventory" {
        println!(
            "balance={} nonce={} height={} outpoints={:?}",
            provider.balance(address, BlockTag::Latest).await?,
            provider
                .transaction_count(address, BlockTag::Latest)
                .await?,
            provider.block_number(Zone::Cyprus1.into()).await?,
            provider
                .outpoints(QiAddress::try_from(key(130).public_key().address())?)
                .await?
        );
        return Ok(());
    }
    if mode == "receipt" {
        let hash: Hash32 = std::env::args().nth(2).ok_or("hash required")?.parse()?;
        println!("{:?}", provider.receipt(Zone::Cyprus1, hash).await?);
        return Ok(());
    }
    if mode.starts_with("qi-") {
        // Already-public scalar fixtures; two independent funding transactions create
        // repeated ownership without violating the node's per-transaction output rule.
        let keys: Vec<_> = (131..100000)
            .map(|n| (n, key(n)))
            .filter(|(_, k)| {
                QiAddress::try_from(k.public_key().address())
                    .is_ok_and(|a| a.zone() == Zone::Cyprus1)
            })
            .take(6)
            .collect();
        let addresses: Vec<_> = keys
            .iter()
            .map(|(_, k)| QiAddress::try_from(k.public_key().address()).unwrap())
            .collect();
        for (i, (scalar, _)) in keys.iter().enumerate() {
            println!(
                "fixture={i} scalar={scalar} address={} outpoints={:?}",
                addresses[i],
                provider.outpoints(addresses[i]).await?
            );
        }
        if mode == "qi-inventory" {
            return Ok(());
        }
        if mode == "qi-split" {
            let bootstrap = key(130);
            let tx = QiTransaction {
                chain_id: U256::from(1337),
                inputs: vec![QiInput {
                    previous_output: OutPoint {
                        transaction_hash:
                            "0x0080008011111111111111111111111111111111111111111111111111111111"
                                .parse()?,
                        index: 0,
                    },
                    public_key: bootstrap.public_key(),
                }],
                outputs: addresses[..3]
                    .iter()
                    .map(|a| QiOutput {
                        address: a.address(),
                        denomination: Denomination::new(13).unwrap(),
                    })
                    .collect(),
                data: vec![],
            };
            println!(
                "split={:?}",
                provider.broadcast_qi(&tx.sign_single(&bootstrap)?).await?
            );
        } else if mode == "qi-duplicate-fund" {
            for index in [0, 1] {
                let points = provider.outpoints(addresses[index]).await?;
                if points.len() != 1 || points[0].denomination != 13 {
                    return Err("expected one denomination-13 funding outpoint".into());
                }
                let tx = QiTransaction {
                    chain_id: U256::from(1337),
                    inputs: vec![QiInput {
                        previous_output: OutPoint {
                            transaction_hash: points[0].outpoint.tx_hash,
                            index: points[0].outpoint.index,
                        },
                        public_key: keys[index].1.public_key(),
                    }],
                    outputs: vec![QiOutput {
                        address: addresses[3].address(),
                        denomination: Denomination::new(12)?,
                    }],
                    data: vec![],
                };
                println!(
                    "fund-duplicate-{index}={:?}",
                    provider
                        .broadcast_qi(&tx.sign_single(&keys[index].1)?)
                        .await?
                );
            }
        } else if mode == "qi-multi" {
            let mut inputs = vec![];
            for (index, count, denomination) in [(3, 2, 12), (2, 1, 13)] {
                let mut points = provider.outpoints(addresses[index]).await?;
                points.sort_by_key(|point| point.outpoint);
                if points.len() != count
                    || points
                        .iter()
                        .any(|p| p.denomination != denomination || p.lock != U256::ZERO)
                {
                    return Err("unexpected aggregate funding inventory".into());
                }
                for point in points {
                    inputs.push(QiInput {
                        previous_output: OutPoint {
                            transaction_hash: point.outpoint.tx_hash,
                            index: point.outpoint.index,
                        },
                        public_key: keys[index].1.public_key(),
                    });
                }
            }
            let tx = QiTransaction {
                chain_id: U256::from(1337),
                inputs,
                outputs: vec![
                    QiOutput {
                        address: addresses[4].address(),
                        denomination: Denomination::new(13)?,
                    },
                    QiOutput {
                        address: addresses[5].address(),
                        denomination: Denomination::new(12)?,
                    },
                ],
                data: vec![],
            };
            println!(
                "multi-fee-estimate={} ordered-inputs={:?} outputs={:?}",
                provider.estimate_qi_fee(&tx).await?,
                tx.inputs,
                tx.outputs
            );
            let signed = tx.sign_local(&[&keys[3].1, &keys[3].1, &keys[2].1])?;
            println!("multi={:?}", provider.broadcast_qi(&signed).await?);
        } else {
            return Err("unknown Qi mode".into());
        }
        return Ok(());
    }
    if mode == "qi" {
        let secret = key(130);
        let receiver = (131..100000)
            .map(key)
            .map(|key| key.public_key().address())
            .find(|address| {
                QiAddress::try_from(*address).is_ok_and(|address| address.zone() == Zone::Cyprus1)
            })
            .ok_or("no Qi fixture receiver")?;
        let tx = QiTransaction {
            chain_id: U256::from(1337),
            inputs: vec![QiInput {
                previous_output: OutPoint {
                    transaction_hash:
                        "0x0080008011111111111111111111111111111111111111111111111111111111"
                            .parse()?,
                    index: 0,
                },
                public_key: secret.public_key(),
            }],
            outputs: vec![QiOutput {
                address: receiver,
                denomination: Denomination::new(13)?,
            }],
            data: vec![],
        };
        println!(
            "fee-estimate={} recipient={}",
            provider.estimate_qi_fee(&tx).await?,
            receiver
        );
        let signed = tx.sign_single(&secret)?;
        let result = provider.broadcast_qi(&signed).await?;
        println!("{result:?}");
        return Ok(());
    }
    let nonce = provider
        .transaction_count(address, BlockTag::Latest)
        .await?;
    let mut tx = QuaiTransaction {
        chain_id: U256::from(1337),
        nonce,
        to: Some("0x0011223344556677889900112233445566778899".parse()?),
        value: U256::from(1_000_000),
        gas_limit: 100000,
        gas_price: provider.gas_price(Zone::Cyprus1).await?,
        data: vec![],
        access_list: vec![],
    };
    if mode == "replacement" {
        tx.value = U256::from(111);
        let first = tx.sign(&secret)?;
        println!(
            "original={:?} nonce={} gas-price={}",
            provider.broadcast(&first).await?,
            tx.nonce,
            tx.gas_price
        );
        tx.value = U256::from(222);
        tx.gas_price = tx
            .gas_price
            .checked_mul(U256::from(2))
            .ok_or("gas price overflow")?;
        println!(
            "replacement={:?} nonce={} gas-price={}",
            provider.broadcast(&tx.sign(&secret)?).await?,
            tx.nonce,
            tx.gas_price
        );
        return Ok(());
    }
    if mode == "deploy" {
        tx.to = None;
        tx.value = U256::ZERO;
        tx.gas_limit = 1000000;
        let init = hex("600a600c600039600a6000f3602a60005260206000f3");
        let mut found = false;
        for counter in 0u32..100000 {
            let mut data = init.clone();
            data.extend_from_slice(&counter.to_be_bytes());
            let contract = contract_address(address.address(), nonce, &data);
            if QuaiAddress::try_from(contract).is_ok_and(|a| a.zone() == Zone::Cyprus1) {
                tx.data = data;
                tx.access_list = vec![AccessTuple {
                    address: contract,
                    storage_keys: vec![],
                }];
                println!("contract={contract} grind={counter}");
                found = true;
                break;
            }
        }
        if !found {
            return Err("contract grinding limit".into());
        }
    } else if mode != "transfer" {
        return Err("unknown mode".into());
    }
    let signed = tx.sign(&secret)?;
    let result = provider.broadcast(&signed).await?;
    println!("{result:?}");
    Ok(())
}
fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}
