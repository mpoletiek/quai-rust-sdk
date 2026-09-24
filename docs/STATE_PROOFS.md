# State proofs: account values a node must prove

`Provider::prove_accounts` returns balances, nonces, code hashes and storage
values that the node has proven, with `quai_getProof`, against one block's state
root (`evmRoot`). The SDK checks every proof itself. A node that reports a value
its proof does not show gets an error, never a value.

This moves what a wallet trusts from every number a node reports to one block
header per observation. The header is still the node's report of the canonical
chain: the SDK rechecks it across the read but cannot yet check it independently.
[What proofs do not cover](#what-proofs-do-not-cover) lists the remaining limits.

## What a wallet can do today

| Need | Call | Proven |
| --- | --- | --- |
| Balance and nonce for a review | `prove_accounts(genesis, &[(address, &[])], block)` | Balance in Its, nonce, or a proven absence |
| Pin a contract by runtime hash | `Contract::prove_deployment(genesis, Some(hash), block)` | Code hash, without downloading bytecode |
| ERC-20 balance or allowance | `prove_accounts` with a mapping slot from `solidity_mapping_slot` | The storage word the token reads |
| Proxy implementation | `prove_accounts` with the EIP-1967 implementation slot | The implementation address the proxy delegates to |
| An audit record | Store `ProvenAccount`'s header identity and proofs | Re-verifiable later with no node |

Every call takes the trusted genesis. The first round reads only the genesis and
the header, so a node on the wrong network is refused with `GenesisMismatch`
before it learns any address. The second round sends every `quai_getProof` by
that block's hash, with the header and genesis rechecked in the same batch.

### Balances and nonces

```rust,ignore
let proven = provider
    .prove_accounts(trusted_genesis, &[(sender, &[]), (recipient, &[])], BlockTag::Latest)
    .await?;
let (sender, recipient) = (&proven[0], &proven[1]);
// An absent account proves zero balance and nonce, with no code.
println!("{} Its, nonce {}", sender.balance(), sender.nonce());
println!("recipient is a contract: {}", recipient.has_code());
```

Both accounts are proven at the same block, `sender.block`. Show that block in
the review, so the user sees what the numbers were read against.

### Contract pins

`Contract::prove_deployment` replaces `Contract::verify_deployment` when the
wallet pins contracts by runtime hash. It returns the same errors:
`GenesisMismatch`, `MissingCode` and `RuntimeMismatch`. The node sends a proof
of about 2 KB instead of the runtime bytes.

```rust,ignore
let router = Contract::new(router_address, AbiInterface::default(), &provider);
match router.prove_deployment(trusted_genesis, Some(pinned_hash), BlockTag::Latest).await {
    Ok(proven) => { /* proven.code_hash() == pinned_hash at proven.block */ }
    Err(error) if error.class() == ErrorClass::Stale => { /* observe again */ }
    Err(error) => return Err(error.into()), // mismatch, missing, wrong network or a bad proof
}
```

To pin several contracts of one zone at the same block, call `prove_accounts`
with each address and no slots, then compare each `code_hash()`.

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
    .prove_accounts(trusted_genesis, &[(token, &[balance_slot, allowance_slot])], BlockTag::Latest)
    .await?;
let balance = proven[0].storage_value(balance_slot).unwrap();
```

The SDK cannot discover `base_slot`. Take it from the token's source or storage
layout, and confirm it once per token: the proven value must equal what the
token's own `balanceOf` returns through `quai_call` at the same block. For
WQUAI, `balanceOf` at slot 3 was checked this way on mainnet: a proven balance
of 28,068,095,470,218,650,736,502 matched `balanceOf`. A proxy token keeps its
balances in the proxy's storage, not the implementation's.

### Proxy implementations

An EIP-1967 proxy stores its implementation address at a fixed slot,
`0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc`. Proving
that slot tells a wallet which implementation the proxy delegates to, and then
`prove_deployment` can pin that implementation's code.

### Audit records

`ProvenAccount` keeps its proof nodes. Store the chain ID, genesis, block number
and hash, `state_root`, `account_proof` and each slot's `proof`. Later, with no
node:

```rust,ignore
use quai_sdk::provider::state_proof::{EMPTY_TRIE_ROOT, verify_account_proof, verify_storage_proof};

let account = verify_account_proof(record.state_root, record.address, &record.account_proof)?;
let storage_root = account.map(|a| a.storage_root).unwrap_or(EMPTY_TRIE_ROOT);
let value = verify_storage_proof(storage_root, record.slot, &record.slot_proof)?;
```

That shows what the state root held. Whether that block was canonical is a
separate question, answered by the header you stored and any header check you
ran.

## Errors

| Error | Class | What to do |
| --- | --- | --- |
| `ProviderError::GenesisMismatch` | `NetworkMismatch` | Stop. The node is on another network. |
| `ProviderError::ObservationChanged` | `Stale` | Observe again. The block was reorganized during the read. |
| `ProviderError::Proof(_)` | `Invalid` | Do not retry against the same node. It sent a proof that does not verify, or values its proof contradicts. |
| `ProviderError::InvalidRequest(_)` | `Invalid` | Fix the request. Nothing was sent. |

`ProofError::Contradicted` means the proof verified but the node's own
`balance`, `nonce`, `codeHash`, `storageHash` or slot value differed from it.
That is a faulty or dishonest node, not a transient failure.

## Limits and cost

- 1 to 32 accounts per call, all in one zone, and up to 64 slots per account.
  A proof path is at most 64 nodes of at most 1 KiB each.
- Against `rpc.quai.network` on 2026-09-24, an account proof took one round trip
  (246 ms) against two for the same contract's bytecode (489 ms). The response
  was 4.7 KB of JSON against 14.5 KB. Four storage slots raised it to 14 KB and
  two round trips.
- Every proof arrives in one batch response. On a slow link a large response
  takes extra round trips, so prove only the slots the review needs.
- The public gateway served proofs 1,000,000 blocks back. A node that prunes
  old state can only prove recent blocks.
- A transport that does not batch makes the reads one at a time, in the same
  order.

## What proofs do not cover

- **The header.** The block is the node's report of the canonical chain. A
  wallet can narrow this today by reading the same block hash at that height
  from a second, independent endpoint. Checking the header's proof-of-work,
  and Quai's prime-chain interlinks, is ongoing research.
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

The runnable example `crates/quai-sdk/examples/prove_state.rs` proves an
account and, optionally, one token balance, then re-verifies them offline:

```sh
QUAI_RPC_URL=https://rpc.quai.network/cyprus1 QUAI_EXPECTED_CHAIN_ID=9 \
QUAI_EXPECTED_GENESIS=0xac81c28f1a72591b87b5f16c9793cdc0e87c45c6d426d1a364c3b8f6386b5b8b \
cargo run -p quai-sdk --features http --example prove_state
```
