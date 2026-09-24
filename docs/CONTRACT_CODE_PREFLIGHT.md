# Contract and wrapper deployment preflight

`WrappedQuai::new_verified` and `WrappedQi::new_verified` require nonempty runtime
code in an explicitly trusted network before returning a wrapper. They return the
code observation alongside the wrapper so the application can retain its exact
block, genesis, address, runtime bytes and Keccak-256 hash. ABI-only contracts use
`Contract::verify_deployment` for the same check.

```rust,ignore
let (wrapper, observation) = WrappedQuai::new_verified(
    address,
    &provider,
    trusted_genesis,
    Some(expected_runtime_keccak), // None checks presence without pinning bytes.
    BlockTag::Latest,
).await?;
let deposit = wrapper.deposit(amount_in_its)?;
// Review the exact deposit and observation before the wallet's prepare/sign flow.
```

The provider's configured chain ID is checked before every RPC read or batch. An
observation takes two rounds where the transport batches:

1. The genesis and the selected header. No address is sent in this round, and a
   checked binding stops with `GenesisMismatch` here, so an endpoint on the wrong
   network never learns which contract was asked about.
2. The code, read by that header's block hash, with the header and genesis
   rechecked in the same batch. Reading by hash pins the bytes to that exact
   block even if the chain reorganizes during the read.

A transport that does not batch makes the same five reads one after another.
`Latest` is sampled once; positive explicit heights up to `i64::MAX` are
supported. Pending and genesis selectors fail before any I/O. Missing/changed
headers, a changing genesis, RPC failures and chain mismatches remain errors
without an automatic workflow retry. A changed anchor outranks a failed code read,
since a block reorganized away can also make its code unreadable. Cancellation
drops the active transport future.

`Provider::observe_contract_codes` observes up to `MAX_CONTRACT_CODE_TARGETS`
contracts of one zone at the same block, with the same two rounds, and requires
the trusted genesis. `Contract::verify_deployment` and `ContractCodeTarget` use
it. It is for same-block consistency and fewer requests, not always speed: every
runtime arrives in one response, and on a high-latency link a large response
needs extra round trips. Concurrent single observations were faster against the
public gateway (about 0.75 s for five contracts, against 1.0 s in one call).

Only `Provider::observe_contract_code`, which is not given a trusted genesis,
still sends the address before the network is confirmed; prefer the checked
forms when the genesis is known.

`Provider::observe_contract_code` preserves empty code as an observation. Checked
contract/wrapper bindings reject it with `ContractError::MissingCode`, distinguish
an unexpected genesis with `GenesisMismatch`, and reject a supplied runtime hash
mismatch with `RuntimeMismatch`. A `None` expected hash only checks code presence;
even a single STOP opcode is nonempty code. Neither code presence nor a hash match
establishes ABI correctness, proxy implementation semantics, finality, future code
availability or authorization for a later transaction. Applications supply their
trusted network and expected runtime identities independently.

The existing `new` constructors remain explicitly offline intent builders and do
not request node state. The confirmed address constants do not assert that a
contract exists on every network or at every block. In particular, the 2026-09-13
Orchard read-only check found WQI runtime at its configured address and empty code
at the configured WQUAI address. Checked WQUAI binding rejected that observation;
the observation used the mainnet WQUAI address. Orchard now uses
`WQUAI_ORCHARD_ADDRESS` (`0x005c46f661Baef20671943f2b4c087Df3E7CEb13`),
verified on September 14. See the historical
[qualification report](../test-infra/reports/contract-code-preflight-2026-09-13.json).

The opt-in SDK `contract_code` test accepts `QUAI_RPC_URL`, a decimal
`QUAI_EXPECTED_CHAIN_ID` and `QUAI_EXPECTED_GENESIS`. Chain 15000 selects the
Orchard WQUAI address; other chains retain the mainnet default. An optional
`QUAI_WQUAI_ADDRESS` explicitly overrides that choice. It only reads both wrapper
addresses and checks availability at exact observed blocks:

```sh
cargo test -p quai-sdk --features abi --test contract_code \
  explicit_endpoint_wrapper_code_availability -- --ignored --nocapture
```

No fixture key, signature or transaction submission is involved. A passing read
test records availability or explicit absence; it is not funded wrap/unwrap
acceptance or a contract source review.
