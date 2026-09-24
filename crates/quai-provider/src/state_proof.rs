//! Merkle-Patricia proofs of Quai account state, checked without trusting the node.
//!
//! A proof shows what an account or storage slot holds under one state root.
//! It says nothing about whether that root belongs to the canonical chain: the
//! root comes from a header the caller must trust separately.
use quai_primitives::{Hash32, QuaiAddress};
use quai_rpc::U256;
use thiserror::Error;

/// Most nodes one proof path may hold. A 64-nibble key cannot need more.
pub const MAX_PROOF_NODES: usize = 64;
/// Largest proof node accepted. A full branch node is 532 bytes.
pub const MAX_PROOF_NODE_BYTES: usize = 1024;
/// Root of a trie with no entries: Keccak-256 of the RLP empty string.
pub const EMPTY_TRIE_ROOT: Hash32 = Hash32::from_bytes([
    0x56, 0xe8, 0x1f, 0x17, 0x1b, 0xcc, 0x55, 0xa6, 0xff, 0x83, 0x45, 0xe6, 0x92, 0xc0, 0xf8, 0x6e,
    0x5b, 0x48, 0xe0, 0x1b, 0x99, 0x6c, 0xad, 0xc0, 0x01, 0x62, 0x2f, 0xb5, 0xe3, 0x63, 0xb4, 0x21,
]);
/// Code hash of an account without code: Keccak-256 of no bytes.
pub const EMPTY_CODE_HASH: Hash32 = Hash32::from_bytes([
    0xc5, 0xd2, 0x46, 0x01, 0x86, 0xf7, 0x23, 0x3c, 0x92, 0x7e, 0x7d, 0xb2, 0xdc, 0xc7, 0x03, 0xc0,
    0xe5, 0x00, 0xb6, 0x53, 0xca, 0x82, 0x27, 0x3b, 0x7b, 0xfa, 0xd8, 0x04, 0x5d, 0x85, 0xa4, 0x70,
]);

/// Why a proof does not establish a value.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
#[non_exhaustive]
pub enum ProofError {
    /// More nodes, or a larger node, than the verifier accepts.
    #[error("state proof exceeds its size bounds")]
    TooLarge,
    /// A node is not canonical RLP or not a trie node of the expected shape.
    #[error("malformed state proof: {0}")]
    Malformed(&'static str),
    /// A node does not hash to the reference its parent (or the root) names.
    #[error("state proof node does not match its expected hash")]
    HashMismatch,
    /// The proof ends before the key's path is resolved.
    #[error("state proof is incomplete")]
    Incomplete,
    /// Nodes remain after the key's path is resolved.
    #[error("state proof has unused nodes")]
    UnusedNodes,
    /// The proof verifies, but the values the node reported beside it differ.
    #[error("reported values contradict the state proof")]
    Contradicted,
}

/// A Quai account as its state-trie leaf encodes it.
///
/// Quai adds `size` to Ethereum's four fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Account {
    /// Transactions sent, or contracts created for a contract account.
    pub nonce: u64,
    /// Balance in Its.
    pub balance: U256,
    /// Root of the account's storage trie; [`EMPTY_TRIE_ROOT`] when it has none.
    pub storage_root: Hash32,
    /// Keccak-256 of the runtime code; [`EMPTY_CODE_HASH`] when it has none.
    pub code_hash: Hash32,
    /// Size of the storage trie, as go-quai records it.
    pub size: U256,
}

/// Verify an account proof under `state_root`, a header's `evmRoot`.
///
/// `Ok(None)` is a verified absence: the account has no state at that root.
/// Nodes are in path order from the root, as `quai_getProof` returns them.
pub fn verify_account_proof<B: AsRef<[u8]>>(
    state_root: Hash32,
    address: QuaiAddress,
    proof: &[B],
) -> Result<Option<Account>, ProofError> {
    let key = quai_crypto::keccak256(address.bytes());
    let Some(leaf) = walk(state_root, &key, proof)? else {
        return Ok(None);
    };
    let fields = match rlp::decode(leaf)? {
        rlp::Item::List(fields) if fields.len() == 5 => fields,
        _ => return Err(ProofError::Malformed("account leaf is not five fields")),
    };
    fn bytes<'a>(item: &rlp::Item<'a>) -> Result<&'a [u8], ProofError> {
        match item {
            rlp::Item::Bytes(bytes) => Ok(bytes),
            rlp::Item::List(_) => Err(ProofError::Malformed("account field is a list")),
        }
    }
    let nonce = rlp::uint(bytes(&fields[0])?)?;
    Ok(Some(Account {
        nonce: nonce
            .try_into()
            .map_err(|_| ProofError::Malformed("account nonce exceeds 64 bits"))?,
        balance: rlp::uint(bytes(&fields[1])?)?,
        storage_root: hash32(bytes(&fields[2])?)?,
        code_hash: hash32(bytes(&fields[3])?)?,
        size: rlp::uint(bytes(&fields[4])?)?,
    }))
}

/// Verify a storage proof under an account's `storage_root`.
///
/// A slot with no entry proves zero, which is how the EVM reads it.
pub fn verify_storage_proof<B: AsRef<[u8]>>(
    storage_root: Hash32,
    slot: Hash32,
    proof: &[B],
) -> Result<U256, ProofError> {
    let key = quai_crypto::keccak256(slot.bytes());
    match walk(storage_root, &key, proof)? {
        None => Ok(U256::ZERO),
        Some(leaf) => match rlp::decode(leaf)? {
            rlp::Item::Bytes(value) if !value.is_empty() => rlp::uint(value),
            _ => Err(ProofError::Malformed(
                "storage leaf is not a nonzero integer",
            )),
        },
    }
}

/// The storage slot of `key` in a Solidity mapping declared at `base_slot`:
/// `keccak(key ‖ base_slot)`, both as 32-byte words.
///
/// For an ERC-20 `balanceOf`, `key` is the owner's address left-padded to 32
/// bytes and `base_slot` is where the contract declares the mapping. That slot
/// is the contract's own layout; nothing here can discover it. A nested
/// mapping, such as `allowance[owner][spender]`, applies this twice.
pub fn solidity_mapping_slot(key: Hash32, base_slot: U256) -> Hash32 {
    let mut word = [0u8; 64];
    word[..32].copy_from_slice(key.bytes());
    word[32..].copy_from_slice(&base_slot.to_be_bytes::<32>());
    Hash32::from_bytes(quai_crypto::keccak256(&word))
}

/// An address as a 32-byte mapping key, left-padded with zeros.
pub fn address_word(address: QuaiAddress) -> Hash32 {
    let mut word = [0u8; 32];
    word[12..].copy_from_slice(address.bytes());
    Hash32::from_bytes(word)
}

fn hash32(bytes: &[u8]) -> Result<Hash32, ProofError> {
    <[u8; 32]>::try_from(bytes)
        .map(Hash32::from_bytes)
        .map_err(|_| ProofError::Malformed("expected a 32-byte hash"))
}

/// Where the walk goes next: a node named by hash, or one embedded in its parent.
enum Next<'a> {
    Hash(Hash32),
    Inline(rlp::Item<'a>),
}

/// Follow `key`'s nibbles from `root` through `proof`, returning the leaf value,
/// or `None` for a verified absence.
fn walk<'a, B: AsRef<[u8]>>(
    root: Hash32,
    key: &[u8; 32],
    proof: &'a [B],
) -> Result<Option<&'a [u8]>, ProofError> {
    if proof.len() > MAX_PROOF_NODES
        || proof
            .iter()
            .any(|node| node.as_ref().len() > MAX_PROOF_NODE_BYTES)
    {
        return Err(ProofError::TooLarge);
    }
    if proof.is_empty() {
        return if root == EMPTY_TRIE_ROOT {
            Ok(None)
        } else {
            Err(ProofError::Incomplete)
        };
    }
    let path: Vec<u8> = key.iter().flat_map(|b| [b >> 4, b & 0x0f]).collect();
    let mut at = 0;
    let mut used = 0;
    let mut next = Next::Hash(root);
    let found = loop {
        let node = match next {
            Next::Hash(expected) => {
                let raw = proof.get(used).ok_or(ProofError::Incomplete)?.as_ref();
                used += 1;
                if Hash32::from_bytes(quai_crypto::keccak256(raw)) != expected {
                    return Err(ProofError::HashMismatch);
                }
                rlp::decode(raw)?
            }
            Next::Inline(item) => item,
        };
        let rlp::Item::List(items) = node else {
            return Err(ProofError::Malformed("trie node is not a list"));
        };
        match items.len() {
            17 => {
                // Keys are fixed-length, so a value never ends inside a branch.
                let Some(&nibble) = path.get(at) else {
                    return Err(ProofError::Malformed("path ends at a branch"));
                };
                at += 1;
                match child(
                    items
                        .into_iter()
                        .nth(usize::from(nibble))
                        .expect("17 items"),
                )? {
                    Some(child) => next = child,
                    None => break None,
                }
            }
            2 => {
                let mut items = items.into_iter();
                let (Some(rlp::Item::Bytes(encoded)), Some(value)) = (items.next(), items.next())
                else {
                    return Err(ProofError::Malformed("short node has no encoded path"));
                };
                let (leaf, nibbles) = hex_prefix(encoded)?;
                let rest = &path[at..];
                if leaf && nibbles.len() != rest.len() {
                    // Fixed-length keys: a leaf always spans the whole remaining path.
                    return Err(ProofError::Malformed("leaf path has the wrong length"));
                }
                if !rest.starts_with(&nibbles) {
                    break None;
                }
                at += nibbles.len();
                if leaf {
                    let rlp::Item::Bytes(value) = value else {
                        return Err(ProofError::Malformed("leaf value is a list"));
                    };
                    break Some(value);
                }
                if nibbles.is_empty() || at == path.len() {
                    return Err(ProofError::Malformed("extension node has no effect"));
                }
                next = child(value)?.ok_or(ProofError::Malformed("extension to nothing"))?;
            }
            _ => return Err(ProofError::Malformed("trie node has the wrong arity")),
        }
    };
    if used != proof.len() {
        return Err(ProofError::UnusedNodes);
    }
    Ok(found)
}

/// Interpret a child reference: empty, a 32-byte hash, or an embedded node.
fn child(item: rlp::Item<'_>) -> Result<Option<Next<'_>>, ProofError> {
    match item {
        rlp::Item::Bytes([]) => Ok(None),
        rlp::Item::Bytes(bytes) => Ok(Some(Next::Hash(hash32(bytes)?))),
        list @ rlp::Item::List(_) => Ok(Some(Next::Inline(list))),
    }
}

/// Decode a compact (hex-prefix) path: whether it ends at a leaf, and its nibbles.
fn hex_prefix(encoded: &[u8]) -> Result<(bool, Vec<u8>), ProofError> {
    let (&first, rest) = encoded
        .split_first()
        .ok_or(ProofError::Malformed("empty encoded path"))?;
    let flag = first >> 4;
    if flag > 3 || (flag & 1 == 0 && first & 0x0f != 0) {
        return Err(ProofError::Malformed("invalid encoded path prefix"));
    }
    let mut nibbles = Vec::with_capacity(rest.len() * 2 + 1);
    if flag & 1 == 1 {
        nibbles.push(first & 0x0f);
    }
    nibbles.extend(rest.iter().flat_map(|b| [b >> 4, b & 0x0f]));
    Ok((flag >= 2, nibbles))
}

/// Strict RLP decoding: canonical lengths only, no trailing bytes.
pub(crate) mod rlp {
    use super::ProofError;
    use quai_rpc::U256;

    /// A decoded item borrowing from its input.
    #[derive(Debug, PartialEq)]
    pub(crate) enum Item<'a> {
        Bytes(&'a [u8]),
        List(Vec<Item<'a>>),
    }

    /// Deepest nesting a proof node can need: a branch holding an embedded node.
    const MAX_DEPTH: usize = 8;

    /// Decode one item that spans all of `input`.
    pub(crate) fn decode(input: &[u8]) -> Result<Item<'_>, ProofError> {
        let (item, rest) = item(input, 0)?;
        if !rest.is_empty() {
            return Err(ProofError::Malformed("trailing bytes after RLP item"));
        }
        Ok(item)
    }

    fn item(input: &[u8], depth: usize) -> Result<(Item<'_>, &[u8]), ProofError> {
        if depth > MAX_DEPTH {
            return Err(ProofError::Malformed("RLP nesting too deep"));
        }
        let (&prefix, after) = input
            .split_first()
            .ok_or(ProofError::Malformed("truncated RLP"))?;
        let (list, length, body) = match prefix {
            0x00..=0x7f => return Ok((Item::Bytes(&input[..1]), after)),
            0x80..=0xb7 => (false, usize::from(prefix - 0x80), after),
            0xb8..=0xbf => long(after, usize::from(prefix - 0xb7))
                .map(|(length, body)| (false, length, body))?,
            0xc0..=0xf7 => (true, usize::from(prefix - 0xc0), after),
            0xf8..=0xff => long(after, usize::from(prefix - 0xf7))
                .map(|(length, body)| (true, length, body))?,
        };
        if body.len() < length {
            return Err(ProofError::Malformed("truncated RLP"));
        }
        let (content, rest) = body.split_at(length);
        if !list {
            if length == 1 && content[0] < 0x80 {
                return Err(ProofError::Malformed("non-canonical single-byte RLP"));
            }
            return Ok((Item::Bytes(content), rest));
        }
        let mut items = Vec::new();
        let mut remaining = content;
        while !remaining.is_empty() {
            let (child, next) = item(remaining, depth + 1)?;
            items.push(child);
            remaining = next;
        }
        Ok((Item::List(items), rest))
    }

    /// Read a long-form length: `size` big-endian bytes, canonical and over 55.
    fn long(input: &[u8], size: usize) -> Result<(usize, &[u8]), ProofError> {
        if input.len() < size {
            return Err(ProofError::Malformed("truncated RLP length"));
        }
        let (bytes, body) = input.split_at(size);
        if bytes[0] == 0 || size > std::mem::size_of::<usize>() {
            return Err(ProofError::Malformed("non-canonical RLP length"));
        }
        let length = bytes.iter().fold(0usize, |n, &b| (n << 8) | usize::from(b));
        if length <= 55 {
            return Err(ProofError::Malformed("non-canonical RLP length"));
        }
        Ok((length, body))
    }

    /// A canonical big-endian unsigned integer of at most 32 bytes.
    pub(crate) fn uint(bytes: &[u8]) -> Result<U256, ProofError> {
        if bytes.first() == Some(&0) {
            return Err(ProofError::Malformed("integer has a leading zero"));
        }
        U256::try_from_be_slice(bytes).ok_or(ProofError::Malformed("integer exceeds 256 bits"))
    }
}

#[cfg(test)]
mod tests {
    use super::rlp::{Item, decode};
    use super::*;

    #[test]
    fn constants_are_the_hashes_they_name() {
        assert_eq!(
            Hash32::from_bytes(quai_crypto::keccak256(&[0x80])),
            EMPTY_TRIE_ROOT
        );
        assert_eq!(
            Hash32::from_bytes(quai_crypto::keccak256(b"")),
            EMPTY_CODE_HASH
        );
    }

    #[test]
    fn rlp_accepts_canonical_forms_and_rejects_the_rest() {
        assert_eq!(decode(&[0x05]).unwrap(), Item::Bytes(&[0x05]));
        assert_eq!(decode(&[0x80]).unwrap(), Item::Bytes(&[]));
        assert_eq!(decode(&[0x81, 0x80]).unwrap(), Item::Bytes(&[0x80]));
        assert_eq!(
            decode(&[0xc2, 0x01, 0x80]).unwrap(),
            Item::List(vec![Item::Bytes(&[1]), Item::Bytes(&[])])
        );
        let mut long = vec![0xb8, 56];
        long.extend([7u8; 56]);
        assert_eq!(decode(&long).unwrap(), Item::Bytes(&[7; 56]));
        for bad in [
            &[][..],                      // empty
            &[0x81, 0x05],                // single byte below 0x80 wrapped
            &[0x82, 0x01],                // truncated
            &[0xb8, 0x05, 1, 2, 3, 4, 5], // long form for a short string
            &[0xb9, 0x00, 0x38],          // leading zero in the length
            &[0x05, 0x06],                // trailing bytes
            &[0xc1],                      // truncated list
        ] {
            assert!(decode(bad).is_err(), "{bad:?}");
        }
        let mut deep = vec![0x80];
        for _ in 0..20 {
            let mut outer = vec![0xc0 + deep.len() as u8];
            outer.extend(deep);
            deep = outer;
        }
        assert!(decode(&deep).is_err(), "nesting is bounded");
    }

    #[test]
    fn integers_are_canonical_and_bounded() {
        assert_eq!(rlp::uint(&[]).unwrap(), U256::ZERO);
        assert_eq!(rlp::uint(&[1, 0]).unwrap(), U256::from(256));
        assert!(rlp::uint(&[0, 1]).is_err());
        assert!(rlp::uint(&[1; 33]).is_err());
    }

    #[test]
    fn an_empty_trie_proves_absence_only_under_the_empty_root() {
        let address = "0x0000000000000000000000000000000000000001"
            .parse()
            .unwrap();
        let none: [&[u8]; 0] = [];
        assert_eq!(
            verify_account_proof(EMPTY_TRIE_ROOT, address, &none).unwrap(),
            None
        );
        assert_eq!(
            verify_account_proof(Hash32::from_bytes([1; 32]), address, &none),
            Err(ProofError::Incomplete)
        );
    }

    #[test]
    fn mapping_slots_match_solidity() {
        // keccak256(bytes32(0) ‖ bytes32(0)), the slot of key 0 in a mapping at slot 0.
        assert_eq!(
            solidity_mapping_slot(Hash32::ZERO, U256::ZERO).to_string(),
            "0xad3228b676f7d3cd4284a5443f17f1962b36e491b30a40b2405849e597ba5fb5"
        );
        let owner: QuaiAddress = "0x006C3e2AaAE5DB1bCd11A1a097cE572312EADdBB"
            .parse()
            .unwrap();
        let word = address_word(owner);
        assert_eq!(&word.bytes()[..12], &[0; 12]);
        assert_eq!(&word.bytes()[12..], owner.bytes());
    }
}
