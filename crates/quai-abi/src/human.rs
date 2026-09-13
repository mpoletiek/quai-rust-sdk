//! Bounded source-like declaration import and lossless JSON metadata export.
use crate::{
    AbiError, AbiInterface, AbiType, MAX_DATA_BYTES, MAX_DEPTH, MAX_FIELDS, MAX_SCHEMA_BYTES,
    StateMutability,
};
use serde_json::{Value, json};

impl AbiInterface {
    /// Parse explicit source-like function, event, error, constructor, fallback and
    /// receive declarations. Bare function signatures are also accepted. Supports
    /// named nested tuples/arrays, indexed event fields, returns and mutability.
    /// At most 1,024 fragments, 4,096 bytes each, 65,536 combined ASCII bytes and
    /// 1,024 parameter/array nodes per fragment. No Solidity source, comments,
    /// structs, gas annotations, function bodies or unknown modifiers are accepted.
    /// Every entry must validate; invalid fragments are never silently discarded.
    pub fn from_human_readable(fragments: &[&str]) -> Result<Self, AbiError> {
        if fragments.len() > MAX_FIELDS {
            return Err(AbiError::Limit);
        }
        let total = fragments
            .iter()
            .try_fold(0usize, |n, s| n.checked_add(s.len()).ok_or(AbiError::Limit))?;
        if total > MAX_SCHEMA_BYTES || fragments.iter().any(|s| s.len() > 4096) {
            return Err(AbiError::Limit);
        }
        let mut entries = Vec::with_capacity(fragments.len());
        for text in fragments {
            if !text.is_ascii() {
                return Err(AbiError::Schema);
            }
            let mut parser = Parser {
                text,
                pos: 0,
                nodes: 0,
            };
            entries.push(parser.fragment()?);
        }
        let bytes = serde_json::to_vec(&entries).map_err(|_| AbiError::Schema)?;
        Self::from_json(&bytes)
    }
    /// Export the validated JSON ABI in declaration order, preserving names,
    /// tuple components, internalType and explicit legacy flags. Whitespace/key
    /// order are normalized by JSON serialization, but type spelling and optional
    /// fields are retained. This is not byte-identical quais.js `formatJson` output.
    pub fn format_json(&self) -> Result<String, AbiError> {
        let text = serde_json::to_string(&self.declarations).map_err(|_| AbiError::Schema)?;
        if text.len() > MAX_DATA_BYTES {
            return Err(AbiError::Limit);
        }
        Ok(text)
    }
    /// Format each declaration in its original order. `minimal` omits parameter
    /// names and comma spacing, retaining indexed flags, returns and mutability.
    /// Full mode includes all retained tuple/input/output names. Source-only storage
    /// locations, internalType and legacy JSON flags are not printed as ABI syntax.
    /// Canonical type aliases are used; total output is capped at one MiB.
    pub fn format_human_readable(&self, minimal: bool) -> Result<Vec<String>, AbiError> {
        let mut result = Vec::with_capacity(self.declarations.len());
        let mut total = 0usize;
        for entry in &self.declarations {
            let mut out = Output(String::new());
            let object = entry.as_object().ok_or(AbiError::Schema)?;
            let kind = object
                .get("type")
                .and_then(Value::as_str)
                .ok_or(AbiError::Schema)?;
            out.push(kind)?;
            if matches!(kind, "function" | "event" | "error") {
                out.push(" ")?;
                out.push(
                    object
                        .get("name")
                        .and_then(Value::as_str)
                        .ok_or(AbiError::Schema)?,
                )?;
            }
            out.params(object.get("inputs"), minimal)?;
            if kind == "event" {
                if object.get("anonymous").and_then(Value::as_bool) == Some(true) {
                    out.push(" anonymous")?;
                }
            } else if kind != "error" {
                match crate::interface::mutability(object)? {
                    StateMutability::Pure => out.push(" pure")?,
                    StateMutability::View => out.push(" view")?,
                    StateMutability::Payable => out.push(" payable")?,
                    StateMutability::Nonpayable => {}
                }
                if let Some(Value::Array(outputs)) = object.get("outputs")
                    && !outputs.is_empty()
                {
                    out.push(" returns ")?;
                    out.params(object.get("outputs"), minimal)?;
                }
            }
            total = total.checked_add(out.0.len()).ok_or(AbiError::Limit)?;
            if total > MAX_DATA_BYTES {
                return Err(AbiError::Limit);
            }
            result.push(out.0);
        }
        Ok(result)
    }
}
struct Output(String);
impl Output {
    fn push(&mut self, s: &str) -> Result<(), AbiError> {
        if self.0.len().saturating_add(s.len()) > MAX_DATA_BYTES {
            return Err(AbiError::Limit);
        }
        self.0.push_str(s);
        Ok(())
    }
    fn params(&mut self, values: Option<&Value>, minimal: bool) -> Result<(), AbiError> {
        self.push("(")?;
        if let Some(values) = values {
            for (i, value) in values
                .as_array()
                .ok_or(AbiError::Schema)?
                .iter()
                .enumerate()
            {
                if i > 0 {
                    self.push(if minimal { "," } else { ", " })?;
                }
                self.param(value, minimal)?;
            }
        }
        self.push(")")
    }
    fn param(&mut self, value: &Value, minimal: bool) -> Result<(), AbiError> {
        let ty = value["type"].as_str().ok_or(AbiError::Schema)?;
        if let Some(suffix) = ty.strip_prefix("tuple") {
            self.params(value.get("components"), minimal)?;
            let normalized = AbiType::parse(&format!("bool{suffix}"))?.canonical_name();
            self.push(normalized.strip_prefix("bool").ok_or(AbiError::Schema)?)?;
        } else {
            self.push(&AbiType::parse(ty)?.canonical_name())?;
        }
        if value.get("indexed").and_then(Value::as_bool) == Some(true) {
            self.push(" indexed")?;
        }
        if !minimal
            && let Some(name) = value.get("name").and_then(Value::as_str)
            && !name.is_empty()
        {
            self.push(" ")?;
            self.push(name)?;
        }
        Ok(())
    }
}
struct Parser<'a> {
    text: &'a str,
    pos: usize,
    nodes: usize,
}
impl<'a> Parser<'a> {
    fn space(&mut self) {
        while self
            .text
            .as_bytes()
            .get(self.pos)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.pos += 1;
        }
    }
    fn eat(&mut self, b: u8) -> bool {
        self.space();
        if self.text.as_bytes().get(self.pos) == Some(&b) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn word(&mut self) -> Result<&'a str, AbiError> {
        self.space();
        let start = self.pos;
        while self
            .text
            .as_bytes()
            .get(self.pos)
            .is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'$'))
        {
            self.pos += 1;
        }
        if self.pos == start {
            return Err(AbiError::Schema);
        }
        Ok(&self.text[start..self.pos])
    }
    fn keyword(&mut self, w: &str) -> bool {
        let old = self.pos;
        if self.word() == Ok(w) {
            true
        } else {
            self.pos = old;
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
    fn params(&mut self, event: bool, depth: usize) -> Result<Vec<Value>, AbiError> {
        if depth > MAX_DEPTH {
            return Err(AbiError::Limit);
        }
        if !self.eat(b'(') {
            return Err(AbiError::Schema);
        }
        let mut values = vec![];
        if self.eat(b')') {
            return Ok(values);
        }
        loop {
            values.push(self.param(event, depth)?);
            if self.eat(b')') {
                break;
            }
            if !self.eat(b',') {
                return Err(AbiError::Schema);
            }
        }
        Ok(values)
    }
    fn param(&mut self, event: bool, depth: usize) -> Result<Value, AbiError> {
        self.count(depth)?;
        self.space();
        let tuple = self.keyword("tuple") || self.text.as_bytes().get(self.pos) == Some(&b'(');
        let (mut ty, components) = if tuple {
            ("tuple".to_string(), Some(self.params(false, depth + 1)?))
        } else {
            (AbiType::parse(self.word()?)?.canonical_name(), None)
        };
        // Source address payable has the same ABI type.
        if ty == "address" {
            self.keyword("payable");
        }
        while self.eat(b'[') {
            self.count(depth + 1)?;
            self.space();
            let start = self.pos;
            while self
                .text
                .as_bytes()
                .get(self.pos)
                .is_some_and(u8::is_ascii_digit)
            {
                self.pos += 1;
            }
            let size = &self.text[start..self.pos];
            if !size.is_empty() {
                let n = size.parse::<usize>().map_err(|_| AbiError::Limit)?;
                if n.to_string() != size {
                    return Err(AbiError::Schema);
                }
            }
            if !self.eat(b']') {
                return Err(AbiError::Schema);
            }
            ty.push('[');
            ty.push_str(size);
            ty.push(']');
        }
        let mut name = "";
        let mut indexed = false;
        let mut location = false;
        loop {
            self.space();
            if matches!(self.text.as_bytes().get(self.pos), Some(b',' | b')')) {
                break;
            }
            let word = self.word()?;
            match word {
                "indexed" if event && !indexed && name.is_empty() => indexed = true,
                "memory" | "calldata" | "storage" if !event && !location && name.is_empty() => {
                    location = true
                }
                "indexed" | "memory" | "calldata" | "storage" | "payable" => {
                    return Err(AbiError::Schema);
                }
                _ if name.is_empty() => name = word,
                _ => return Err(AbiError::Schema),
            }
        }
        let mut value = json!({"type":ty,"name":name});
        if event {
            value["indexed"] = json!(indexed);
        }
        if let Some(components) = components {
            value["components"] = json!(components);
        }
        Ok(value)
    }
    fn fragment(&mut self) -> Result<Value, AbiError> {
        let first = self.word()?;
        let kind = match first {
            "function" | "event" | "error" | "constructor" | "fallback" | "receive" => first,
            _ => "function",
        };
        let name = if matches!(kind, "function" | "event" | "error") {
            Some(if kind == "function" && first != "function" {
                first
            } else {
                self.word()?
            })
        } else {
            None
        };
        let inputs = self.params(kind == "event", 0)?;
        let mut mutability = None;
        let mut visibility = None;
        let mut outputs = None;
        let mut anonymous = false;
        loop {
            self.space();
            if self.pos == self.text.len() {
                break;
            }
            let word = self.word()?;
            match word {
                "pure" | "view" | "payable" | "nonpayable"
                    if !matches!(kind, "event" | "error") && mutability.is_none() =>
                {
                    mutability = Some(word)
                }
                "constant" if kind == "function" && mutability.is_none() => {
                    mutability = Some("view")
                }
                "external" | "public" if kind == "function" && visibility.is_none() => {
                    visibility = Some(word)
                }
                "returns" if matches!(kind, "function" | "fallback") && outputs.is_none() => {
                    outputs = Some(self.params(false, 0)?)
                }
                "anonymous" if kind == "event" && !anonymous => anonymous = true,
                _ => return Err(AbiError::Schema),
            }
        }
        let mut value = json!({"type":kind});
        if let Some(name) = name {
            value["name"] = json!(name);
        }
        if matches!(kind, "fallback" | "receive") {
            if kind == "receive" && !inputs.is_empty() {
                return Err(AbiError::Schema);
            }
            if kind == "fallback" {
                if !inputs.is_empty() && (inputs.len() != 1 || inputs[0]["type"] != "bytes") {
                    return Err(AbiError::Schema);
                }
                if let Some(outputs) = &outputs
                    && (outputs.len() != 1 || outputs[0]["type"] != "bytes")
                {
                    return Err(AbiError::Schema);
                }
            }
        } else {
            value["inputs"] = json!(inputs);
        }
        if kind == "function" {
            value["outputs"] = json!(outputs.unwrap_or_default());
        }
        if kind == "event" {
            value["anonymous"] = json!(anonymous);
        }
        if !matches!(kind, "event" | "error") {
            value["stateMutability"] = json!(mutability.unwrap_or(if kind == "receive" {
                "payable"
            } else {
                "nonpayable"
            }));
        }
        Ok(value)
    }
}
