//! Validated RPC request and response values for the pinned go-quai API.
use crate::{ProviderError, quantity};
use quai_primitives::{Address, Hash32, QuaiAddress, Zone};
use quai_rpc::U256;
use serde_json::{Map, Value, json};
use std::{collections::BTreeSet, fmt, str::FromStr};

type Object = Map<String, Value>;
/// Maximum decoded byte payload accepted by these typed APIs.
pub const MAX_RPC_DATA_BYTES: usize = 1024 * 1024;
const MAX_ITEMS: usize = 65_536;
const MAX_ACCESS_ENTRIES: usize = 4_096;

/// Hex-encoded RPC bytes, distinct from a numeric quantity. Debug reveals length only.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct RpcData(Vec<u8>);
impl RpcData {
    /// Validate the allocation bound before retaining bytes.
    pub fn new(bytes: Vec<u8>) -> Result<Self, ProviderError> {
        if bytes.len() > MAX_RPC_DATA_BYTES {
            return Err(invalid("byte payload exceeds limit"));
        }
        Ok(Self(bytes))
    }
    /// Borrow decoded bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
    /// Encode bytes, including leading zeros, with a lowercase `0x` prefix.
    pub fn to_hex(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut result = String::with_capacity(2 + self.0.len() * 2);
        result.push_str("0x");
        for byte in &self.0 {
            result.push(HEX[(byte >> 4) as usize] as char);
            result.push(HEX[(byte & 15) as usize] as char);
        }
        result
    }
}
impl fmt::Debug for RpcData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RpcData")
            .field("bytes", &self.0.len())
            .finish()
    }
}
impl FromStr for RpcData {
    type Err = ProviderError;
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let hex = input
            .strip_prefix("0x")
            .ok_or(invalid("expected hex bytes"))?;
        if hex.len() > MAX_RPC_DATA_BYTES * 2 || !hex.len().is_multiple_of(2) {
            return Err(invalid("invalid byte payload length"));
        }
        fn nibble(byte: u8) -> Result<u8, ProviderError> {
            match byte {
                b'0'..=b'9' => Ok(byte - b'0'),
                b'a'..=b'f' => Ok(byte - b'a' + 10),
                b'A'..=b'F' => Ok(byte - b'A' + 10),
                _ => Err(invalid("invalid hex bytes")),
            }
        }
        let mut bytes = Vec::with_capacity(hex.len() / 2);
        for pair in hex.as_bytes().chunks_exact(2) {
            bytes.push(nibble(pair[0])? * 16 + nibble(pair[1])?);
        }
        Ok(Self(bytes))
    }
}

/// Unknown response fields retained explicitly; diagnostic formatting omits values.
#[derive(Clone, Default, PartialEq)]
pub struct Extensions(Object);
impl Extensions {
    /// Inspect untrusted extension fields explicitly. They are not verified protocol facts.
    pub fn fields(&self) -> &Object {
        &self.0
    }
}
impl fmt::Debug for Extensions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Extensions")
            .field("field_count", &self.0.len())
            .finish()
    }
}

/// An account transaction's storage access declaration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccessListItem {
    /// Account address; arbitrary address bytes are permitted by the wire schema.
    pub address: Address,
    /// Exact storage-key words; ordering is preserved.
    pub storage_keys: Vec<Hash32>,
}

/// Account-only simulation request. This does not authorize or submit a transaction.
///
/// Sender determines routing. Cross-zone recipients are not supported by this
/// initial simulation API; use a future dedicated ETX/conversion request model.
#[derive(Clone, Debug)]
pub struct CallRequest {
    /// Required sender, avoiding implicit node-local zero-address defaults.
    pub from: QuaiAddress,
    /// Same-zone account/contract recipient; None requests contract creation.
    pub to: Option<QuaiAddress>,
    /// Gas budget in node uint64 units.
    pub gas: Option<u64>,
    /// Gas price in base units; Quai uses gasPrice, not Ethereum dynamic-fee fields.
    pub gas_price: Option<U256>,
    /// Transferred base units.
    pub value: Option<U256>,
    /// Optional account nonce; node simulation may override it from state.
    pub nonce: Option<u64>,
    /// Call or creation data, encoded under the unambiguous `input` key.
    pub input: RpcData,
    /// Storage-access declarations.
    pub access_list: Vec<AccessListItem>,
}
impl CallRequest {
    /// Construct a zero-value account call with no explicit gas/nonce overrides.
    pub fn new(from: QuaiAddress, to: QuaiAddress) -> Self {
        Self {
            from,
            to: Some(to),
            gas: None,
            gas_price: None,
            value: None,
            nonce: None,
            input: RpcData::default(),
            access_list: Vec::new(),
        }
    }
    pub(crate) fn rpc_value(&self) -> Result<Value, ProviderError> {
        if self.to.is_some_and(|to| to.zone() != self.from.zone()) {
            return Err(ProviderError::InvalidRequest(
                "cross-zone simulation is unsupported",
            ));
        }
        if self.to.is_none() && self.input.bytes().is_empty() {
            return Err(ProviderError::InvalidRequest(
                "creation requires input bytes",
            ));
        }
        validate_access_list(&self.access_list)?;
        let mut value = Map::new();
        value.insert("from".into(), json!(self.from.to_string()));
        if let Some(to) = self.to {
            value.insert("to".into(), json!(to.to_string()));
        }
        value.insert("input".into(), json!(self.input.to_hex()));
        value.insert("txType".into(), json!(0));
        for (key, val) in [
            ("gas", self.gas.map(U256::from)),
            ("gasPrice", self.gas_price),
            ("value", self.value),
            ("nonce", self.nonce.map(U256::from)),
        ] {
            if let Some(val) = val {
                value.insert(key.into(), json!(format!("{val:#x}")));
            }
        }
        if !self.access_list.is_empty() {
            value.insert("accessList".into(), Value::Array(self.access_list.iter().map(|entry| json!({"address":entry.address.to_string(),"storageKeys":entry.storage_keys.iter().map(ToString::to_string).collect::<Vec<_>>()})).collect()));
        }
        Ok(Value::Object(value))
    }
}
impl TryFrom<Value> for CallRequest {
    type Error = ProviderError;
    fn try_from(value: Value) -> Result<Self, Self::Error> {
        let mut object = object(value)?;
        let from = address(take(&mut object, "from")?)?
            .try_into()
            .map_err(|_| invalid("expected Quai sender"))?;
        let to = optional(&mut object, "to")
            .map(address)
            .transpose()?
            .map(QuaiAddress::try_from)
            .transpose()
            .map_err(|_| invalid("expected Quai recipient"))?;
        let gas = optional(&mut object, "gas").map(uint64).transpose()?;
        let gas_price = optional(&mut object, "gasPrice")
            .map(quantity)
            .transpose()?;
        let value = optional(&mut object, "value").map(quantity).transpose()?;
        let nonce = optional(&mut object, "nonce").map(uint64).transpose()?;
        let input = optional(&mut object, "input")
            .map(data)
            .transpose()?
            .unwrap_or_default();
        let access_list = optional(&mut object, "accessList")
            .map(access_list)
            .transpose()?
            .unwrap_or_default();
        if let Some(tx_type) = object.remove("txType")
            && tx_type != json!(0)
        {
            return Err(ProviderError::InvalidRequest(
                "only account txType 0 is supported",
            ));
        }
        if !object.is_empty() {
            return Err(ProviderError::InvalidRequest(
                "unknown or ambiguous call request field",
            ));
        }
        let request = Self {
            from,
            to,
            gas,
            gas_price,
            value,
            nonce,
            input,
            access_list,
        };
        request.rpc_value()?;
        Ok(request)
    }
}

/// Node transaction discriminator. Unknown transaction kinds are rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransactionKind {
    /// Account-based transaction (type 0).
    Quai,
    /// External transaction generated by the protocol (type 1).
    External,
    /// UTXO transaction (type 2).
    Qi,
}

pub(crate) fn genesis_hash(value: Value) -> Result<Hash32, ProviderError> {
    let mut header = object(value)?;
    let mut work = object(take(&mut header, "woHeader")?)?;
    let hash = hash_field(&mut work, "hash")?;
    if hash == Hash32::ZERO
        || uint64(take(&mut work, "number")?)? != 0
        || !data(take(&mut work, "location")?)?.bytes().is_empty()
        || hash_field(&mut work, "parentHash")? != Hash32::ZERO
    {
        return Err(invalid("invalid root genesis header"));
    }
    Ok(hash)
}

/// Fields needed to verify a receipt's canonical block association on a zone.
#[derive(Clone, Debug, PartialEq)]
pub struct ZoneHeader {
    /// Work-object identity reported by the node.
    pub hash: Hash32,
    /// Parent work-object identity.
    pub parent_hash: Hash32,
    /// Zone block height.
    pub number: u64,
    /// Validated node location.
    pub zone: Zone,
    /// Referenced Prime height, not interchangeable with the zone height.
    pub prime_terminus_number: u64,
    /// Block execution gas capacity.
    pub gas_limit: u64,
    /// Block state capacity.
    pub state_limit: u64,
    /// Additional top-level header fields.
    pub extensions: Extensions,
    /// Additional work-object header fields.
    pub work_object_extensions: Extensions,
}
impl TryFrom<Value> for ZoneHeader {
    type Error = ProviderError;
    fn try_from(value: Value) -> Result<Self, Self::Error> {
        let mut o = object(value)?;
        let mut work = object(take(&mut o, "woHeader")?)?;
        let hash = hash_field(&mut work, "hash")?;
        let parent_hash = hash_field(&mut work, "parentHash")?;
        let number = uint64(take(&mut work, "number")?)?;
        let prime_terminus_number = uint64(take(&mut work, "primeTerminusNumber")?)?;
        let location = data(take(&mut work, "location")?)?;
        let location = location.bytes();
        if location.len() != 2 || location[0] > 2 || location[1] > 2 {
            return Err(invalid("invalid zone header location"));
        }
        let zone = Zone::from_byte((location[0] << 4) | location[1])
            .map_err(|_| invalid("unknown zone header location"))?;
        Ok(Self {
            hash,
            parent_hash,
            number,
            zone,
            prime_terminus_number,
            gas_limit: uint64(take(&mut o, "gasLimit")?)?,
            state_limit: uint64(take(&mut o, "stateLimit")?)?,
            extensions: Extensions(o),
            work_object_extensions: Extensions(work),
        })
    }
}
/// Included transaction's untrusted RPC location metadata.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Inclusion {
    /// Block hash.
    pub block_hash: Hash32,
    /// Zone block height.
    pub block_number: u64,
    /// Transaction offset in the block.
    pub transaction_index: u64,
}
/// ECDSA values reported by the node; parsing does not verify the signature.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EcdsaValues {
    /// Recovery discriminator.
    pub v: U256,
    /// Signature scalar r.
    pub r: U256,
    /// Signature scalar s.
    pub s: U256,
}
/// Account transaction RPC fields.
#[derive(Clone, Debug, PartialEq)]
pub struct QuaiTransaction {
    /// Recovered sender reported by the node.
    pub from: QuaiAddress,
    /// Recipient, possibly in the Qi ledger for conversion; None means creation.
    pub to: Option<Address>,
    /// Gas budget.
    pub gas: u64,
    /// Gas price.
    pub gas_price: U256,
    /// Value in base units.
    pub value: U256,
    /// Account nonce.
    pub nonce: u64,
    /// Replay-protection identity.
    pub chain_id: U256,
    /// Reported ECDSA signature values.
    pub signature: EcdsaValues,
    /// Storage-access declarations.
    pub access_list: Vec<AccessListItem>,
}
/// External transaction fields, preserving protocol origin rather than pretending it is signed locally.
#[derive(Clone, Debug, PartialEq)]
pub struct ExternalTransaction {
    /// Reported external sender (either ledger).
    pub from: Address,
    /// Destination (either ledger).
    pub to: Address,
    /// Gas budget.
    pub gas: u64,
    /// Value in destination base units.
    pub value: U256,
    /// Originating transaction hash.
    pub originating_tx_hash: Hash32,
    /// External output index, constrained to node uint16.
    pub etx_index: u16,
    /// External subtype; its semantics depend on the verified fork profile.
    pub etx_type: u64,
}
/// Exact UTXO identity.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct OutPoint {
    /// Creating transaction.
    pub tx_hash: Hash32,
    /// Output index.
    pub index: u16,
}
/// Qi transaction input. Keys are structural bytes until cryptographic verification.
#[derive(Clone, Debug, PartialEq)]
pub struct QiInput {
    /// Referenced output.
    pub previous_out_point: OutPoint,
    /// Compressed or uncompressed secp256k1 public key in node wire form.
    pub public_key: RpcData,
}
/// Qi output, including Quai destinations used by conversions.
#[derive(Clone, Debug, PartialEq)]
pub struct QiOutput {
    /// Denomination index in the pinned 0..14 table.
    pub denomination: u8,
    /// Destination address, not restricted to Qi for conversion outputs.
    pub address: Address,
    /// Optional absolute lock value. None preserves wire null.
    pub lock: Option<U256>,
}
/// Qi transaction fields.
#[derive(Clone, Debug, PartialEq)]
pub struct QiTransaction {
    /// Replay-protection identity.
    pub chain_id: U256,
    /// Ordered inputs; aggregation must not reorder these.
    pub inputs: Vec<QiInput>,
    /// Ordered outputs.
    pub outputs: Vec<QiOutput>,
    /// Exactly 64 Schnorr-signature bytes. Not verified during RPC parsing.
    pub signature: RpcData,
}
/// A transaction's complete initial typed variant.
#[derive(Clone, Debug, PartialEq)]
pub enum TransactionDetails {
    /// Account-based fields.
    Quai(QuaiTransaction),
    /// Protocol-created external fields.
    External(ExternalTransaction),
    /// UTXO fields.
    Qi(QiTransaction),
}
/// Typed transaction lookup result. Pending inclusion is represented as None.
#[derive(Clone, Debug, PartialEq)]
pub struct Transaction {
    /// Claimed transaction identity, checked against the lookup argument by Provider.
    pub hash: Hash32,
    /// Block inclusion or pending state.
    pub inclusion: Option<Inclusion>,
    /// Transaction data.
    pub input: RpcData,
    /// Type-specific fields.
    pub details: TransactionDetails,
    /// Uninterpreted extension fields, never used to authorize an operation.
    pub extensions: Extensions,
}
impl Transaction {
    /// Return the validated discriminator.
    pub fn kind(&self) -> TransactionKind {
        match self.details {
            TransactionDetails::Quai(_) => TransactionKind::Quai,
            TransactionDetails::External(_) => TransactionKind::External,
            TransactionDetails::Qi(_) => TransactionKind::Qi,
        }
    }
}
impl TryFrom<Value> for Transaction {
    type Error = ProviderError;
    fn try_from(value: Value) -> Result<Self, Self::Error> {
        let mut o = object(value)?;
        let kind = kind(take(&mut o, "type")?)?;
        let hash = hash(take(&mut o, "hash")?)?;
        let inclusion = inclusion(&mut o)?;
        let input = data(take(&mut o, "input")?)?;
        // These fields are emitted for every Go variant, including zero-valued Qi/ETX defaults.
        let gas = uint64(take(&mut o, "gas")?)?;
        let nonce = uint64(take(&mut o, "nonce")?)?;
        let details = match kind {
            TransactionKind::Quai => TransactionDetails::Quai(QuaiTransaction {
                from: address(take(&mut o, "from")?)?
                    .try_into()
                    .map_err(|_| invalid("invalid Quai sender"))?,
                to: optional(&mut o, "to").map(address).transpose()?,
                gas,
                nonce,
                gas_price: quantity(take(&mut o, "gasPrice")?)?,
                value: quantity(take(&mut o, "value")?)?,
                chain_id: quantity(take(&mut o, "chainId")?)?,
                signature: EcdsaValues {
                    v: quantity(take(&mut o, "v")?)?,
                    r: quantity(take(&mut o, "r")?)?,
                    s: quantity(take(&mut o, "s")?)?,
                },
                access_list: optional(&mut o, "accessList")
                    .map(access_list)
                    .transpose()?
                    .unwrap_or_default(),
            }),
            TransactionKind::External => TransactionDetails::External(ExternalTransaction {
                from: address(take(&mut o, "from")?)?,
                to: address(take(&mut o, "to")?)?,
                gas,
                value: quantity(take(&mut o, "value")?)?,
                originating_tx_hash: hash_field(&mut o, "originatingTxHash")?,
                etx_index: uint16(take(&mut o, "etxIndex")?)?,
                etx_type: uint64(take(&mut o, "etxType")?)?,
            }),
            TransactionKind::Qi => {
                if gas != 0 || nonce != 0 {
                    return Err(invalid("unexpected gas/nonce on Qi transaction"));
                }
                let inputs = parse_inputs(take(&mut o, "inputs")?)?;
                let outputs = parse_outputs(take(&mut o, "outputs")?)?;
                if inputs.is_empty() || outputs.is_empty() {
                    return Err(invalid("Qi transaction requires inputs and outputs"));
                }
                let signature = data(take(&mut o, "utxoSignature")?)?;
                if signature.bytes().len() != 64 {
                    return Err(invalid("invalid Schnorr signature length"));
                }
                TransactionDetails::Qi(QiTransaction {
                    chain_id: quantity(take(&mut o, "chainId")?)?,
                    inputs,
                    outputs,
                    signature,
                })
            }
        };
        Ok(Self {
            hash,
            inclusion,
            input,
            details,
            extensions: Extensions(o),
        })
    }
}

/// Contract log and its block/transaction association.
#[derive(Clone, Debug, PartialEq)]
pub struct Log {
    /// Emitting account.
    pub address: QuaiAddress,
    /// At most four EVM topics.
    pub topics: Vec<Hash32>,
    /// Event data.
    pub data: RpcData,
    /// Transaction association.
    pub transaction_hash: Hash32,
    /// Inclusion metadata.
    pub inclusion: Inclusion,
    /// Offset among the block's logs.
    pub log_index: u64,
    /// True when a log was removed by a reorganization.
    pub removed: bool,
    /// Uninterpreted extension fields.
    pub extensions: Extensions,
}
/// Receipt execution outcome, preserving a historical post-state root when supplied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReceiptOutcome {
    /// Node reports status zero.
    Failed,
    /// Node reports status one.
    Succeeded,
    /// Legacy receipt supplies a post-state root instead of status.
    PostState(Hash32),
}
/// Initial typed transaction receipt for all three transaction kinds.
#[derive(Clone, Debug, PartialEq)]
pub struct Receipt {
    /// Queried identity.
    pub transaction_hash: Hash32,
    /// Confirmed inclusion claimed by the node.
    pub inclusion: Inclusion,
    /// Wire transaction kind.
    pub kind: TransactionKind,
    /// Sender when reported; required to be in the Quai ledger for type-0 receipts.
    pub from: Option<Address>,
    /// Recipient when reported; may be absent for creation and ETX/Qi receipts.
    pub to: Option<Address>,
    /// Gas consumed by this transaction.
    pub gas_used: u64,
    /// Node-reported cumulative gas. ETX receipts may report zero.
    pub cumulative_gas_used: u64,
    /// Node-reported effective price. The pinned Go implementation narrows this to uint64.
    pub effective_gas_price: U256,
    /// Created contract, if any.
    pub contract_address: Option<QuaiAddress>,
    /// Outcome or legacy root.
    pub outcome: ReceiptOutcome,
    /// Logs checked against this receipt's transaction and block association.
    pub logs: Vec<Log>,
    /// Exactly 10,240 bytes in Quai, unlike Ethereum's bloom size.
    pub logs_bloom: RpcData,
    /// Typed external transactions emitted during execution.
    pub outbound_etxs: Vec<Transaction>,
    /// Origin hash for type-1 receipts.
    pub originating_tx_hash: Option<Hash32>,
    /// External subtype for type-1 receipts.
    pub etx_type: Option<u64>,
    /// Additional node fields, including optional sender/recipient.
    pub extensions: Extensions,
}
impl TryFrom<Value> for Receipt {
    type Error = ProviderError;
    fn try_from(value: Value) -> Result<Self, Self::Error> {
        let mut o = object(value)?;
        let transaction_hash = hash_field(&mut o, "transactionHash")?;
        let inclusion = inclusion(&mut o)?.ok_or(invalid("receipt lacks inclusion"))?;
        let kind = kind(take(&mut o, "type")?)?;
        let from = optional(&mut o, "from").map(address).transpose()?;
        let to = optional(&mut o, "to").map(address).transpose()?;
        if kind == TransactionKind::Quai
            && from
                .and_then(|address| QuaiAddress::try_from(address).ok())
                .is_none()
        {
            return Err(invalid("Quai receipt requires account sender"));
        }
        let gas_used = uint64(take(&mut o, "gasUsed")?)?;
        let cumulative_gas_used = uint64(take(&mut o, "cumulativeGasUsed")?)?;
        let effective_gas_price = quantity(take(&mut o, "effectiveGasPrice")?)?;
        let contract_address = optional(&mut o, "contractAddress")
            .map(address)
            .transpose()?
            .map(QuaiAddress::try_from)
            .transpose()
            .map_err(|_| invalid("invalid contract address ledger"))?;
        let outcome = match (optional(&mut o, "status"), optional(&mut o, "root")) {
            (Some(status), None) => match uint64(status)? {
                0 => ReceiptOutcome::Failed,
                1 => ReceiptOutcome::Succeeded,
                _ => return Err(invalid("invalid receipt status")),
            },
            (None, Some(root)) => ReceiptOutcome::PostState(hash(root)?),
            _ => return Err(invalid("receipt requires exactly one status or root")),
        };
        let logs_bloom = data(take(&mut o, "logsBloom")?)?;
        if logs_bloom.bytes().len() != 10_240 {
            return Err(invalid("invalid Quai bloom width"));
        }
        let mut logs = Vec::new();
        let mut seen = BTreeSet::new();
        for value in array(take(&mut o, "logs")?, MAX_ITEMS)? {
            let log = parse_log(value)?;
            if log.transaction_hash != transaction_hash
                || log.inclusion != inclusion
                || !seen.insert(log.log_index)
            {
                return Err(invalid("receipt/log association mismatch"));
            }
            logs.push(log);
        }
        let mut outbound_etxs = Vec::new();
        if let Some(values) = optional(&mut o, "outboundEtxs") {
            for value in array(values, MAX_ITEMS)? {
                let tx = Transaction::try_from(value)?;
                if tx.kind() != TransactionKind::External {
                    return Err(invalid("outbound transaction is not external"));
                }
                outbound_etxs.push(tx);
            }
        }
        let originating_tx_hash = optional(&mut o, "originatingTxHash")
            .map(hash)
            .transpose()?;
        let etx_type = optional(&mut o, "etxType").map(uint64).transpose()?;
        if (kind == TransactionKind::External)
            != (originating_tx_hash.is_some() && etx_type.is_some())
            || (kind != TransactionKind::External
                && (originating_tx_hash.is_some() || etx_type.is_some()))
        {
            return Err(invalid("invalid receipt external metadata"));
        }
        Ok(Self {
            transaction_hash,
            inclusion,
            kind,
            from,
            to,
            gas_used,
            cumulative_gas_used,
            effective_gas_price,
            contract_address,
            outcome,
            logs,
            logs_bloom,
            outbound_etxs,
            originating_tx_hash,
            etx_type,
            extensions: Extensions(o),
        })
    }
}

/// Address-index result; presence alone does not prove spendability or index completeness.
#[derive(Clone, Debug, PartialEq)]
pub struct AddressOutpoint {
    /// Creating transaction and output index.
    pub outpoint: OutPoint,
    /// Valid denomination-table index.
    pub denomination: u8,
    /// Absolute lock value reported by the node.
    pub lock: U256,
    /// Uninterpreted extension fields.
    pub extensions: Extensions,
}
pub(crate) fn parse_outpoints(value: Value) -> Result<Vec<AddressOutpoint>, ProviderError> {
    let values = array(value, MAX_ITEMS)?;
    let mut result = Vec::with_capacity(values.len());
    let mut seen = BTreeSet::new();
    for value in values {
        let mut o = object(value)?;
        let outpoint = OutPoint {
            tx_hash: hash_field(&mut o, "txHash")?,
            index: uint16(take(&mut o, "index")?)?,
        };
        if !seen.insert(outpoint) {
            return Err(invalid("duplicate outpoint"));
        }
        let denomination = denomination(take(&mut o, "denomination")?)?;
        let lock = quantity(take(&mut o, "lock")?)?;
        result.push(AddressOutpoint {
            outpoint,
            denomination,
            lock,
            extensions: Extensions(o),
        });
    }
    Ok(result)
}
pub(crate) fn parse_log(value: Value) -> Result<Log, ProviderError> {
    let mut o = object(value)?;
    let address = address(take(&mut o, "address")?)?
        .try_into()
        .map_err(|_| invalid("invalid log address"))?;
    let topics = array(take(&mut o, "topics")?, 4)?
        .into_iter()
        .map(hash)
        .collect::<Result<_, _>>()?;
    let data = data(take(&mut o, "data")?)?;
    let transaction_hash = hash_field(&mut o, "transactionHash")?;
    let inclusion = inclusion(&mut o)?.ok_or(invalid("log lacks inclusion"))?;
    let log_index = uint64(take(&mut o, "logIndex")?)?;
    let removed = take(&mut o, "removed")?
        .as_bool()
        .ok_or(invalid("invalid removed flag"))?;
    Ok(Log {
        address,
        topics,
        data,
        transaction_hash,
        inclusion,
        log_index,
        removed,
        extensions: Extensions(o),
    })
}
fn parse_inputs(value: Value) -> Result<Vec<QiInput>, ProviderError> {
    let mut seen = BTreeSet::new();
    array(value, MAX_ITEMS)?
        .into_iter()
        .map(|value| {
            let mut o = object(value)?;
            let mut point = object(take(&mut o, "previousOutPoint")?)?;
            let previous_out_point = OutPoint {
                tx_hash: hash_field(&mut point, "txHash")?,
                index: uint16(take(&mut point, "index")?)?,
            };
            if !seen.insert(previous_out_point) {
                return Err(invalid("duplicate Qi input"));
            }
            let public_key = data(take(&mut o, "pubKey")?)?;
            let key = public_key.bytes();
            if !matches!((key.len(), key.first()), (33, Some(2 | 3)) | (65, Some(4))) {
                return Err(invalid("invalid public key encoding"));
            }
            Ok(QiInput {
                previous_out_point,
                public_key,
            })
        })
        .collect()
}
fn parse_outputs(value: Value) -> Result<Vec<QiOutput>, ProviderError> {
    array(value, MAX_ITEMS)?
        .into_iter()
        .map(|value| {
            let mut o = object(value)?;
            Ok(QiOutput {
                denomination: denomination(take(&mut o, "denomination")?)?,
                address: address(take(&mut o, "address")?)?,
                lock: optional(&mut o, "lock").map(quantity).transpose()?,
            })
        })
        .collect()
}
fn access_list(value: Value) -> Result<Vec<AccessListItem>, ProviderError> {
    let entries = array(value, MAX_ACCESS_ENTRIES)?
        .into_iter()
        .map(|value| {
            let mut o = object(value)?;
            let entry = AccessListItem {
                address: address(take(&mut o, "address")?)?,
                storage_keys: array(take(&mut o, "storageKeys")?, MAX_ITEMS)?
                    .into_iter()
                    .map(hash)
                    .collect::<Result<_, _>>()?,
            };
            if !o.is_empty() {
                return Err(invalid("unknown access-list entry field"));
            }
            Ok(entry)
        })
        .collect::<Result<Vec<_>, ProviderError>>()?;
    validate_access_list(&entries)?;
    Ok(entries)
}
fn validate_access_list(entries: &[AccessListItem]) -> Result<(), ProviderError> {
    if entries.len() > MAX_ACCESS_ENTRIES
        || entries
            .iter()
            .try_fold(0usize, |sum, e| sum.checked_add(e.storage_keys.len()))
            .is_none_or(|sum| sum > MAX_ITEMS)
    {
        return Err(invalid("access list exceeds bound"));
    }
    Ok(())
}
fn inclusion(o: &mut Object) -> Result<Option<Inclusion>, ProviderError> {
    match (
        optional(o, "blockHash"),
        optional(o, "blockNumber"),
        optional(o, "transactionIndex"),
    ) {
        (None, None, None) => Ok(None),
        (Some(h), Some(n), Some(i)) => Ok(Some(Inclusion {
            block_hash: hash(h)?,
            block_number: uint64(n)?,
            transaction_index: uint64(i)?,
        })),
        _ => Err(invalid("partial inclusion metadata")),
    }
}
fn kind(value: Value) -> Result<TransactionKind, ProviderError> {
    match uint64(value)? {
        0 => Ok(TransactionKind::Quai),
        1 => Ok(TransactionKind::External),
        2 => Ok(TransactionKind::Qi),
        _ => Err(invalid("unknown transaction type")),
    }
}
fn denomination(value: Value) -> Result<u8, ProviderError> {
    let value = uint64(value)?;
    if value > 14 {
        return Err(invalid("unknown denomination"));
    }
    Ok(value as u8)
}
pub(crate) fn uint64(value: Value) -> Result<u64, ProviderError> {
    quantity(value)?
        .try_into()
        .map_err(|_| invalid("quantity exceeds uint64"))
}
fn uint16(value: Value) -> Result<u16, ProviderError> {
    uint64(value)?
        .try_into()
        .map_err(|_| invalid("quantity exceeds uint16"))
}
pub(crate) fn data(value: Value) -> Result<RpcData, ProviderError> {
    string(value)?.parse()
}
pub(crate) fn hash(value: Value) -> Result<Hash32, ProviderError> {
    string(value)?
        .parse()
        .map_err(|_| invalid("invalid 32-byte hash"))
}
fn hash_field(o: &mut Object, key: &'static str) -> Result<Hash32, ProviderError> {
    hash(take(o, key)?)
}
fn address(value: Value) -> Result<Address, ProviderError> {
    string(value)?
        .parse()
        .map_err(|_| invalid("invalid address"))
}
fn string(value: Value) -> Result<String, ProviderError> {
    if let Value::String(s) = value {
        Ok(s)
    } else {
        Err(invalid("expected string"))
    }
}
fn object(value: Value) -> Result<Object, ProviderError> {
    if let Value::Object(o) = value {
        Ok(o)
    } else {
        Err(invalid("expected object"))
    }
}
fn array(value: Value, limit: usize) -> Result<Vec<Value>, ProviderError> {
    if let Value::Array(a) = value
        && a.len() <= limit
    {
        return Ok(a);
    }
    Err(invalid("expected bounded array"))
}
fn take(o: &mut Object, key: &'static str) -> Result<Value, ProviderError> {
    o.remove(key).ok_or(invalid("required field missing"))
}
fn optional(o: &mut Object, key: &'static str) -> Option<Value> {
    o.remove(key).filter(|value| !value.is_null())
}
fn invalid(message: &'static str) -> ProviderError {
    ProviderError::InvalidResult(message)
}
