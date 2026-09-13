# Browser account preparation and submission

Enable `backup,browser` with native defaults disabled. Add `abi` for contract
and wrapper calls. `account_preflight::{AccountIntent, FeePolicy,
AccountObservationPolicy}` are portable; the existing native `accounts` paths
re-export those same types for compatibility.

`BrowserAccountSession::new(&provider, &book)` binds an explicitly configured
provider and initialized account journal. Construction does not contact a node
or request wallet access. The default observation policy is Pending. Select
PinnedLatest explicitly if needed; there is no silent pending-state fallback.

1. Call `prepare(id, intent, fee_policy)` for an ordinary same-zone transfer or
   contract call, or `prepare_cross_zone` explicitly for another Quai zone.
2. Preparation reads the journal revision before RPC, checks chain/genesis,
   selects max(remote nonce, retained nonce floor), and simulates that exact
   nonce with the original ordered access list. Gas margins round upward;
   gas/price/total fee and observed value-plus-fee balance are bounded.
3. PinnedLatest uses a sampled numeric block for nonce/balance/estimation and
   rechecks the latest hash/height. Chain/genesis are checked again before return.
4. The resulting nonce claim is committed with the revision captured before RPC.
   A concurrent write rejects the whole attempt. Failed quotation/cancellation
   before dispatch does not burn a nonce; an ambiguous final write may have
   reserved it, so inspect the same ID before retrying.
5. Review `PreparedBrowserAccountTransaction::{transaction,maximum_fee,
   signing_digest}`. Calling its `sign(&signer)` checks the exact payload and
   persists canonical signed bytes in its original book before returning them.
6. Call `session.broadcast(id)` explicitly. It loads the persisted signed root,
   checks the network, commits Submitted with CAS, then makes one send attempt.
   Timeout or cancellation may follow node acceptance. Reopening the book and
   explicitly broadcasting the same ID retains the same bytes.

`prepare_reserved` and `prepare_cross_zone_reserved` reprepare an existing
unsigned nonce after restart. Released gaps must be explicitly reopened first.
A remote nonce beyond the reserved value rejects. Signed IDs cannot be reused
for a new preparation. Dropping a prepared result retains the unsigned claim.

`ContractCall::into_account_intent` is now available with `wallet` on both targets,
so WQUAI deposit/withdraw, WQI claim/redemption and ordinary contract/ERC-20 calls
retain native value, calldata and required access declarations. These are account
contract calls, distinct from native Qi wrapping or Quai-to-Qi conversion.

`account_preflight::quote_account` exposes the same bounded quotation independently
of storage on native and Wasm. `AccountNonce::AtLeast` uses an explicit retained
floor; `Exact` supports an existing reservation. A quote alone reserves nothing.
There is no implicit access-list discovery or node-local signer.

This initial workflow covers ordinary same/cross-zone Quai transfers and calls,
including account-side wrapper operations. Native conversion/deployment planners,
replacement candidate selection, destination settlement and full browser Qi
preparation/recovery still require their own integration. The root broadcast
method does not silently select a replacement. Current balances and fee estimates
are advisory observations; preparing several operations does not reserve aggregate
balance or guarantee that the node will accept them.

[Tests](../crates/quai-sdk/tests/account_preflight.rs) exercise native and Wasm
quotation, real Chromium IndexedDB nonce races/cancellation, exact signed-byte
recovery after an ambiguous mock submission, and a real Fetch HTTP fixture plus
IndexedDB/local signing. Mock acknowledgements and HTTP fixtures are not funded
node-acceptance evidence. [Existing account tests](../crates/quai-sdk/tests/accounts.rs)
retain native behavior after the portable policy/type extraction.
