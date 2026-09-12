//! Public deterministic payment identities for an offline example. Never fund.
use quai_sdk::Zone;
use quai_sdk::payments::{PaymentDirection, PaymentSearch, PrivatePaymentCode};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let alice = PrivatePaymentCode::from_seed(&[1; 32], 0)?;
    let bob = PrivatePaymentCode::from_seed(&[2; 32], 0)?;
    let destination = alice.search(
        bob.public_code(),
        PaymentDirection::Send,
        PaymentSearch {
            zone: Zone::Cyprus1,
            start_index: 0,
            max_attempts: 10_000,
        },
        || false,
    )?;
    let received = bob.receive_public_key(alice.public_code(), destination.index)?;
    assert_eq!(received, destination.public_key);
    println!("PUBLIC TEST IDENTITIES — never fund");
    println!("Bob payment code: {}", bob.public_code().to_base58());
    println!(
        "Matching Qi child {}: {}",
        destination.index, destination.address
    );
    Ok(())
}
