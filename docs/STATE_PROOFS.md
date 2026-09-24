# State proofs: account values a node must prove

`Provider::prove_accounts` returns balances, nonces, code hashes and storage
values that the node has proven, with `quai_getProof`, against one block's state
root (`evmRoot`). The SDK checks every proof itself, and it recomputes the
block's `headerHash` from the header's own fields, so the root is bound to that
hash. A node that reports a value its proof does not show, or a root its header
does not hash to, gets an error, never a value.

What that establishes depends on where the header came from:

- **From one node,** a proven value is *consistent with the header that node
  reported*. A dishonest node can report a header of its own making, with a
  matching `headerHash`, and prove whatever that header's root holds.
- **Confirmed by a second, independent node,** it is *the value at a block both
  nodes hold canonical*. `Provider::confirm_anchor` makes that check: the
  second node's header at the same height must recompute to the same
  `headerHash`. Two nodes would have to collude to pass it.

So label an unconfirmed result "consistent with this node", and a confirmed
one "verified". [What proofs do not cover](#what-proofs-do-not-cover) lists the
remaining limits.

## What a wallet can do today

| Need | Call | Proven |
| --- | --- | --- |
| A block two nodes agree on | `state_anchor`, then `confirm_anchor` on the second node | The header hash, and every root in it |
| Balance and nonce for a review | `prove_accounts_at(&anchor, &[(address, &[])])` | Balance in Its, nonce, or a proven absence |
| Pin a contract by runtime hash | `Contract::prove_deployment_at(&anchor, Some(hash))` | Code hash, without downloading bytecode |
| ERC-20 balance or allowance | `prove_accounts_at` with a mapping slot from `solidity_mapping_slot` | The storage word the token reads |
| Proxy implementation | `prove_accounts_at` with the EIP-1967 implementation slot | The implementation address the proxy delegates to |
| Refuse an old block | `StateAnchor::require_number_at_least` | The block is at least that high |
| An audit record | Store `ProvenAccount`'s header hash and proofs | Re-verifiable later against the stored root, and the root against any node |

`prove_accounts(genesis, targets, block)` and `prove_deployment(genesis, hash,
block)` do both steps with one node: an anchor, then the proofs, two round
trips. Use them when a second node is not available, and label the result
accordingly.

### Confirming the block with a second node

```rust,ignore
// Round one on the serving node: the genesis and the header, no address.
let anchor = provider.state_anchor(trusted_genesis, Zone::Cyprus1, BlockTag::Latest).await?;
// The second node's header at that height must hash the same. No address is sent.
second.confirm_anchor(&anchor).await?;
// Refuse a block older than the second node's head allows.
anchor.require_number_at_least(second_head.saturating_sub(4))?;
// One round trip per call from here, all at the same block.
let proven = provider
    .prove_accounts_at(&anchor, &[(sender, &[]), (recipient, &[])])
    .await?;
```

Near the head the second node may not have the block yet
(`ObservationChanged`), or may briefly hold a different one at that height
(`AnchorDisputed`). Both are class `Stale`. Anchoring a few blocks below the
head, with `BlockTag::Number`, avoids the race. An `AnchorDisputed` that
persists at depth means the nodes disagree about the chain: treat neither
node's proofs as verified.

`state_anchor` checks the genesis before the header is used, and refuses a
header whose fields do not hash to its `headerHash` with
`ProviderError::Header`. The anchor is read once and shared: each
`prove_accounts_at` call sends every `quai_getProof` by the anchor's block
hash with `requireCanonical`, and rechecks the header and genesis in the same
batch. A block reorganized away is `ObservationChanged`.

### Balances and nonces

```rust,ignore
let proven = provider
    .prove_accounts_at(&anchor, &[(sender, &[]), (recipient, &[])])
    .await?;
let (sender, recipient) = (&proven[0], &proven[1]);
// An absent account proves zero balance and nonce, with no code.
println!("{} Its, nonce {}", sender.balance(), sender.nonce());
println!("recipient is a contract: {}", recipient.has_code());
```

Both accounts are proven at the same block, `sender.block`. Show that block in
the review, so the user sees what the numbers were read against.

### Contract pins

`Contract::prove_deployment_at` and `Contract::prove_deployment` replace
`Contract::verify_deployment` when the wallet pins contracts by runtime hash.
They return the same errors: `GenesisMismatch`, `MissingCode` and
`RuntimeMismatch`. The node sends a proof of about 2 KB instead of the runtime
bytes.

```rust,ignore
let router = Contract::new(router_address, AbiInterface::default(), &provider);
match router.prove_deployment_at(&anchor, Some(pinned_hash)).await {
    Ok(proven) => { /* proven.code_hash() == pinned_hash at anchor.block */ }
    Err(error) if error.class() == ErrorClass::Stale => { /* anchor again */ }
    Err(error) => return Err(error.into()), // mismatch, missing or a bad proof
}
```

To pin several contracts of one zone at the same block, share one anchor, or
call `prove_accounts_at` once with each address and no slots, then compare each
`code_hash()`.

### Token balances and allowances

A token balance is a storage slot, and its location is the token's own layout.
Solidity places `balanceOf[owner]` at `keccak(owner ‖ base_slot)`, where
`base_slot` is where the contract declares the mapping:

```rust,ignore
use quai_sdk::provider::state_proof::{address_word, solidity_mapping_slot};

let balance_slot = solidity_mapping_slot(address_word(owner), U256::from(3)); // WQUAI: slot 3
// allowance[owner][spender]: the inner slot becomes the outer mapping's base.
let inner = solidity_mapping_slot(address_word(owner), allowance_base);
let allowance_slot =
    solidity_mapping_slot(address_word(spender), U256::from_be_bytes(inner.into_bytes()));
let proven = provider
    .prove_accounts_at(&anchor, &[(token, &[balance_slot, allowance_slot])])
    .await?;
let balance = proven[0].storage_value(balance_slot).unwrap();
```

The SDK cannot discover `base_slot`. Take it from the token's source or storage
layout, and confirm it once per token: the proven value must equal what the
token's own `balanceOf` returns through `quai_call` at the same block. For
WQUAI, `balanceOf` at slot 3 was checked this way on mainnet: a proven balance
of 28,068,095,470,218,650,736,502 matched `balanceOf`. Quai Terminal confirmed
WQUAI's `allowance` at slot 4 the same way. A proxy token keeps its balances in
the proxy's storage, not the implementation's.

### Proxy implementations

An EIP-1967 proxy stores its implementation address at a fixed slot,
`0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc`. Proving
that slot tells a wallet which implementation the proxy delegates to, and then
`prove_deployment_at` can pin that implementation's code.

### Audit records

`ProvenAccount` keeps its proof nodes. Store the chain ID, genesis, block number
and hash, `header_hash`, `state_root`, `account_proof` and each slot's `proof`.
Later, with no node:

```rust,ignore
use quai_sdk::provider::state_proof::{EMPTY_TRIE_ROOT, verify_account_proof, verify_storage_proof};

let account = verify_account_proof(record.state_root, record.address, &record.account_proof)?;
let storage_root = account.map(|a| a.storage_root).unwrap_or(EMPTY_TRIE_ROOT);
let value = verify_storage_proof(storage_root, record.slot, &record.slot_proof)?;
```

That shows what the stored root held, and nothing about the root itself. To
confirm the root, read the header at the record's height from any node and
compare header hashes:

```rust,ignore
let header = node
    .verified_header(zone, BlockTag::Number(U256::from(record.block.number)))
    .await?
    .ok_or("that node has no block at this height")?;
assert_eq!(header.header_hash, record.header_hash);
assert_eq!(header.evm_root, record.state_root);
```

## Errors

| Error | Class | What to do |
| --- | --- | --- |
| `ProviderError::GenesisMismatch` | `NetworkMismatch` | Stop. The node is on another network. |
| `ProviderError::ObservationChanged` | `Stale` | Anchor again. The block was reorganized during the read, or a node behind a gateway lags the others, or the confirming node has no block at that height yet. |
| `ProviderError::AnchorDisputed` | `Stale` | Anchor deeper and confirm again. If it persists, the nodes disagree about the chain. |
| `ProviderError::StaleAnchor` | `Stale` | Anchor again, or use another node. The block is older than the caller's bound. |
| `ProviderError::Proof(_)` | `Invalid` | Do not retry against the same node. It sent a proof that does not verify, or values its proof contradicts. |
| `ProviderError::Header(_)` | `Invalid` | Do not retry against the same node, except for `UnknownField`, which means the node runs a newer go-quai and the SDK needs updating. |
| `ProviderError::InvalidRequest(_)` | `Invalid` | Fix the request. Nothing was sent. |

`ProofError::Contradicted` means the proof verified but the node's own
`balance`, `nonce`, `codeHash`, `storageHash` or slot value differed from it.
`HeaderHashError::HeaderHashMismatch` means the header's roots do not hash to
the header hash it reported. Both are a faulty or dishonest node, not a
transient failure.

## Limits and cost

- 1 to 32 accounts per call, all in one zone, and up to 64 slots per account.
  A proof path is at most 65 nodes of at most 1 KiB each.
- Against `rpc.quai.network` on 2026-09-24, an account proof took one round trip
  (246 ms) against two for the same contract's bytecode (489 ms). `state_anchor`
  is one more round trip, paid once per anchor.
- `HttpTransport` asks for gzip. An account proof arrives about 1.9 times
  smaller, about 2.3 KB for an account without storage.
- A request larger than about half a megabyte of proofs is split into several
  batches, sent concurrently, each rechecking the block. A mainnet storage
  proof runs to about 2.3 KB, so up to about 190 slots fit in one batch.
  Prove only the slots the review needs: a large response takes extra round
  trips on a slow link.
- The public gateway served proofs 1,000,000 blocks back. A node that prunes
  old state can only prove recent blocks.
- A transport that does not batch makes the reads one at a time, in the same
  order.

## What proofs do not cover

- **The block's standing.** The SDK recomputes the header's `headerHash`, which
  covers `evmRoot`, `utxoRoot` and every other root, but not whether the block
  is canonical. Before the KawPoW fork it also recomputes the block hash from
  the header. After it, the block hash needs the merged-mining `auxpow`, which
  default nodes omit from `quai_getHeaderByNumber`; `verify_header_hash` checks
  it, and that the coinbase commits the seal, when given a v2 work object. It
  never checks proof-of-work, so a confirming second node is what makes a
  proof independent of the serving one.
- **Freshness, unless asked.** An old block that is still canonical proves its
  true, old state. Pass a minimum height from a second node's head with
  `require_number_at_least`. The block's time is part of the seal, not of
  `headerHash`, so `require_timestamp_at_least` catches a lagging honest node
  but not a dishonest one after the fork.
- **Privacy from a dishonest node.** The genesis is checked before any address
  is sent, so an honest node on the wrong network learns nothing. A node that
  echoes the right genesis receives every address and slot in the request.
  Storage slots are `keccak(owner ‖ base_slot)`, so such a node can confirm
  which tokens an owner holds.
- **Qi balances.** Qi lives in the UTXO set under `utxoRoot`, which
  `quai_getProof` does not cover.
- **Call results.** A proof shows storage, not what a contract function returns.
  A `quai_call` result remains the node's report.
- **Storage layout.** A proven slot means only what the contract's layout says
  it means.

## Account encoding

Quai's state trie is keyed by `keccak(address)`, as in Ethereum. Its account
leaf has five fields, one more than Ethereum's:
`[nonce, balance, storageRoot, codeHash, size]`. Storage leaves are keyed by
`keccak(slot)` and hold the value as a canonical big-endian integer. The
verifier accepts only these shapes, in canonical RLP.

A zone header's `headerHash` is BLAKE3 over the protobuf encoding of its sealed
fields, as go-quai's `Header.SealEncode` builds it. The block hash is BLAKE3
over a mix hash, seal hash and nonce before the KawPoW fork, and over the
`auxpow` encoding after it. `verify_header_hash` refuses a header with a field
it does not know, since a new sealed field would otherwise read as a forgery.

With the `test-fixtures` feature, `state_proof::test_trie::TestTrie` builds
tries with go-quai's node rules, so a wallet's tests can serve proofs a node
could have sent.

The runnable example `crates/quai-sdk/examples/prove_state.rs` anchors a block,
optionally confirms it with a second node, proves an account and, optionally,
one token balance, then re-verifies them offline:

```sh
QUAI_RPC_URL=https://rpc.quai.network/cyprus1 QUAI_EXPECTED_CHAIN_ID=9 \
QUAI_EXPECTED_GENESIS=0xac81c28f1a72591b87b5f16c9793cdc0e87c45c6d426d1a364c3b8f6386b5b8b \
QUAI_CONFIRM_RPC_URL=https://your-own-node:9200 \
cargo run -p quai-sdk --features http --example prove_state
```
