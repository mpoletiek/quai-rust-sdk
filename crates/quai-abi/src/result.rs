use crate::{AbiError, MAX_FIELDS, MAX_SCHEMA_BYTES};
use std::collections::BTreeMap;

/// Ordered decoded fields with optional unique names. Values are eagerly decoded;
/// there are no deferred errors, proxy properties or method-name collisions.
/// Nested ABI tuples remain positional values; use parameter metadata to interpret them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbiResult<T = serde_json::Value> {
    values: Vec<T>,
    names: Vec<Option<String>>,
}
impl<T> AbiResult<T> {
    /// Attach names to already decoded values. Empty and duplicate names become
    /// unnamed, matching published Result lookup. At most 1,024 fields and 65,536
    /// name bytes; this generic container does not limit the heap size of `T`.
    pub fn from_items(values: Vec<T>, names: Vec<Option<String>>) -> Result<Self, AbiError> {
        if values.len() != names.len() {
            return Err(AbiError::Value);
        }
        if values.len() > MAX_FIELDS
            || names
                .iter()
                .try_fold(0usize, |n, s| {
                    n.checked_add(s.as_ref().map_or(0, String::len))
                })
                .is_none_or(|n| n > MAX_SCHEMA_BYTES)
        {
            return Err(AbiError::Limit);
        }
        let mut counts = BTreeMap::new();
        for name in names.iter().flatten().filter(|s| !s.is_empty()) {
            *counts.entry(name.as_str()).or_insert(0usize) += 1;
        }
        let unique: Vec<_> = names
            .iter()
            .map(|s| {
                s.as_ref()
                    .is_some_and(|s| counts.get(s.as_str()) == Some(&1))
            })
            .collect();
        let names = names
            .into_iter()
            .zip(unique)
            .map(|(s, unique)| if unique { s } else { None })
            .collect();
        Ok(Self { values, names })
    }
    pub(crate) fn decoded(values: Vec<T>, names: &[String]) -> Result<Self, AbiError> {
        Self::from_items(values, names.iter().cloned().map(Some).collect())
    }
    /// Ordered values; standard Rust slice iteration, filtering and indexing apply.
    pub fn values(&self) -> &[T] {
        &self.values
    }
    /// Unique names aligned with the values; unnamed fields are `None`.
    pub fn names(&self) -> &[Option<String>] {
        &self.names
    }
    /// Exact case-sensitive name lookup, including names such as `length` or `then`.
    pub fn get_value(&self, name: &str) -> Option<&T> {
        self.names
            .iter()
            .position(|n| n.as_deref() == Some(name))
            .map(|i| &self.values[i])
    }
    /// Consume this result into its positional values.
    pub fn into_values(self) -> Vec<T> {
        self.values
    }
    /// Borrow an object view only when every field has a unique nonempty name.
    /// Rust map keys cannot collide with inherited JavaScript object properties.
    pub fn to_object(&self) -> Result<BTreeMap<&str, &T>, AbiError> {
        self.names
            .iter()
            .zip(&self.values)
            .map(|(name, value)| Ok((name.as_deref().ok_or(AbiError::Value)?, value)))
            .collect()
    }
    /// Copy selected fields in order, retaining their original unique names.
    pub fn filter(&self, mut predicate: impl FnMut(usize, &T) -> bool) -> Self
    where
        T: Clone,
    {
        let mut values = Vec::new();
        let mut names = Vec::new();
        for (index, value) in self.values.iter().enumerate() {
            if predicate(index, value) {
                values.push(value.clone());
                names.push(self.names[index].clone());
            }
        }
        Self { values, names }
    }
    /// Copy a checked half-open range while retaining the existing unique names.
    pub fn slice(&self, range: std::ops::Range<usize>) -> Result<Self, AbiError>
    where
        T: Clone,
    {
        let values = self
            .values
            .get(range.clone())
            .ok_or(AbiError::Value)?
            .to_vec();
        Ok(Self {
            values,
            names: self.names[range].to_vec(),
        })
    }
}
