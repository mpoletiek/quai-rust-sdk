use crate::abi_type::Kind;
use crate::{AbiError, AbiType, MAX_DATA_BYTES, MAX_DEPTH, MAX_FIELDS, MAX_VALUE_NODES};
use quai_primitives::Address;
use ruint::aliases::U256;
use serde_json::Value;

/// Canonical, bounded Solidity ABI parameter encoding and decoding.
///
/// Tuples and arrays are represented as JSON arrays, integer output as decimal
/// strings, addresses as checksum strings, and bytes as `0x` hexadecimal strings.
/// Decode rejects aliases, gaps, trailing bytes and noncanonical primitive words.
pub struct AbiCoder;
impl AbiCoder {
    /// Create zero/empty defaults for an entire parameter sequence. Integer
    /// defaults are decimal strings, dynamic arrays are empty and tuple values
    /// are positional arrays. Shared field, node, text and encoded-byte limits
    /// are checked before allocating any nested default values.
    pub fn default_values(types: &[AbiType]) -> Result<Vec<Value>, AbiError> {
        crate::typed_value::default_values(types)
    }
    /// Encode one parameter sequence after validating values and measuring the
    /// entire bounded output; the output buffer is allocated once.
    pub fn encode(types: &[AbiType], values: &[Value]) -> Result<Vec<u8>, AbiError> {
        let size = validate(types, values)?;
        let sequence = Sequence::Fields(types);
        let mut output = vec![0; size];
        let end = write_sequence(sequence, values, 0, &mut output)?;
        if end != output.len() {
            return Err(AbiError::Encoding);
        }
        Ok(output)
    }
    /// Decode a complete canonical parameter sequence with bounds checked before
    /// allocation. A bounded length of zero-sized elements is preserved exactly.
    pub fn decode(types: &[AbiType], bytes: &[u8]) -> Result<Vec<Value>, AbiError> {
        sequence_limit(types)?;
        if bytes.len() > MAX_DATA_BYTES {
            return Err(AbiError::Limit);
        }
        if !bytes.len().is_multiple_of(32) {
            return Err(AbiError::Encoding);
        }
        let (values, end) =
            decode_sequence(Sequence::Fields(types), bytes, 0, &mut Budget::default(), 0)?;
        if end != bytes.len() {
            return Err(AbiError::Encoding);
        }
        Ok(values)
    }
}
pub(crate) fn validate(types: &[AbiType], values: &[Value]) -> Result<usize, AbiError> {
    sequence_limit(types)?;
    if types.len() != values.len() {
        return Err(AbiError::Value);
    }
    preflight_values(values)?;
    measure_sequence(Sequence::Fields(types), values, 0)
}
pub(crate) fn sequence_limit(types: &[AbiType]) -> Result<(), AbiError> {
    if types.len() > MAX_FIELDS {
        return Err(AbiError::Limit);
    }
    let nodes = types.iter().try_fold(0usize, |n, t| {
        n.checked_add(t.minimum_nodes).ok_or(AbiError::Limit)
    })?;
    if nodes > MAX_VALUE_NODES {
        return Err(AbiError::Limit);
    }
    Ok(())
}
fn preflight_values(values: &[Value]) -> Result<(), AbiError> {
    if values.len() > MAX_VALUE_NODES {
        return Err(AbiError::Limit);
    }
    let mut stack: Vec<_> = values.iter().map(|v| (v, 0usize)).collect();
    let mut budget = Budget::default();
    while let Some((v, depth)) = stack.pop() {
        budget.node(depth)?;
        match v {
            Value::String(s) => budget.text(s.len())?,
            Value::Array(a) => {
                if a.len() > MAX_VALUE_NODES - budget.nodes
                    || stack.len() + a.len() > MAX_VALUE_NODES
                {
                    return Err(AbiError::Limit);
                }
                stack.extend(a.iter().map(|v| (v, depth + 1)));
            }
            Value::Object(_) => return Err(AbiError::Value),
            _ => {}
        }
    }
    Ok(())
}
#[derive(Clone, Copy)]
enum Sequence<'a> {
    Fields(&'a [AbiType]),
    Repeat(&'a AbiType, usize),
}
impl<'a> Sequence<'a> {
    fn len(self) -> usize {
        match self {
            Self::Fields(f) => f.len(),
            Self::Repeat(_, n) => n,
        }
    }
    fn get(self, i: usize) -> &'a AbiType {
        match self {
            Self::Fields(f) => &f[i],
            Self::Repeat(t, _) => t,
        }
    }
    fn head(self) -> Result<usize, AbiError> {
        match self {
            Self::Fields(f) => f.iter().try_fold(0usize, |sum, t| {
                sum.checked_add(t.head).ok_or(AbiError::Limit)
            }),
            Self::Repeat(t, n) => t.head.checked_mul(n).ok_or(AbiError::Limit),
        }
    }
}
fn checked_size(n: usize) -> Result<usize, AbiError> {
    if n > MAX_DATA_BYTES {
        Err(AbiError::Limit)
    } else {
        Ok(n)
    }
}
fn padded(n: usize) -> Result<usize, AbiError> {
    n.checked_add(31).map(|n| n & !31).ok_or(AbiError::Limit)
}
fn measure_sequence(
    types: Sequence<'_>,
    values: &[Value],
    depth: usize,
) -> Result<usize, AbiError> {
    if types.len() != values.len() {
        return Err(AbiError::Value);
    }
    let mut size = checked_size(types.head()?)?;
    for (i, value) in values.iter().enumerate() {
        let ty = types.get(i);
        let n = measure(ty, value, depth)?;
        if ty.dynamic {
            size = checked_size(size.checked_add(n).ok_or(AbiError::Limit)?)?;
        }
    }
    Ok(size)
}
fn measure(ty: &AbiType, value: &Value, depth: usize) -> Result<usize, AbiError> {
    if depth > MAX_DEPTH {
        return Err(AbiError::Limit);
    }
    match &ty.kind {
        Kind::Tuple(fields) => measure_sequence(
            Sequence::Fields(fields),
            value.as_array().ok_or(AbiError::Value)?,
            depth + 1,
        ),
        Kind::Array(child, n) => {
            let values = value.as_array().ok_or(AbiError::Value)?;
            if n.is_some_and(|n| n != values.len()) {
                return Err(AbiError::Value);
            }
            let size = measure_sequence(Sequence::Repeat(child, values.len()), values, depth + 1)?;
            checked_size(
                size.checked_add(if n.is_none() { 32 } else { 0 })
                    .ok_or(AbiError::Limit)?,
            )
        }
        Kind::String => checked_size(32 + padded(value.as_str().ok_or(AbiError::Value)?.len())?),
        Kind::Bytes => checked_size(
            32 + padded(
                crate::value::decode_hex(value)
                    .map_err(|_| AbiError::Value)?
                    .len(),
            )?,
        ),
        _ => {
            primitive_word(ty, value)?;
            Ok(32)
        }
    }
}
pub(crate) fn primitive_word(ty: &AbiType, value: &Value) -> Result<[u8; 32], AbiError> {
    let mut word = [0; 32];
    match ty.kind {
        Kind::Int(signed, bits) => {
            return crate::value::integer(value, signed, bits).map_err(|_| AbiError::Value);
        }
        Kind::Bool => word[31] = u8::from(value.as_bool().ok_or(AbiError::Value)?),
        Kind::Address => {
            let address: Address = value
                .as_str()
                .ok_or(AbiError::Value)?
                .parse()
                .map_err(|_| AbiError::Value)?;
            word[12..].copy_from_slice(address.bytes());
        }
        Kind::FixedBytes(n) => {
            let bytes = crate::value::decode_hex(value).map_err(|_| AbiError::Value)?;
            if bytes.len() != n {
                return Err(AbiError::Value);
            }
            word[..n].copy_from_slice(&bytes);
        }
        _ => return Err(AbiError::Value),
    }
    Ok(word)
}
fn put_usize(output: &mut [u8], offset: usize, n: usize) {
    output[offset..offset + 32].copy_from_slice(&U256::from(n).to_be_bytes::<32>());
}
fn write_sequence(
    types: Sequence<'_>,
    values: &[Value],
    base: usize,
    output: &mut [u8],
) -> Result<usize, AbiError> {
    let mut head = base;
    let mut tail = base + types.head()?;
    for (i, value) in values.iter().enumerate() {
        let ty = types.get(i);
        if ty.dynamic {
            put_usize(output, head, tail - base);
            tail = write(ty, value, tail, output)?;
        } else {
            write(ty, value, head, output)?;
        }
        head += ty.head;
    }
    Ok(tail)
}
fn write(ty: &AbiType, value: &Value, offset: usize, output: &mut [u8]) -> Result<usize, AbiError> {
    match &ty.kind {
        Kind::Tuple(fields) => write_sequence(
            Sequence::Fields(fields),
            value.as_array().ok_or(AbiError::Value)?,
            offset,
            output,
        ),
        Kind::Array(child, n) => {
            let values = value.as_array().ok_or(AbiError::Value)?;
            let base = if n.is_none() {
                put_usize(output, offset, values.len());
                offset + 32
            } else {
                offset
            };
            write_sequence(Sequence::Repeat(child, values.len()), values, base, output)
        }
        Kind::Bytes | Kind::String => {
            let owned;
            let bytes = if matches!(ty.kind, Kind::Bytes) {
                owned = crate::value::decode_hex(value).map_err(|_| AbiError::Value)?;
                owned.as_slice()
            } else {
                value.as_str().ok_or(AbiError::Value)?.as_bytes()
            };
            put_usize(output, offset, bytes.len());
            output[offset + 32..offset + 32 + bytes.len()].copy_from_slice(bytes);
            Ok(offset + 32 + padded(bytes.len())?)
        }
        _ => {
            output[offset..offset + 32].copy_from_slice(&primitive_word(ty, value)?);
            Ok(offset + 32)
        }
    }
}
#[derive(Default)]
struct Budget {
    nodes: usize,
    bytes: usize,
}
impl Budget {
    fn node(&mut self, depth: usize) -> Result<(), AbiError> {
        self.nodes += 1;
        if self.nodes > MAX_VALUE_NODES || depth > MAX_DEPTH {
            Err(AbiError::Limit)
        } else {
            Ok(())
        }
    }
    fn text(&mut self, n: usize) -> Result<(), AbiError> {
        self.bytes = self.bytes.checked_add(n).ok_or(AbiError::Limit)?;
        checked_size(self.bytes).map(|_| ())
    }
}
fn word(bytes: &[u8], offset: usize) -> Result<&[u8; 32], AbiError> {
    bytes
        .get(offset..offset.checked_add(32).ok_or(AbiError::Encoding)?)
        .and_then(|b| b.try_into().ok())
        .ok_or(AbiError::Encoding)
}
fn size_word(bytes: &[u8], offset: usize) -> Result<usize, AbiError> {
    let value = word(bytes, offset)?;
    let prefix = 32 - size_of::<usize>();
    if value[..prefix].iter().any(|b| *b != 0) {
        return Err(AbiError::Encoding);
    }
    Ok(usize::from_be_bytes(
        value[prefix..].try_into().map_err(|_| AbiError::Encoding)?,
    ))
}
fn decode_sequence(
    types: Sequence<'_>,
    bytes: &[u8],
    base: usize,
    budget: &mut Budget,
    depth: usize,
) -> Result<(Vec<Value>, usize), AbiError> {
    let head_size = types.head()?;
    let mut head = base;
    let mut tail = base.checked_add(head_size).ok_or(AbiError::Encoding)?;
    if tail > bytes.len() {
        return Err(AbiError::Encoding);
    }
    let minimum = match types {
        Sequence::Repeat(ty, n) => ty.minimum_nodes.checked_mul(n).ok_or(AbiError::Limit)?,
        Sequence::Fields(fields) => fields.iter().try_fold(0usize, |a, t| {
            a.checked_add(t.minimum_nodes).ok_or(AbiError::Limit)
        })?,
    };
    if minimum > MAX_VALUE_NODES - budget.nodes || types.len() > MAX_VALUE_NODES {
        return Err(AbiError::Limit);
    }
    let mut values = Vec::with_capacity(types.len());
    for i in 0..types.len() {
        let ty = types.get(i);
        let offset = if ty.dynamic {
            let relative = size_word(bytes, head)?;
            if relative != tail - base {
                return Err(AbiError::Encoding);
            }
            tail
        } else {
            head
        };
        let (value, end) = decode(ty, bytes, offset, budget, depth)?;
        if ty.dynamic {
            tail = end;
        } else if end != head + ty.head {
            return Err(AbiError::Encoding);
        }
        values.push(value);
        head += ty.head;
    }
    Ok((values, tail))
}
fn decode(
    ty: &AbiType,
    bytes: &[u8],
    offset: usize,
    budget: &mut Budget,
    depth: usize,
) -> Result<(Value, usize), AbiError> {
    budget.node(depth)?;
    match &ty.kind {
        Kind::Tuple(fields) => {
            let (values, end) =
                decode_sequence(Sequence::Fields(fields), bytes, offset, budget, depth + 1)?;
            Ok((Value::Array(values), end))
        }
        Kind::Array(child, n) => {
            let (n, base) = if let Some(n) = n {
                (*n, offset)
            } else {
                (size_word(bytes, offset)?, offset + 32)
            };
            if n > MAX_VALUE_NODES {
                return Err(AbiError::Limit);
            }
            let (values, end) =
                decode_sequence(Sequence::Repeat(child, n), bytes, base, budget, depth + 1)?;
            Ok((Value::Array(values), end))
        }
        Kind::String | Kind::Bytes => {
            let len = size_word(bytes, offset)?;
            if len > MAX_DATA_BYTES {
                return Err(AbiError::Limit);
            }
            let start = offset + 32;
            let end = start.checked_add(padded(len)?).ok_or(AbiError::Encoding)?;
            let raw = bytes.get(start..end).ok_or(AbiError::Encoding)?;
            if raw[len..].iter().any(|b| *b != 0) {
                return Err(AbiError::Encoding);
            }
            let value = if matches!(ty.kind, Kind::String) {
                budget.text(len)?;
                std::str::from_utf8(&raw[..len])
                    .map_err(|_| AbiError::Encoding)?
                    .to_owned()
            } else {
                budget.text(len * 2 + 2)?;
                to_hex(&raw[..len])
            };
            Ok((Value::String(value), end))
        }
        _ => {
            let word = word(bytes, offset)?;
            let value = match ty.kind {
                Kind::Bool => {
                    if word[..31].iter().any(|b| *b != 0) || word[31] > 1 {
                        return Err(AbiError::Encoding);
                    }
                    Value::Bool(word[31] == 1)
                }
                Kind::Int(signed, bits) => {
                    let start = 32 - bits / 8;
                    let negative = signed && word[start] & 0x80 != 0;
                    let pad = if negative { 255 } else { 0 };
                    if word[..start].iter().any(|b| *b != pad) {
                        return Err(AbiError::Encoding);
                    }
                    let n = U256::from_be_bytes(*word);
                    let value = if negative {
                        format!("-{}", n.wrapping_neg())
                    } else {
                        n.to_string()
                    };
                    budget.text(value.len())?;
                    Value::String(value)
                }
                Kind::Address => {
                    if word[..12].iter().any(|b| *b != 0) {
                        return Err(AbiError::Encoding);
                    }
                    let value =
                        Address::from_bytes(word[12..].try_into().map_err(|_| AbiError::Encoding)?)
                            .to_string();
                    budget.text(value.len())?;
                    Value::String(value)
                }
                Kind::FixedBytes(n) => {
                    if word[n..].iter().any(|b| *b != 0) {
                        return Err(AbiError::Encoding);
                    }
                    budget.text(n * 2 + 2)?;
                    Value::String(to_hex(&word[..n]))
                }
                _ => return Err(AbiError::Encoding),
            };
            Ok((value, offset + 32))
        }
    }
}
pub(crate) fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(2 + bytes.len() * 2);
    result.push_str("0x");
    for b in bytes {
        result.push(HEX[usize::from(b >> 4)] as char);
        result.push(HEX[usize::from(b & 15)] as char);
    }
    result
}

/// Compute a Solidity indexed-event topic. Compound values use the specification's
/// special in-place encoding, not normal ABI encoding or packed encoding.
/// Hashes of dynamic/compound indexed values cannot recover the original value.
pub fn indexed_event_topic(
    ty: &AbiType,
    value: &Value,
) -> Result<quai_primitives::Hash32, AbiError> {
    preflight_values(std::slice::from_ref(value))?;
    measure(ty, value, 0)?;
    if !matches!(
        ty.kind,
        Kind::Bytes | Kind::String | Kind::Tuple(_) | Kind::Array(_, _)
    ) {
        return Ok(primitive_word(ty, value)?.into());
    }
    use tiny_keccak::{Hasher, Keccak};
    let mut hasher = Keccak::v256();
    indexed(ty, value, false, &mut hasher)?;
    let mut word = [0; 32];
    hasher.finalize(&mut word);
    Ok(word.into())
}
fn indexed(
    ty: &AbiType,
    value: &Value,
    nested: bool,
    hasher: &mut tiny_keccak::Keccak,
) -> Result<(), AbiError> {
    use tiny_keccak::Hasher;
    match &ty.kind {
        Kind::String | Kind::Bytes => {
            let owned;
            let bytes = if matches!(ty.kind, Kind::Bytes) {
                owned = crate::value::decode_hex(value).map_err(|_| AbiError::Value)?;
                owned.as_slice()
            } else {
                value.as_str().ok_or(AbiError::Value)?.as_bytes()
            };
            hasher.update(bytes);
            if nested {
                hasher.update(&[0; 32][..(32 - bytes.len() % 32) % 32]);
            }
        }
        Kind::Tuple(fields) => {
            let values = value.as_array().ok_or(AbiError::Value)?;
            for (ty, value) in fields.iter().zip(values) {
                indexed(ty, value, true, hasher)?;
            }
        }
        Kind::Array(child, _) => {
            for value in value.as_array().ok_or(AbiError::Value)? {
                indexed(child, value, true, hasher)?;
            }
        }
        _ => hasher.update(&primitive_word(ty, value)?),
    }
    Ok(())
}
