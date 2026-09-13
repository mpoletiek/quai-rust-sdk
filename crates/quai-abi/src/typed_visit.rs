use crate::{
    TypedDataEncoder, TypedDataError,
    parameter::JsonBudget,
    schema::{Base, Field, expression},
};
use serde_json::Value;

/// A validated EIP-712 type expression bound to an immutable compiled schema.
/// Unlike an ABI encoder, bytes/strings and arrays produce a hash word.
#[derive(Clone, Debug)]
pub struct TypedValueEncoder<'a> {
    encoder: &'a TypedDataEncoder,
    field: Field,
}
impl TypedValueEncoder<'_> {
    /// Encode the complete value. Structs include their type hash and field words;
    /// primitives and arrays produce exactly one 32-byte word. Full JSON shape,
    /// numeric, byte, address and resource checks precede a successful result.
    pub fn encode(&self, value: &Value) -> Result<Vec<u8>, TypedDataError> {
        crate::value::preflight(value)?;
        if self.field.dimensions.is_empty()
            && let Base::Struct(name) = &self.field.base
        {
            return self.encoder.encode_struct(name, value, 0);
        }
        Ok(self
            .encoder
            .word(&self.field, self.field.dimensions.len(), value, 0)?
            .to_vec())
    }
}
impl TypedDataEncoder {
    /// Resolve a primitive, array or declared struct expression once, returning
    /// an encoder bound to this schema. Unknown structs reject even for empty
    /// arrays. Uses the same explicit-width and array bounds as schema fields.
    pub fn encoder(&self, type_name: &str) -> Result<TypedValueEncoder<'_>, TypedDataError> {
        let (base, dimensions) = expression(type_name)?;
        if let Base::Struct(name) = &base
            && !self.structs.contains_key(name)
        {
            return Err(TypedDataError::UnknownType);
        }
        Ok(TypedValueEncoder {
            encoder: self,
            field: Field {
                name: String::new(),
                base,
                dimensions,
            },
        })
    }
    /// Transform primary-type primitive leaves while preserving named objects
    /// and array order. Complete shape validation occurs before callbacks run;
    /// leaf values are intentionally not coerced or hashed. Encode the result
    /// separately before signing. Errors do not roll back callback side effects.
    pub fn visit(
        &self,
        value: &Value,
        process: impl FnMut(&str, &Value) -> Result<Value, TypedDataError>,
    ) -> Result<Value, TypedDataError> {
        self.visit_type(&self.primary, value, process)
    }
    /// Transform a specific supported type expression, including primitive and
    /// array roots. Reject missing/extra fields and fixed-array length mismatch.
    /// Input and combined callback output obey global node/text/depth limits;
    /// caller allocations inside callbacks remain the caller's responsibility.
    pub fn visit_type(
        &self,
        type_name: &str,
        value: &Value,
        mut process: impl FnMut(&str, &Value) -> Result<Value, TypedDataError>,
    ) -> Result<Value, TypedDataError> {
        let encoder = self.encoder(type_name)?;
        crate::value::preflight(value)?;
        let mut leaves = Vec::new();
        let plan = self.visit_plan(
            &encoder.field.base,
            &encoder.field.dimensions,
            value,
            0,
            &mut leaves,
        )?;
        let mut budget = JsonBudget::default();
        plan.containers(&mut budget, 0)?;
        let mut output = Vec::with_capacity(leaves.len());
        for (base, value, depth) in leaves {
            let transformed = process(&primitive_name(base), value)?;
            budget
                .value(&transformed, depth)
                .map_err(|_| TypedDataError::Limit)?;
            output.push(Some(transformed));
        }
        Ok(plan.finish(&mut output))
    }
    fn visit_plan<'a>(
        &'a self,
        base: &'a Base,
        dimensions: &[Option<usize>],
        value: &'a Value,
        depth: usize,
        leaves: &mut Vec<Leaf<'a>>,
    ) -> Result<Plan, TypedDataError> {
        if depth > crate::MAX_DEPTH {
            return Err(TypedDataError::Limit);
        }
        if let Some((length, inner)) = dimensions.split_last() {
            let items = value.as_array().ok_or(TypedDataError::Value)?;
            if length.is_some_and(|n| n != items.len()) {
                return Err(TypedDataError::ArrayLength);
            }
            return Ok(Plan::Array(
                items
                    .iter()
                    .map(|v| self.visit_plan(base, inner, v, depth + 1, leaves))
                    .collect::<Result<_, _>>()?,
            ));
        }
        if let Base::Struct(name) = base {
            let fields = &self
                .structs
                .get(name)
                .ok_or(TypedDataError::UnknownType)?
                .fields;
            let object = value.as_object().ok_or(TypedDataError::Value)?;
            if object.len() != fields.len() {
                return Err(TypedDataError::Value);
            }
            let mut items = Vec::with_capacity(fields.len());
            for field in fields {
                let value = object.get(&field.name).ok_or(TypedDataError::Value)?;
                items.push((
                    field.name.clone(),
                    self.visit_plan(&field.base, &field.dimensions, value, depth + 1, leaves)?,
                ));
            }
            return Ok(Plan::Object(items));
        }
        let index = leaves.len();
        leaves.push((base, value, depth));
        Ok(Plan::Leaf(index))
    }
}
fn primitive_name(base: &Base) -> String {
    match base {
        Base::Int { signed, bits } => format!("{}int{bits}", if *signed { "" } else { "u" }),
        Base::FixedBytes(n) => format!("bytes{n}"),
        Base::Address => "address".into(),
        Base::Bool => "bool".into(),
        Base::Bytes => "bytes".into(),
        Base::String => "string".into(),
        Base::Struct(_) => unreachable!("visit leaves are primitive"),
    }
}
type Leaf<'a> = (&'a Base, &'a Value, usize);
enum Plan {
    Array(Vec<Self>),
    Object(Vec<(String, Self)>),
    Leaf(usize),
}
impl Plan {
    fn containers(&self, budget: &mut JsonBudget, depth: usize) -> Result<(), TypedDataError> {
        match self {
            Self::Array(items) => {
                budget.node(depth).map_err(|_| TypedDataError::Limit)?;
                for item in items {
                    item.containers(budget, depth + 1)?;
                }
            }
            Self::Object(items) => {
                budget.node(depth).map_err(|_| TypedDataError::Limit)?;
                for (key, item) in items {
                    budget.text(key.len()).map_err(|_| TypedDataError::Limit)?;
                    item.containers(budget, depth + 1)?;
                }
            }
            Self::Leaf(_) => {}
        }
        Ok(())
    }
    fn finish(self, output: &mut [Option<Value>]) -> Value {
        match self {
            Self::Array(items) => {
                Value::Array(items.into_iter().map(|v| v.finish(output)).collect())
            }
            Self::Object(items) => Value::Object(
                items
                    .into_iter()
                    .map(|(key, v)| (key, v.finish(output)))
                    .collect(),
            ),
            Self::Leaf(index) => output[index].take().unwrap(),
        }
    }
}
