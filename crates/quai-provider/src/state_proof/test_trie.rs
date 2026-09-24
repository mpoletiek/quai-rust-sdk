//! A Merkle-Patricia trie builder for tests: go-quai's (geth's) node rules,
//! so a consumer's tests can serve proofs a node could have sent. Enabled by
//! `test-fixtures`. It checks nothing and is not for production use.
//!
//! Nodes whose encoding is under 32 bytes are embedded in their parent;
//! shared key prefixes become extension nodes; branches carry no value,
//! because every key is 32 bytes.
use super::{Account, EMPTY_TRIE_ROOT};
use quai_primitives::{Hash32, QuaiAddress};
use quai_rpc::U256;

/// A trie over 32-byte paths, with the proof nodes for any path.
#[derive(Clone, Debug)]
pub struct TestTrie {
    root: Option<Node>,
}

#[derive(Clone, Debug)]
enum Node {
    Leaf(Vec<u8>, Vec<u8>),
    Extension(Vec<u8>, Box<Node>),
    Branch(Box<[Option<Node>; 16]>),
}

impl TestTrie {
    /// A trie of raw leaf values keyed by their trie paths. A later entry for
    /// the same path replaces an earlier one.
    pub fn from_paths(entries: impl IntoIterator<Item = ([u8; 32], Vec<u8>)>) -> Self {
        let mut items: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        for (path, value) in entries {
            let path = nibbles(&path);
            match items.iter_mut().find(|(p, _)| *p == path) {
                Some(item) => item.1 = value,
                None => items.push((path, value)),
            }
        }
        items.sort();
        Self {
            root: (!items.is_empty()).then(|| build(&items, 0)),
        }
    }
    /// A storage trie: each nonzero value at `keccak(slot)`, as the EVM keeps it.
    pub fn storage(slots: &[(Hash32, U256)]) -> Self {
        Self::from_paths(
            slots
                .iter()
                .filter(|(_, v)| !v.is_zero())
                .map(|(slot, value)| {
                    (
                        quai_crypto::keccak256(slot.bytes()),
                        bytes(&value.to_be_bytes_trimmed_vec()),
                    )
                }),
        )
    }
    /// A state trie of Quai accounts, each at `keccak(address)`.
    pub fn accounts(accounts: &[(QuaiAddress, Account)]) -> Self {
        Self::from_paths(accounts.iter().map(|(address, account)| {
            let leaf = list(&[
                bytes(&U256::from(account.nonce).to_be_bytes_trimmed_vec()),
                bytes(&account.balance.to_be_bytes_trimmed_vec()),
                bytes(account.storage_root.bytes()),
                bytes(account.code_hash.bytes()),
                bytes(&account.size.to_be_bytes_trimmed_vec()),
            ]);
            (quai_crypto::keccak256(address.bytes()), leaf)
        }))
    }
    /// The root hash; [`EMPTY_TRIE_ROOT`] for an empty trie.
    pub fn root(&self) -> Hash32 {
        self.root.as_ref().map_or(EMPTY_TRIE_ROOT, |root| {
            Hash32::from_bytes(quai_crypto::keccak256(&encode(root)))
        })
    }
    /// The proof for `path`, in root-first order as `quai_getProof` sends it:
    /// its value, or its absence.
    pub fn prove(&self, path: &[u8; 32]) -> Vec<Vec<u8>> {
        let key = nibbles(path);
        let mut proof = Vec::new();
        let (mut node, mut at) = (self.root.as_ref(), 0);
        while let Some(current) = node {
            let encoded = encode(current);
            // The root is always sent; any other node only when hashed.
            if proof.is_empty() || encoded.len() >= 32 {
                proof.push(encoded);
            }
            node = match current {
                Node::Leaf(..) => None,
                Node::Extension(prefix, child) => key[at..].starts_with(prefix).then(|| {
                    at += prefix.len();
                    child.as_ref()
                }),
                Node::Branch(children) => {
                    at += 1;
                    children[usize::from(key[at - 1])].as_ref()
                }
            };
        }
        proof
    }
    /// The proof for a storage slot.
    pub fn prove_slot(&self, slot: Hash32) -> Vec<Vec<u8>> {
        self.prove(&quai_crypto::keccak256(slot.bytes()))
    }
    /// The proof for an account.
    pub fn prove_account(&self, address: QuaiAddress) -> Vec<Vec<u8>> {
        self.prove(&quai_crypto::keccak256(address.bytes()))
    }
    /// How many nodes are embedded in a parent, and how many are extensions.
    pub fn shapes(&self) -> (usize, usize) {
        fn walk(node: &Node, root: bool, counts: &mut (usize, usize)) {
            if !root && encode(node).len() < 32 {
                counts.0 += 1;
            }
            match node {
                Node::Leaf(..) => (),
                Node::Extension(_, child) => {
                    counts.1 += 1;
                    walk(child, false, counts);
                }
                Node::Branch(children) => {
                    for child in children.iter().flatten() {
                        walk(child, false, counts);
                    }
                }
            }
        }
        let mut counts = (0, 0);
        if let Some(root) = &self.root {
            walk(root, true, &mut counts);
        }
        counts
    }
}

fn nibbles(path: &[u8; 32]) -> Vec<u8> {
    path.iter().flat_map(|b| [b >> 4, b & 0x0f]).collect()
}

/// Build from sorted, distinct paths that agree on their first `depth` nibbles.
fn build(items: &[(Vec<u8>, Vec<u8>)], depth: usize) -> Node {
    if let [(path, value)] = items {
        return Node::Leaf(path[depth..].to_vec(), value.clone());
    }
    let (first, last) = (&items[0].0, &items[items.len() - 1].0);
    let shared = first[depth..]
        .iter()
        .zip(&last[depth..])
        .take_while(|(a, b)| a == b)
        .count();
    if shared > 0 {
        return Node::Extension(
            first[depth..depth + shared].to_vec(),
            Box::new(build(items, depth + shared)),
        );
    }
    let mut children: [Option<Node>; 16] = Default::default();
    for (nibble, child) in children.iter_mut().enumerate() {
        let group = items
            .iter()
            .filter(|(path, _)| usize::from(path[depth]) == nibble)
            .cloned()
            .collect::<Vec<_>>();
        if !group.is_empty() {
            *child = Some(build(&group, depth + 1));
        }
    }
    Node::Branch(Box::new(children))
}

fn encode(node: &Node) -> Vec<u8> {
    match node {
        Node::Leaf(path, value) => list(&[bytes(&hex_prefix(path, true)), bytes(value)]),
        Node::Extension(path, child) => list(&[bytes(&hex_prefix(path, false)), reference(child)]),
        Node::Branch(children) => {
            let mut items = children
                .iter()
                .map(|child| child.as_ref().map_or(vec![0x80], reference))
                .collect::<Vec<_>>();
            items.push(vec![0x80]);
            list(&items)
        }
    }
}

/// A child as its parent holds it: embedded when short, otherwise its hash.
fn reference(node: &Node) -> Vec<u8> {
    let encoded = encode(node);
    if encoded.len() < 32 {
        encoded
    } else {
        bytes(&quai_crypto::keccak256(&encoded))
    }
}

fn hex_prefix(path: &[u8], leaf: bool) -> Vec<u8> {
    let flag = if leaf { 2 } else { 0 } + (path.len() % 2) as u8;
    let (mut out, rest) = if path.len() % 2 == 1 {
        (vec![(flag << 4) | path[0]], &path[1..])
    } else {
        (vec![flag << 4], path)
    };
    out.extend(rest.chunks(2).map(|pair| (pair[0] << 4) | pair[1]));
    out
}

fn length(length: usize, offset: u8) -> Vec<u8> {
    if length <= 55 {
        return vec![offset + length as u8];
    }
    let be = length.to_be_bytes();
    let skip = be.iter().position(|&b| b != 0).expect("nonzero length");
    let mut out = vec![offset + 55 + (be.len() - skip) as u8];
    out.extend_from_slice(&be[skip..]);
    out
}

fn bytes(value: &[u8]) -> Vec<u8> {
    if let [byte] = value
        && *byte < 0x80
    {
        return vec![*byte];
    }
    let mut out = length(value.len(), 0x80);
    out.extend_from_slice(value);
    out
}

fn list(items: &[Vec<u8>]) -> Vec<u8> {
    let body = items.concat();
    let mut out = length(body.len(), 0xc0);
    out.extend(body);
    out
}
