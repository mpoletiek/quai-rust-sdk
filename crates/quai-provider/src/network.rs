//! Explicit network labels and bounded caller-owned aliases; no trust or routing side effects.
use crate::{Provider, ProviderError};
use quai_primitives::Zone;
use quai_rpc::{Transport, U256};
use serde_json::{Value, json};
use std::collections::BTreeMap;
/// Invalid or conflicting network metadata. Names are labels, never trusted identities.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum NetworkError {
    /// Empty/control-bearing name or invalid exact chain quantity/object.
    #[error("invalid network metadata")]
    Invalid,
    /// Alias/chain key is already registered.
    #[error("network key is already registered")]
    Conflict,
    /// Configured registry entry budget is exhausted.
    #[error("network registry limit exceeded")]
    Limit,
    /// No entry exists under this explicit name.
    #[error("unknown network name")]
    Unknown,
}
fn name_valid(name: &str) -> bool {
    !name.is_empty() && name.len() <= 128 && !name.chars().any(char::is_control)
}
/// Immutable human-readable label and exact chain ID. This does not include a
/// genesis hash, RPC endpoint, activation state or any authenticated chain claim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Network {
    name: String,
    chain_id: U256,
}
/// Explicit matching intent, avoiding ambiguous numeric-name string coercion.
#[derive(Clone, Copy, Debug)]
pub enum NetworkMatch<'a> {
    /// Compare exact label spelling.
    Name(&'a str),
    /// Compare an exact unsigned chain ID.
    ChainId(U256),
    /// Compare another descriptor by chain ID (name is only metadata).
    Descriptor(&'a Network),
}
impl Network {
    /// Construct a bounded label and chain ID, including zero for custom networks.
    pub fn new(name: &str, chain_id: U256) -> Result<Self, NetworkError> {
        if !name_valid(name) {
            return Err(NetworkError::Invalid);
        }
        Ok(Self {
            name: name.into(),
            chain_id,
        })
    }
    /// Exact descriptive label.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Exact unsigned chain identifier; equality alone does not authenticate a network.
    pub fn chain_id(&self) -> U256 {
        self.chain_id
    }
    /// Compare label or chain identity according to an explicit selector.
    pub fn matches(&self, other: NetworkMatch<'_>) -> bool {
        match other {
            NetworkMatch::Name(n) => n == self.name,
            NetworkMatch::ChainId(id) => id == self.chain_id,
            NetworkMatch::Descriptor(n) => n.chain_id == self.chain_id,
        }
    }
    /// JSON with a decimal-string chain ID, matching the source's lossless export.
    pub fn to_json(&self) -> Value {
        json!({"name":self.name,"chainId":self.chain_id.to_string()})
    }
    /// Parse exactly name/chainId fields. IDs accept integer JSON or unsigned
    /// decimal/0x strings; floating, negative, unknown and missing fields reject.
    pub fn from_json(value: &Value) -> Result<Self, NetworkError> {
        let o = value.as_object().ok_or(NetworkError::Invalid)?;
        if o.len() != 2 {
            return Err(NetworkError::Invalid);
        }
        let name = o
            .get("name")
            .and_then(Value::as_str)
            .ok_or(NetworkError::Invalid)?;
        let raw = o.get("chainId").ok_or(NetworkError::Invalid)?;
        let id = if let Some(n) = raw.as_u64() {
            U256::from(n)
        } else {
            let n = raw.as_str().ok_or(NetworkError::Invalid)?;
            if n.is_empty() || n.len() > 80 {
                return Err(NetworkError::Invalid);
            }
            let (digits, radix) = n.strip_prefix("0x").map_or((n, 10), |s| (s, 16));
            if digits.is_empty()
                || !digits.bytes().all(|b| {
                    if radix == 16 {
                        b.is_ascii_hexdigit()
                    } else {
                        b.is_ascii_digit()
                    }
                })
            {
                return Err(NetworkError::Invalid);
            }
            U256::from_str_radix(digits, radix).map_err(|_| NetworkError::Invalid)?
        };
        Self::new(name, id)
    }
}
/// Caller-owned network registry, containing at most 256 combined name/chain keys.
/// No default mainnet entry, endpoint selection or process-global mutation occurs.
#[derive(Clone, Debug)]
pub struct NetworkRegistry {
    names: BTreeMap<String, Network>,
    chains: BTreeMap<U256, Network>,
    capacity: usize,
}
impl NetworkRegistry {
    /// Select a combined key limit in 1..=256.
    pub fn new(capacity: usize) -> Result<Self, NetworkError> {
        if !(1..=256).contains(&capacity) {
            return Err(NetworkError::Limit);
        }
        Ok(Self {
            names: BTreeMap::new(),
            chains: BTreeMap::new(),
            capacity,
        })
    }
    /// Number of registered name/chain keys, including aliases.
    pub fn len(&self) -> usize {
        self.names.len() + self.chains.len()
    }
    /// Whether no names or chains are registered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Register canonical name and chain atomically. Either collision leaves both unchanged.
    pub fn register(&mut self, network: Network) -> Result<(), NetworkError> {
        if self.names.contains_key(network.name()) || self.chains.contains_key(&network.chain_id) {
            return Err(NetworkError::Conflict);
        }
        if self.len() + 2 > self.capacity {
            return Err(NetworkError::Limit);
        }
        self.names.insert(network.name.clone(), network.clone());
        self.chains.insert(network.chain_id, network);
        Ok(())
    }
    /// Register an additional exact name. Descriptor values are cloned and cannot
    /// change later through an external mutable factory or stale reference.
    pub fn register_alias(&mut self, alias: &str, network: &Network) -> Result<(), NetworkError> {
        if !name_valid(alias) {
            return Err(NetworkError::Invalid);
        }
        if self.names.contains_key(alias) {
            return Err(NetworkError::Conflict);
        }
        if self.len() == self.capacity {
            return Err(NetworkError::Limit);
        }
        self.names.insert(alias.into(), network.clone());
        Ok(())
    }
    /// Resolve an explicit name; unknown names fail instead of guessing a network.
    pub fn by_name(&self, name: &str) -> Result<Network, NetworkError> {
        self.names.get(name).cloned().ok_or(NetworkError::Unknown)
    }
    /// Resolve a chain ID, using the label unknown when it was not registered.
    pub fn by_chain_id(&self, chain_id: U256) -> Network {
        self.chains
            .get(&chain_id)
            .cloned()
            .unwrap_or_else(|| Network {
                name: "unknown".into(),
                chain_id,
            })
    }
}
/// Optional exact gas price view. Absence is metadata, never an automatic zero fee.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeeData {
    /// Reported gas price when available.
    pub gas_price: Option<U256>,
}
impl FeeData {
    /// Decimal-string or null gasPrice, retaining exact quantities.
    pub fn to_json(self) -> Value {
        json!({"gasPrice":self.gas_price.map(|n|n.to_string())})
    }
}
impl<T: Transport> Provider<T> {
    /// Read and chain-check the selected zone's gas price into a detached view.
    /// RPC failure propagates; it never silently becomes None or a zero fee.
    pub async fn fee_data(&self, zone: Zone) -> Result<FeeData, ProviderError> {
        Ok(FeeData {
            gas_price: Some(self.gas_price(zone).await?),
        })
    }
}
