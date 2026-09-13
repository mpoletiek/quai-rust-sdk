use crate::schema::{Base, Field};
use crate::{
    MAX_DATA_BYTES, MAX_DEPTH, MAX_VALUE_NODES, TypedDataEncoder, TypedDataError, TypedDataField,
    TypedDataTypes, keccak,
};
use quai_primitives::{Address, Hash32};
use ruint::aliases::U256;
use serde_json::{Map, Value};

pub(crate) fn preflight(value: &Value) -> Result<(), TypedDataError> {
    let mut stack = vec![(value, 0usize)];
    let mut nodes = 0usize;
    let mut bytes = 0usize;
    while let Some((value, depth)) = stack.pop() {
        nodes += 1;
        if nodes > MAX_VALUE_NODES || depth > MAX_DEPTH {
            return Err(TypedDataError::Limit);
        }
        let mut add = |n: usize| -> Result<(), TypedDataError> {
            bytes = bytes.checked_add(n).ok_or(TypedDataError::Limit)?;
            if bytes > MAX_DATA_BYTES {
                return Err(TypedDataError::Limit);
            }
            Ok(())
        };
        match value {
            Value::String(s) => add(s.len())?,
            Value::Array(a) => {
                if a.len() > MAX_VALUE_NODES - nodes || stack.len() + a.len() > MAX_VALUE_NODES {
                    return Err(TypedDataError::Limit);
                }
                stack.extend(a.iter().map(|v| (v, depth + 1)));
            }
            Value::Object(o) => {
                if o.len() > MAX_VALUE_NODES - nodes || stack.len() + o.len() > MAX_VALUE_NODES {
                    return Err(TypedDataError::Limit);
                }
                for (key, value) in o {
                    add(key.len())?;
                    stack.push((value, depth + 1));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

pub(crate) fn integer(
    value: &Value,
    signed: bool,
    bits: usize,
) -> Result<[u8; 32], TypedDataError> {
    let text = match value {
        Value::String(s) => std::borrow::Cow::Borrowed(s.as_str()),
        Value::Number(n) => {
            // Only exact JSON integer tokens in the JS safe-integer range.
            if let Some(i) = n.as_i64() {
                if !(-9_007_199_254_740_991..=9_007_199_254_740_991).contains(&i) {
                    return Err(TypedDataError::Integer);
                }
                std::borrow::Cow::Owned(i.to_string())
            } else {
                return Err(TypedDataError::Integer);
            }
        }
        _ => return Err(TypedDataError::Integer),
    };
    if text.len() > 80 {
        return Err(TypedDataError::Integer);
    }
    let (negative, magnitude) = if let Some(rest) = text.strip_prefix('-') {
        (true, rest)
    } else {
        (false, text.as_ref())
    };
    let (digits, radix) = if let Some(rest) = magnitude
        .strip_prefix("0x")
        .or_else(|| magnitude.strip_prefix("0X"))
    {
        (rest, 16)
    } else {
        (magnitude, 10)
    };
    if digits.is_empty()
        || !digits.bytes().all(|b| {
            if radix == 16 {
                b.is_ascii_hexdigit()
            } else {
                b.is_ascii_digit()
            }
        })
    {
        return Err(TypedDataError::Integer);
    }
    let n = U256::from_str_radix(digits, radix).map_err(|_| TypedDataError::Integer)?;
    if negative && n != U256::ZERO {
        if !signed || n > (U256::from(1) << (bits - 1)) {
            return Err(TypedDataError::Integer);
        }
        Ok(n.wrapping_neg().to_be_bytes())
    } else {
        if n.bit_len() > bits - usize::from(signed) {
            return Err(TypedDataError::Integer);
        }
        Ok(n.to_be_bytes())
    }
}

pub(crate) fn decode_hex(value: &Value) -> Result<Vec<u8>, TypedDataError> {
    let text = value.as_str().ok_or(TypedDataError::Bytes)?;
    let digits = text.strip_prefix("0x").ok_or(TypedDataError::Bytes)?;
    if digits.len() % 2 != 0 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(TypedDataError::Bytes);
    }
    if digits.len() / 2 > MAX_DATA_BYTES {
        return Err(TypedDataError::Limit);
    }
    digits
        .as_bytes()
        .chunks_exact(2)
        .map(|p| {
            let nibble = |b: u8| {
                if b <= b'9' {
                    b - b'0'
                } else {
                    (b | 32) - b'a' + 10
                }
            };
            Ok((nibble(p[0]) << 4) | nibble(p[1]))
        })
        .collect()
}

impl TypedDataEncoder {
    /// Encode a supported EIP-712 type: structs produce type hash plus field
    /// words; primitive and array expressions produce one 32-byte word.
    ///
    /// The supplied value is checked in full before encoding; unknown object
    /// fields are rejected rather than silently omitted from the signed hash.
    pub fn encode_data(&self, name: &str, value: &Value) -> Result<Vec<u8>, TypedDataError> {
        self.encoder(name)?.encode(value)
    }

    /// Hash a complete encoded value. Declared structs include their canonical
    /// type hash; primitive/array roots hash their single encoded word.
    pub fn hash_struct(&self, name: &str, value: &Value) -> Result<Hash32, TypedDataError> {
        Ok(keccak(&self.encode_data(name, value)?).into())
    }

    /// Hash a value of the schema's primary type.
    pub fn hash(&self, value: &Value) -> Result<Hash32, TypedDataError> {
        self.hash_struct(&self.primary, value)
    }

    /// The 66-byte EIP-191 version-1 preimage, `0x1901 || domain || message`.
    pub fn signing_preimage(
        &self,
        domain: &Value,
        value: &Value,
    ) -> Result<[u8; 66], TypedDataError> {
        let mut result = [0; 66];
        result[..2].copy_from_slice(&[0x19, 0x01]);
        result[2..34].copy_from_slice(hash_domain(domain)?.bytes());
        result[34..].copy_from_slice(self.hash(value)?.bytes());
        Ok(result)
    }

    /// Hash the domain-separated preimage for the caller's signing layer.
    pub fn signing_hash(&self, domain: &Value, value: &Value) -> Result<Hash32, TypedDataError> {
        Ok(keccak(&self.signing_preimage(domain, value)?).into())
    }

    pub(crate) fn encode_struct(
        &self,
        name: &str,
        value: &Value,
        depth: usize,
    ) -> Result<Vec<u8>, TypedDataError> {
        if depth > MAX_DEPTH {
            return Err(TypedDataError::Limit);
        }
        let structure = self.structs.get(name).ok_or(TypedDataError::UnknownType)?;
        let object = value.as_object().ok_or(TypedDataError::Value)?;
        if object.len() != structure.fields.len() {
            return Err(TypedDataError::Value);
        }
        let mut bytes = Vec::with_capacity((structure.fields.len() + 1) * 32);
        bytes.extend_from_slice(&structure.type_hash);
        for field in &structure.fields {
            let value = object.get(&field.name).ok_or(TypedDataError::Value)?;
            bytes.extend_from_slice(&self.word(field, field.dimensions.len(), value, depth + 1)?);
        }
        Ok(bytes)
    }

    pub(crate) fn word(
        &self,
        field: &Field,
        dimensions: usize,
        value: &Value,
        depth: usize,
    ) -> Result<[u8; 32], TypedDataError> {
        if depth > MAX_DEPTH {
            return Err(TypedDataError::Limit);
        }
        if dimensions > 0 {
            let values = value.as_array().ok_or(TypedDataError::Value)?;
            if field.dimensions[dimensions - 1].is_some_and(|n| n != values.len()) {
                return Err(TypedDataError::ArrayLength);
            }
            // Streaming avoids allocating the concatenated array words.
            use tiny_keccak::{Hasher, Keccak};
            let mut hasher = Keccak::v256();
            for value in values {
                hasher.update(&self.word(field, dimensions - 1, value, depth + 1)?);
            }
            let mut result = [0; 32];
            hasher.finalize(&mut result);
            return Ok(result);
        }
        let mut word = [0; 32];
        match &field.base {
            Base::Int { signed, bits } => integer(value, *signed, *bits),
            Base::FixedBytes(n) => {
                let bytes = decode_hex(value)?;
                if bytes.len() != *n {
                    return Err(TypedDataError::Bytes);
                }
                word[..*n].copy_from_slice(&bytes);
                Ok(word)
            }
            Base::Address => {
                let address: Address = value
                    .as_str()
                    .ok_or(TypedDataError::Bytes)?
                    .parse()
                    .map_err(|_| TypedDataError::Bytes)?;
                word[12..].copy_from_slice(address.bytes());
                Ok(word)
            }
            Base::Bool => {
                word[31] = u8::from(value.as_bool().ok_or(TypedDataError::Value)?);
                Ok(word)
            }
            Base::Bytes => Ok(keccak(&decode_hex(value)?)),
            Base::String => Ok(keccak(
                value.as_str().ok_or(TypedDataError::Value)?.as_bytes(),
            )),
            Base::Struct(name) => Ok(keccak(&self.encode_struct(name, value, depth)?)),
        }
    }
}

pub(crate) fn domain_parts(domain: &Value) -> Result<(Vec<TypedDataField>, Value), TypedDataError> {
    preflight(domain)?;
    let domain = domain.as_object().ok_or(TypedDataError::Domain)?;
    const FIELDS: [(&str, &str); 5] = [
        ("name", "string"),
        ("version", "string"),
        ("chainId", "uint256"),
        ("verifyingContract", "address"),
        ("salt", "bytes32"),
    ];
    if domain
        .keys()
        .any(|key| !FIELDS.iter().any(|(name, _)| key == name))
    {
        return Err(TypedDataError::Domain);
    }
    let mut fields = Vec::new();
    let mut values = Map::new();
    for (name, ty) in FIELDS {
        if let Some(value) = domain.get(name).filter(|v| !v.is_null()) {
            fields.push(TypedDataField {
                name: name.into(),
                type_name: ty.into(),
            });
            values.insert(name.into(), value.clone());
        }
    }
    Ok((fields, Value::Object(values)))
}

/// Hash the standard domain, using its prescribed field order and omitting nulls.
/// Unknown keys are rejected even when null; no name resolution is performed.
pub fn hash_domain(domain: &Value) -> Result<Hash32, TypedDataError> {
    let (fields, values) = domain_parts(domain)?;
    let types = TypedDataTypes::from([("EIP712Domain".into(), fields)]);
    TypedDataEncoder::new(&types)?.hash(&values)
}

/// Validate a schema and hash its domain-separated primary value.
pub fn hash_typed_data(
    domain: &Value,
    types: &TypedDataTypes,
    value: &Value,
) -> Result<Hash32, TypedDataError> {
    TypedDataEncoder::new(types)?.signing_hash(domain, value)
}
