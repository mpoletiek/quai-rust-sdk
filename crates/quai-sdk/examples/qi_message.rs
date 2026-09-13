//! Offline Qi message round trip using a PUBLIC TOY KEY. Never fund this address.
use quai_sdk::U256;
use quai_sdk::crypto::SecretKey;
use quai_sdk::primitives::hexlify;
use quai_sdk::signer::{LocalSigner, Signer, verify_qi_message};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut scalar = [0; 32];
    scalar[31] = 130; // Public fixture, not an application wallet.
    let signer = LocalSigner::new(SecretKey::from_bytes(&scalar)?, U256::from(15000))?;
    let message = "Qi message: café 🐬".as_bytes();
    let signature = signer.sign_qi_message(message)?;
    verify_qi_message(
        signer.address().try_into()?,
        &signer.public_key(),
        message,
        &signature,
    )?;
    println!(
        "{}",
        serde_json::json!({
            "publicFixture": true,
            "address": signer.address().to_string(),
            "publicKey": hexlify(&signer.public_key().to_compressed())?,
            "message": hexlify(message)?,
            "signature": hexlify(&signature.to_bytes())?,
        })
    );
    Ok(())
}
