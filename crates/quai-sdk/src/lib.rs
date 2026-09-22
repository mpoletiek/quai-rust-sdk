//! Modular Quai/Qi alpha SDK with native and browser wallet workflows and recovery.
/// Rust package version; independent of the pinned quais.js reference version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Recover the address signing an exact validated typed-data document. This does
/// not approve its domain or authorize the recovered account; compare the result
/// and apply the application's expected chain/domain policy independently.
#[cfg(all(feature = "wallet", feature = "abi"))]
pub fn recover_typed_data_signer(
    document: &quai_abi::TypedData,
    signature: &quai_crypto::RecoverableSignature,
) -> Result<quai_primitives::Address, quai_crypto::CryptoError> {
    signature
        .recover_prehash(document.signing_hash().bytes())
        .map(quai_crypto::PublicKey::address)
}
#[cfg(feature = "wallet")]
pub mod account_preflight;
/// Portable fee-only account replacement quotation.
#[cfg(feature = "wallet")]
pub mod account_replacement;
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
pub mod accounts;
#[cfg(all(target_arch = "wasm32", feature = "backup", feature = "browser"))]
pub mod browser_accounts;
#[cfg(all(target_arch = "wasm32", feature = "wallet", feature = "browser"))]
pub mod browser_addresses;
#[cfg(all(target_arch = "wasm32", feature = "backup", feature = "browser"))]
pub mod browser_backups;
#[cfg(all(
    target_arch = "wasm32",
    feature = "wallet",
    feature = "browser",
    feature = "payments"
))]
pub mod browser_payments;
#[cfg(all(target_arch = "wasm32", feature = "backup", feature = "browser"))]
pub mod browser_qi;
/// Browser Qi selection, exact preparation and durable signing.
#[cfg(all(target_arch = "wasm32", feature = "backup", feature = "browser"))]
pub mod browser_qi_transactions;
/// Persisted browser candidate submission and canonical reconciliation.
#[cfg(all(target_arch = "wasm32", feature = "backup", feature = "browser"))]
pub mod browser_recovery;
#[cfg(all(target_arch = "wasm32", feature = "backup", feature = "browser"))]
pub mod browser_transactions;
/// Portable canonical observations for signed candidate families.
#[cfg(feature = "wallet")]
pub mod candidate_observation;
#[cfg(feature = "abi")]
pub mod contracts;
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
pub mod deployments;
#[cfg(feature = "wallet")]
pub mod discovery;
#[cfg(feature = "wallet")]
mod network;
#[cfg(all(feature = "sqlite", feature = "payments", not(target_arch = "wasm32")))]
pub mod payment_channels;
/// Pelagus-compatible payment-channel mailbox announcements.
#[cfg(all(feature = "abi", feature = "payments"))]
pub mod payment_mailbox;
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
pub mod qi;
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
pub mod qi_discovery;
/// Portable exact-denomination Qi preparation.
#[cfg(feature = "wallet")]
pub mod qi_preflight;
/// Portable same-input Qi replacement quotation.
#[cfg(feature = "wallet")]
pub mod qi_replacement;
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
pub mod recovery;
/// Portable verified remote account signing and submission acknowledgements.
#[cfg(feature = "wallet")]
pub mod rpc_signer;
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
pub mod settlement;
/// Portable destination observations from exact signed payloads.
#[cfg(feature = "wallet")]
pub mod settlement_observation;
#[cfg(feature = "abi")]
pub mod wrappers;
#[cfg(feature = "abi")]
pub use quai_abi as abi;
#[cfg(feature = "browser")]
pub use quai_browser as browser;
#[cfg(any(feature = "wallet", feature = "abi"))]
pub use quai_consensus as consensus;
#[cfg(feature = "wallet")]
pub use quai_crypto as crypto;
#[cfg(feature = "keystore")]
pub use quai_keystore as keystore;
#[cfg(feature = "payments")]
pub use quai_payments as payments;
pub use quai_primitives as primitives;
pub use quai_primitives::{
    Address, ErrorClass, Ledger, QiAddress, QuaiAddress, Region, Shard, Zone,
};
pub use quai_provider as provider;
pub use quai_provider::{BlockTag, Provider, ProviderError};
pub use quai_rpc as rpc;
#[cfg(not(target_arch = "wasm32"))]
pub use quai_rpc::DynTransport;
pub use quai_rpc::{Endpoint, Routing, U256, parse_quantity, parse_use_pathing};
#[cfg(all(feature = "http", not(target_arch = "wasm32")))]
pub use quai_rpc::{HttpConfig, HttpProxy, HttpTransport};
#[cfg(all(feature = "ws", not(target_arch = "wasm32")))]
pub use quai_rpc::{WsConfig, WsTransport};
#[cfg(feature = "wallet")]
pub use quai_signer as signer;
#[cfg(feature = "wallet")]
pub use quai_wallet as wallet;
