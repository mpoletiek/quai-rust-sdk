use crate::{
    MAX_DATA_BYTES, MAX_DEPTH, MAX_VALUE_NODES, TypedDataEncoder, TypedDataError, TypedDataTypes,
};
use quai_primitives::Hash32;
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use std::fmt;

/// An immutable, validated EIP-712 RPC document with its cached signing hash.
///
/// JSON parsing rejects duplicate keys at every level before conversion to a
/// map. The declared primary type and optional EIP712Domain declaration are
/// checked rather than trusted. Signing and key handling belong to the signer.
#[derive(Clone, Debug)]
pub struct TypedData {
    encoder: TypedDataEncoder,
    types: TypedDataTypes,
    domain: Value,
    message: Value,
    hash: Hash32,
}
impl TypedData {
    /// Parse a bounded UTF-8 JSON document with `types`, `primaryType`, `domain`
    /// and `message`. An explicit domain declaration must match the canonical
    /// present domain fields, in canonical order; it may also be omitted.
    pub fn from_json(bytes: &[u8]) -> Result<Self, TypedDataError> {
        let parsed = parse_json(bytes)?;
        let Value::Object(mut object) = parsed else {
            return Err(TypedDataError::Document);
        };
        if object.len() != 4 {
            return Err(TypedDataError::Document);
        }
        let types = object.remove("types").ok_or(TypedDataError::Document)?;
        let mut types: TypedDataTypes =
            serde_json::from_value(types).map_err(|_| TypedDataError::Schema)?;
        let primary = object
            .remove("primaryType")
            .ok_or(TypedDataError::Document)?;
        let domain = object.remove("domain").ok_or(TypedDataError::Document)?;
        let message = object.remove("message").ok_or(TypedDataError::Document)?;
        if let Some(fields) = types.remove("EIP712Domain")
            && fields != crate::value::domain_parts(&domain)?.0
        {
            return Err(TypedDataError::Domain);
        }
        let encoder = TypedDataEncoder::new(&types)?;
        if primary.as_str() != Some(encoder.primary_type()) {
            return Err(TypedDataError::PrimaryType);
        }
        let hash = encoder.signing_hash(&domain, &message)?;
        Ok(Self {
            encoder,
            types,
            domain,
            message,
            hash,
        })
    }

    /// Serialize a validated EIP-712 v4 RPC document. Integer values become decimal
    /// strings so a wallet's JavaScript JSON parser cannot round wide integers.
    /// The canonical domain declaration is included; null domain fields are omitted.
    pub fn to_rpc_json(&self) -> Result<String, TypedDataError> {
        fn exact_numbers(value: &Value) -> Value {
            match value {
                Value::Number(number) => Value::String(number.to_string()),
                Value::Array(values) => Value::Array(values.iter().map(exact_numbers).collect()),
                Value::Object(values) => Value::Object(
                    values
                        .iter()
                        .map(|(k, v)| (k.clone(), exact_numbers(v)))
                        .collect(),
                ),
                other => other.clone(),
            }
        }
        let mut types = self.types.clone();
        types.insert(
            "EIP712Domain".to_owned(),
            crate::value::domain_parts(&self.domain)?.0,
        );
        let mut domain = exact_numbers(&self.domain);
        if let Value::Object(fields) = &mut domain {
            fields.retain(|_, value| !value.is_null());
        }
        let document = serde_json::json!({"types": types, "primaryType": self.primary_type(), "domain": domain, "message": exact_numbers(&self.message)});
        let encoded = serde_json::to_string(&document).map_err(|_| TypedDataError::Document)?;
        if encoded.len() > MAX_DATA_BYTES {
            return Err(TypedDataError::Limit);
        }
        Ok(encoded)
    }

    /// Validated schema root; the JSON `primaryType` was checked against it.
    pub fn primary_type(&self) -> &str {
        self.encoder.primary_type()
    }
    /// Borrow the compiled schema.
    pub const fn encoder(&self) -> &TypedDataEncoder {
        &self.encoder
    }
    /// Borrow the complete validated domain, including optional null fields.
    pub const fn domain(&self) -> &Value {
        &self.domain
    }
    /// Borrow the validated message; no mutable access can invalidate its hash.
    pub const fn message(&self) -> &Value {
        &self.message
    }
    /// The cached domain-separated hash suitable for ECDSA prehash signing.
    pub const fn signing_hash(&self) -> Hash32 {
        self.hash
    }
    /// Hash only the domain separator.
    pub fn domain_hash(&self) -> Result<Hash32, TypedDataError> {
        crate::hash_domain(&self.domain)
    }
}

struct State {
    nodes: usize,
    exceeded: bool,
}
struct Seed<'a> {
    state: &'a mut State,
    depth: usize,
}
impl<'de> DeserializeSeed<'de> for Seed<'_> {
    type Value = Value;
    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        self.state.nodes += 1;
        if self.state.nodes > MAX_VALUE_NODES || self.depth > MAX_DEPTH {
            self.state.exceeded = true;
            return Err(de::Error::custom("resource limit"));
        }
        deserializer.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for Seed<'_> {
    type Value = Value;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("bounded typed-data JSON")
    }
    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }
    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }
    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("invalid number"))
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.into()))
    }
    fn visit_string<E: de::Error>(self, value: String) -> Result<Value, E> {
        Ok(Value::String(value))
    }
    fn visit_none<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = access.next_element_seed(Seed {
            state: self.state,
            depth: self.depth + 1,
        })? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Value, A::Error> {
        let mut object = Map::new();
        while let Some(key) = access.next_key::<String>()? {
            if object.contains_key(&key) {
                return Err(de::Error::custom("duplicate key"));
            }
            let value = access.next_value_seed(Seed {
                state: self.state,
                depth: self.depth + 1,
            })?;
            object.insert(key, value);
        }
        Ok(Value::Object(object))
    }
}

pub(crate) fn parse_json(bytes: &[u8]) -> Result<Value, TypedDataError> {
    if bytes.len() > MAX_DATA_BYTES {
        return Err(TypedDataError::Limit);
    }
    let mut state = State {
        nodes: 0,
        exceeded: false,
    };
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let result = Seed {
        state: &mut state,
        depth: 0,
    }
    .deserialize(&mut deserializer);
    let parsed = result.map_err(|_| {
        if state.exceeded {
            TypedDataError::Limit
        } else {
            TypedDataError::Document
        }
    })?;
    deserializer.end().map_err(|_| TypedDataError::Document)?;
    Ok(parsed)
}
