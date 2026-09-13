# Native account workflow

**September 12 update:** See [current wallet workflows](WALLET_WORKFLOWS.md) for
the added node-backed Qi scanner, mixed-origin sessions, conversion/wrapping
preparation and recovery APIs. Historical evidence and limitations below retain
their original scope; the newer guide describes the additional implementations.

`quai_sdk::accounts` (feature `sqlite`) joins a `Provider`, a synchronous local
`Signer` and a scoped `SqliteStore`. Register validated public-key metadata in the
store first. `NetworkScope` must use a trusted chain ID, genesis hash and zone.

1. `AccountSession::prepare` checks chain/genesis and the destination zone, fetches
   pending nonce, price, estimate and balance, enforces explicit gas/price/total-fee
   ceilings, and atomically reserves a nonce. It returns an immutable payload with
   a cached signing digest. The current workflow supports ordinary same-zone
   account transfers/calls; deployment, ETX and conversion have separate builders.
2. The application displays/reviews `PreparedAccountTransaction::transaction()`
   and `maximum_fee()`. `sign` never re-estimates or changes that payload. It checks
   the signer result equals the reviewed bytes and atomically persists the verified
   canonical signed transaction before returning it.
3. `broadcast` reloads and verifies the durable bytes, checks network identity,
   records Submitted before network I/O, and makes one explicit send. Timeout,
   cancellation and malformed acknowledgement retain the signed claim. An explicit
   later call resends the same stored transaction, never a newly numbered payment.

Gas estimation and pending balance are node observations, not inclusion promises
or atomic balance reservations. Concurrent prepared payments can overcommit a
balance and be rejected/delayed by the node. Fee limits apply to the exact gas
parameters, not arbitrary later replacements. The node may execute a call at
changed state after estimation; applications need contract-level protections.

Dropping a prepared object does not release it. Only an explicitly unsigned claim
can be released, and the current durable nonce cursor never rewinds. An abandoned
nonce can therefore require a separately authorized gap-filling transaction.
Automatic gap repair, replacement and verified reorg reconciliation remain open;
this workflow must not be presented as production-qualified pending management.

`Signer` currently describes synchronous local custody. Remote/injected signing
requires its own explicit authorization/cancellation protocol. The store contains
public metadata and signed transactions, not private keys. Native database
confidentiality, filesystem permissions and rollback protection remain application
responsibilities; the encrypted seed backup is a separate format.

Tests in `crates/quai-sdk/tests/accounts.rs` exercise exact-byte preparation,
fee rejection before allocation, changed-payload refusal, persistent signed-byte
custody, one-send ambiguity, cancellation and explicit same-ID recovery. They use
real SQLite files and a deterministic transport; no real transaction is submitted.


## Contract deployment

With `sqlite,abi`, use `AccountSession::reserve_deployment_nonce(id)` before the
pure `contracts::prepare_deployment` address search. Pass that exact nonce and
signer/chain to the builder, then give its immutable `PreparedDeployment` to
`AccountSession::prepare_deployment(id, deployment, fee_policy)`. The session
requires an existing unsigned reservation, verifies the CREATE prediction against
its actual signer, and estimates the final nonce/init code/mandatory address
access list. `PreparedAccountTransaction::created_address()` returns the same
prediction. Ordinary `sign` and `broadcast` persist and submit the exact bytes.

This two-stage allocation is necessary because the deployed address depends on
the nonce. Search, estimate or fee failures retain the reserved nonce and operation
ID. An unsigned release does not rewind the cursor: recover/reuse the existing
reservation deliberately or handle its gap before later nonces can mine. No
failure silently chooses another nonce or changes already reviewed init code.
Gas estimates and pending balances remain node observations, not acceptance or
balance reservations. Appended grind suffixes can affect custom init code; inspect
and simulate the actual final bytes.

## Explicit confirmed-state preparation

`AccountSession::with_observation_policy(AccountObservationPolicy::PinnedLatest)`
pins nonce, balance and gas simulation to one sampled numeric block height and
rechecks the latest hash before reservation and before returning a prepared payload.
This supports nodes whose pending-state RPC is unavailable. The default remains
`Pending`; errors never trigger silent fallback. Confirmed observations exclude
mempool changes and do not reserve balance. All modes retain durable nonce claims,
exact payload review and signed-byte recovery. A head change after a nonce was
reserved leaves that unsigned reservation available for explicit recovery.

## Durable fee replacements

`prepare_replacement(id, parent_hash, ReplacementPolicy)` reviews a higher gas
price while preserving the exact sender, nonce, recipient, value, data, access list
and gas limit. The policy selects the minimum percentage bump and maximum fees;
pinned go-quai defaults to 5%, but operators can change that pool setting. The
session checks the confirmed nonce, simulates the same payload and verifies its
full maximum debit. `sign_replacement` commits the candidate before exposure.

`broadcast_candidate(id, hash)` submits one exact persisted candidate once.
`original` and replacement hashes all remain recoverable through
`signed_candidates` and `observe_candidates`; up to 32 replacement edges share
one permanent nonce claim. Observation checks every candidate and rejects two
canonical members for one nonce. An unobserved candidate is never treated as
safely dropped. SQLite schema v3 and authenticated wallet backup v4 preserve the
whole graph; prior schemas/backups migrate or remain readable. This API implements
fee-only speedups, including existing conversions/deployments, without silently
changing the payment or cancelling it.
