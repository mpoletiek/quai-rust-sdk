use crate::{
    AbiCoder, AbiError, AbiType, MAX_DEPTH, MAX_FIELDS, TypedDataError, indexed_event_topic, keccak,
};
use quai_primitives::Hash32;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Solidity's declared function/constructor/fallback mutability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateMutability {
    /// May neither read nor write chain state.
    Pure,
    /// May read but not write chain state.
    View,
    /// May write state but does not accept a native value transfer.
    Nonpayable,
    /// May write state and accept a native value transfer.
    Payable,
}

/// A compiled contract function with an exact four-byte selector.
#[derive(Clone, Debug)]
pub struct AbiFunction {
    name: String,
    signature: String,
    selector: [u8; 4],
    inputs: Vec<AbiType>,
    outputs: Vec<AbiType>,
    names: Vec<String>,
    mutability: StateMutability,
}
impl AbiFunction {
    /// Function name without parameter types.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Canonical function signature, excluding return types.
    pub fn signature(&self) -> &str {
        &self.signature
    }
    /// First four bytes of the canonical signature's Keccak hash.
    pub const fn selector(&self) -> [u8; 4] {
        self.selector
    }
    /// Positional input types.
    pub fn inputs(&self) -> &[AbiType] {
        &self.inputs
    }
    /// Original input names; empty names remain empty.
    pub fn input_names(&self) -> &[String] {
        &self.names
    }
    /// Positional return types.
    pub fn outputs(&self) -> &[AbiType] {
        &self.outputs
    }
    /// Declared state mutability; does not establish runtime behavior.
    pub const fn state_mutability(&self) -> StateMutability {
        self.mutability
    }
    /// Encode selector-prefixed call data with positional arguments.
    pub fn encode_call(&self, values: &[Value]) -> Result<Vec<u8>, AbiError> {
        with_selector(self.selector, AbiCoder::encode(&self.inputs, values)?)
    }
    /// Verify the selector and decode all call arguments canonically.
    pub fn decode_call(&self, data: &[u8]) -> Result<Vec<Value>, AbiError> {
        AbiCoder::decode(&self.inputs, strip_selector(self.selector, data)?)
    }
    /// Encode a canonical return-value sequence.
    pub fn encode_returns(&self, values: &[Value]) -> Result<Vec<u8>, AbiError> {
        AbiCoder::encode(&self.outputs, values)
    }
    /// Decode the complete canonical return data.
    pub fn decode_returns(&self, data: &[u8]) -> Result<Vec<Value>, AbiError> {
        AbiCoder::decode(&self.outputs, data)
    }
}

/// A declared custom error; matching bytes do not authenticate their origin.
#[derive(Clone, Debug)]
pub struct AbiCustomError {
    name: String,
    signature: String,
    selector: [u8; 4],
    inputs: Vec<AbiType>,
}
impl AbiCustomError {
    /// Declared error name.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Canonical error signature.
    pub fn signature(&self) -> &str {
        &self.signature
    }
    /// Four-byte selector; collisions require explicit declaration selection.
    pub const fn selector(&self) -> [u8; 4] {
        self.selector
    }
    /// Positional error fields.
    pub fn inputs(&self) -> &[AbiType] {
        &self.inputs
    }
    /// Encode selector-prefixed custom-error data.
    pub fn encode(&self, values: &[Value]) -> Result<Vec<u8>, AbiError> {
        with_selector(self.selector, AbiCoder::encode(&self.inputs, values)?)
    }
    /// Check the selector and decode all fields.
    pub fn decode(&self, data: &[u8]) -> Result<Vec<Value>, AbiError> {
        AbiCoder::decode(&self.inputs, strip_selector(self.selector, data)?)
    }
}

/// An event field decoded from data/a static topic, or an unrecoverable topic hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AbiEventValue {
    /// Fully decoded value in its declared positional slot.
    Value(Value),
    /// Indexed string, bytes, array or tuple represented only by its topic hash.
    IndexedHash(Hash32),
}

/// A compiled event declaration, including indexed-field positions.
#[derive(Clone, Debug)]
pub struct AbiEvent {
    name: String,
    signature: String,
    topic: Hash32,
    inputs: Vec<AbiType>,
    indexed: Vec<bool>,
    anonymous: bool,
}
impl AbiEvent {
    /// Declared event name.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Canonical event signature; indexed flags are not part of it.
    pub fn signature(&self) -> &str {
        &self.signature
    }
    /// Signature hash. Anonymous logs do not include this topic automatically.
    pub const fn topic_hash(&self) -> Hash32 {
        self.topic
    }
    /// All event argument types in declaration order.
    pub fn inputs(&self) -> &[AbiType] {
        &self.inputs
    }
    /// Indexed flags in the same order as the input types.
    pub fn indexed(&self) -> &[bool] {
        &self.indexed
    }
    /// Whether the signature hash is omitted from emitted topics.
    pub const fn anonymous(&self) -> bool {
        self.anonymous
    }
    /// Encode topics and non-indexed data for all positional arguments.
    pub fn encode_log(&self, values: &[Value]) -> Result<(Vec<Hash32>, Vec<u8>), AbiError> {
        // Validate the total input budget before partitioning or cloning values.
        crate::codec::validate(&self.inputs, values)?;
        let mut topics = Vec::new();
        let mut types = Vec::new();
        let mut data = Vec::new();
        if !self.anonymous {
            topics.push(self.topic);
        }
        for ((ty, indexed), value) in self.inputs.iter().zip(&self.indexed).zip(values) {
            if *indexed {
                topics.push(indexed_event_topic(ty, value)?);
            } else {
                types.push(ty.clone());
                data.push(value.clone());
            }
        }
        Ok((topics, AbiCoder::encode(&types, &data)?))
    }
    /// Verify exact topic count/topic0 and decode the complete canonical data.
    /// Indexed compound values remain hashes rather than fabricated values.
    pub fn decode_log(
        &self,
        topics: &[Hash32],
        data: &[u8],
    ) -> Result<Vec<AbiEventValue>, AbiError> {
        let expected = self.indexed.iter().filter(|b| **b).count() + usize::from(!self.anonymous);
        if topics.len() != expected || (!self.anonymous && topics.first() != Some(&self.topic)) {
            return Err(AbiError::Selector);
        }
        let types: Vec<_> = self
            .inputs
            .iter()
            .zip(&self.indexed)
            .filter(|(_, indexed)| !**indexed)
            .map(|(t, _)| t.clone())
            .collect();
        let mut decoded = AbiCoder::decode(&types, data)?.into_iter();
        let mut topic = usize::from(!self.anonymous);
        let mut result = Vec::with_capacity(self.inputs.len());
        for (ty, indexed) in self.inputs.iter().zip(&self.indexed) {
            if *indexed {
                let word = topics[topic];
                topic += 1;
                let compound = matches!(
                    ty.kind,
                    crate::abi_type::Kind::Array(_, _) | crate::abi_type::Kind::Tuple(_)
                ) || ty.dynamic;
                result.push(if compound {
                    AbiEventValue::IndexedHash(word)
                } else {
                    AbiEventValue::Value(
                        AbiCoder::decode(std::slice::from_ref(ty), word.bytes())?.remove(0),
                    )
                });
            } else {
                result.push(AbiEventValue::Value(
                    decoded.next().ok_or(AbiError::Encoding)?,
                ));
            }
        }
        Ok(result)
    }
}

/// Constructor arguments, encoded without a function selector.
#[derive(Clone, Debug)]
pub struct AbiConstructor {
    inputs: Vec<AbiType>,
    mutability: StateMutability,
}
impl AbiConstructor {
    /// Constructor input types in declaration order.
    pub fn inputs(&self) -> &[AbiType] {
        &self.inputs
    }
    /// Whether the declaration permits value with deployment.
    pub const fn state_mutability(&self) -> StateMutability {
        self.mutability
    }
    /// Encode arguments to append to deployment bytecode.
    pub fn encode_arguments(&self, values: &[Value]) -> Result<Vec<u8>, AbiError> {
        AbiCoder::encode(&self.inputs, values)
    }
}

/// Bounded validated JSON ABI. Name/selector collisions never silently choose a declaration.
#[derive(Clone, Debug, Default)]
pub struct AbiInterface {
    pub(crate) declarations: Vec<Value>,
    functions: BTreeMap<String, AbiFunction>,
    errors: BTreeMap<String, AbiCustomError>,
    events: BTreeMap<String, AbiEvent>,
    constructor: Option<AbiConstructor>,
    fallback: Option<StateMutability>,
    receive: bool,
}
impl AbiInterface {
    /// Parse a standard JSON ABI array, rejecting duplicates and unknown shapes.
    /// Human-readable string fragments are not accepted by this entry point.
    pub fn from_json(bytes: &[u8]) -> Result<Self, AbiError> {
        let document = crate::document::parse_json(bytes).map_err(|e| {
            if e == TypedDataError::Limit {
                AbiError::Limit
            } else {
                AbiError::Schema
            }
        })?;
        let entries = document.as_array().ok_or(AbiError::Schema)?;
        if entries.len() > MAX_FIELDS {
            return Err(AbiError::Limit);
        }
        let mut result = Self::default();
        for entry in entries {
            let object = entry.as_object().ok_or(AbiError::Schema)?;
            let kind = text(object, "type")?;
            match kind {
                "function" => {
                    keys(
                        object,
                        &[
                            "type",
                            "name",
                            "inputs",
                            "outputs",
                            "stateMutability",
                            "constant",
                            "payable",
                        ],
                    )?;
                    let name = text(object, "name")?.to_owned();
                    let input = params(object.get("inputs"), false, 0)?;
                    let output = params(object.get("outputs"), false, 0)?;
                    let signature = signature(&name, &input.types)?;
                    let selector = selector_of(&signature);
                    let item = AbiFunction {
                        name,
                        signature: signature.clone(),
                        selector,
                        inputs: input.types,
                        outputs: output.types,
                        names: input.names,
                        mutability: mutability(object)?,
                    };
                    if result.functions.insert(signature, item).is_some() {
                        return Err(AbiError::Ambiguous);
                    }
                }
                "error" => {
                    keys(object, &["type", "name", "inputs"])?;
                    let name = text(object, "name")?.to_owned();
                    let input = params(object.get("inputs"), false, 0)?;
                    let signature = signature(&name, &input.types)?;
                    let selector = selector_of(&signature);
                    if result
                        .errors
                        .insert(
                            signature.clone(),
                            AbiCustomError {
                                name,
                                signature,
                                selector,
                                inputs: input.types,
                            },
                        )
                        .is_some()
                    {
                        return Err(AbiError::Ambiguous);
                    }
                }
                "event" => {
                    keys(object, &["type", "name", "inputs", "anonymous"])?;
                    let name = text(object, "name")?.to_owned();
                    let input = params(object.get("inputs"), true, 0)?;
                    let anonymous = boolean(object, "anonymous")?.unwrap_or(false);
                    if input.indexed.iter().filter(|b| **b).count() > if anonymous { 4 } else { 3 }
                    {
                        return Err(AbiError::Schema);
                    }
                    let signature = signature(&name, &input.types)?;
                    let topic = keccak(signature.as_bytes()).into();
                    if result
                        .events
                        .insert(
                            signature.clone(),
                            AbiEvent {
                                name,
                                signature,
                                topic,
                                inputs: input.types,
                                indexed: input.indexed,
                                anonymous,
                            },
                        )
                        .is_some()
                    {
                        return Err(AbiError::Ambiguous);
                    }
                }
                "constructor" => {
                    keys(object, &["type", "inputs", "stateMutability", "payable"])?;
                    let inputs = params(object.get("inputs"), false, 0)?.types;
                    let state = mutability(object)?;
                    if matches!(state, StateMutability::Pure | StateMutability::View)
                        || result.constructor.is_some()
                    {
                        return Err(AbiError::Schema);
                    }
                    result.constructor = Some(AbiConstructor {
                        inputs,
                        mutability: state,
                    });
                }
                "fallback" | "receive" => {
                    keys(object, &["type", "stateMutability", "payable"])?;
                    let state = mutability(object)?;
                    if matches!(state, StateMutability::Pure | StateMutability::View) {
                        return Err(AbiError::Schema);
                    }
                    if kind == "fallback" {
                        if result.fallback.replace(state).is_some() {
                            return Err(AbiError::Schema);
                        }
                    } else {
                        if result.receive || state != StateMutability::Payable {
                            return Err(AbiError::Schema);
                        }
                        result.receive = true;
                    }
                }
                _ => return Err(AbiError::Schema),
            }
        }
        result.declarations = entries.clone();
        Ok(result)
    }
    /// Look up an unambiguous function name or canonicalizable signature.
    pub fn function(&self, name: &str) -> Result<&AbiFunction, AbiError> {
        lookup(&self.functions, name, |f| &f.name)
    }
    /// Look up an unambiguous custom-error name or signature.
    pub fn error(&self, name: &str) -> Result<&AbiCustomError, AbiError> {
        lookup(&self.errors, name, |f| &f.name)
    }
    /// Look up an unambiguous event name or signature.
    pub fn event(&self, name: &str) -> Result<&AbiEvent, AbiError> {
        lookup(&self.events, name, |f| &f.name)
    }
    /// Match a function selector; selector collisions are reported as ambiguous.
    pub fn function_by_selector(&self, selector: [u8; 4]) -> Result<&AbiFunction, AbiError> {
        unique(self.functions.values().filter(|f| f.selector == selector))
    }
    /// Match an error selector; matching bytes do not authenticate their source.
    pub fn error_by_selector(&self, selector: [u8; 4]) -> Result<&AbiCustomError, AbiError> {
        unique(self.errors.values().filter(|f| f.selector == selector))
    }
    /// Match an event signature hash.
    pub fn event_by_topic(&self, topic: Hash32) -> Result<&AbiEvent, AbiError> {
        unique(self.events.values().filter(|f| f.topic == topic))
    }
    /// Borrow the explicit constructor, if present.
    pub const fn constructor(&self) -> Option<&AbiConstructor> {
        self.constructor.as_ref()
    }
    /// Fallback declaration's mutability, if present.
    pub const fn fallback_mutability(&self) -> Option<StateMutability> {
        self.fallback
    }
    /// Whether a payable receive declaration is present.
    pub const fn has_receive(&self) -> bool {
        self.receive
    }
    /// Iterate all function declarations by canonical signature.
    pub fn functions(&self) -> impl Iterator<Item = &AbiFunction> {
        self.functions.values()
    }
    /// Iterate all error declarations by canonical signature.
    pub fn errors(&self) -> impl Iterator<Item = &AbiCustomError> {
        self.errors.values()
    }
    /// Iterate all event declarations by canonical signature.
    pub fn events(&self) -> impl Iterator<Item = &AbiEvent> {
        self.events.values()
    }
}

/// Canonicalize and hash a function/error/event signature, normalizing aliases.
pub fn signature_hash(text: &str) -> Result<Hash32, AbiError> {
    Ok(keccak(canonical_signature(text)?.as_bytes()).into())
}
/// Canonical function/error selector derived from its signature.
pub fn function_selector(text: &str) -> Result<[u8; 4], AbiError> {
    let hash = signature_hash(text)?;
    hash.bytes()[..4].try_into().map_err(|_| AbiError::Schema)
}
fn selector_of(signature: &str) -> [u8; 4] {
    let hash = keccak(signature.as_bytes());
    [hash[0], hash[1], hash[2], hash[3]]
}
fn identifier(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.bytes().enumerate().all(|(i, b)| {
            b.is_ascii_alphabetic() || b == b'_' || b == b'$' || (i > 0 && b.is_ascii_digit())
        })
}
fn signature(name: &str, types: &[AbiType]) -> Result<String, AbiError> {
    if !identifier(name) {
        return Err(AbiError::Schema);
    }
    let mut result = format!("{name}(");
    for (i, ty) in types.iter().enumerate() {
        let part = ty.canonical_name();
        if result.len() + part.len() + 2 > 4096 {
            return Err(AbiError::Limit);
        }
        if i > 0 {
            result.push(',');
        }
        result.push_str(&part);
    }
    result.push(')');
    if result.len() > crate::MAX_SCHEMA_BYTES {
        return Err(AbiError::Limit);
    }
    Ok(result)
}
fn canonical_signature(text: &str) -> Result<String, AbiError> {
    if text.len() > 4096 {
        return Err(AbiError::Limit);
    }
    let start = text.find('(').ok_or(AbiError::Schema)?;
    let ty = AbiType::parse(&text[start..])?;
    let crate::abi_type::Kind::Tuple(fields) = ty.kind else {
        return Err(AbiError::Schema);
    };
    signature(text[..start].trim(), &fields)
}
fn unique<'a, T: 'a>(mut items: impl Iterator<Item = &'a T>) -> Result<&'a T, AbiError> {
    let first = items.next().ok_or(AbiError::NotFound)?;
    if items.next().is_some() {
        Err(AbiError::Ambiguous)
    } else {
        Ok(first)
    }
}
fn lookup<'a, T>(
    items: &'a BTreeMap<String, T>,
    name: &str,
    get_name: impl Fn(&T) -> &str,
) -> Result<&'a T, AbiError> {
    if name.contains('(') {
        items
            .get(&canonical_signature(name)?)
            .ok_or(AbiError::NotFound)
    } else {
        unique(items.values().filter(|item| get_name(item) == name))
    }
}
fn with_selector(selector: [u8; 4], data: Vec<u8>) -> Result<Vec<u8>, AbiError> {
    let mut result = Vec::with_capacity(4 + data.len());
    result.extend_from_slice(&selector);
    result.extend_from_slice(&data);
    Ok(result)
}
fn strip_selector(selector: [u8; 4], data: &[u8]) -> Result<&[u8], AbiError> {
    if data.get(..4) != Some(selector.as_slice()) {
        return Err(AbiError::Selector);
    }
    Ok(&data[4..])
}
fn keys(object: &Map<String, Value>, allowed: &[&str]) -> Result<(), AbiError> {
    if object.keys().any(|k| !allowed.contains(&k.as_str())) {
        Err(AbiError::Schema)
    } else {
        Ok(())
    }
}
fn text<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a str, AbiError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or(AbiError::Schema)
}
fn boolean(object: &Map<String, Value>, key: &str) -> Result<Option<bool>, AbiError> {
    object
        .get(key)
        .map(|v| v.as_bool().ok_or(AbiError::Schema))
        .transpose()
}
pub(crate) fn mutability(object: &Map<String, Value>) -> Result<StateMutability, AbiError> {
    let payable = boolean(object, "payable")?;
    let constant = boolean(object, "constant")?;
    let state = match object.get("stateMutability") {
        Some(v) => match v.as_str() {
            Some("pure") => StateMutability::Pure,
            Some("view") => StateMutability::View,
            Some("nonpayable") => StateMutability::Nonpayable,
            Some("payable") => StateMutability::Payable,
            _ => return Err(AbiError::Schema),
        },
        None => {
            if payable == Some(true) {
                StateMutability::Payable
            } else if constant == Some(true) {
                StateMutability::View
            } else {
                StateMutability::Nonpayable
            }
        }
    };
    if payable.is_some_and(|p| p != (state == StateMutability::Payable))
        || constant
            .is_some_and(|c| c != matches!(state, StateMutability::Pure | StateMutability::View))
    {
        return Err(AbiError::Schema);
    }
    Ok(state)
}
struct Parameters {
    types: Vec<AbiType>,
    names: Vec<String>,
    indexed: Vec<bool>,
}
fn params(value: Option<&Value>, event: bool, depth: usize) -> Result<Parameters, AbiError> {
    if depth > MAX_DEPTH {
        return Err(AbiError::Limit);
    }
    let empty = Vec::new();
    let values = if let Some(v) = value {
        v.as_array().ok_or(AbiError::Schema)?
    } else {
        &empty
    };
    if values.len() > MAX_FIELDS {
        return Err(AbiError::Limit);
    }
    let mut result = Parameters {
        types: Vec::new(),
        names: Vec::new(),
        indexed: Vec::new(),
    };
    let mut names = BTreeSet::new();
    for value in values {
        let p = value.as_object().ok_or(AbiError::Schema)?;
        keys(
            p,
            &["type", "name", "components", "internalType", "indexed"],
        )?;
        let name = if p.contains_key("name") {
            text(p, "name")?
        } else {
            ""
        };
        if !name.is_empty() && (!identifier(name) || !names.insert(name.to_owned())) {
            return Err(AbiError::Schema);
        }
        if p.get("internalType").is_some_and(|v| !v.is_string()) {
            return Err(AbiError::Schema);
        }
        let indexed = boolean(p, "indexed")?;
        if !event && indexed.is_some() {
            return Err(AbiError::Schema);
        }
        let text = text(p, "type")?;
        let ty = if let Some(suffix) = text.strip_prefix("tuple") {
            if !suffix.is_empty() && !suffix.starts_with('[') {
                return Err(AbiError::Schema);
            }
            let components = params(
                Some(p.get("components").ok_or(AbiError::Schema)?),
                false,
                depth + 1,
            )?;
            let inner = AbiType::tuple(components.types)?;
            AbiType::parse(&format!("{inner}{suffix}"))?
        } else {
            if p.contains_key("components") {
                return Err(AbiError::Schema);
            }
            AbiType::parse(text)?
        };
        result.types.push(ty);
        result.names.push(name.into());
        result.indexed.push(indexed.unwrap_or(false));
    }
    // Validate the combined expanded shape without cloning the type trees.
    let minimum = result.types.iter().try_fold(0usize, |sum, t| {
        sum.checked_add(t.minimum_nodes).ok_or(AbiError::Limit)
    })?;
    if minimum > crate::MAX_VALUE_NODES {
        return Err(AbiError::Limit);
    }
    Ok(result)
}
