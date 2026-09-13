use crate::{
    AbiCoder, AbiCustomError, AbiError, AbiEvent, AbiEventValue, AbiFunction, AbiInterface,
    AbiType, MAX_DATA_BYTES, indexed_event_topic,
};
use quai_primitives::Hash32;
use ruint::aliases::U256;
use serde_json::Value;

/// Maximum alternatives in one event topic, matching the provider filter policy.
pub const MAX_FILTER_ALTERNATIVES: usize = 128;

/// A filter in event declaration order, including non-indexed fields.
/// Non-indexed fields must be `Any`; an omitted suffix is also unrestricted.
#[derive(Clone, Copy, Debug)]
pub enum AbiFilterValue<'a> {
    /// Match every value at this position.
    Any,
    /// Hash/encode one exact typed value (including a compound value).
    Exact(&'a Value),
    /// Match any of 1 through 128 exact values; distinct from an array value.
    AnyOf(&'a [Value]),
}

/// Encoded positional event topic, ready to map into a provider log filter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AbiFilterTopic {
    /// Unrestricted topic at this position.
    Any,
    /// One exact topic.
    Exact(Hash32),
    /// A nonempty bounded list of alternative topics.
    AnyOf(Vec<Hash32>),
}

impl AbiEvent {
    /// Encode indexed filters from typed values in full declaration order.
    /// Inserts the signature for nonanonymous events and trims trailing wildcards.
    /// All alternatives share ABI node, text and canonical encoded-byte budgets.
    /// Compound indexed values use Solidity's special event encoding; their hashes
    /// do not uniquely identify the original value or authenticate an emitted log.
    pub fn encode_filter_topics(
        &self,
        filters: &[AbiFilterValue<'_>],
    ) -> Result<Vec<AbiFilterTopic>, AbiError> {
        if filters.len() > self.inputs().len() {
            return Err(AbiError::Value);
        }
        let mut values = Vec::new();
        for ((ty, indexed), filter) in self.inputs().iter().zip(self.indexed()).zip(filters) {
            if !indexed && !matches!(filter, AbiFilterValue::Any) {
                return Err(AbiError::Value);
            }
            match filter {
                AbiFilterValue::Any => {}
                AbiFilterValue::Exact(value) => values.push((ty, *value)),
                AbiFilterValue::AnyOf(alternatives) => {
                    if alternatives.is_empty() {
                        return Err(AbiError::Value);
                    }
                    if alternatives.len() > MAX_FILTER_ALTERNATIVES {
                        return Err(AbiError::Limit);
                    }
                    values.extend(alternatives.iter().map(|value| (ty, value)));
                }
            }
        }
        crate::codec::preflight_values(values.iter().map(|(_, value)| *value))?;
        let mut size = 0usize;
        for (ty, value) in values {
            size = size
                .checked_add(crate::codec::validate(
                    std::slice::from_ref(ty),
                    std::slice::from_ref(value),
                )?)
                .ok_or(AbiError::Limit)?;
            if size > MAX_DATA_BYTES {
                return Err(AbiError::Limit);
            }
        }
        let mut topics = Vec::with_capacity(4);
        if !self.anonymous() {
            topics.push(AbiFilterTopic::Exact(self.topic_hash()));
        }
        for ((ty, indexed), filter) in self.inputs().iter().zip(self.indexed()).zip(filters) {
            if !indexed {
                continue;
            }
            topics.push(match filter {
                AbiFilterValue::Any => AbiFilterTopic::Any,
                AbiFilterValue::Exact(value) => {
                    AbiFilterTopic::Exact(indexed_event_topic(ty, value)?)
                }
                AbiFilterValue::AnyOf(alternatives) => AbiFilterTopic::AnyOf(
                    alternatives
                        .iter()
                        .map(|value| indexed_event_topic(ty, value))
                        .collect::<Result<_, _>>()?,
                ),
            });
        }
        while topics.last() == Some(&AbiFilterTopic::Any) {
            topics.pop();
        }
        Ok(topics)
    }
}

/// Canonically decoded call data; declaration matching does not prove execution.
#[derive(Debug)]
pub struct ParsedCall<'a> {
    /// Exact unambiguous declaration selected by the four-byte selector.
    pub function: &'a AbiFunction,
    /// Positional decoded arguments, using exact decimal strings for integers.
    pub arguments: Vec<Value>,
}
/// Canonically decoded nonanonymous log, without emitter or chain authentication.
#[derive(Debug)]
pub struct ParsedLog<'a> {
    /// Declaration selected by the signature topic.
    pub event: &'a AbiEvent,
    /// Positional values; indexed compound values remain unrecoverable hashes.
    pub values: Vec<AbiEventValue>,
}
/// Decoded, unauthenticated revert data. It may be forged or forwarded by a contract.
#[derive(Debug)]
pub enum ParsedRevert<'a> {
    /// Solidity's builtin `Error(string)`, even when absent from the JSON ABI.
    Error(String),
    /// Solidity's builtin `Panic(uint256)`; unknown codes remain exact integers.
    Panic(U256),
    /// A custom error from the supplied interface.
    Custom {
        /// Exact unambiguous declaration.
        error: &'a AbiCustomError,
        /// Canonically decoded positional arguments.
        arguments: Vec<Value>,
    },
}
impl ParsedRevert<'_> {
    /// Canonical builtin or custom error signature.
    pub fn signature(&self) -> &str {
        match self {
            Self::Error(_) => "Error(string)",
            Self::Panic(_) => "Panic(uint256)",
            Self::Custom { error, .. } => error.signature(),
        }
    }
    /// Builtin or declared error name.
    pub fn name(&self) -> &str {
        match self {
            Self::Error(_) => "Error",
            Self::Panic(_) => "Panic",
            Self::Custom { error, .. } => error.name(),
        }
    }
    /// Four-byte selector; it does not authenticate the error's origin.
    pub fn selector(&self) -> [u8; 4] {
        match self {
            Self::Error(_) => [0x08, 0xc3, 0x79, 0xa0],
            Self::Panic(_) => [0x4e, 0x48, 0x7b, 0x71],
            Self::Custom { error, .. } => error.selector(),
        }
    }
}

impl AbiInterface {
    /// Select and canonically decode selector-prefixed call data. Unknown selectors
    /// return `NotFound`, collisions `Ambiguous`, and malformed bytes an error.
    /// The caller retains transaction value, sender and other external context.
    pub fn parse_call(&self, data: &[u8]) -> Result<ParsedCall<'_>, AbiError> {
        let function = self.function_by_selector(selector(data)?)?;
        Ok(ParsedCall {
            function,
            arguments: function.decode_call(data)?,
        })
    }
    /// Select and decode a nonanonymous log. Empty/unknown signature topics return
    /// `NotFound`; anonymous events require explicit `AbiEvent::decode_log`.
    /// Use the SDK's contract workflow to additionally check the emitter address.
    pub fn parse_log(&self, topics: &[Hash32], data: &[u8]) -> Result<ParsedLog<'_>, AbiError> {
        if topics.len() > 4 || data.len() > MAX_DATA_BYTES {
            return Err(AbiError::Limit);
        }
        let event = self.event_by_topic(*topics.first().ok_or(AbiError::NotFound)?)?;
        if event.anonymous() {
            return Err(AbiError::NotFound);
        }
        Ok(ParsedLog {
            event,
            values: event.decode_log(topics, data)?,
        })
    }
    /// Decode builtin or declared revert data with canonical padding and length checks.
    /// A custom declaration colliding with a builtin selector returns `Ambiguous`.
    /// Empty revert bytes have no decodable selector and return `Encoding`.
    pub fn parse_revert(&self, data: &[u8]) -> Result<ParsedRevert<'_>, AbiError> {
        let selector = selector(data)?;
        let builtin = match selector {
            [0x08, 0xc3, 0x79, 0xa0] => Some(("Error(string)", "string")),
            [0x4e, 0x48, 0x7b, 0x71] => Some(("Panic(uint256)", "uint256")),
            _ => None,
        };
        if let Some((signature, ty)) = builtin {
            if self
                .errors()
                .any(|error| error.selector() == selector && error.signature() != signature)
            {
                return Err(AbiError::Ambiguous);
            }
            let mut values = AbiCoder::decode(&[AbiType::parse(ty)?], &data[4..])?;
            let Value::String(value) = values.pop().ok_or(AbiError::Encoding)? else {
                return Err(AbiError::Encoding);
            };
            return if ty == "string" {
                Ok(ParsedRevert::Error(value))
            } else {
                Ok(ParsedRevert::Panic(
                    value.parse().map_err(|_| AbiError::Encoding)?,
                ))
            };
        }
        let error = self.error_by_selector(selector)?;
        Ok(ParsedRevert::Custom {
            error,
            arguments: error.decode(data)?,
        })
    }
}
fn selector(data: &[u8]) -> Result<[u8; 4], AbiError> {
    if data.len() > MAX_DATA_BYTES + 4 {
        return Err(AbiError::Limit);
    }
    data.get(..4)
        .ok_or(AbiError::Encoding)?
        .try_into()
        .map_err(|_| AbiError::Encoding)
}
