//! Bounded Solidity ABI encoding, contract interfaces and EIP-712 hashing.
//!
//! Validated against pinned quais.js with explicit stricter input policies.
//! Signing, contract execution and provider network policy belong to other crates.
mod abi_type;
mod typed_value;
pub use typed_value::AbiValue;
mod codec;
mod document;
mod interface;
mod schema;
mod value;

pub use document::TypedData;
pub use schema::{TypedDataEncoder, TypedDataField, TypedDataTypes};
pub use value::{hash_domain, hash_typed_data};

/// Maximum UTF-8 JSON document or combined value string/key bytes.
pub const MAX_DATA_BYTES: usize = 1 << 20;
/// Maximum visited JSON values, including array elements and object values.
pub const MAX_VALUE_NODES: usize = 32_768;
/// Maximum nested schema dependencies, JSON containers or encoding recursion.
pub const MAX_DEPTH: usize = 64;
/// Maximum declared struct types in one schema.
pub const MAX_TYPES: usize = 128;
/// Maximum total fields in one schema.
pub const MAX_FIELDS: usize = 1024;
/// Maximum combined schema identifier and type-expression bytes.
pub const MAX_SCHEMA_BYTES: usize = 65_536;

/// Errors never include a message's potentially sensitive value contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TypedDataError {
    /// Input exceeds a fixed resource policy.
    #[error("typed data exceeds resource limits")]
    Limit,
    /// Malformed JSON, duplicate keys or unsupported document shape.
    #[error("invalid typed-data document")]
    Document,
    /// Invalid identifier, primitive, array expression or duplicate field.
    #[error("invalid typed-data schema")]
    Schema,
    /// A field references an undeclared struct.
    #[error("unknown typed-data type")]
    UnknownType,
    /// More than one root, no root, or an unused disconnected struct.
    #[error("typed-data schema requires one primary type")]
    PrimaryType,
    /// Recursive structs are not supported by pinned quais.js.
    #[error("cyclic typed-data schema")]
    Cycle,
    /// Missing/extra fields or a value with an incorrect JSON kind.
    #[error("typed-data value does not match schema")]
    Value,
    /// Integer outside its declared range or not an exact integer.
    #[error("invalid typed-data integer")]
    Integer,
    /// Invalid hex, byte width, address or address checksum.
    #[error("invalid typed-data bytes or address")]
    Bytes,
    /// Fixed array length differs from its declaration.
    #[error("typed-data array length mismatch")]
    ArrayLength,
    /// An unrecognized domain field or inconsistent domain declaration.
    #[error("invalid typed-data domain")]
    Domain,
}

pub(crate) fn keccak(bytes: &[u8]) -> [u8; 32] {
    use tiny_keccak::{Hasher, Keccak};
    let mut hasher = Keccak::v256();
    hasher.update(bytes);
    let mut result = [0; 32];
    hasher.finalize(&mut result);
    result
}

pub use abi_type::{AbiError, AbiType};
pub use codec::{AbiCoder, indexed_event_topic};
pub use interface::{
    AbiConstructor, AbiCustomError, AbiEvent, AbiEventValue, AbiFunction, AbiInterface,
    StateMutability, function_selector, signature_hash,
};
