use crate::{AbiError, AbiType, MAX_DATA_BYTES, MAX_DEPTH, MAX_VALUE_NODES, abi_type::Kind};
use serde_json::{Value, json};

/// ABI fragment and parameter formatting, with explicit names and indexed-field policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbiFormat {
    /// Canonical selector type without names or indexed flags.
    Signature,
    /// Human-readable type, nested names and indexed flags.
    Full,
    /// Human-readable type and indexed flags without names.
    Minimal,
    /// Standard JSON parameter metadata, including retained internalType.
    Json,
}

/// Validated parameter tree for binding generation and tuple/array traversal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbiParameter {
    ty: AbiType,
    name: String,
    indexed: Option<bool>,
    internal_type: Option<String>,
    components: Vec<Self>,
    child: Option<Box<Self>>,
}
impl AbiParameter {
    pub(crate) fn new(
        ty: AbiType,
        name: String,
        indexed: Option<bool>,
        internal_type: Option<String>,
        components: Vec<Self>,
    ) -> Self {
        let (child, components) = if let Kind::Array(inner, _) = &ty.kind {
            (
                Some(Box::new(Self::new(
                    (**inner).clone(),
                    String::new(),
                    None,
                    None,
                    components,
                ))),
                Vec::new(),
            )
        } else {
            (None, components)
        };
        Self {
            ty,
            name,
            indexed,
            internal_type,
            components,
            child,
        }
    }
    /// Parse one standard JSON parameter. `allow_indexed` is for event fields.
    /// Uses the same duplicate-key, schema, depth and size limits as JSON interfaces.
    pub fn from_json(bytes: &[u8], allow_indexed: bool) -> Result<Self, AbiError> {
        let value = crate::document::parse_json(bytes).map_err(|e| {
            if e == crate::TypedDataError::Limit {
                AbiError::Limit
            } else {
                AbiError::Schema
            }
        })?;
        let values = Value::Array(vec![value]);
        Ok(crate::interface::params(Some(&values), allow_indexed, 0)?
            .parameters
            .remove(0))
    }
    /// Parse one human-readable parameter using the validated fragment parser.
    pub fn from_human_readable(text: &str, allow_indexed: bool) -> Result<Self, AbiError> {
        let values = Value::Array(vec![crate::human::parameter(text, allow_indexed)?]);
        Ok(crate::interface::params(Some(&values), allow_indexed, 0)?
            .parameters
            .remove(0))
    }

    /// Full canonical ABI type.
    pub fn abi_type(&self) -> &AbiType {
        &self.ty
    }
    /// Declared name, empty when unnamed.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Optional indexed flag. `Some(false)` preserves explicit event metadata.
    pub const fn indexed(&self) -> Option<bool> {
        self.indexed
    }
    /// Optional compiler internal type; metadata, not a second encoding schema.
    pub fn internal_type(&self) -> Option<&str> {
        self.internal_type.as_deref()
    }
    /// Whether this node is an array.
    pub fn is_array(&self) -> bool {
        self.child.is_some()
    }
    /// Whether this node is a tuple.
    pub fn is_tuple(&self) -> bool {
        matches!(self.ty.kind, Kind::Tuple(_))
    }
    /// Array length: outer None is non-array; inner None is dynamic length.
    pub fn array_length(&self) -> Option<Option<usize>> {
        if let Kind::Array(_, n) = self.ty.kind {
            Some(n)
        } else {
            None
        }
    }
    /// Element parameter; its name and indexed/internalType metadata are empty.
    pub fn array_child(&self) -> Option<&Self> {
        self.child.as_deref()
    }
    /// Tuple components in declaration order; empty for every non-tuple node.
    pub fn components(&self) -> &[Self] {
        &self.components
    }
    /// Format validated metadata. JSON retains explicit false indexed flags and
    /// internal types; array indexed flags are not lost as in the published formatter.
    pub fn format(&self, style: AbiFormat) -> Result<String, AbiError> {
        let text = if style == AbiFormat::Json {
            self.json().to_string()
        } else if style == AbiFormat::Signature {
            self.ty.canonical_name()
        } else {
            self.human(style == AbiFormat::Minimal)
        };
        if text.len() > MAX_DATA_BYTES {
            return Err(AbiError::Limit);
        }
        Ok(text)
    }
    pub(crate) fn json(&self) -> Value {
        let mut value = if let Some(child) = &self.child {
            let mut value = child.json();
            value["type"] = json!(format!(
                "{}[{}]",
                value["type"].as_str().unwrap(),
                self.array_length()
                    .flatten()
                    .map_or(String::new(), |n| n.to_string())
            ));
            value
        } else if self.is_tuple() {
            json!({"type":"tuple", "components":self.components.iter().map(Self::json).collect::<Vec<_>>()})
        } else {
            json!({"type":self.ty.canonical_name()})
        };
        value["name"] = json!(self.name);
        if let Some(indexed) = self.indexed {
            value["indexed"] = json!(indexed);
        }
        if let Some(internal) = &self.internal_type {
            value["internalType"] = json!(internal);
        }
        value
    }
    fn human(&self, minimal: bool) -> String {
        let mut value = if let Some(child) = &self.child {
            format!(
                "{}[{}]",
                child.human(minimal),
                self.array_length()
                    .flatten()
                    .map_or(String::new(), |n| n.to_string())
            )
        } else if self.is_tuple() {
            format!(
                "({})",
                self.components
                    .iter()
                    .map(|p| p.human(minimal))
                    .collect::<Vec<_>>()
                    .join(if minimal { "," } else { ", " })
            )
        } else {
            self.ty.canonical_name()
        };
        if self.indexed == Some(true) {
            value.push_str(" indexed");
        }
        if !minimal && !self.name.is_empty() {
            value.push(' ');
            value.push_str(&self.name);
        }
        value
    }
    /// Visit primitive leaves in declaration/index order and return positional
    /// arrays. Tuples accept positional arrays or exact objects with named fields.
    /// Shape validation finishes before callbacks run. Leaf transformations need
    /// not be ABI values; the resulting JSON still obeys global size/depth limits.
    /// Callbacks may have side effects; an error does not roll those back.
    pub fn walk<F>(&self, value: &Value, mut process: F) -> Result<Value, AbiError>
    where
        F: FnMut(&AbiType, &Value) -> Result<Value, AbiError>,
    {
        let (plan, leaves, mut budget) = self.plan(value)?;
        let mut output = Vec::with_capacity(leaves.len());
        for (ty, value, depth) in leaves {
            let transformed = process(ty, value)?;
            budget.value(&transformed, depth)?;
            output.push(Some(transformed));
        }
        Ok(plan.finish(&mut output))
    }
    /// Async leaf traversal in deterministic order, without Send or an executor
    /// requirement. Futures run sequentially; dropping this future stops further
    /// callbacks, but does not undo work already performed by a callback.
    pub async fn walk_async<'a, F, Fut>(
        &'a self,
        value: &'a Value,
        mut process: F,
    ) -> Result<Value, AbiError>
    where
        F: FnMut(&'a AbiType, &'a Value) -> Fut,
        Fut: std::future::Future<Output = Result<Value, AbiError>>,
    {
        let (plan, leaves, mut budget) = self.plan(value)?;
        let mut output = Vec::with_capacity(leaves.len());
        for (ty, value, depth) in leaves {
            let transformed = process(ty, value).await?;
            budget.value(&transformed, depth)?;
            output.push(Some(transformed));
        }
        Ok(plan.finish(&mut output))
    }
    fn plan<'a>(&'a self, value: &'a Value) -> Result<(Plan, Vec<Leaf<'a>>, JsonBudget), AbiError> {
        JsonBudget::default().value(value, 0)?;
        let mut leaves = Vec::new();
        let plan = self.shape(value, 0, &mut leaves)?;
        let mut budget = JsonBudget::default();
        plan.containers(&mut budget, 0)?;
        Ok((plan, leaves, budget))
    }
    fn shape<'a>(
        &'a self,
        value: &'a Value,
        depth: usize,
        leaves: &mut Vec<Leaf<'a>>,
    ) -> Result<Plan, AbiError> {
        if let Some(child) = &self.child {
            let items = value.as_array().ok_or(AbiError::Value)?;
            if self
                .array_length()
                .flatten()
                .is_some_and(|n| n != items.len())
            {
                return Err(AbiError::Value);
            }
            return Ok(Plan::Array(
                items
                    .iter()
                    .map(|v| child.shape(v, depth + 1, leaves))
                    .collect::<Result<_, _>>()?,
            ));
        }
        if self.is_tuple() {
            let values: Vec<&Value> = if let Some(items) = value.as_array() {
                if items.len() != self.components.len() {
                    return Err(AbiError::Value);
                }
                items.iter().collect()
            } else if let Some(items) = value.as_object() {
                if items.len() != self.components.len() {
                    return Err(AbiError::Value);
                }
                self.components
                    .iter()
                    .map(|p| {
                        if p.name.is_empty() {
                            Err(AbiError::Value)
                        } else {
                            items.get(&p.name).ok_or(AbiError::Value)
                        }
                    })
                    .collect::<Result<_, _>>()?
            } else {
                return Err(AbiError::Value);
            };
            return Ok(Plan::Array(
                self.components
                    .iter()
                    .zip(values)
                    .map(|(p, v)| p.shape(v, depth + 1, leaves))
                    .collect::<Result<_, _>>()?,
            ));
        }
        let index = leaves.len();
        leaves.push((&self.ty, value, depth));
        Ok(Plan::Leaf(index))
    }
}
type Leaf<'a> = (&'a AbiType, &'a Value, usize);
enum Plan {
    Array(Vec<Self>),
    Leaf(usize),
}
impl Plan {
    fn containers(&self, budget: &mut JsonBudget, depth: usize) -> Result<(), AbiError> {
        if let Self::Array(items) = self {
            budget.node(depth)?;
            for item in items {
                item.containers(budget, depth + 1)?;
            }
        }
        Ok(())
    }
    fn finish(self, output: &mut [Option<Value>]) -> Value {
        match self {
            Self::Array(items) => {
                Value::Array(items.into_iter().map(|p| p.finish(output)).collect())
            }
            Self::Leaf(i) => output[i].take().unwrap(),
        }
    }
}
#[derive(Default)]
pub(crate) struct JsonBudget {
    nodes: usize,
    bytes: usize,
}
impl JsonBudget {
    pub(crate) fn node(&mut self, depth: usize) -> Result<(), AbiError> {
        if depth > MAX_DEPTH || self.nodes == MAX_VALUE_NODES {
            return Err(AbiError::Limit);
        }
        self.nodes += 1;
        Ok(())
    }
    pub(crate) fn text(&mut self, n: usize) -> Result<(), AbiError> {
        self.bytes = self.bytes.checked_add(n).ok_or(AbiError::Limit)?;
        if self.bytes > MAX_DATA_BYTES {
            return Err(AbiError::Limit);
        }
        Ok(())
    }
    pub(crate) fn value(&mut self, value: &Value, depth: usize) -> Result<(), AbiError> {
        self.node(depth)?;
        match value {
            Value::String(s) => self.text(s.len())?,
            Value::Array(items) => {
                for value in items {
                    self.value(value, depth + 1)?;
                }
            }
            Value::Object(items) => {
                for (key, value) in items {
                    self.text(key.len())?;
                    self.value(value, depth + 1)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}
