//! Recompute a Quai zone header's hashes from the node's JSON, binding its
//! state roots to the hashes other nodes report.
//!
//! go-quai hashes protobuf encodings with BLAKE3. `headerHash` covers every
//! root in the header (`evmRoot`, `utxoRoot`, `etxSetRoot` and the rest) and is
//! recomputable from any `quai_getHeaderByNumber` response. The block hash
//! covers `headerHash` through the work-object seal. It is recomputable before
//! the KawPoW fork from the same response, and after it only from the v2
//! fields (`auxpow` and the share counters) that a default node omits.
//!
//! A recomputed hash shows the header is self-consistent. It does not show the
//! block is canonical, and a node that lies can report a consistent header of
//! its own. What it adds is that one hash, compared with a second node at the
//! same height, confirms every root at once.
use quai_primitives::{Hash32, Zone};
use serde_json::{Map, Value};
use thiserror::Error;

/// Prime terminus height at which KawPoW merged mining replaced ProgPoW.
pub const KAWPOW_FORK_PRIME_TERMINUS: u64 = 1_171_500;

/// Header fields this verifier knows, hashed or not. Any other field fails
/// closed: a new seal field would otherwise surface as a hash mismatch.
const HEADER_FIELDS: &[&str] = &[
    "avgTxFees",
    "baseFeePerGas",
    "conversionFlowAmount",
    "efficiencyScore",
    "etxEligibleSlices",
    "etxRollupRoot",
    "etxSetRoot",
    "evmRoot",
    "exchangeRate",
    "expansionNumber",
    "extraData",
    "gasLimit",
    "gasUsed",
    "interlinkRootHash",
    "kQuaiDiscount",
    "manifestHash",
    "minerDifficulty",
    "number",
    "outboundEtxsRoot",
    "parentDeltaEntropy",
    "parentEntropy",
    "parentHash",
    "parentUncledDeltaEntropy",
    "primeStateRoot",
    "primeTerminusHash",
    "quaiStateSize",
    "receiptsRoot",
    "regionStateRoot",
    "size",
    "stateLimit",
    "stateUsed",
    "thresholdCount",
    "totalFees",
    "transactionsRoot",
    "uncleHash",
    "uncledEntropy",
    "utxoRoot",
    "woHeader",
];
/// Work-object header fields in v1 responses, then those only v2 adds.
const WORK_OBJECT_FIELDS: &[&str] = &[
    "data",
    "difficulty",
    "hash",
    "headerHash",
    "location",
    "lock",
    "mixHash",
    "nonce",
    "number",
    "parentHash",
    "primaryCoinbase",
    "primeTerminusNumber",
    "timestamp",
    "txHash",
    "auxpow",
    "scryptDiffAndCount",
    "shaDiffAndCount",
    "shaShareTarget",
    "scryptShareTarget",
    "kawpowDifficulty",
];
const AUXPOW_FIELDS: &[&str] = &[
    "auxpow2",
    "header",
    "merkleBranch",
    "powId",
    "signature",
    "transaction",
];
/// Proof-of-work chains whose coinbase commits the seal hash directly.
/// Scrypt (4) commits a merged-mining merkle root instead.
const DIRECT_COMMITMENT_POW_IDS: &[u64] = &[1, 2, 3];

/// Why a header's hashes could not be confirmed.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
#[non_exhaustive]
pub enum HeaderHashError {
    /// A field the hash needs is absent.
    #[error("header field {0} is missing")]
    Missing(&'static str),
    /// A field is not the shape go-quai sends.
    #[error("header field {0} is malformed")]
    Malformed(&'static str),
    /// A field this SDK does not know. The node may run a newer go-quai whose
    /// header hashes a field this version cannot; update the SDK.
    #[error("header field {0} is not known to this SDK version")]
    UnknownField(String),
    /// The header's fields do not hash to its reported `headerHash`.
    #[error("header fields do not hash to the reported header hash")]
    HeaderHashMismatch,
    /// The work-object fields do not hash to the reported block hash.
    #[error("work-object fields do not hash to the reported block hash")]
    HashMismatch,
    /// The merged-mining coinbase does not commit to this header's seal.
    #[error("merged-mining coinbase does not commit to the header's seal")]
    SealNotCommitted,
}

/// A header whose hashes were recomputed from its own fields.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct VerifiedHeader {
    /// Recomputed, and equal to the reported `woHeader.headerHash`. It covers
    /// every root below.
    pub header_hash: Hash32,
    /// The work-object seal hash, when every seal field was present.
    pub seal_hash: Option<Hash32>,
    /// The reported block hash, `woHeader.hash`.
    pub hash: Hash32,
    /// Whether `hash` was recomputed and matched. Before the KawPoW fork this
    /// needs only v1 fields; after it, the v2 `auxpow`, whose coinbase must
    /// also commit the seal hash. Proof-of-work itself is not checked.
    pub hash_verified: bool,
    /// Zone block height.
    pub number: u64,
    /// Zone the header belongs to; `None` for the genesis, whose location is
    /// the root.
    pub zone: Option<Zone>,
    /// Prime height this block references.
    pub prime_terminus_number: u64,
    /// Block time, in seconds since the Unix epoch. Part of the seal, not of
    /// `header_hash`.
    pub timestamp: u64,
    /// Root of the account state trie that `quai_getProof` proves against.
    pub evm_root: Hash32,
    /// Root of the Qi UTXO set.
    pub utxo_root: Hash32,
    /// Root of the pending external-transaction set.
    pub etx_set_root: Hash32,
}

/// Recompute a zone header's hashes from a `quai_getHeaderByNumber` result, or
/// from a work object with its header under `woBody` (as `newHeadsV2` sends).
///
/// Always recomputes `headerHash` and requires it to equal the reported one.
/// Recomputes the block hash when the fields allow; a mismatch there is an
/// error too. Fails closed on any header field this SDK does not know.
pub fn verify_header_hash(value: &Value) -> Result<VerifiedHeader, HeaderHashError> {
    let top = value
        .as_object()
        .ok_or(HeaderHashError::Malformed("header"))?;
    let work = top
        .get("woHeader")
        .ok_or(HeaderHashError::Missing("woHeader"))?
        .as_object()
        .ok_or(HeaderHashError::Malformed("woHeader"))?;
    let header = match top.get("woBody") {
        Some(body) => body
            .get("header")
            .and_then(Value::as_object)
            .ok_or(HeaderHashError::Malformed("woBody"))?,
        None => top,
    };
    known(header, HEADER_FIELDS)?;
    known(work, WORK_OBJECT_FIELDS)?;
    let header_hash = header_seal_hash(header, hash(work, "headerHash")?)?;

    let prime_terminus_number = uint(work, "primeTerminusNumber")?;
    let kawpow = prime_terminus_number >= KAWPOW_FORK_PRIME_TERMINUS;
    let reported_hash = hash(work, "hash")?;
    let seal_hash = seal_hash(work, kawpow)?;
    let hash_verified = match (seal_hash, kawpow) {
        (Some(seal), false) => {
            let mut preimage = Vec::with_capacity(72);
            preimage.extend_from_slice(&fixed::<32>(work, "mixHash")?);
            preimage.extend_from_slice(&seal);
            preimage.extend_from_slice(&fixed::<8>(work, "nonce")?);
            check(blake3(&preimage), reported_hash)?
        }
        (Some(seal), true) => match work.get("auxpow") {
            Some(auxpow) => aux_hash(auxpow, seal, reported_hash)?,
            // A transition block without auxpow, or a v1 view: not recomputable.
            None => false,
        },
        (None, _) => false,
    };

    let zone = match bytes(work, "location")?[..] {
        [] => None,
        [region, zone] if region <= 2 && zone <= 2 => Some(
            Zone::from_byte((region << 4) | zone)
                .map_err(|_| HeaderHashError::Malformed("location"))?,
        ),
        _ => return Err(HeaderHashError::Malformed("location")),
    };
    Ok(VerifiedHeader {
        header_hash,
        seal_hash: seal_hash.map(Hash32::from_bytes),
        hash: reported_hash,
        hash_verified,
        number: uint(work, "number")?,
        zone,
        prime_terminus_number,
        timestamp: uint(work, "timestamp")?,
        evm_root: hash(header, "evmRoot")?,
        utxo_root: hash(header, "utxoRoot")?,
        etx_set_root: hash(header, "etxSetRoot")?,
    })
}

fn check(recomputed: [u8; 32], reported: Hash32) -> Result<bool, HeaderHashError> {
    if Hash32::from_bytes(recomputed) == reported {
        Ok(true)
    } else {
        Err(HeaderHashError::HashMismatch)
    }
}

/// `blake3(proto(Header.SealEncode()))`, go-quai `Header.Hash`, when it equals
/// `reported`.
fn header_seal_hash(h: &Map<String, Value>, reported: Hash32) -> Result<Hash32, HeaderHashError> {
    let mut out = Proto::default();
    for parent in list(h, "parentHash", 2)? {
        out.hash(1, &fixed_value::<32>(parent, "parentHash")?);
    }
    out.hash(2, &fixed::<32>(h, "uncleHash")?);
    out.hash(3, &fixed::<32>(h, "evmRoot")?);
    out.hash(4, &fixed::<32>(h, "transactionsRoot")?);
    out.hash(5, &fixed::<32>(h, "outboundEtxsRoot")?);
    out.hash(6, &fixed::<32>(h, "etxRollupRoot")?);
    for manifest in list(h, "manifestHash", 3)? {
        out.hash(7, &fixed_value::<32>(manifest, "manifestHash")?);
    }
    out.hash(8, &fixed::<32>(h, "receiptsRoot")?);
    // 9 (difficulty) is not sealed.
    for (field, name) in [
        (10, "parentEntropy"),
        (11, "parentDeltaEntropy"),
        (12, "parentUncledDeltaEntropy"),
    ] {
        for entropy in list(h, name, 3)? {
            out.bytes(field, &big_value(entropy, name)?);
        }
    }
    out.bytes(13, &big(h, "uncledEntropy")?);
    for number in list(h, "number", 2)? {
        out.bytes(14, &big_value(number, "number")?);
    }
    out.varint(15, uint(h, "gasLimit")?);
    out.varint(16, uint(h, "gasUsed")?);
    out.bytes(17, &big(h, "baseFeePerGas")?);
    // 18 (location), 20 (mixHash) and 21 (nonce) are not sealed.
    let extra = bytes(h, "extraData")?;
    let extra_at = out.0.len();
    out.bytes(19, &extra);
    let extra_end = out.0.len();
    out.hash(22, &fixed::<32>(h, "utxoRoot")?);
    out.hash(23, &fixed::<32>(h, "etxSetRoot")?);
    out.varint(24, uint(h, "efficiencyScore")?);
    out.varint(25, uint(h, "thresholdCount")?);
    out.varint(26, uint(h, "expansionNumber")?);
    out.hash(27, &fixed::<32>(h, "etxEligibleSlices")?);
    out.hash(28, &fixed::<32>(h, "primeTerminusHash")?);
    out.hash(29, &fixed::<32>(h, "interlinkRootHash")?);
    out.varint(30, uint(h, "stateLimit")?);
    out.varint(31, uint(h, "stateUsed")?);
    out.bytes(32, &big(h, "quaiStateSize")?);
    out.bytes(33, &big(h, "exchangeRate")?);
    out.bytes(35, &big(h, "avgTxFees")?);
    out.bytes(36, &big(h, "totalFees")?);
    out.bytes(37, &big(h, "kQuaiDiscount")?);
    out.bytes(38, &big(h, "conversionFlowAmount")?);
    out.bytes(39, &big(h, "minerDifficulty")?);
    out.hash(40, &fixed::<32>(h, "primeStateRoot")?);
    out.hash(41, &fixed::<32>(h, "regionStateRoot")?);
    // `extra` is an optional field, and go-quai sends present-but-empty and
    // absent alike as "0x", so an empty one is tried both ways.
    let mut without = out.0[..extra_at].to_vec();
    without.extend_from_slice(&out.0[extra_end..]);
    let candidates = [Some(&out.0), extra.is_empty().then_some(&without)];
    candidates
        .into_iter()
        .flatten()
        .map(|encoded| Hash32::from_bytes(blake3(encoded)))
        .find(|recomputed| *recomputed == reported)
        .ok_or(HeaderHashError::HeaderHashMismatch)
}

/// `WorkObjectHeader.SealHash`, or `None` when a seal field is absent (a v1
/// view of a post-fork header).
fn seal_hash(w: &Map<String, Value>, kawpow: bool) -> Result<Option<[u8; 32]>, HeaderHashError> {
    let post_fork = [
        "scryptDiffAndCount",
        "shaDiffAndCount",
        "shaShareTarget",
        "scryptShareTarget",
        "kawpowDifficulty",
    ];
    if kawpow && post_fork.iter().any(|field| !w.contains_key(*field)) {
        return Ok(None);
    }
    let mut out = Proto::default();
    out.hash(1, &fixed::<32>(w, "headerHash")?);
    out.hash(2, &fixed::<32>(w, "parentHash")?);
    out.bytes(3, &big(w, "number")?);
    out.bytes(4, &big(w, "difficulty")?);
    out.hash(5, &fixed::<32>(w, "txHash")?);
    let location = bytes(w, "location")?;
    let mut inner = Proto::default();
    inner.optional_bytes(1, &location);
    out.bytes(7, &inner.0);
    out.varint(9, uint(w, "timestamp")?);
    out.bytes(10, &big(w, "primeTerminusNumber")?);
    out.varint(11, uint(w, "lock")?);
    let coinbase = fixed::<20>(w, "primaryCoinbase")?;
    if !kawpow {
        let mut address = Proto::default();
        address.bytes(1, &coinbase);
        out.bytes(12, &address.0);
    }
    out.optional_bytes(13, &bytes(w, "data")?);
    if !kawpow {
        return Ok(Some(blake3(&out.0)));
    }
    out.bytes(15, &share(w, "scryptDiffAndCount")?);
    out.bytes(16, &share(w, "shaDiffAndCount")?);
    out.bytes(17, &big(w, "shaShareTarget")?);
    out.bytes(18, &big(w, "scryptShareTarget")?);
    out.bytes(19, &big(w, "kawpowDifficulty")?);
    let mut preimage = blake3(&out.0).to_vec();
    preimage.extend_from_slice(&coinbase);
    Ok(Some(blake3(&preimage)))
}

/// `PowShareDiffAndCount.ProtoEncode`: zero is `[0]`, not empty.
fn share(w: &Map<String, Value>, name: &'static str) -> Result<Vec<u8>, HeaderHashError> {
    let share = w
        .get(name)
        .and_then(Value::as_object)
        .ok_or(HeaderHashError::Malformed(name))?;
    let mut out = Proto::default();
    for (field, key) in [(1, "difficulty"), (2, "count"), (3, "uncled")] {
        let value = big(share, key).map_err(|_| HeaderHashError::Malformed(name))?;
        out.bytes(field, if value.is_empty() { &[0] } else { &value });
    }
    Ok(out.0)
}

/// Recompute a post-fork block hash from its auxpow, after checking that the
/// merged-mined coinbase commits the seal. `false` for a chain whose
/// commitment this SDK does not check.
fn aux_hash(auxpow: &Value, seal: [u8; 32], reported: Hash32) -> Result<bool, HeaderHashError> {
    let a = auxpow
        .as_object()
        .ok_or(HeaderHashError::Malformed("auxpow"))?;
    known(a, AUXPOW_FIELDS)?;
    let pow_id = uint(a, "powId")?;
    if !DIRECT_COMMITMENT_POW_IDS.contains(&pow_id) {
        return Ok(false);
    }
    let transaction = bytes(a, "transaction")?;
    if coinbase_commitment(&transaction)? != seal {
        return Err(HeaderHashError::SealNotCommitted);
    }
    let mut out = Proto::default();
    out.varint(1, pow_id);
    out.bytes(2, &bytes(a, "header")?);
    out.bytes(3, &bytes(a, "signature")?);
    for branch in a
        .get("merkleBranch")
        .and_then(Value::as_array)
        .ok_or(HeaderHashError::Malformed("merkleBranch"))?
    {
        out.bytes(4, &bytes_value(branch, "merkleBranch")?);
    }
    out.bytes(5, &transaction);
    // 6 (signature time) is never encoded.
    out.bytes(7, &bytes(a, "auxpow2")?);
    check(blake3(&out.0), reported)
}

/// The 32 bytes after the `fabe6d6d` marker in a coinbase scriptSig, as
/// go-quai `ExtractSealHashFromCoinbase` reads them.
fn coinbase_commitment(transaction: &[u8]) -> Result<[u8; 32], HeaderHashError> {
    const BAD: HeaderHashError = HeaderHashError::Malformed("auxpow transaction");
    let mut at = 4; // version
    let (_, next) = compact_size(transaction, at).ok_or(BAD)?; // input count
    at = next.checked_add(36).ok_or(BAD)?; // previous outpoint
    let (length, next) = compact_size(transaction, at).ok_or(BAD)?;
    let script = usize::try_from(length)
        .ok()
        .and_then(|length| transaction.get(next..next.checked_add(length)?))
        .ok_or(BAD)?;
    let push = |at: usize| {
        let length = usize::from(*script.get(at)?);
        if length > 75 {
            return None;
        }
        Some((script.get(at + 1..at + 1 + length)?, at + 1 + length))
    };
    let (height, at) = push(0).ok_or(BAD)?;
    let (payload, _) = push(at).ok_or(BAD)?;
    if height.len() > 5 || payload.len() != 44 || payload[..4] != [0xfa, 0xbe, 0x6d, 0x6d] {
        return Err(HeaderHashError::SealNotCommitted);
    }
    Ok(payload[4..36].try_into().expect("32 bytes"))
}

/// A Bitcoin compact-size integer at `at`, and the offset after it.
fn compact_size(input: &[u8], at: usize) -> Option<(u64, usize)> {
    let width = match *input.get(at)? {
        first @ 0..0xfd => return Some((u64::from(first), at + 1)),
        0xfd => 2,
        0xfe => 4,
        0xff => 8,
    };
    let bytes = input.get(at + 1..at + 1 + width)?;
    let mut value = [0; 8];
    value[..width].copy_from_slice(bytes);
    Some((u64::from_le_bytes(value), at + 1 + width))
}

fn blake3(input: &[u8]) -> [u8; 32] {
    *::blake3::hash(input).as_bytes()
}

/// A protobuf message writer for the wire types go-quai's seals use.
#[derive(Default)]
struct Proto(Vec<u8>);
impl Proto {
    fn raw_varint(&mut self, mut value: u64) {
        while value >= 0x80 {
            self.0.push((value as u8) | 0x80);
            value >>= 7;
        }
        self.0.push(value as u8);
    }
    /// An optional scalar: emitted even when zero.
    fn varint(&mut self, field: u64, value: u64) {
        self.raw_varint(field << 3);
        self.raw_varint(value);
    }
    /// An optional or repeated bytes field: emitted even when empty.
    fn bytes(&mut self, field: u64, value: &[u8]) {
        self.raw_varint((field << 3) | 2);
        self.raw_varint(value.len() as u64);
        self.0.extend_from_slice(value);
    }
    /// A plain proto3 bytes field: omitted when empty.
    fn optional_bytes(&mut self, field: u64, value: &[u8]) {
        if !value.is_empty() {
            self.bytes(field, value);
        }
    }
    /// A `ProtoHash` message, `{1: 32 bytes}`.
    fn hash(&mut self, field: u64, value: &[u8; 32]) {
        let mut inner = Proto::default();
        inner.bytes(1, value);
        self.bytes(field, &inner.0);
    }
}

fn known(object: &Map<String, Value>, fields: &[&str]) -> Result<(), HeaderHashError> {
    match object.keys().find(|key| !fields.contains(&key.as_str())) {
        Some(key) => Err(HeaderHashError::UnknownField(key.clone())),
        None => Ok(()),
    }
}
fn field<'a>(o: &'a Map<String, Value>, name: &'static str) -> Result<&'a Value, HeaderHashError> {
    o.get(name).ok_or(HeaderHashError::Missing(name))
}
fn list<'a>(
    o: &'a Map<String, Value>,
    name: &'static str,
    length: usize,
) -> Result<&'a [Value], HeaderHashError> {
    match field(o, name)?.as_array() {
        Some(items) if items.len() == length => Ok(items),
        _ => Err(HeaderHashError::Malformed(name)),
    }
}
/// Even-length `0x` hex bytes.
fn bytes_value(value: &Value, name: &'static str) -> Result<Vec<u8>, HeaderHashError> {
    let text = value
        .as_str()
        .and_then(|text| text.strip_prefix("0x"))
        .ok_or(HeaderHashError::Malformed(name))?;
    decode_hex(text).ok_or(HeaderHashError::Malformed(name))
}
fn bytes(o: &Map<String, Value>, name: &'static str) -> Result<Vec<u8>, HeaderHashError> {
    bytes_value(field(o, name)?, name)
}
fn fixed_value<const N: usize>(
    value: &Value,
    name: &'static str,
) -> Result<[u8; N], HeaderHashError> {
    bytes_value(value, name)?
        .try_into()
        .map_err(|_| HeaderHashError::Malformed(name))
}
fn fixed<const N: usize>(
    o: &Map<String, Value>,
    name: &'static str,
) -> Result<[u8; N], HeaderHashError> {
    fixed_value(field(o, name)?, name)
}
fn hash(o: &Map<String, Value>, name: &'static str) -> Result<Hash32, HeaderHashError> {
    fixed::<32>(o, name).map(Hash32::from_bytes)
}
/// A hex quantity as minimal big-endian bytes, as `big.Int.Bytes` gives them:
/// zero is empty.
fn big_value(value: &Value, name: &'static str) -> Result<Vec<u8>, HeaderHashError> {
    let text = value
        .as_str()
        .and_then(|text| text.strip_prefix("0x"))
        .filter(|text| !text.is_empty() && text.len() <= 128)
        .ok_or(HeaderHashError::Malformed(name))?;
    let padded = if text.len() % 2 == 1 {
        format!("0{text}")
    } else {
        text.to_owned()
    };
    let decoded = decode_hex(&padded).ok_or(HeaderHashError::Malformed(name))?;
    let first = decoded
        .iter()
        .position(|&b| b != 0)
        .unwrap_or(decoded.len());
    Ok(decoded[first..].to_vec())
}
fn big(o: &Map<String, Value>, name: &'static str) -> Result<Vec<u8>, HeaderHashError> {
    big_value(field(o, name)?, name)
}
fn uint(o: &Map<String, Value>, name: &'static str) -> Result<u64, HeaderHashError> {
    let bytes = big(o, name)?;
    if bytes.len() > 8 {
        return Err(HeaderHashError::Malformed(name));
    }
    Ok(bytes.iter().fold(0, |n, &b| (n << 8) | u64::from(b)))
}
fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 == 1 {
        return None;
    }
    let digit = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    text.as_bytes()
        .chunks(2)
        .map(|pair| Some((digit(pair[0])? << 4) | digit(pair[1])?))
        .collect()
}
