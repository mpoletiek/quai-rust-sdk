use crate::{MAX_DEPTH, MAX_FIELDS, MAX_VALUE_NODES};
use std::{fmt, str::FromStr};

/// Errors in bounded canonical Solidity ABI operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum AbiError {
    /// Type expression, JSON interface or signature is malformed.
    #[error("invalid ABI type or interface")]
    Schema,
    /// Resource limits would be exceeded.
    #[error("ABI resource limit exceeded")]
    Limit,
    /// Value kind, arity, integer range or fixed length does not match its type.
    #[error("ABI value does not match its type")]
    Value,
    /// Encoded bytes are truncated, out of bounds or not canonical.
    #[error("invalid or noncanonical ABI encoding")]
    Encoding,
    /// Function/error selector or event topics do not match the selected item.
    #[error("ABI selector or topic mismatch")]
    Selector,
    /// No declaration matches the supplied name or signature.
    #[error("ABI declaration not found")]
    NotFound,
    /// A name, selector or event lookup matches multiple declarations.
    #[error("ambiguous ABI declaration")]
    Ambiguous,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Bool,
    Int(bool, usize),
    Address,
    FixedBytes(usize),
    Bytes,
    String,
    Array(Box<AbiType>, Option<usize>),
    Tuple(Vec<AbiType>),
}

/// Validated ABI type with cached size/depth metadata; construction is bounded.
///
/// Supports exact primitive names, int/uint aliases, nested tuples and arrays.
/// Solidity source parameter names and storage-location suffixes are not types.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbiType {
    pub(crate) kind: Kind,
    pub(crate) dynamic: bool,
    pub(crate) head: usize,
    pub(crate) minimum_nodes: usize,
    pub(crate) depth: usize,
}
impl AbiType {
    /// Parse a type expression of at most 4096 bytes and 1024 syntax nodes.
    pub fn parse(text: &str) -> Result<Self, AbiError> {
        if text.len() > 4096 {
            return Err(AbiError::Limit);
        }
        let mut parser = Parser {
            bytes: text.as_bytes(),
            pos: 0,
            nodes: 0,
        };
        let ty = parser.ty(0)?;
        parser.space();
        if parser.pos != parser.bytes.len() {
            return Err(AbiError::Schema);
        }
        Ok(ty)
    }
    /// Canonical signature spelling, including normalization of int/uint aliases.
    pub fn canonical_name(&self) -> String {
        self.to_string()
    }
    /// Whether this type is encoded through an offset in an enclosing sequence.
    pub const fn is_dynamic(&self) -> bool {
        self.dynamic
    }
    /// Create a tuple from already validated component types.
    pub fn tuple(fields: Vec<Self>) -> Result<Self, AbiError> {
        Self::new(Kind::Tuple(fields))
    }
    /// Create a fixed or dynamic array from an already validated element type.
    pub fn array(element: Self, length: Option<usize>) -> Result<Self, AbiError> {
        Self::new(Kind::Array(Box::new(element), length))
    }
    pub(crate) fn new(kind: Kind) -> Result<Self, AbiError> {
        let (dynamic, head, minimum_nodes, depth) = match &kind {
            Kind::Bytes | Kind::String => (true, 32, 1, 0),
            Kind::Array(child, length) => {
                if length.is_some_and(|n| n > MAX_VALUE_NODES) {
                    return Err(AbiError::Limit);
                }
                let nodes = 1usize
                    .checked_add(
                        length
                            .unwrap_or(0)
                            .checked_mul(child.minimum_nodes)
                            .ok_or(AbiError::Limit)?,
                    )
                    .ok_or(AbiError::Limit)?;
                let dynamic = length.is_none() || child.dynamic;
                let head = if dynamic {
                    32
                } else {
                    child
                        .head
                        .checked_mul(length.unwrap_or(0))
                        .ok_or(AbiError::Limit)?
                };
                (dynamic, head, nodes, child.depth + 1)
            }
            Kind::Tuple(fields) => {
                if fields.len() > MAX_FIELDS {
                    return Err(AbiError::Limit);
                }
                let dynamic = fields.iter().any(|f| f.dynamic);
                let nodes = fields.iter().try_fold(1usize, |a, f| {
                    a.checked_add(f.minimum_nodes).ok_or(AbiError::Limit)
                })?;
                let size = fields
                    .iter()
                    .try_fold(0usize, |a, f| a.checked_add(f.head).ok_or(AbiError::Limit))?;
                (
                    dynamic,
                    if dynamic { 32 } else { size },
                    nodes,
                    fields.iter().map(|f| f.depth + 1).max().unwrap_or(0),
                )
            }
            _ => (false, 32, 1, 0),
        };
        if minimum_nodes > MAX_VALUE_NODES || head > crate::MAX_DATA_BYTES || depth > MAX_DEPTH {
            return Err(AbiError::Limit);
        }
        Ok(Self {
            kind,
            dynamic,
            head,
            minimum_nodes,
            depth,
        })
    }
}
impl FromStr for AbiType {
    type Err = AbiError;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}
impl fmt::Display for AbiType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            Kind::Bool => f.write_str("bool"),
            Kind::Int(signed, bits) => write!(f, "{}int{bits}", if *signed { "" } else { "u" }),
            Kind::Address => f.write_str("address"),
            Kind::FixedBytes(n) => write!(f, "bytes{n}"),
            Kind::Bytes => f.write_str("bytes"),
            Kind::String => f.write_str("string"),
            Kind::Array(child, Some(n)) => write!(f, "{child}[{n}]"),
            Kind::Array(child, None) => write!(f, "{child}[]"),
            Kind::Tuple(fields) => {
                f.write_str("(")?;
                for (i, field) in fields.iter().enumerate() {
                    if i > 0 {
                        f.write_str(",")?;
                    }
                    write!(f, "{field}")?;
                }
                f.write_str(")")
            }
        }
    }
}
struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    nodes: usize,
}
impl Parser<'_> {
    fn space(&mut self) {
        while self
            .bytes
            .get(self.pos)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.pos += 1;
        }
    }
    fn eat(&mut self, byte: u8) -> bool {
        self.space();
        if self.bytes.get(self.pos) == Some(&byte) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn count(&mut self, depth: usize) -> Result<(), AbiError> {
        self.nodes += 1;
        if self.nodes > MAX_FIELDS || depth > MAX_DEPTH {
            Err(AbiError::Limit)
        } else {
            Ok(())
        }
    }
    fn ty(&mut self, depth: usize) -> Result<AbiType, AbiError> {
        self.count(depth)?;
        self.space();
        if self.bytes.get(self.pos..self.pos + 5) == Some(b"tuple") {
            self.pos += 5;
            self.space();
            if self.bytes.get(self.pos) != Some(&b'(') {
                return Err(AbiError::Schema);
            }
        }
        let mut value = if self.eat(b'(') {
            let mut fields = Vec::new();
            if !self.eat(b')') {
                loop {
                    fields.push(self.ty(depth + 1)?);
                    if self.eat(b')') {
                        break;
                    }
                    if !self.eat(b',') {
                        return Err(AbiError::Schema);
                    }
                }
            }
            AbiType::tuple(fields)?
        } else {
            let start = self.pos;
            while self
                .bytes
                .get(self.pos)
                .is_some_and(u8::is_ascii_alphanumeric)
            {
                self.pos += 1;
            }
            let name =
                std::str::from_utf8(&self.bytes[start..self.pos]).map_err(|_| AbiError::Schema)?;
            let kind = match name {
                "bool" => Kind::Bool,
                "address" => Kind::Address,
                "bytes" => Kind::Bytes,
                "string" => Kind::String,
                "int" => Kind::Int(true, 256),
                "uint" => Kind::Int(false, 256),
                _ => {
                    if let Some(n) = name
                        .strip_prefix("uint")
                        .or_else(|| name.strip_prefix("int"))
                    {
                        Kind::Int(!name.starts_with('u'), width(n, 256, 8)?)
                    } else if let Some(n) = name.strip_prefix("bytes") {
                        Kind::FixedBytes(width(n, 32, 1)?)
                    } else {
                        return Err(AbiError::Schema);
                    }
                }
            };
            AbiType::new(kind)?
        };
        while self.eat(b'[') {
            self.count(depth + 1)?;
            self.space();
            let start = self.pos;
            while self.bytes.get(self.pos).is_some_and(u8::is_ascii_digit) {
                self.pos += 1;
            }
            let digits =
                std::str::from_utf8(&self.bytes[start..self.pos]).map_err(|_| AbiError::Schema)?;
            let n = if digits.is_empty() {
                None
            } else {
                let n: usize = digits.parse().map_err(|_| AbiError::Limit)?;
                if digits != n.to_string() {
                    return Err(AbiError::Schema);
                }
                Some(n)
            };
            if !self.eat(b']') {
                return Err(AbiError::Schema);
            }
            value = AbiType::array(value, n)?;
        }
        Ok(value)
    }
}
fn width(text: &str, max: usize, multiple: usize) -> Result<usize, AbiError> {
    let n: usize = text.parse().map_err(|_| AbiError::Schema)?;
    if n == 0 || n > max || !n.is_multiple_of(multiple) || text != n.to_string() {
        Err(AbiError::Schema)
    } else {
        Ok(n)
    }
}
