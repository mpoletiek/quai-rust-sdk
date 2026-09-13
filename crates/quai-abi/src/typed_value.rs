//! Validated values paired with their explicit Solidity types.
use crate::{AbiCoder, AbiError, AbiType, MAX_DATA_BYTES, abi_type::Kind};
use ruint::aliases::U256;
use serde_json::{Value, json};
use std::fmt;
/// An immutable, bounded ABI value whose type, range, arity and byte lengths were
/// checked on construction. Debug prints the type and encoded size, not contents.
#[derive(Clone, PartialEq, Eq)]
pub struct AbiValue {
    ty: AbiType,
    value: Value,
    encoded_size: usize,
}
impl AbiValue {
    /// Validate before retaining a value. Integer values use the codec's exact
    /// JSON integer/decimal-string policy; no JS truthiness or numeric coercion.
    pub fn new(ty: AbiType, value: Value) -> Result<Self, AbiError> {
        let encoded_size =
            crate::codec::validate(std::slice::from_ref(&ty), std::slice::from_ref(&value))?;
        Ok(Self {
            ty,
            value,
            encoded_size,
        })
    }
    /// Construct the type-correct zero/empty default, with bounds checked before
    /// allocating nested arrays or strings. Dynamic arrays default to empty.
    pub fn default_for(ty: AbiType) -> Result<Self, AbiError> {
        default_text_size(&ty)?;
        let body = default_body_size(&ty)?;
        if body
            .checked_add(if ty.dynamic { 32 } else { 0 })
            .is_none_or(|n| n > MAX_DATA_BYTES)
        {
            return Err(AbiError::Limit);
        }
        let value = default_value(&ty);
        Self::new(ty, value)
    }
    /// Borrow the validated Solidity type.
    pub fn abi_type(&self) -> &AbiType {
        &self.ty
    }
    /// Borrow the validated value without permitting mutation.
    pub fn value(&self) -> &Value {
        &self.value
    }
    /// Return both owned components; changed values require validation again.
    pub fn into_parts(self) -> (AbiType, Value) {
        (self.ty, self.value)
    }
    /// Encode as one complete ABI parameter sequence, including any dynamic offset.
    pub fn encode(&self) -> Result<Vec<u8>, AbiError> {
        AbiCoder::encode(
            std::slice::from_ref(&self.ty),
            std::slice::from_ref(&self.value),
        )
    }
    /// Retrieve a value only when the caller's expected type matches exactly.
    pub fn expect_type(&self, ty: &AbiType) -> Result<&Value, AbiError> {
        if &self.ty == ty {
            Ok(&self.value)
        } else {
            Err(AbiError::Value)
        }
    }
}
impl fmt::Debug for AbiValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AbiValue")
            .field("type", &self.ty.canonical_name())
            .field("encoded_bytes", &self.encoded_size)
            .finish()
    }
}
impl AbiType {
    /// Exact decimal integer bounds, or None for non-integer types.
    pub fn integer_bounds(&self) -> Option<(String, String)> {
        let Kind::Int(signed, bits) = self.kind else {
            return None;
        };
        Some(if signed {
            let edge = U256::from(1) << (bits - 1);
            (format!("-{edge}"), (edge - U256::from(1)).to_string())
        } else {
            ("0".into(), (U256::MAX >> (256 - bits)).to_string())
        })
    }
    /// Whether this is a signed or unsigned Solidity integer.
    pub fn is_integer(&self) -> bool {
        matches!(self.kind, Kind::Int(..))
    }
    /// Whether this is fixed or dynamic bytes, excluding strings.
    pub fn is_bytes(&self) -> bool {
        matches!(self.kind, Kind::Bytes | Kind::FixedBytes(_))
    }
    /// Whether this is a Solidity UTF-8 string.
    pub fn is_string(&self) -> bool {
        matches!(self.kind, Kind::String)
    }
    /// For arrays, return the element type and fixed length (None for dynamic).
    /// Non-array types return None for the entire result.
    pub fn array_info(&self) -> Option<(&AbiType, Option<usize>)> {
        if let Kind::Array(child, length) = &self.kind {
            Some((child, *length))
        } else {
            None
        }
    }
    /// Borrow tuple component types in declaration order; non-tuples return None.
    pub fn tuple_components(&self) -> Option<&[AbiType]> {
        if let Kind::Tuple(fields) = &self.kind {
            Some(fields)
        } else {
            None
        }
    }
}
fn default_text_size(ty: &AbiType) -> Result<usize, AbiError> {
    let size = match &ty.kind {
        Kind::Bool | Kind::String => 0,
        Kind::Int(..) => 1,
        Kind::Address => 42,
        Kind::Bytes => 2,
        Kind::FixedBytes(n) => 2 + 2 * n,
        Kind::Array(_, Some(0)) => 0,
        Kind::Array(child, Some(n)) => default_text_size(child)?
            .checked_mul(*n)
            .ok_or(AbiError::Limit)?,
        Kind::Array(_, None) => 0,
        Kind::Tuple(fields) => fields.iter().try_fold(0usize, |sum, t| {
            sum.checked_add(default_text_size(t)?)
                .ok_or(AbiError::Limit)
        })?,
    };
    if size > MAX_DATA_BYTES {
        Err(AbiError::Limit)
    } else {
        Ok(size)
    }
}
fn default_value(ty: &AbiType) -> Value {
    match &ty.kind {
        Kind::Bool => json!(false),
        Kind::Int(..) => json!("0"),
        Kind::String => json!(""),
        Kind::Bytes => json!("0x"),
        Kind::Address => json!(format!("0x{}", "00".repeat(20))),
        Kind::FixedBytes(n) => json!(format!("0x{}", "00".repeat(*n))),
        Kind::Array(child, Some(n)) => {
            Value::Array((0..*n).map(|_| default_value(child)).collect())
        }
        Kind::Array(_, None) => json!([]),
        Kind::Tuple(fields) => Value::Array(fields.iter().map(default_value).collect()),
    }
}

fn default_body_size(ty: &AbiType) -> Result<usize, AbiError> {
    fn entry(ty: &AbiType) -> Result<usize, AbiError> {
        default_body_size(ty)?
            .checked_add(if ty.dynamic { 32 } else { 0 })
            .ok_or(AbiError::Limit)
    }
    let size = match &ty.kind {
        Kind::Array(_, Some(0)) => 0,
        Kind::Array(child, Some(n)) => entry(child)?.checked_mul(*n).ok_or(AbiError::Limit)?,
        Kind::Tuple(fields) => fields.iter().try_fold(0usize, |sum, t| {
            sum.checked_add(entry(t)?).ok_or(AbiError::Limit)
        })?,
        _ => 32,
    };
    if size > MAX_DATA_BYTES {
        Err(AbiError::Limit)
    } else {
        Ok(size)
    }
}
