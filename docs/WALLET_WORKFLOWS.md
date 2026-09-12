# Wallet workflows

Implemented against `quais@1.0.0-alpha.57` and go-quai v0.56.0
`f3f345c877300c044e3e0081a48bf3cf786fb9cc`. These APIs are development-stage;
[remaining acceptance gates](FEATURE_COMPLETENESS_REVIEW_2026-09-12.md) are explicit.
Enable `sqlite` for native sessions, `payments` for channel orchestration, and
`abi` for wrappers. Providers and stores take an explicit chain, genesis and zone.

## Qi derivation, discovery and balances

`HdWallet` derives both coin types (`Quai=994`, `Qi=969`), hardened accounts,
receive/change children and zone/ledger matches. `from_master_xprv` restores
BIP44 derivation without reinterpreting an account key as a master. `AccountPublic`
provides watch-only derivation. Public metadata retains the exact raw child index.

`qi_discovery::scan_and_refresh_qi` supplies ordinary node-backed Qi discovery.
The default gap is **50 consecutive matching Qi addresses with no current
outpoints on each receive/change branch**. Skipped raw BIP32 children do not
count toward the gap. A funded address resets the gap; denomination is always
interpreted through the fixed fifteen-entry table, never as an arbitrary value.

`QiScanOptions` bounds receive/change raw index ranges and total matching address
queries. Set `gap_limit: None` for an explicit deep scan. Reports include first
unexamined raw indexes and stop reasons; use them for explicit continuation.
`scan_qi` can return a cancelled partial report without modifying storage.

`refresh_qi` queries **every persisted Qi address**, including imported private-key
metadata, known BIP47 receive exposures, and allocated change beyond the scan gap.
It validates chain/genesis, rejects duplicate outpoints and changed head samples,
and atomically updates the current coin view with generation checks. Existing
signed claims remain held. Outpoint locks are preserved, and
`qi_balance(store, candidate_height)` returns exact total/spendable/reserved/locked/
expired Qit buckets across all stored origins. Claims take category precedence.

The outpoint RPC is latest-only. Matching before/after headers reduces races but
is not an atomic or historical snapshot. Fully spent addresses look empty, so a
gap stop cannot prove full historical recovery. A mnemonic alone also cannot
reconstruct all deliberately burned allocation ranges. Restore metadata/cursors
and deep-scan explicit ranges when needed; no external indexer is required for
ordinary current-state use. The separate history-capable `ObservationSource`
contract retains its stronger consistency requirements.

Runnable public-only scan (Cyprus-1, no seed or submission):

```sh
cargo run -p quai-sdk --features sqlite --example qi_scan -- \
  URL true_or_false DECIMAL_CHAIN_ID GENESIS_HASH wallet.sqlite ACCOUNT_XPUB ACCOUNT_INDEX
```

## Ordinary Qi sends, sweep and aggregation

1. Register public ownership and construct `QiSession` with an HD wallet, or
   `QiSession::with_keys` with a `QiKeyring` for mixed HD/imported/BIP47 inputs.
2. Allocate a bounded `QiChangePool` **before** refreshing the wallet snapshot.
   Allocation commits burned raw ranges; cancellation never reuses them.
3. Use `prepare(id, QiIntent, QiPolicy, pool)` to select inputs and converge fees
   against the exact final payload. Supply a distinct recipient address per
   denomination output. Inspect the frozen transaction, fee and signing digest.
4. Explicit `sign` persists verified bytes and exact claims before returning.
   Explicit `broadcast(id)` submits those bytes. After restart, reload the store
   and broadcast the same ID; timeout does not authorize a new transaction.

`prepare_cross_zone` uses the same process for another single destination zone;
the fee request includes the actual cross-zone outputs. Observe ETX execution
separately using the provider's bounded `scan_external_transactions` API. Origin
acceptance does not establish destination settlement.

`prepare_sweep` spends every eligible coin in the scoped snapshot into a fresh
owned output pool. Exceeding input limits fails instead of silently omitting
coins. `SweepMode::PreserveDenominations` respects the full input denomination
inventory; ordinary transfers cannot combine small inputs into larger outputs.
`SweepMode::Aggregate { maximum }` combines denominations and requires fewer
outputs than inputs. On the pinned node, **only the first Qi transaction in a
block may aggregate denominations**. Preparation cannot reserve that block
position. The planner pays the full fee; it does not copy the reference
selector's willingness to accept a fee shortfall.

## Payment codes

`PrivatePaymentCode` supports effective seeds, depth-zero master xprvs, and
explicit depth-three payment-account xprvs. Account xprv import validates depth
and account child but cannot prove the omitted ancestor path; the caller asserts
`m/47'/969'`. Full native backups cover seed/master payment origins. Standalone
account-xprv origins require a separate backup policy and are rejected by that
full-wallet format.

Exchange public codes out of band and register an owner-validated `PaymentChannel`
using `SqliteStore::import_payment_channel`. `payment_channels::payment_intent`
allocates enough fresh send destinations for denomination outputs, preserving
all burned ranges on partial failure. It feeds ordinary `QiSession::prepare`.

`scan_payment_channel` searches the registered peer's receive derivation with
default gap 50 or explicit deep ranges, persists locally ownership-verified
receive exposures, and refreshes all known Qi addresses. Repeating a scan is
idempotent for exposures and never rewinds a cursor. `QiKeyring::load_payment_channel`
derives and verifies those receive keys, enabling mixed-origin spending.
Notification transaction discovery and automatic peer-code exchange are not
implemented. A sender with large burned ranges can require explicit deeper
receive scanning or shared recovery metadata.

```sh
cargo run -p quai-sdk --features payments --example payment_codes
```

This offline example uses public test seeds and demonstrates that independently
derived sender and receiver points match. Never fund its identities.

## Native conversions

Provider methods `qi_to_quai`, `quai_to_qi` and `calculate_conversion_amount`
retain exact Qit/Its quantities. Null rate results remain unknown. The pinned
`quaiToQi` implementation uses the current prime terminus even when given a
historical selector; this API does not present that as a historical quote.
Controller-discounted estimates are not guaranteed settlement amounts.

`AccountSession::prepare_conversion` prepares Quai-to-Qi with explicit slippage,
exact conversion simulation, balance/fee limits, durable nonce claims, and the
usual frozen sign/broadcast stages. Pending-state RPC failures propagate; there
is no silent substitution of latest balance/nonce observations.

`QiSession::prepare_special` accepts `QiSpecialIntent::Conversion` with destination,
refund, slippage and an **explicit authorized fee in Qits**. The ordinary node fee
estimator drops specialized data and is unsuitable for this operation.
`estimate_qi_conversion_fee` remains an explicit capability error; caller-provided
fees do not imply a qualified automatic quote. `sign_special` commits the exact
verified conversion; normal session broadcast recovers the persisted operation.

Use `ConversionReference` / `observe_conversion` for bounded origin, changed-hash
ETX and refund correlation. Current observations do not independently prove a
specific conversion mature or spendable. Refresh actual destination outputs
and respect their observed locks before selecting them.

## WQI and WQUAI

User-confirmed addresses for mainnet and Orchard, Cyprus-1:

| Token | Address |
| --- | --- |
| WQI | `0x002b2596EcF05C93a31ff916E8b456DF6C77c750` |
| WQUAI | `0x006C3e2AaAE5DB1bCd11A1a097cE572312EADdBB` |

`wrappers::{WQI_ADDRESS, WQUAI_ADDRESS}` expose these constants; adapters also
accept an explicit deployment address. Mainnet code presence was observed for
both; the [read-only report](../test-infra/reports/wrapper-deployments-2026-09-12.json)
records runtime SHA-256 values. Orchard returned HTTP 403. Code presence is not
an implementation audit or funded workflow acceptance.

- Native Qi wrapping uses `QiWrappingTransaction` or
  `QiSpecialIntent::Wrapping`: exactly 20 owner-contract bytes and same-zone Quai
  beneficiary outputs. The operation remains distinct from 22-byte conversion
  data and ordinary empty-data Qi. Preparation uses an explicit Qit fee.
- `WrappedQi::unclaimed` reads protocol backing in Qits; `claim_deposit` prepares
  `claimDeposit()`. `token()` exposes ERC-20 balances, transfers and allowances.
- `WrappedQi::unwrap` prepares `unwrapQi(address,uint256,uint64)`. Its input is
  native Qits, converted exactly to 18-decimal WQI atoms (1 Qit = 10^15 atoms).
  Origin account gas and destination ETX gas are separate.
- `QiRedemptionPlan::go_quai_v056` reports surviving denominations, discarded
  Qits and minimum ETX output gas. `unwrap` rejects discarded dust and insufficient
  gas under that profile: denominations 0–5 are discarded, so supported exact
  redemption amounts are multiples of 1000 Qits. This bound is profile-specific.
  Final output locks come from node observations; origin success alone does not
  establish full destination credit or spendability.
- `WrappedQuai::deposit(its)` sets native value exactly; `withdraw(atoms)` has zero
  native call value. `token()` provides ERC-20 helpers.

Contract calls feed `ContractCall::into_account_intent` and the durable account
session. No wrapper helper signs or sends implicitly.

```sh
cargo run -p quai-sdk --features abi --example wrapper_intents
```

Four wrapping wire/signature fixtures agree across JS, Rust and the independent
[pinned Go oracle](../test-infra/go-oracle/WRAPPING-RESULTS.json). Four wrapper
calldata fixtures match the pinned JS ABI coder. These are encoding evidence,
not execution or production qualification.

## Recovery, history and remaining boundaries

`recovery::reconcile_operation` verifies chain/genesis and exact persisted bytes,
checks canonical inclusion, and records receipt outcome plus sampled depth.
Reorg invalidation retains all signed bytes and claims. Missing receipt/pending
entries yield `NotObserved`, never a proven dropped state or release authority.
`SqliteStore::reservations` provides bounded durable operation history.

`AccountSession::prepare_reserved` reviews a new intent for an existing unsigned
nonce after interruption. `SqliteStore::reopen_unsigned_nonce` explicitly reopens
a released, never-signed nonce with its original ID/claim; a zero-value self
transfer can fill the gap. Cursors never rewind, and signed nonces cannot enter
this path. Replacement graphs, automatic terminal claim release, full ancestor
replay and browser persistence remain unfinished. Existing native WS streams
terminate explicitly; applications must reconnect and use bounded canonical
queries for recovery until an integrated reconnect/backfill engine is supplied.

The provider also exposes `outpoints_many` (bounded sequential current queries)
and `outpoint_deltas` (strict full address coverage over inclusive hash bounds).
The latter requires retained indexing and caller-verified canonical ranges.
Pinned noncanonical delta behavior must not be used as directional reorg undo.
