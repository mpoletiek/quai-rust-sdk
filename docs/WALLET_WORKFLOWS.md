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
`m/47'/969'`. Full native backups cover seed/master payment origins and imported payment-account
xprvs. `BackupOrigin::from_payment_account_xprv` stores the asserted account index
and guarded key in authenticated QUAIWALT v3. Restore verifies every channel and
receive exposure; the origin cannot derive BIP44 accounts or other payment accounts.
Existing v1/v2 backups remain readable. A v3 backup cannot be downgraded by changing
its version header.

Exchange public codes out of band and register an owner-validated `PaymentChannel`
using `SqliteStore::import_payment_channel`. `payment_channels::payment_intent`
allocates enough fresh send destinations for denomination outputs, preserving
all burned ranges on partial failure. It feeds ordinary `QiSession::prepare`.

`scan_payment_channel` searches the registered peer's receive derivation with
default gap 50 or explicit deep ranges, persists locally ownership-verified
receive exposures, and refreshes all known Qi addresses. Repeating a scan is
idempotent for exposures and never rewinds a cursor. `QiKeyring::load_payment_channels`
derives and verifies those receive keys for every registered channel, enabling
mixed-origin spending; a channel left unloaded makes a spend that selects its
outputs fail at preparation.
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
a conservative conversion gas budget, balance/fee limits, durable nonce claims, and the
usual frozen sign/broadcast stages. The default pending-state policy propagates
RPC failures. Select `AccountObservationPolicy::PinnedLatest` explicitly on nodes
that lack working pending-state reads. Nonce, balance and simulation then share
one numeric block selector, with head rechecks before reservation and return.
This excludes mempool effects; the durable local nonce cursor still prevents local
nonce reuse. Transfers, conversions and deployments use the same selected policy.
The conversion budget includes origin gas and a denomination-count bound below
the current nominal quote, since discounts can require more outputs. Raw node
estimates omit origin costs. Gas margins apply to the combined budget; small
caps may now fail before reservation. Quote increases and gas schedule changes
remain risks; this does not guarantee destination success or full credit.

`QiSession::prepare_special` accepts `QiSpecialIntent::Conversion` with destination,
refund, slippage and an **explicit authorized fee in Qits**. The ordinary node fee
estimator drops specialized data and is unsuitable for this operation.
`estimate_qi_conversion_fee` remains an explicit capability error without a selected
node profile. `prepare_special_estimated` uses `QiFeeProfile::V056ShaAnchored` for
known go-quai v0.56.0 nodes at/after prime 1,755,000. It quotes the exact selected
shape, applies UTXO gas scaling plus one 100,000-gas special ETX charge, the node
estimator’s 20% base-fee margin, and round-up conversion into Qits. Earlier fork
state, inconsistent rates, changing sampled heads, fee-budget overflow, and
nonconvergence fail before input reservation. `fee_quote()` retains the final
advisory quote. Selecting a profile is a caller assertion of node rules, not
software attestation; fees can still change before inclusion. The same planner
supports native Qi wrapping. `sign_special` commits the exact
verified conversion; normal session broadcast recovers the persisted operation.

Use `ConversionReference` / `observe_conversion` for bounded origin, changed-hash
ETX and refund correlation. Current observations do not independently prove a
specific conversion mature or spendable. Refresh actual destination outputs
and respect their observed locks before selecting them.

## WQI and WQUAI

Configured deployments in Cyprus-1:

| Token | Network | Address |
| --- | --- | --- |
| WQI | Mainnet and Orchard | `0x002b2596EcF05C93a31ff916E8b456DF6C77c750` |
| WQUAI | Mainnet | `0x006C3e2AaAE5DB1bCd11A1a097cE572312EADdBB` |
| WQUAI | Orchard | `0x005c46f661Baef20671943f2b4c087Df3E7CEb13` |

`wrappers::{WQI_ADDRESS, WQUAI_MAINNET_ADDRESS, WQUAI_ORCHARD_ADDRESS}`
expose these constants. `WQUAI_ADDRESS` remains a mainnet compatibility alias. Adapters also
accept an explicit deployment address. Mainnet code presence was observed for
both; the [read-only report](../test-infra/reports/wrapper-deployments-2026-09-12.json)
records runtime SHA-256 values. Later [public mainnet reads](../test-infra/reports/mainnet-public-read-recheck-2026-09-13.json)
confirmed both mainnet deployments. The corrected Orchard WQUAI address returned
2,029 code bytes, name `Wrapped Quai`, symbol `WQUAI` and 18 decimals on September 14.
Earlier empty-code reports used the mainnet address on Orchard. Code presence is not
an implementation audit or funded workflow acceptance.

- Native Qi wrapping uses `QiWrappingTransaction` or
  `QiSpecialIntent::Wrapping`: exactly 20 owner-contract bytes and same-zone Quai
  beneficiary outputs. The operation remains distinct from 22-byte conversion
  data and ordinary empty-data Qi. Preparation uses an explicit Qit fee.
- `WrappedQi::unclaimed` reads protocol backing in Qits (the exact pinned
  `no wrapped Qi balance` error maps to zero; other errors propagate); `claim_deposit` prepares
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
session. WQI claim/redemption calls retain the zone's `0x0a` lockup contract
in `ContractCall::access_list`; `into_account_intent` preserves that declaration.
External signers must also copy it. A successful `quai_call` or gas estimate can
automatically discover access and does not prove a signed empty list will execute.
Generic calls can use `ContractCall::with_access_list` with explicit bounded,
ordered entries; simulation preserves them. No wrapper helper signs or sends implicitly.

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
this path. Both account and Qi sessions expose `signed_candidates`,
`broadcast_candidate` and `observe_candidates`. These retain the original claim
while checking every candidate after restart or an ambiguous send. Account
replacements raise only gas price with an explicit bump/fee policy. Qi
`prepare_replacement` reduces explicitly selected locally owned change, retaining
all other outputs and ordered inputs. Qi conflicting candidates may coexist in
the pool; a higher fee does not guarantee replacement or inclusion preference.
Repeated signing of a prepared Qi candidate returns its persisted signature.
Backup v5 carries Qi families; account-only families remain v4. Older versions
remain readable and cannot silently discard candidates through a format downgrade.

`AccountSession::prepare_cross_zone` and `prepare_cross_zone_reserved` simulate
at the sender zone and preserve the exact destination/data through nonce custody,
review, signing and restart broadcast. Origin success is separate from destination
ETX execution. Normal `prepare` continues to require a same-zone recipient.

`HeadTracker` provides bounded canonical replay with explicit removed/added
blocks. `WsHeadFollower` reconnects read subscriptions and fills missed headers.
A reorg beyond retained ancestry reports unavailable history. Applications still
own wallet-state application and finality policy; signed claims are not released
on absence. Browser applications can persist opaque snapshots with scoped
revision checks using `BrowserSnapshotStore`, including cross-tab conflicts and
tombstones. Full browser wallet integration and automatic terminal claim release
remain separate work.

The provider also exposes `outpoints_many` (bounded sequential current queries)
and `outpoint_deltas` (strict full address coverage over inclusive hash bounds).
The latter requires retained indexing and caller-verified canonical ranges.
Pinned noncanonical delta behavior must not be used as directional reorg undo.

## Destination settlement and observed Qi maturity

`ExternalReference` reconstructs wrapping, WQI redemption and both-ledger
cross-zone references from exact signed bytes. `Provider::observe_external`
checks the original receipt/emission, exact beneficiary/sender/value/data/subtype,
then scans a caller-bounded destination range using the stable origin hash and
ETX index. It rechecks canonical anchors and preserves missing/failed outcomes.
`from_wqi_unwrap` checks the exact canonical ABI call and atom-to-Qit conversion;
it binds an explicit contract and ETX index without attesting its bytecode.

`observe_conversion_qi_credit` and `observe_external_qi_credit` attribute current
indexed outputs and reported locks. A Qi refund uses its signed refund address.
Cross-zone Qi is special: the ETX value is a denomination index and the UTXO uses
the original Qi transaction hash/output index, rather than the final ETX hash.
`QiCreditObservation` exposes both identities and separates locked, unlocked and
unobserved Qits. Unobserved value may be spent, trimmed, truncated by destination
gas or absent from indexing; it is never classified as lost. Unlocked observations
still require wallet claims and fresh spend preparation. No method treats a
successful wrapping receipt as a subsequent WQI token mint or claimDeposit.

`settlement::track_settlement` starts from a durable reservation/candidate,
reconstructs the appropriate reference, performs the bounded observation and
persists a compact public JSON summary before returning. SQLite schema v4 adds
candidate/ETX-index cache slots with revision checks; versions 1–3 migrate
atomically. `observation_cache` and `cached_settlement` expose restart summaries.
Applications must recheck their anchors before choosing continuation ranges.
Errors attempt a revision-checked invalidation; signed claims never change.
Caches are disposable and excluded from full backups; restore clears them and
retains the exact signed candidates used to reconstruct references.


## Resume a saved destination scan

`settlement::revalidate_settlement_cursor` loads a candidate's public observation
and checks its identity, network/genesis, origin, scanned page end and any
execution block against the current source. It returns a `SettlementCursor` only
when those anchors still agree. Missing or reorganized anchors require the
application to restart its explicitly chosen range; an unavailable receipt is
never interpreted as rejection or permission to repay.

Use `SettlementCursor::track` to continue. It carries the saved revision through
the next observation and final storage compare-and-exchange, so a stale cursor
cannot overwrite another handle's newer result. Invalidation similarly targets
only the revision originally read. The caller supplies an inclusive ending
height and transaction budgets; pages contain at most 256 blocks. When an
execution has already been found, the cursor rereads that one block to refresh
its receipt and currently indexed output locks. Signed intent is reconstructed
again before tracking; no cached summary authorizes a payment or claim release.

Cursors cover the requested saved page, not complete wallet history. A valid
incomplete observation keeps its UI summary but cannot produce a continuation.
Caches remain excluded from backups; restored exact signed candidates can be
tracked again with an explicit starting range. Tests use actual independent
SQLite handles and synthetic source headers, including a concurrent write during
reorg invalidation and another write during resumed observation.

## Confirm deployed runtime code

Build a provider `DeploymentReference` from the exact signed creation, trusted
genesis and optional expected runtime-code Keccak hash. `observe_deployment`
returns source inclusion/code observations; native `wait_for_deployment` adds
explicit confirmation, timeout and polling limits. Runtime code is queried at
the inclusion block, so later code or empty code cannot silently substitute for
what was observed there. Missing receipts, failed/locked outcomes, empty code,
mismatched code and reorgs remain separate results.

For native durable wallets, `deployments::track_deployment` reconstructs a root
or replacement candidate from stored bytes and saves a compact public observation
before returning. Runtime bytes are returned directly; the cache retains only
length, hash and expected-hash comparison. It survives reopening the database;
backup restore discards this reconstructible cache but retains signed candidates.
Source/validation errors tombstone only the revision read by that observer and
never release a nonce or trigger a broadcast. The isolated harness's
`verify-deployment` mode verified this complete read/reopen path against the
existing public fixture deployment without submitting a transaction.

The pinned JS factory requires a 46-character `IPFSHash` property before sending,
but does not encode or otherwise use it in the transaction. Rust does not require
that unused metadata guard. Exact init code, constructor arguments, grinding,
access list and fee authorization remain explicit and are tested against the
node. Solidity artifact import and broader factory conveniences are tracked
separately in the declaration inventory.

Load compiler output with `abi::SolidityArtifact::from_json`, or select one
contract explicitly with `from_compilation_json`. Pass `artifact.interface()`
and `artifact.init_code()` with constructor arguments to `prepare_deployment`;
this preserves the existing reserved-nonce, exact-code, grinding and fee checks.
The offline example uses a public fixture sender/nonce, prints the prepared
address and sizes, and does not sign or submit:

```sh
cargo run -p quai-sdk --features abi --example artifact_deployment -- contract.json '[]'
```

Use a single-contract artifact for this example. Constructor arguments are a JSON
array and must match the artifact's ABI. It does not compile Solidity, resolve
libraries or infer runtime code for deployment verification.


## Observe replacement families after restart

`recovery::track_family(provider, store, reservation_id)` needs the scoped database
and provider, without a wallet key or signer. It reconstructs all durable account
or Qi candidates (up to 33), validates receipt identity, checks each inclusion
and the sampled head, and returns a `FamilyUpdate` after persisting its public
summary. Failed executions can be canonical winners; absent transactions remain
`NotObserved` and never imply a definitive drop. Cross-zone destination and
conversion settlement still use the separate settlement observer.

The version-1 JSON summary occupies root candidate cache slot 65535, reserved
for family recovery. Its head and confirmations are sampled observations, so
requery before using them as current wallet state. New candidates invalidate the
summary atomically. A concurrent candidate append or cache revision prevents a
stale observer from saving its result; retry by re-reading the full family.
Errors invalidate only the revision read by that observer. Signed payloads,
nonce/input claims and the root reservation inclusion record remain independent.
Cache data is excluded from backups and must be rebuilt after restore.

`reconcile_operation` remains the root-only inclusion updater. It now validates
the receipt's ledger and account fields against the stored signed bytes and
rechecks the confirmation head before recording inclusion. It does not select
replacement winners. Neither API unlocks spent claims, rebroadcasts transactions,
or treats confirmation counts as irreversible finality.


## Browser account discovery

`discovery::AccountRpcSource` is available with `wallet` on native and wasm32.
Compose it with a `Provider<BrowserFetchTransport>` and pass it to
`wallet::discovery::discover` with an account xpub and explicit index bounds.
The source queries balance/nonce at a numbered block, validates network identity,
and rechecks canonicality around its reads. Browser transports retain their
thread-local JS handles; native discovery keeps its Send/Sync requirements.
No signer or native async runtime is required for this observation workflow.

The current-state source refuses `require_history = true` and does not interpret
zero balances/nonces as proof of no historical activity. Qi still uses the
separate current-outpoint workflow rather than this block-pinned account source.
Actual Chromium dedicated-worker tests cover complete facade discovery and
checkpoint changes; run `run_browser.py --suite sdk-worker` with the existing
wasm runner setup. This does not add browser SQLite sessions or a complete
portable wallet-state engine.


`discovery::discover_qi` provides a portable account-xpub current-outpoint scan.
`QiDiscoveryOptions::default()` uses gap 50 independently for receive and change,
explicit raw ranges of one million each, and separate address/output budgets.
Set `gap_limit: None` for a bounded deep scan. The result preserves actual child
indices, empty matching addresses, fixed denominations and reported unlock
heights; it discards arbitrary RPC extensions and rejects duplicate outputs
across addresses. Network identity is checked before and after scanning.

The initial, final and numbered heads must agree for `balance_at` to succeed.
A cancelled scan returns continuation metadata with unchecked canonicality;
head changes return `CanonicalStatus::Changed`. Neither can produce a checked
balance summary. Reported unlocked value still needs local reservation, expiry
and node-state checks before spending. Header agreement does not make multiple
latest-only RPC reads atomic or establish historical usage beyond the bounds.
Native durable import/refresh continues to use `qi_discovery::scan_and_refresh_qi`;
browser applications own persistence and reservation reconciliation.

For a read-only native scan without SQLite or private keys:

```sh
cargo run -p quai-sdk --example watch_qi -- RPC_URL CHAIN_ID_HEX GENESIS_HASH ZONE ACCOUNT_INDEX ACCOUNT_XPUB
```

Supply the trusted network genesis and a depth-three Qi account xpub, with its
correct account index. The example reports bounded coverage and exact Qit totals
without printing the xpub, deriving private keys or submitting transactions.

## Optional Qi address-use hints

`discovery::discover_qi_with_use_checker` accepts an async callback receiving the
explicit `NetworkScope` and Qi address. The callback runs only when current
outpoints are empty. Return true for known use to reset the gap, false to count
an empty address, or `QiDiscoveryError::UseCheckFailed` when the check is
unavailable. A returned `use_hint` records why an empty address reset the gap;
it adds no coins and does not certify complete historical recovery. Default
`discover_qi` still requires no checker or indexer.

Native `qi_discovery::scan_qi_with_use_checker` and
`scan_and_refresh_qi_with_use_checker` offer the same gap behavior with `QiError`.
A failed scan/checker leaves storage unchanged; after successful metadata import,
refresh queries every known origin and preserves durable claims. Callbacks must
bound their own I/O and should use known address history scoped to the supplied
network. Browser callbacks may hold thread-local state and need no Tokio runtime.

The pinned published `QiHDWallet.setAddressUseChecker` installs mutable wallet
state. Rust takes the checker per scan, so its scope and lifetime are explicit.
Eight JS cases verify current-output short circuiting and callback errors;
native database and real Chromium worker regressions cover integration.

## Qi message signing

`signer::Signer::sign_qi_message` uses the published Qi wallet's BIP340-over-Keccak
format, returning a 64-byte `SchnorrSignature`. Pass raw bytes, or UTF-8 with
`text.as_bytes()`. No personal-message prefix, chain ID or application domain is
added. `signer::verify_qi_message` requires both the expected Qi address and its
full public key; signatures have no recovery byte and x-only public keys cannot
identify the address's Y parity.

For registered native wallet origins, `qi::sign_message(resolver, metadata,
message)` checks the resolver's full public key and address before signing.
`HdWallet` and `QiKeyring` support HD, imported and loaded payment receive keys;
public metadata alone never grants signing authority. Browser/local key users
can call `LocalSigner::sign_qi_message` without SQLite. Watch-only signers reject
this operation. The API performs no RPC or transaction-state mutation.

```sh
cargo run -p quai-sdk --example qi_message
```

This offline example uses an explicitly public toy key and emits a JSON message,
public key and randomized signature for independent verification. Never fund its
address. CI checks the output with the pinned JS Schnorr verifier; 16 JS wallet
vectors exercise empty/Unicode/binary messages and both public-key parities.

## Recover channel lists and read Qi balances

`SqliteStore::payment_channels(owner)` enumerates the owner's registered peers
without requiring the application to remember each peer code. It returns up to
1,024 validated channels with their database generations, ordered by peer-code
bytes. The chain, genesis and owner/account are checked; channel cursors span all
zones on that network. No channel is opened and no cursor is advanced by listing.
After restart, use each `counterparty_code()` with `payment_addresses`, channel
refresh and explicit keyring loading. Malformed metadata fails the complete read.

`qi_discovery::qi_balance(store, candidate_height)` sums every stored Qi origin,
including receive/change, imported keys and registered payment receive addresses.
It requires an available snapshot at or before the supplied height and performs
no implicit RPC. Refresh explicitly first when current node state is required.
`total` equals the mutually exclusive `spendable + reserved + locked + expired`
buckets. Durable claims take priority; expiry uses an explicitly supplied profile,
not a guessed node trim schedule. Unlock boundaries apply at the candidate height.

The pinned wallet's `getBalanceForZone`, `getLockedBalance` and
`getSpendableBalance` optionally refresh their own caches. Rust makes refresh and
height explicit and excludes locally claimed or expired coins from spendable
funds. These observations neither include pending incoming transactions nor prove
that a node will still accept an output when it is eventually submitted.

The WQUAI adapter now has [funded isolated acceptance evidence](../test-infra/local-chain/wquai-evidence/README.md)
for deployment of bytecode matching the observed mainnet runtime, followed by
deposit, exact finite approval, transfer and withdrawal. It checks canonical
receipts, exact token/native balances and gas fees, overdraw reverts and durable
state after reopening. Only public toy-key development funds were used. Explorer
bytecode is not verified source; this is separate from contract audit, unmodified
node and funded Orchard qualification.

## Access-list discovery before account signing

Native `AccountSession::with_access_list_policy(AccountAccessListPolicy::Discover)`
adds node-backed access discovery to ordinary account/cross-zone origin calls and
deployments. The default `Preserve` retains the application's exact entries.
Discovery uses the same block selector, call bytes, value, gas price and nonce as
fee preparation, then re-estimates the final declaration. If durable nonce
allocation advances the pending nonce, discovery repeats from the original caller
requirements at the actual reserved nonce. Failed post-reservation discovery
leaves an unsigned reservation available for explicit recovery.

The node's generated list must still cover every supplied address and storage
key, including mandatory CREATE and WQI lockup entries. The returned list is
bounded and visible through `PreparedAccountTransaction::transaction` before
signing. Pinned-latest preparation rechecks its head; RPC errors or missing
requirements cannot produce signed payloads. The node may add unnecessary access
entries, which is why the final fee cap and payload review still apply.

Conversions have their own preparation path. Fee replacements and restart
broadcast preserve the already signed list; selecting discovery does not mutate
an existing signature or silently repopulate it. For cross-zone calls this only
simulates the origin; it does not prove destination execution or access needs.

The [isolated acceptance](../test-infra/local-chain/access-evidence/README.md)
starts with an empty generic contract access list, signs the discovered list,
broadcasts in a separate process and checks actual state, fees and reopened custody.

## Browser WebSocket subscriptions

`quai_browser::BrowserWebSocketTransport::connect` creates an explicit bounded
ws/wss session usable by `Provider` in a window or worker. `subscribe` returns an
owned stream; `next` distinguishes a quiet timeout from lost history, and
`unsubscribe` awaits remote cleanup. Reconcile canonical history after
`SubscriptionLagged` or disconnect before consuming new heads as continuous.
See the [example and lifecycle limits](../crates/quai-browser/README.md#websocket).

## Applying head replays to native wallet state

`recovery::reconcile_head_replay(&provider, &mut store, &mut tracker)` checks the
store/tracker network identity and polls a cloned `HeadTracker`. When canonical
history removes a suffix, one SQLite transaction reverts affected root inclusions
to Submitted, invalidates the entire current coin snapshot and tombstones all
public observation caches. It then advances the caller's head cursor. Missing
history, failed writes and concurrent snapshot changes leave that cursor unchanged.

Exact signed bytes, candidate families, input/nonce claims, addresses and allocation
cursors survive rollback and restart. Cache revisions and the scope generation
advance together. Built-in deployment, family, inclusion and settlement observers
capture the scope generation before RPC calls and reject writes after it changes,
including writes to previously nonexistent cache slots. Custom asynchronous
observers should capture `observation_generation()` and use
`compare_exchange_observation_scoped` or the corresponding family/inclusion method;
the older slot-only convenience methods cannot fence a pre-RPC scope change.

A returned `refresh_required` means requery current discoveries and operation
observations before displaying updated balances or settlement. Added headers do
not supply historical UTXOs: replay does not invent outpoint deltas absent from
the node. For restart-safe ancestry use
`recovery::reconcile_persisted_head_replay(&provider, &mut store, initial.as_ref())`.
On first use supply a trusted `HeadTracker`; subsequent calls can pass `None`.
The saved cursor is authoritative when present. One transaction compares both
pre-RPC scope generation and ancestry revision, applies any rollback, and stores
the new ancestry. Reopening the database resumes the retained history and detects
forks within it. A missing cursor or a deeper fork fails explicitly; after an
explicit full source invalidation/reset, supply a trusted older cursor again.

`HeadTracker::export_state`/`from_state` also support bounded public cursor storage
in caller-owned native/browser stores. The format retains at most 4,096 numbered
hashes, zone, genesis and page bounds; decoding checks exact length, contiguous
heights and distinct nonzero hashes. Restored cursors still recheck the node.
They are observations, not chain proofs. Wallet backups exclude ancestry caches;
restore tombstones any existing destination cursor so a stale writer cannot
reinsert it. No replay path broadcasts transactions or releases signed claims.

## Signing and submitting through an injected browser wallet

`InjectedProvider::sign_quai_transaction` validates a fully populated account
transaction and independently verifies the returned signature and exact payload.
After preserving the signed bytes in application state, compose
`signed_submission_transport()` with `Provider` for exact-hash acknowledgement and
ambiguous-send handling. The capability also accepts locally signed ordinary Qi,
conversions and wrapping. See the [example and extension support boundary](../crates/quai-browser/README.md#injected-transaction-signing-and-submission).

## Wallet-owned approval and sending

For extensions which only expose `quai_sendTransaction`, use
`InjectedProvider::send_quai_transaction` with the full explicit request. Save its
`WalletSendIdentity` first, then save the acknowledgement's reported hash. The
identity's signing digest is not a signed transaction ID. Use `observe` for one
readonly, cryptographically verified lookup and inspect `matches_request()` to
identify wallet changes before tracking the actual receipt. Timeouts/cancellation
cannot prove rejection; an unknown hash may require consulting wallet activity.
See [the full acknowledgement/recovery contract](../crates/quai-browser/README.md#wallet-mediated-sending).


## Packed contract/hash utilities

The ABI facade exposes `solidity_packed`, `solidity_packed_keccak256` and
`solidity_packed_sha256`, validated against 513 pinned reference observations.
They preserve declared-width scalar encoding and 32-byte primitive array padding,
with explicit strict Rust values and documented reference-helper extensions.
See [packed encoding semantics](../crates/quai-abi/README.md#packed-encoding-and-hashes).
These bytes are not canonical contract calldata or an unambiguous structured
signing format; normal calls continue to use `AbiCoder` and typed intents.

## Checked contract deployments

Use the [runtime-code preflight](CONTRACT_CODE_PREFLIGHT.md) before a funded wrapper
workflow. It checks nonempty code, trusted genesis, an optional expected runtime
hash and canonical block consistency. Offline wrapper constructors only encode
intents; address constants alone do not prove code is deployed on a network.
