use crate::{MAX_DEPTH, MAX_FIELDS, MAX_SCHEMA_BYTES, MAX_TYPES, TypedDataError, keccak};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// One named field, preserving the declared order in its enclosing struct.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypedDataField {
    /// Solidity identifier for this field.
    pub name: String,
    /// Exact EIP-712 type expression, including any array suffixes.
    #[serde(rename = "type")]
    pub type_name: String,
}

/// Named struct definitions; each field vector retains its declared order.
pub type TypedDataTypes = BTreeMap<String, Vec<TypedDataField>>;

#[derive(Clone, Debug)]
pub(crate) enum Base {
    Int { signed: bool, bits: usize },
    FixedBytes(usize),
    Address,
    Bool,
    Bytes,
    String,
    Struct(String),
}
#[derive(Clone, Debug)]
pub(crate) struct Field {
    pub name: String,
    pub base: Base,
    pub dimensions: Vec<Option<usize>>,
}
#[derive(Clone, Debug)]
pub(crate) struct Struct {
    pub fields: Vec<Field>,
    pub encoded_type: String,
    pub type_hash: [u8; 32],
}

/// Validated acyclic schema with cached canonical type strings and type hashes.
///
/// Exactly one root is required and every declared struct must be reachable.
/// Identifiers and values are strict: JavaScript coercion is not performed.
#[derive(Clone, Debug)]
pub struct TypedDataEncoder {
    pub(crate) structs: BTreeMap<String, Struct>,
    pub(crate) primary: String,
}

fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes().enumerate().all(|(i, b)| {
            b.is_ascii_alphabetic() || b == b'_' || b == b'$' || (i > 0 && b.is_ascii_digit())
        })
}
fn width(s: &str, max: usize, multiple: usize) -> Result<usize, TypedDataError> {
    let n: usize = s.parse().map_err(|_| TypedDataError::Schema)?;
    if n == 0 || n > max || !n.is_multiple_of(multiple) || s != n.to_string() {
        return Err(TypedDataError::Schema);
    }
    Ok(n)
}
fn base(s: &str) -> Result<Base, TypedDataError> {
    match s {
        "address" => Ok(Base::Address),
        "bool" => Ok(Base::Bool),
        "string" => Ok(Base::String),
        "bytes" => Ok(Base::Bytes),
        _ => {
            if let Some(rest) = s.strip_prefix("uint").or_else(|| s.strip_prefix("int"))
                && rest.bytes().all(|b| b.is_ascii_digit())
            {
                return Ok(Base::Int {
                    signed: !s.starts_with('u'),
                    bits: width(rest, 256, 8)?,
                });
            }
            if let Some(rest) = s.strip_prefix("bytes")
                && !rest.is_empty()
                && rest.bytes().all(|b| b.is_ascii_digit())
            {
                return Ok(Base::FixedBytes(width(rest, 32, 1)?));
            }
            if !identifier(s) {
                return Err(TypedDataError::Schema);
            }
            Ok(Base::Struct(s.into()))
        }
    }
}
fn expression(s: &str) -> Result<(Base, Vec<Option<usize>>), TypedDataError> {
    if s.len() > 256 {
        return Err(TypedDataError::Limit);
    }
    let end = s.find('[').unwrap_or(s.len());
    let base = base(&s[..end])?;
    let mut rest = &s[end..];
    let mut dimensions = Vec::new();
    while !rest.is_empty() {
        if dimensions.len() >= 16 {
            return Err(TypedDataError::Limit);
        }
        rest = rest.strip_prefix('[').ok_or(TypedDataError::Schema)?;
        let end = rest.find(']').ok_or(TypedDataError::Schema)?;
        let n = &rest[..end];
        let len = if n.is_empty() {
            None
        } else {
            // Zero-length arrays are accepted by the pinned JS encoder.
            if !n.bytes().all(|b| b.is_ascii_digit()) {
                return Err(TypedDataError::Schema);
            }
            let value = n.parse::<usize>().map_err(|_| TypedDataError::Limit)?;
            if value > crate::MAX_VALUE_NODES {
                return Err(TypedDataError::Limit);
            }
            if n != value.to_string() {
                return Err(TypedDataError::Schema);
            }
            Some(value)
        };
        dimensions.push(len);
        rest = &rest[end + 1..];
    }
    Ok((base, dimensions))
}

impl TypedDataEncoder {
    /// Validate and compile a schema, before allocating its cached encodings.
    pub fn new(types: &TypedDataTypes) -> Result<Self, TypedDataError> {
        if types.is_empty() {
            return Err(TypedDataError::PrimaryType);
        }
        if types.len() > MAX_TYPES {
            return Err(TypedDataError::Limit);
        }
        let mut field_count = 0usize;
        let mut bytes = 0usize;
        for (name, fields) in types {
            field_count = field_count
                .checked_add(fields.len())
                .ok_or(TypedDataError::Limit)?;
            bytes = bytes.checked_add(name.len()).ok_or(TypedDataError::Limit)?;
            if field_count > MAX_FIELDS {
                return Err(TypedDataError::Limit);
            }
            for field in fields {
                bytes = bytes
                    .checked_add(field.name.len())
                    .and_then(|n| n.checked_add(field.type_name.len()))
                    .ok_or(TypedDataError::Limit)?;
                if bytes > MAX_SCHEMA_BYTES {
                    return Err(TypedDataError::Limit);
                }
            }
            if bytes > MAX_SCHEMA_BYTES {
                return Err(TypedDataError::Limit);
            }
            if !identifier(name) || !matches!(base(name)?, Base::Struct(_)) {
                return Err(TypedDataError::Schema);
            }
        }
        let mut structs = BTreeMap::new();
        let mut dependencies: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut referenced = BTreeSet::new();
        let mut own_types = BTreeMap::new();
        for (name, fields) in types {
            let mut names = BTreeSet::new();
            let mut compiled = Vec::with_capacity(fields.len());
            let mut deps = BTreeSet::new();
            let mut own = format!("{name}(");
            for (i, field) in fields.iter().enumerate() {
                if !identifier(&field.name) || !names.insert(&field.name) {
                    return Err(TypedDataError::Schema);
                }
                let (base, dimensions) = expression(&field.type_name)?;
                if let Base::Struct(child) = &base {
                    if !types.contains_key(child) {
                        return Err(TypedDataError::UnknownType);
                    }
                    deps.insert(child.clone());
                    referenced.insert(child.clone());
                }
                if i != 0 {
                    own.push(',');
                }
                own.push_str(&field.type_name);
                own.push(' ');
                own.push_str(&field.name);
                compiled.push(Field {
                    name: field.name.clone(),
                    base,
                    dimensions,
                });
            }
            own.push(')');
            own_types.insert(name.clone(), own);
            dependencies.insert(name.clone(), deps);
            structs.insert(
                name.clone(),
                Struct {
                    fields: compiled,
                    encoded_type: String::new(),
                    type_hash: [0; 32],
                },
            );
        }
        let mut closures = BTreeMap::new();
        let mut active = BTreeSet::new();
        for name in types.keys() {
            closure(name, &dependencies, &mut closures, &mut active, 0)?;
        }
        let roots: Vec<_> = types.keys().filter(|n| !referenced.contains(*n)).collect();
        if roots.len() != 1 {
            return Err(TypedDataError::PrimaryType);
        }
        let primary = roots[0].clone();
        if closures[&primary].0.len() + 1 != types.len() {
            return Err(TypedDataError::PrimaryType);
        }
        for (name, structure) in &mut structs {
            let mut full = own_types[name].clone();
            for child in &closures[name].0 {
                full.push_str(&own_types[child]);
            }
            structure.type_hash = keccak(full.as_bytes());
            structure.encoded_type = full;
        }
        Ok(Self { structs, primary })
    }

    /// The unique schema root inferred from its dependency graph.
    pub fn primary_type(&self) -> &str {
        &self.primary
    }

    /// Canonical root declaration followed by lexically sorted dependencies.
    pub fn encode_type(&self, name: &str) -> Result<&str, TypedDataError> {
        self.structs
            .get(name)
            .map(|s| s.encoded_type.as_str())
            .ok_or(TypedDataError::UnknownType)
    }
}

type Closures = BTreeMap<String, (BTreeSet<String>, usize)>;
fn closure(
    name: &str,
    deps: &BTreeMap<String, BTreeSet<String>>,
    cache: &mut Closures,
    active: &mut BTreeSet<String>,
    depth: usize,
) -> Result<(), TypedDataError> {
    if depth > MAX_DEPTH {
        return Err(TypedDataError::Limit);
    }
    if cache.contains_key(name) {
        return Ok(());
    }
    if !active.insert(name.into()) {
        return Err(TypedDataError::Cycle);
    }
    let mut result = BTreeSet::new();
    let mut height = 0;
    for child in &deps[name] {
        closure(child, deps, cache, active, depth + 1)?;
        result.insert(child.clone());
        result.extend(cache[child].0.iter().cloned());
        height = height.max(cache[child].1 + 1);
        if height > MAX_DEPTH {
            return Err(TypedDataError::Limit);
        }
    }
    active.remove(name);
    cache.insert(name.into(), (result, height));
    Ok(())
}
