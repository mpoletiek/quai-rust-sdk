use crate::{Address, Hash32};
use tiny_keccak::{Hasher, Keccak};

fn address_hash(parts: &[&[u8]]) -> Address {
    let mut hasher = Keccak::v256();
    for part in parts {
        hasher.update(part);
    }
    let mut digest = [0u8; 32];
    hasher.finalize(&mut digest);
    let mut address = [0u8; 20];
    address.copy_from_slice(&digest[12..]);
    Address::from_bytes(address)
}

/// Predict the Quai CREATE address from exact init code and a 64-bit nonce.
///
/// Matches pinned go-quai `crypto.CreateAddress`: Keccak(sender || nonce as
/// eight big-endian bytes || init_code), taking the final 20 bytes. A predicted
/// address can select the wrong ledger/zone; deployment must grind and validate
/// the result separately. This pure function does not claim deployment success.
///
/// Deliberate correction to quais.js `getContractAddress`: leading zero code
/// bytes are preserved, as the node hashes them. Stripping them can predict an
/// incorrect destination for the exact transaction being authorized.
pub fn contract_address(sender: Address, nonce: u64, init_code: &[u8]) -> Address {
    address_hash(&[sender.bytes(), &nonce.to_be_bytes(), init_code])
}

/// Predict CREATE2 from a 32-byte salt and the hash of exact init code.
///
/// Matches go-quai: Keccak(0xff || sender || salt || init_code_hash)[12..].
/// The caller must separately validate the resulting ledger/zone and deployment.
pub fn create2_address(sender: Address, salt: Hash32, init_code_hash: Hash32) -> Address {
    address_hash(&[
        &[0xff],
        sender.bytes(),
        salt.bytes(),
        init_code_hash.bytes(),
    ])
}
