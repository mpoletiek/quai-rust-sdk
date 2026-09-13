//! Bounded import of one Solidity artifact or an explicitly selected compiler output.
use crate::{AbiError, AbiInterface, MAX_DATA_BYTES, TypedDataError, document::parse_json};
use serde_json::Value;
use std::fmt;
/// Validated contract ABI and exact nonempty creation bytecode. Compiler metadata
/// is not trusted as evidence that ABI and bytecode correspond to one another.
#[derive(Clone)]
pub struct SolidityArtifact {
    interface: AbiInterface,
    init_code: Vec<u8>,
}
impl SolidityArtifact {
    /// Parse one compiler contract entry: `abi` plus `bytecode` or `evm.bytecode`.
    /// Bytecode can be a hex string or an object containing `object`. A missing
    /// lowercase 0x prefix is accepted; unresolved linker placeholders are rejected.
    /// Duplicate keys are rejected throughout the bounded document. If both code
    /// locations exist, their decoded bytes must agree, avoiding ambiguous selection.
    pub fn from_json(bytes: &[u8]) -> Result<Self, AbiError> {
        Self::from_value(parse(bytes)?)
    }
    /// Select one exact source/contract entry in a full Solidity compiler output.
    /// No file lookup, compiler execution or heuristic selection takes place.
    pub fn from_compilation_json(
        bytes: &[u8],
        source: &str,
        contract: &str,
    ) -> Result<Self, AbiError> {
        if source.is_empty() || contract.is_empty() || source.len() > 4096 || contract.len() > 4096
        {
            return Err(AbiError::Schema);
        }
        let mut value = parse(bytes)?;
        let entry = value
            .get_mut("contracts")
            .and_then(|v| v.get_mut(source))
            .and_then(|v| v.get_mut(contract))
            .ok_or(AbiError::NotFound)?
            .take();
        Self::from_value(entry)
    }
    fn from_value(value: Value) -> Result<Self, AbiError> {
        let Value::Object(mut object) = value else {
            return Err(AbiError::Schema);
        };
        let abi = object.remove("abi").ok_or(AbiError::Schema)?;
        let interface =
            AbiInterface::from_json(&serde_json::to_vec(&abi).map_err(|_| AbiError::Schema)?)?;
        let direct = object.get("bytecode").map(decode_code).transpose()?;
        let nested = object
            .get("evm")
            .and_then(|v| v.get("bytecode"))
            .map(decode_code)
            .transpose()?;
        let init_code = match (direct, nested) {
            (Some(a), Some(b)) if a != b => return Err(AbiError::Schema),
            (Some(a), _) | (_, Some(a)) => a,
            _ => return Err(AbiError::Schema),
        };
        if init_code.is_empty() {
            return Err(AbiError::Value);
        }
        Ok(Self {
            interface,
            init_code,
        })
    }
    /// Borrow the validated contract interface.
    pub fn interface(&self) -> &AbiInterface {
        &self.interface
    }
    /// Borrow exact creation bytecode, retaining leading zeros and compiler metadata.
    pub fn init_code(&self) -> &[u8] {
        &self.init_code
    }
    /// Encode and append exact constructor arguments, before Quai address grinding.
    /// Native value/payability, nonce, fee and network checks belong to deployment preparation.
    pub fn init_data(&self, arguments: &[Value]) -> Result<Vec<u8>, AbiError> {
        let encoded = if let Some(constructor) = self.interface.constructor() {
            constructor.encode_arguments(arguments)?
        } else if arguments.is_empty() {
            vec![]
        } else {
            return Err(AbiError::Value);
        };
        let size = self
            .init_code
            .len()
            .checked_add(encoded.len())
            .filter(|n| *n <= MAX_DATA_BYTES)
            .ok_or(AbiError::Limit)?;
        let mut output = Vec::with_capacity(size);
        output.extend_from_slice(&self.init_code);
        output.extend_from_slice(&encoded);
        Ok(output)
    }
    /// Extract the owned interface and exact creation bytes for a contract factory workflow.
    pub fn into_parts(self) -> (AbiInterface, Vec<u8>) {
        (self.interface, self.init_code)
    }
}
impl fmt::Debug for SolidityArtifact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SolidityArtifact")
            .field("creation_bytes", &self.init_code.len())
            .finish()
    }
}
fn parse(bytes: &[u8]) -> Result<Value, AbiError> {
    parse_json(bytes).map_err(|e| {
        if e == TypedDataError::Limit {
            AbiError::Limit
        } else {
            AbiError::Schema
        }
    })
}
fn decode_code(value: &Value) -> Result<Vec<u8>, AbiError> {
    let text = value
        .as_str()
        .or_else(|| value.get("object").and_then(Value::as_str))
        .ok_or(AbiError::Schema)?;
    if text.len() > MAX_DATA_BYTES * 2 + 2 {
        return Err(AbiError::Limit);
    }
    let owned;
    let text = if text.starts_with("0x") {
        text
    } else {
        owned = format!("0x{text}");
        &owned
    };
    quai_primitives::get_bytes(text).map_err(|_| AbiError::Value)
}
