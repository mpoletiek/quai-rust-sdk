//! Pinned quais.js packed byte semantics with strict type/range/resource checks.
use crate::{AbiError, AbiType, MAX_DATA_BYTES, abi_type::Kind};
use serde_json::Value;

/// Concatenate packed values using the pinned quais.js `solidityPacked` layout.
/// Scalars use their declared widths, primitive array elements use 32-byte words,
/// and strings/dynamic bytes contribute raw bytes without lengths. Tuples fail.
///
/// As in the reference helper, nested arrays are flattened and string/bytes array
/// elements are NOT padded. These are reference extensions, not Solidity compiler
/// `abi.encodePacked` qualification. Values retain strict ABI checks, including
/// declared integer widths inside arrays and real booleans (no JS coercion).
///
/// Shared ABI field/node/text/encoded-size limits apply before output allocation.
/// Packed bytes are ambiguous for multiple dynamic fields: they cannot be decoded
/// uniquely or substituted for canonical ABI/EIP-712 signing encodings.
pub fn solidity_packed(types: &[AbiType], values: &[Value]) -> Result<Vec<u8>, AbiError> {
    crate::codec::validate(types, values)?;
    for ty in types {
        supported(ty)?;
    }
    let size = types
        .iter()
        .zip(values)
        .try_fold(0usize, |sum, (ty, value)| {
            sum.checked_add(measure(ty, value, false)?)
                .filter(|n| *n <= MAX_DATA_BYTES)
                .ok_or(AbiError::Limit)
        })?;
    let mut output = Vec::with_capacity(size);
    for (ty, value) in types.iter().zip(values) {
        write(ty, value, false, &mut output)?;
    }
    if output.len() != size {
        return Err(AbiError::Encoding);
    }
    Ok(output)
}
/// Keccak-256 of the exact packed bytes. Dynamic-field ambiguity still applies.
pub fn solidity_packed_keccak256(
    types: &[AbiType],
    values: &[Value],
) -> Result<[u8; 32], AbiError> {
    Ok(crate::keccak(&solidity_packed(types, values)?))
}
/// SHA-256 of the exact packed bytes. Dynamic-field ambiguity still applies.
pub fn solidity_packed_sha256(types: &[AbiType], values: &[Value]) -> Result<[u8; 32], AbiError> {
    use sha2::{Digest, Sha256};
    Ok(Sha256::digest(solidity_packed(types, values)?).into())
}
fn supported(ty: &AbiType) -> Result<(), AbiError> {
    match &ty.kind {
        Kind::Tuple(_) => Err(AbiError::Schema),
        Kind::Array(child, _) => supported(child),
        _ => Ok(()),
    }
}
fn measure(ty: &AbiType, value: &Value, array: bool) -> Result<usize, AbiError> {
    Ok(match &ty.kind {
        Kind::Array(child, _) => {
            value
                .as_array()
                .ok_or(AbiError::Value)?
                .iter()
                .try_fold(0usize, |sum, value| {
                    sum.checked_add(measure(child, value, true)?)
                        .filter(|n| *n <= MAX_DATA_BYTES)
                        .ok_or(AbiError::Limit)
                })?
        }
        Kind::String => value.as_str().ok_or(AbiError::Value)?.len(),
        Kind::Bytes => {
            value
                .as_str()
                .ok_or(AbiError::Value)?
                .len()
                .saturating_sub(2)
                / 2
        }
        Kind::Tuple(_) => return Err(AbiError::Schema),
        _ if array => 32,
        Kind::Bool => 1,
        Kind::Address => 20,
        Kind::Int(_, bits) => bits / 8,
        Kind::FixedBytes(size) => *size,
    })
}
fn write(ty: &AbiType, value: &Value, array: bool, out: &mut Vec<u8>) -> Result<(), AbiError> {
    match &ty.kind {
        Kind::Array(child, _) => {
            for element in value.as_array().ok_or(AbiError::Value)? {
                write(child, element, true, out)?;
            }
        }
        Kind::String => out.extend_from_slice(value.as_str().ok_or(AbiError::Value)?.as_bytes()),
        Kind::Bytes => {
            out.extend_from_slice(&crate::value::decode_hex(value).map_err(|_| AbiError::Value)?)
        }
        Kind::Tuple(_) => return Err(AbiError::Schema),
        _ => {
            let word = crate::codec::primitive_word(ty, value)?;
            if array {
                out.extend_from_slice(&word);
            } else if let Kind::FixedBytes(size) = ty.kind {
                out.extend_from_slice(&word[..size]);
            } else {
                out.extend_from_slice(&word[32 - measure(ty, value, false)?..]);
            }
        }
    }
    Ok(())
}
