//! Bounded transaction interchange with explicit unsigned claims and verified signatures.
use crate::{
    AccessTuple, Denomination, OutPoint, QiConversionTransaction, QiInput, QiOutput, QiTransaction,
    QiWrappingTransaction, QuaiTransaction, SignedQiOperation, SignedQuaiTransaction,
    TransactionError as Error, U256,
};
use quai_crypto::{PublicKey, RecoverableSignature, SchnorrSignature, SignatureMetadata};
use quai_primitives::{Hash32, QuaiAddress, Zone};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

/// Maximum aggregate JSON string/key bytes before conversion (four MiB).
pub const MAX_DOCUMENT_BYTES: usize = 4 * crate::MAX_TRANSACTION_BYTES;
/// Maximum JSON nodes before conversion; object keys count as string bytes.
pub const MAX_DOCUMENT_NODES: usize = 65_536;

/// Explicit transaction states. Unsigned fields remain editable and are revalidated
/// on serialization. Signed variants retain their immutable verified payloads.
#[derive(Clone, Debug)]
pub enum TransactionDocument {
    /// Unsigned account transaction with an optional unverified sender assertion.
    UnsignedQuai {
        /// Exact transaction fields.
        transaction: QuaiTransaction,
        /// Display/routing assertion only, absent from the unsigned wire bytes.
        claimed_sender: Option<QuaiAddress>,
    },
    /// Unsigned Qi fields. Data length selects transfer, wrapping or conversion;
    /// each operation's full validation runs before serialization.
    UnsignedQi(QiTransaction),
    /// Recovered and verified account transaction.
    SignedQuai(SignedQuaiTransaction),
    /// Verified supported Qi operation.
    SignedQi(SignedQiOperation),
}
impl TransactionDocument {
    /// Decode canonical bytes, detecting signature presence without fabricating it.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let p = crate::decode(bytes)?;
        match p.r#type {
            Some(0) if p.v.is_some() || p.r.is_some() || p.s.is_some() => {
                SignedQuaiTransaction::decode(bytes).map(Self::SignedQuai)
            }
            Some(0) => Ok(Self::UnsignedQuai {
                transaction: QuaiTransaction::decode_unsigned(bytes)?,
                claimed_sender: None,
            }),
            Some(2) if p.signature.is_some() => {
                SignedQiOperation::decode(bytes).map(Self::SignedQi)
            }
            Some(2) => {
                let tx = match p.data.as_ref().map_or(0, Vec::len) {
                    0 => QiTransaction::decode_unsigned(bytes)?,
                    20 => QiWrappingTransaction::decode_unsigned(bytes)?
                        .transaction()
                        .clone(),
                    22 => QiConversionTransaction::decode_unsigned(bytes)?
                        .transaction()
                        .clone(),
                    _ => return Err(Error::InvalidField("unsupported Qi operation data")),
                };
                Ok(Self::UnsignedQi(tx))
            }
            _ => Err(Error::WrongType),
        }
    }
    /// Validate a raw protobuf object through the same bounded canonical decoder.
    pub fn from_proto(value: &crate::proto::Transaction) -> Result<Self, Error> {
        Self::decode(&crate::encode_proto_transaction(value)?)
    }
    /// Raw protobuf view, with optional omission of a verified signature.
    pub fn to_proto(&self, include_signature: bool) -> Result<crate::proto::Transaction, Error> {
        let bytes = if include_signature {
            self.to_bytes()?
        } else {
            self.unsigned_bytes()?
        };
        crate::decode(&bytes)
    }
    /// Exact bytes of the current signed or unsigned state.
    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        match self {
            Self::SignedQuai(tx) => tx.signed_bytes(),
            Self::SignedQi(tx) => tx.signed_bytes(),
            _ => self.unsigned_bytes(),
        }
    }
    /// Canonical preimage bytes, always excluding signatures and claimed sender.
    pub fn unsigned_bytes(&self) -> Result<Vec<u8>, Error> {
        match self {
            Self::UnsignedQuai { transaction, .. } => transaction.unsigned_bytes(),
            Self::SignedQuai(tx) => tx.transaction().unsigned_bytes(),
            Self::UnsignedQi(tx) => unsigned_qi_bytes(tx),
            Self::SignedQi(tx) => unsigned_qi_bytes(tx.transaction()),
        }
    }
    /// Exact signing digest, distinct from a signed transaction ID.
    pub fn signing_digest(&self) -> Result<Hash32, Error> {
        Ok(Hash32::from_bytes(quai_crypto::keccak256(
            &self.unsigned_bytes()?,
        )))
    }
    /// Verified identity exists only for a signed variant.
    pub fn hash(&self) -> Result<Option<Hash32>, Error> {
        match self {
            Self::SignedQuai(tx) => tx.hash().map(Some),
            Self::SignedQi(tx) => tx.hash().map(Some),
            _ => Ok(None),
        }
    }
    /// Signature presence is represented by the enum variant, never a mutable flag.
    pub const fn is_signed(&self) -> bool {
        matches!(self, Self::SignedQuai(_) | Self::SignedQi(_))
    }
    /// Explicit consensus kind: zero for account transactions, two for Qi.
    pub const fn type_id(&self) -> u8 {
        if matches!(self, Self::UnsignedQuai { .. } | Self::SignedQuai(_)) {
            0
        } else {
            2
        }
    }
    /// Published transaction kind label, without a nullable inferred type.
    pub const fn type_name(&self) -> &'static str {
        if self.type_id() == 0 {
            "standard"
        } else {
            "utxo"
        }
    }
    /// Replay-protection chain identity.
    pub fn chain_id(&self) -> U256 {
        match self {
            Self::UnsignedQuai { transaction, .. } => transaction.chain_id,
            Self::SignedQuai(tx) => tx.transaction().chain_id,
            Self::UnsignedQi(tx) => tx.chain_id,
            Self::SignedQi(tx) => tx.transaction().chain_id,
        }
    }
    /// Origin from verified sender/Qi inputs or an explicitly unverified unsigned
    /// account sender assertion. None means the unsigned account sender is unknown.
    pub fn origin_zone(&self) -> Result<Option<Zone>, Error> {
        match self {
            Self::UnsignedQuai { claimed_sender, .. } => Ok(claimed_sender.map(QuaiAddress::zone)),
            Self::SignedQuai(tx) => Ok(Some(tx.from().zone())),
            Self::UnsignedQi(tx) => {
                unsigned_qi_bytes(tx)?;
                Ok(Some(
                    tx.inputs[0]
                        .public_key
                        .address()
                        .zone()
                        .map_err(|_| Error::InvalidScope)?,
                ))
            }
            Self::SignedQi(tx) => tx.origin_zone().map(Some),
        }
    }
    /// First output's zone, matching the published convenience property. This
    /// does not summarize every Qi output; creation has no destination yet.
    pub fn destination_zone(&self) -> Result<Option<Zone>, Error> {
        let address = match self {
            Self::UnsignedQuai { transaction, .. } => transaction.to,
            Self::SignedQuai(tx) => tx.transaction().to,
            Self::UnsignedQi(tx) => tx.outputs.first().map(|o| o.address),
            Self::SignedQi(tx) => tx.transaction().outputs.first().map(|o| o.address),
        };
        address
            .map(|a| a.zone().map_err(|_| Error::InvalidScope))
            .transpose()
    }
    /// Whether the first destination crosses zones. Unknown unsigned origin
    /// returns None instead of guessing; creation returns Some(false).
    pub fn is_external(&self) -> Result<Option<bool>, Error> {
        let Some(destination) = self.destination_zone()? else {
            return Ok(Some(false));
        };
        Ok(self.origin_zone()?.map(|origin| origin != destination))
    }
    /// All distinct destination zones in ascending zone order, including Qi change.
    pub fn destination_zones(&self) -> Result<Vec<Zone>, Error> {
        let outputs = match self {
            Self::UnsignedQi(tx) => Some(&tx.outputs),
            Self::SignedQi(tx) => Some(&tx.transaction().outputs),
            _ => None,
        };
        if let Some(outputs) = outputs {
            self.unsigned_bytes()?;
            outputs
                .iter()
                .map(|o| o.address.zone().map_err(|_| Error::InvalidScope))
                .collect::<Result<BTreeSet<_>, _>>()
                .map(|s| s.into_iter().collect())
        } else {
            Ok(self.destination_zone()?.into_iter().collect())
        }
    }
    /// Whether any output crosses the known origin; unlike is_external this
    /// includes every Qi recipient and change output.
    pub fn has_cross_zone_outputs(&self) -> Result<Option<bool>, Error> {
        let zones = self.destination_zones()?;
        if zones.is_empty() {
            return Ok(Some(false));
        }
        Ok(self
            .origin_zone()?
            .map(|origin| zones.iter().any(|zone| *zone != origin)))
    }
    /// JSON-friendly published field names with exact decimal quantities. Nonce
    /// values beyond JavaScript's safe integer range export as decimal strings.
    /// Qi data exports as hex (null when empty), and output locks as empty hex.
    pub fn to_json(&self) -> Result<Value, Error> {
        self.to_bytes()?; // validate size, messages and operation semantics before allocation
        let hash = self.hash()?.map(|h| h.to_string());
        let (account, sender, signature) = match self {
            Self::UnsignedQuai {
                transaction,
                claimed_sender,
            } => (Some(transaction), *claimed_sender, Value::Null),
            Self::SignedQuai(tx) => (
                Some(tx.transaction()),
                Some(tx.from()),
                serde_json::from_str(
                    &SignatureMetadata::from_signature(*tx.signature())
                        .map_err(|_| Error::InvalidSignature)?
                        .to_json(),
                )
                .map_err(|_| Error::InvalidSignature)?,
            ),
            _ => (None, None, Value::Null),
        };
        if let Some(tx) = account {
            let nonce = if tx.nonce <= 9_007_199_254_740_991 {
                json!(tx.nonce)
            } else {
                json!(tx.nonce.to_string())
            };
            return Ok(
                json!({"type":0,"to":tx.to.map(|a|a.to_checksum()),"from":sender.map(|a|a.to_string()),"data":hex(&tx.data)?,"nonce":nonce,"gasLimit":tx.gas_limit.to_string(),"gasPrice":tx.gas_price.to_string(),"value":tx.value.to_string(),"chainId":tx.chain_id.to_string(),"signature":signature,"hash":hash,"accessList":access_list_to_json(&tx.access_list)?}),
            );
        }
        let (tx, signature) = match self {
            Self::UnsignedQi(tx) => (tx, None),
            Self::SignedQi(tx) => (
                tx.transaction(),
                Some(match tx {
                    SignedQiOperation::Transfer(tx) => tx.signature(),
                    SignedQiOperation::Conversion(tx) => tx.signature(),
                    SignedQiOperation::Wrapping(tx) => tx.signature(),
                }),
            ),
            _ => unreachable!(),
        };
        let inputs = tx.inputs.iter().map(|i| Ok(json!({"txhash":i.previous_output.transaction_hash.to_string(),"index":i.previous_output.index,"pubkey":hex(&i.public_key.to_compressed())?}))).collect::<Result<Vec<_>, Error>>()?;
        let outputs: Vec<_> = tx.outputs.iter().map(|o| json!({"address":o.address.to_checksum(),"denomination":o.denomination.index(),"lock":"0x"})).collect();
        Ok(
            json!({"type":2,"chainId":tx.chain_id.to_string(),"signature":signature.map(|s|hex(&s.to_bytes())).transpose()?,"hash":hash,"txInputs":inputs,"txOutputs":outputs,"data":if tx.data.is_empty(){None}else{Some(hex(&tx.data)?)} }),
        )
    }
    /// Import a caller-parsed JSON value. Unknown fields and supplied hash/sender
    /// mismatches reject. Signature imports verify the complete normalized payload.
    /// This API does not inspect duplicate keys already lost by a caller's parser.
    /// Required numerical fields accept exact unsigned decimal/hex strings or u64
    /// JSON integers; negative, fractional, floating or coerced values reject.
    pub fn from_json(value: &Value) -> Result<Self, Error> {
        budget(value)?;
        let object = value.as_object().ok_or(Error::InvalidField("document"))?;
        let qi = object.contains_key("txInputs") || object.contains_key("txOutputs");
        if !value["type"].is_null() && number(&value["type"])? != U256::from(if qi { 2 } else { 0 })
        {
            return Err(Error::WrongType);
        }
        let result = if qi {
            keys(
                object,
                &[
                    "type",
                    "chainId",
                    "signature",
                    "hash",
                    "txInputs",
                    "txOutputs",
                    "data",
                ],
            )?;
            let inputs = array(&value["txInputs"])?
                .iter()
                .map(|i| {
                    keys(
                        i.as_object().ok_or(Error::InvalidField("input"))?,
                        &["txhash", "index", "pubkey"],
                    )?;
                    Ok(QiInput {
                        previous_output: OutPoint {
                            transaction_hash: parse(&i["txhash"], "input hash")?,
                            index: u16::try_from(small(&i["index"])?)
                                .map_err(|_| Error::InvalidField("input index"))?,
                        },
                        public_key: PublicKey::from_sec1_bytes(&bytes(&i["pubkey"], false)?)
                            .map_err(|_| Error::InvalidField("public key"))?,
                    })
                })
                .collect::<Result<Vec<_>, Error>>()?;
            let outputs = array(&value["txOutputs"])?
                .iter()
                .map(|o| {
                    keys(
                        o.as_object().ok_or(Error::InvalidField("output"))?,
                        &["address", "denomination", "lock"],
                    )?;
                    if !o["lock"].is_null() && o["lock"] != "0x" {
                        return Err(Error::InvalidField("output lock"));
                    }
                    Ok(QiOutput {
                        address: parse(&o["address"], "output address")?,
                        denomination: Denomination::new(
                            u8::try_from(small(&o["denomination"])?)
                                .map_err(|_| Error::InvalidField("denomination"))?,
                        )?,
                    })
                })
                .collect::<Result<Vec<_>, Error>>()?;
            let tx = QiTransaction {
                chain_id: number(&value["chainId"])?,
                inputs,
                outputs,
                data: bytes(&value["data"], true)?,
            };
            unsigned_qi_bytes(&tx)?;
            if value["signature"].is_null() {
                Self::UnsignedQi(tx)
            } else {
                let raw: [u8; 64] = bytes(&value["signature"], false)?
                    .try_into()
                    .map_err(|_| Error::InvalidSignature)?;
                let sig =
                    SchnorrSignature::from_bytes(&raw).map_err(|_| Error::InvalidSignature)?;
                Self::SignedQi(match tx.data.len() {
                    0 => SignedQiOperation::Transfer(tx.attach_signature(sig)?),
                    20 => SignedQiOperation::Wrapping(
                        QiWrappingTransaction::from_transaction(tx)?.attach_signature(sig)?,
                    ),
                    22 => SignedQiOperation::Conversion(
                        QiConversionTransaction::from_transaction(tx)?.attach_signature(sig)?,
                    ),
                    _ => return Err(Error::InvalidField("Qi data")),
                })
            }
        } else {
            keys(
                object,
                &[
                    "type",
                    "to",
                    "from",
                    "data",
                    "nonce",
                    "gasLimit",
                    "gasPrice",
                    "value",
                    "chainId",
                    "signature",
                    "hash",
                    "accessList",
                ],
            )?;
            let tx = QuaiTransaction {
                chain_id: number(&value["chainId"])?,
                nonce: small(&value["nonce"])?,
                to: if value["to"].is_null() {
                    None
                } else {
                    Some(parse(&value["to"], "to")?)
                },
                value: number(&value["value"])?,
                gas_limit: small(&value["gasLimit"])?,
                gas_price: number(&value["gasPrice"])?,
                data: bytes(&value["data"], true)?,
                access_list: if value["accessList"].is_null() {
                    Vec::new()
                } else {
                    access_list_from_json(&value["accessList"])?
                },
            };
            tx.unsigned_bytes()?;
            let from: Option<QuaiAddress> = if value["from"].is_null() {
                None
            } else {
                Some(parse(&value["from"], "from")?)
            };
            if value["signature"].is_null() {
                Self::UnsignedQuai {
                    transaction: tx,
                    claimed_sender: from,
                }
            } else {
                let signed = tx.attach_signature(signature(&value["signature"])?)?;
                if from.is_some_and(|f| f != signed.from()) {
                    return Err(Error::InvalidSignature);
                }
                Self::SignedQuai(signed)
            }
        };
        if !value["hash"].is_null() && result.hash()? != Some(parse(&value["hash"], "hash")?) {
            return Err(Error::InvalidSignature);
        }
        Ok(result)
    }
}
fn unsigned_qi_bytes(tx: &QiTransaction) -> Result<Vec<u8>, Error> {
    match tx.data.len() {
        0 => tx.unsigned_bytes(),
        20 => {
            tx.validate_for(crate::qi::QiMode::Wrapping)?;
            QiWrappingTransaction::from_transaction(tx.clone())?.unsigned_bytes()
        }
        22 => {
            tx.validate_for(crate::qi::QiMode::Conversion)?;
            QiConversionTransaction::from_transaction(tx.clone())?.unsigned_bytes()
        }
        _ => Err(Error::InvalidField("unsupported Qi operation data")),
    }
}
fn hex(bytes: &[u8]) -> Result<String, Error> {
    quai_primitives::hexlify(bytes).map_err(|_| Error::TooLarge)
}
fn bytes(value: &Value, empty_null: bool) -> Result<Vec<u8>, Error> {
    if empty_null && value.is_null() {
        return Ok(Vec::new());
    }
    quai_primitives::get_bytes(value.as_str().ok_or(Error::InvalidField("bytes"))?)
        .map_err(|_| Error::InvalidField("bytes"))
}
fn parse<T: std::str::FromStr>(v: &Value, name: &'static str) -> Result<T, Error> {
    v.as_str()
        .ok_or(Error::InvalidField(name))?
        .parse()
        .map_err(|_| Error::InvalidField(name))
}
fn number(value: &Value) -> Result<U256, Error> {
    if let Some(n) = value.as_u64() {
        return Ok(U256::from(n));
    }
    let s = value.as_str().ok_or(Error::InvalidField("quantity"))?;
    let (digits, radix) = s.strip_prefix("0x").map_or((s, 10), |s| (s, 16));
    if digits.is_empty()
        || !digits.bytes().all(|b| {
            if radix == 16 {
                b.is_ascii_hexdigit()
            } else {
                b.is_ascii_digit()
            }
        })
    {
        return Err(Error::InvalidField("quantity"));
    }
    U256::from_str_radix(digits, radix).map_err(|_| Error::InvalidField("quantity"))
}
fn small(value: &Value) -> Result<u64, Error> {
    u64::try_from(number(value)?).map_err(|_| Error::InvalidField("u64 quantity"))
}
fn array(value: &Value) -> Result<&Vec<Value>, Error> {
    value.as_array().ok_or(Error::InvalidField("array"))
}
fn keys(value: &Map<String, Value>, allowed: &[&str]) -> Result<(), Error> {
    if value.keys().any(|k| !allowed.contains(&k.as_str())) {
        Err(Error::InvalidField("unknown field"))
    } else {
        Ok(())
    }
}
fn signature(value: &Value) -> Result<RecoverableSignature, Error> {
    if value.is_string() {
        let bytes = bytes(value, false)?;
        return match bytes.len() {
            64 => RecoverableSignature::from_eip2098(bytes.as_slice().try_into().unwrap()),
            65 => RecoverableSignature::from_quais_bytes(bytes.as_slice().try_into().unwrap()),
            _ => return Err(Error::InvalidSignature),
        }
        .map_err(|_| Error::InvalidSignature);
    }
    keys(
        value.as_object().ok_or(Error::InvalidSignature)?,
        &["_type", "r", "s", "v", "networkV", "yParity"],
    )?;
    if !value["_type"].is_null() && value["_type"] != "signature" {
        return Err(Error::InvalidSignature);
    }
    let r = bytes(&value["r"], false)?
        .try_into()
        .map_err(|_| Error::InvalidSignature)?;
    let s = bytes(&value["s"], false)?
        .try_into()
        .map_err(|_| Error::InvalidSignature)?;
    let v = if value["v"].is_null() {
        number(&value["yParity"])?
    } else {
        number(&value["v"])?
    };
    let sig = SignatureMetadata::from_rs_v(r, s, v)
        .map_err(|_| Error::InvalidSignature)?
        .signature();
    if !value["yParity"].is_null() && number(&value["yParity"])? != U256::from(sig.recovery_id()) {
        return Err(Error::InvalidSignature);
    }
    if !value["networkV"].is_null() {
        let metadata = SignatureMetadata::from_rs_v(r, s, number(&value["networkV"])?)
            .map_err(|_| Error::InvalidSignature)?;
        if metadata.network_v().is_none() || metadata.signature() != sig {
            return Err(Error::InvalidSignature);
        }
    }
    Ok(sig)
}
fn budget(value: &Value) -> Result<(), Error> {
    fn visit(v: &Value, depth: usize, nodes: &mut usize, bytes: &mut usize) -> Result<(), Error> {
        if depth > 64 {
            return Err(Error::TooLarge);
        }
        *nodes = nodes.checked_sub(1).ok_or(Error::TooLarge)?;
        match v {
            Value::String(s) => *bytes = bytes.checked_sub(s.len()).ok_or(Error::TooLarge)?,
            Value::Array(a) => {
                for v in a {
                    visit(v, depth + 1, nodes, bytes)?;
                }
            }
            Value::Object(o) => {
                for (k, v) in o {
                    *bytes = bytes.checked_sub(k.len()).ok_or(Error::TooLarge)?;
                    visit(v, depth + 1, nodes, bytes)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    let mut nodes = MAX_DOCUMENT_NODES;
    let mut bytes = MAX_DOCUMENT_BYTES;
    visit(value, 0, &mut nodes, &mut bytes)
}

/// Normalize published list/tuple/map access-list shapes. Ordered list forms
/// retain duplicates and order. Map keys are sorted by decoded address bytes;
/// storage keys are deduplicated and sorted by decoded bytes. This deliberately
/// avoids host-locale/case-dependent sorting and fixes duplicate hex-case slots.
pub fn access_list_from_json(value: &Value) -> Result<Vec<AccessTuple>, Error> {
    budget(value)?;
    fn tuple(address: &Value, slots: &Value) -> Result<AccessTuple, Error> {
        Ok(AccessTuple {
            address: parse(address, "access address")?,
            storage_keys: array(slots)?
                .iter()
                .map(|k| parse(k, "storage key"))
                .collect::<Result<_, _>>()?,
        })
    }
    let result = if let Some(list) = value.as_array() {
        list.iter()
            .map(|entry| {
                if let Some(pair) = entry.as_array() {
                    if pair.len() != 2 {
                        return Err(Error::InvalidField("access tuple"));
                    }
                    tuple(&pair[0], &pair[1])
                } else {
                    keys(
                        entry
                            .as_object()
                            .ok_or(Error::InvalidField("access tuple"))?,
                        &["address", "storageKeys"],
                    )?;
                    tuple(&entry["address"], &entry["storageKeys"])
                }
            })
            .collect::<Result<Vec<_>, Error>>()?
    } else {
        let map = value
            .as_object()
            .ok_or(Error::InvalidField("access list"))?;
        let mut result = Vec::with_capacity(map.len());
        let mut addresses = BTreeSet::new();
        for (address, slots) in map {
            let mut item = tuple(&Value::String(address.clone()), slots)?;
            if !addresses.insert(item.address) {
                return Err(Error::InvalidField("duplicate access map address"));
            }
            item.storage_keys.sort();
            item.storage_keys.dedup();
            result.push(item);
        }
        result.sort_by_key(|a| a.address);
        result
    };
    check_access_list(&result)?;
    Ok(result)
}
/// Export ordered access tuples without changing duplicates or signing order.
pub fn access_list_to_json(value: &[AccessTuple]) -> Result<Value, Error> {
    check_access_list(value)?;
    Ok(Value::Array(value.iter().map(|a|json!({"address":a.address.to_checksum(),"storageKeys":a.storage_keys.iter().map(ToString::to_string).collect::<Vec<_>>()})).collect()))
}
fn check_access_list(value: &[AccessTuple]) -> Result<(), Error> {
    let count = value
        .iter()
        .try_fold(2usize, |n, a| {
            n.checked_add(1)?.checked_add(a.storage_keys.len())
        })
        .ok_or(Error::TooLarge)?;
    if count > crate::MAX_TRANSACTION_MESSAGES {
        return Err(Error::TooLarge);
    }
    Ok(())
}
