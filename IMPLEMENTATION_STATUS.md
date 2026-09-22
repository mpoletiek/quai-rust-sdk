# Implementation status

**September 20, 2026 update.** `0.1.0-alpha.9` is prepared. A full protocol,
security, performance and parity review against go-quai v0.56.0,
`quais@1.0.0-alpha.57` and Pelagus 1.0 is recorded in
[the protocol alignment review](docs/history/PROTOCOL_ALIGNMENT_REVIEW_2026-09-20.md),
which is the current assessment and supersedes the dated sections below where
they disagree. Three things it establishes are worth carrying here:

- The SDK is pinned to the current tip of every reference: go-quai `main` *is*
  the pinned `v0.56.0` commit, and npm's `quais` `latest` *is* the pinned
  `1.0.0-alpha.57`.
- **The pinned `quais@1.0.0-alpha.57` package ships a compiled `lib/` stamped
  `1.0.0-alpha.52`**, and the export map resolves to `lib/`. The declaration
  ledger and every generated fixture therefore describe alpha.52. The counts
  below are accurate for what was reviewed; they are not a statement about
  alpha.57's full surface. `npm run verify` now pins both halves separately.
- No consensus-level encoding, signing or denomination defect was found.

The remaining open items are listed in that review's sequencing section; the
largest is the custody operation lifecycle, where the portable books retain 256
operations per ledger per scope with no pruning.

Updated September 14, 2026 (US/Central). The `0.1.0-alpha.1` implementation and
published-reference declaration review are complete within the explicit
[parity differences](SDK_PARITY_ANALYSIS.md). All 3928 rows are reconciled: 74
implemented, 3854 deliberate deviations, zero partial and zero pending. Counts
are not independent features or proof of production safety. All twelve packages
are published on crates.io as `0.1.0-alpha.1` (tag `v0.1.0-alpha.1`), and their
docs.rs builds succeeded.

The [SDK guide](SDK_DOCUMENTATION.md) describes current native/browser workflows.
The dated reviews and chronological sections below retain earlier evidence and
backlogs; use the current comparison for today's implementation assessment.
Production qualification remains distinct from alpha package preparation.

The [funded Orchard record](test-infra/orchard/README.md) adds confirmed account
transfers, full corrected conversion output creation and a mature Qi spend.
Orchard WQUAI uses `0x005c46f661Baef20671943f2b4c087Df3E7CEb13`; its
code, metadata and a funded 0.01 QUAI deposit/withdraw round trip are verified.
Earlier empty-code observations queried the
mainnet WQUAI address on Orchard and remain dated evidence below.

## Implemented and locally exercised

| Area | Delivered behavior and evidence |
|---|---|
| Reference identity | Locked `quais@1.0.0-alpha.57`, verified published source/artifact, pinned go-quai v0.56.0; 3,928 declaration rows retained in `compatibility/parity.json` |
| Primitives | Checksummed ledger/zone addresses, exact hashes, bounded signed decimal conversion and unsigned chain amounts, exact-code CREATE and CREATE2 prediction |
| Crypto | Redacted zeroizing keys, OS entropy, deterministic recoverable ECDSA, BIP340 Schnorr, ordered local multi-key aggregation, hash/HMAC helpers |
| Consensus | Canonical bounded protobuf for Quai and ordinary Qi, distinct signing digests and signed transaction IDs, immutable verified signed payloads; malformed/noncanonical input rejection before unbounded allocation |
| HD identities | BIP39 all ten wordlists, normalization/passphrases, BIP32 extended keys, Quai/Qi coin types, receive/change paths, bounded cancellable zone grinding and watch-only account derivation |
| Encrypted backup | Authenticated seed and full native wallet envelopes, bounded Argon2id/XChaCha20-Poly1305, guarded key origins, monotonic reservation restore, signed-claim retention and checkpoint invalidation; independent format vectors. QUAIWALT v5 preserves Qi candidate families, v4 account fee-replacement families, v3 imported payment-account origins; v2 channels/exposures and v1 remain compatible |
| Selection | Full input-denomination capacity across recipient/change, fee convergence and eligibility checks; explicit all-eligible sweep/aggregation policies. Existing 69 JS vectors plus capacity/aggregation regressions pass |
| Signers | Local chain-bound and watch-only adapters, offline Quai/single-input Qi signing and personal-message signing; consensus supports ordered local multi-input Qi signing |
| HTTP/provider | Exact direct URLs and gateway routes, strict envelopes and U256 quantities, bounded native transport, typed headers/account calls/transactions/ETXs/receipts/outpoints, account broadcast ambiguity and canonicality-checked receipt polling |
| WebSocket | Native and browser/worker bounded sessions; browser cancellation/queue ownership tested in actual Chromium. Native bounded session and subscription implementation; deterministic loopback tests and a real LAN mainnet head notification passed; bounded reconnect and canonical head replay are implemented |
| Durable state | SQLite schema v5 persistent ancestry and candidate/ETX observation caches with revision checks, claims/cursors, specialized signed-byte custody, canonical inclusion reconciliation and conservative reorg invalidation, unsigned nonce recovery and authenticated backup |
| ABI/typed data | Bounded canonical and packed ABI/interface/EIP-712, typed contract calls/ERC-20 helpers, value-derived event filters and call/log/revert parsing, bounded queries retaining reorg metadata, same-zone CREATE grinding with required access list |
| Account workflow | Durable prepare/sign/submit, exact conversion simulation, nonce claims, unsigned restart preparation and explicit nonce-gap repair; fee-only candidate families, restart recovery and explicit cross-zone prepare/resume |
| Legacy keystores | Bounded v3 AES-CTR/scrypt/PBKDF2 import/export, NFKC/byte passwords, all-language mnemonic derivation checks; eight tests cover 15 JS vectors, hostile inputs and fresh production-cost exports. Upstream scrypt workspace wiping remains a security-review limitation |
| Qi workflow | Current gap-50 discovery, mixed HD/imported/BIP47 signing, ordinary/cross-zone preparation, sweep/explicit aggregation, durable signed transfers/conversions/wrapping and restart broadcast |
| Payment codes | BIP47 seed/master/account-xprv derivation, registered send destinations and receive gap/deep scanning, verified receive key resolution, monotonic exposure imports and authenticated seed/master/account-xprv channel backup |
| Discovery | Both the history-capable abstract scanner and supplied current-state Qi scan/refresh; gap 50, explicit deep ranges, all stored origins, fixed denominations/locks and balance buckets; latest-only consistency limits remain explicit |
| Conversions | Typed rates/calculation, durable Quai-to-Qi and Qi-to-Quai preparation, explicit specialized Qit fees, signed backup/recovery and bounded ETX correlation and attributed current Qi credit/refund locks; automatic fees require the explicit SHA-anchored v0.56.0 profile; maturity qualification remains open |
| Browser | Real Chromium Fetch/injected-provider tests, recovered personal/typed-data signatures, worker Fetch, HD/imported/BIP47 key resolution, metadata reopen, OS entropy and ordered signing; scoped atomic IndexedDB snapshots; full account/Qi browser sessions, durable custody, coordinated capture/restore and settlement are implemented; real extension interoperability remains unqualified |
| Wrappers | WQI native wrapping/claim/redemption and WQUAI deposit/withdraw; exact units, typed ABI and redemption dust/gas guards; user-confirmed deployment constants; four independent JS/Rust/Go wrapping fixtures |

The native offline example derives both ledger identities and signs/decodes a
Quai transaction. It prints public fixture information only and never sends.
See individual crate READMEs and tests for API limits.

## Evidence from the earlier implementation baseline

- **14 transaction fixtures** agree with pinned JS and the independent Go oracle
  on protobuf bytes, signing digest, transaction ID and signature verification.
  Two fixtures intentionally demonstrate node-invalid Qi data lengths and are
  rejected by ordinary-transfer signing. No fixture is a node acceptance result.
- **11 ordered aggregation vectors** agree across JS, Rust and Go, including
  duplicate/reordered keys. Go verifies both JS and captured Rust signatures.
  Crypto has 15 tests including 19 official BIP340 vectors and 24 ECDSA cases.
- Wallet derivation exercises 50 mnemonic vectors, all 20,480 wordlist entries,
  24 extended-key vectors and six zone-grinding cases. Seven backup tests cover
  an independent vector, wrong passwords, corruption, hostile parameters, seed
  lengths and fresh encryption randomness. Full-cost KDF tests are intentionally
  slower than ordinary unit tests.
- Selection matches **69** JS cases and tests maturity, expiry, reservations,
  malformed snapshots, conservation, fee convergence and budgets.
- Exact amount conversion matches **94** JS cases plus every power of two in a
  256-bit amount at all 81 supported decimal scales. **30** CREATE/CREATE2 cases
  pass Rust and Go; 18 record the JS leading-zero init-code prediction defect.
- The initial address/RPC suite, routing fixtures, Python readiness tests and
  npm regression suite remain in the tree. Tests are rerun as changes land;
  historical aggregate counts must not be read as a current whole-workspace run.
- New native WS loopback tests and live LAN subscription checks passed. Storage
  tests cover actual independent-process contention, restart and rollback.
  The earlier whole-workspace native all-features run passed **254 tests** plus three subprocess contention checks (four
  explicitly gated live tests ignored). This count is evidence for that run, not completion.

The Go evidence is reproducible with `test-infra/go-oracle/run.py`; the retained
report binds source identity and fixture hashes. JS source comparison establishes
reference provenance, not consensus correctness. Security review, fuzzing,
platform tests and actual funded acceptance are independent gates.

## September 12 workflow expansion

See [wallet workflows](docs/WALLET_WORKFLOWS.md) for new public APIs and examples,
and the [updated review](docs/history/FEATURE_COMPLETENESS_REVIEW_2026-09-12.md) for FC01–FC12
delivery and remaining gates. Wrapping bytes/digests/hashes/signatures match all
four independent Go oracle cases in `test-infra/go-oracle/WRAPPING-RESULTS.json`.
Mainnet reports nonempty code at both confirmed wrapper addresses; Orchard
returned HTTP 403. Neither observation establishes funded execution acceptance.

The [retained validation summary](test-infra/reports/wallet-workflows-2026-09-12.json)
records this expansion: **271 native tests plus three subprocess checks**
passed with all features (four explicitly gated live tests ignored); **175**
no-default-features tests passed; all **39** facade tests passed after the final
balance additions. JS reference verification, two npm regression files and the
3,928-row parity check passed (30 implemented, 152 partial, 47 deviations, 3,699
pending). These are evidence counts, not a completeness percentage. Strict
Clippy and rustdoc checks passed. That expansion initially lacked a local wasm toolchain. A later isolated
toolchain now runs real Chromium browser/worker tests and wasm checks, recorded
below and in the retained platform reports.

## Node qualification

| Endpoint | Observed behavior |
|---|---|
| LAN `http://10.0.0.12:9200` | Direct mode, chain 9, mainnet genesis and nonzero execution limits, typed wallet reads passed |
| LAN `ws://10.0.0.12:8200` | Rust chain read, `newHeads` registration, actual notification and unsubscribe passed |
| LAN `http://10.0.0.12:9001` | Prime chain/running-zone reads passed |
| Orchard gateway `/cyprus1` | Chain 15000, typed Rust reads and actual Rust WS head notification/unsubscribe passed |
| Isolated loopback development chain | Real Quai transfer, Qi single/multi-input spend and CREATE deployed-code/call acceptance; stopped-database reset restored funded state. Explicit patched profile, not unmodified consensus qualification |

`quai_clientVersion` reports LAN `go-quai/v0.56.0-f3f345c8` and Orchard
`go-quai/v0.34.0-pre-82368aff`. These are self-reported versions, not build or
synchronization attestations; the differing versions require separate protocol
qualification. (Historical: no mainnet transaction had been submitted at that point;
see the September 14 mainnet section.) The isolated funded harness is available with documented development patches;
both conversion directions, refund and maturity/spend acceptance now also pass
on their separate controller/lock-adjusted profile. Unmodified mature-node writes
and disposable funded Orchard acceptance remain
required. The documented Orchard faucet hostname currently fails DNS here.

## Current qualification boundary

The current implementation includes native and browser preparation, signing,
submission, canonical candidate reconciliation and signed-intent settlement.
Applications explicitly apply observations; absent transactions never implicitly
release signed claims. Current gap-50 discovery needs no indexer, but latest-only
outpoints cannot prove fully spent address history or an atomic historical snapshot.

Funded unmodified-node acceptance, mature WQI redemption spend, broader browser
engines/extensions, sustained fault/reorg/soak/performance/fuzz campaigns and
independent security review remain production gates. Mainnet checks are read-only;
[isolated-chain evidence](test-infra/local-chain/README.md) has its own patched
profile. Registry publication and hosted docs.rs builds completed on
September 14, 2026; neither establishes production qualification.

## Historical internal security review closeout

The [internal review](docs/history/SECURITY_REVIEW_2026-09-11.md) fixed four medium findings
and hardened password normalization. All three bounded sanitizer fuzz targets passed
875,310 executions. Final advisory scans found no known vulnerabilities in the SDK
and separate fuzz lockfiles; two inactive optional unmaintained packages remain.

The [high-level live wallet evidence](test-infra/local-chain/HIGHLEVEL.md) verifies
Qi preparation, fee convergence, signed-byte persistence, process restart and exact
inclusion/output accounting. Pending-state account RPCs crash on the pinned
disposable node: preparation correctly stopped before reservations or signatures.
At that baseline this blocked high-level account/deployment live qualification.
Subsequent explicit confirmed-state preparation and deployment/code observation
passed on the documented isolated profile. Nodes used for the original report
were stopped after that run; subsequent harness runs have separate ownership.
Mainnet remained read-only. Upstream scrypt
workspace wiping and the other documented release gates remain unresolved.

## Qi candidates and destination tracking verification — 2026-09-12

The [retained report](test-infra/reports/qi-candidates-settlement-2026-09-12.json)
records 300 native workspace reported passes, zero failures and four intentionally
ignored external integration tests. Strict native/Wasm Clippy, rustdoc, 11 real
Chromium browser tests and five worker tests pass. The updated SDK revalidated
its previously funded Qi operation on the owned isolated development chain.
New observer fixtures are explicitly synthetic and do not qualify funded wrapping
or multi-zone destination execution. The older source-only/block receipt observer
remains available; current indexed Qi output attribution is a separate API.

## Encoding and provider coverage — 2026-09-12

Bounded byte encoding and signed-width helpers pass 80 generated quais.js
vectors, including zero preservation and signed boundaries. Access-list
creation, protocol expansion, advertised regions, pending-header bytes and
bounded account pool content/inspection are implemented. The read-only
`inspect_pool` example passes on the owned development chain after explicitly
enabling its loopback txpool module. [Evidence](test-infra/reports/encoding-provider-2026-09-12.json)
records 305 workspace reported passes, zero failures, four ignored integration
tests, strict native/Wasm Clippy, rustdoc and real browser/worker results.
The parity tracker reconciles 30 encoding and 264 provider declarations with
explicit typed-Rust differences; its remaining rows are still tracked.


## Fixed-point arithmetic and mined-block reads

Checked fixed-point arithmetic now covers validated formats, exact imports,
arithmetic with explicit rounding, rescaling and numeric comparisons. A retained
172-vector JS corpus records matching behavior and deliberate corrections to
negative rounding and floor/ceiling defects. Fields are bounded to 256 bits and
80 decimals; unsafe wrapping and implicit floating-point amounts remain explicit
Rust deviations.

Mined zone blocks can be read by latest, number or hash, as full executed
transactions or ordered hashes. Exact header-by-hash lookup and the read-only
`inspect_blocks` example are included. The example passed against the isolated
chain and LAN mainnet node; neither probe submitted transactions. Pending work
and root/region block views retain their documented qualification boundaries.

Receipt status 2 now remains `Locked`; conversion/refund and redemption credit
queries preserve locked states and inspect partial outputs after failed execution.
Tests cover both ledgers' conversion beneficiaries and the original Qi refund
address, without treating missing indexed outputs as lost funds.


Destination scan resumption now revalidates saved origin/page/execution anchors
and carries cache revisions through resumed tracking. Existing executions are
reread for current receipts and output locks. Reorg/missing anchors require an
explicit restart range, and concurrent observers cannot clear or overwrite one
another's newer cache. This remains source observation, not historical recovery
or finality qualification.


Validated `AbiValue` pairs now cover explicit Solidity types, immediate range and
shape checks, bounded zero/empty defaults, exact integer bounds and structural
metadata. There are 426 pinned reference encoder cases and 64 integer bound
cases; JavaScript constant-zero helper stubs and delayed validation are recorded
as deliberate differences. The extended ABI/default fuzz run completed 2,485,442
executions with no sanitizer failure. A CI-only Chromium profile deletion race
was reproduced deterministically and fixed without suppressing storage assertions
or permission errors.


Direct signed deployments now have intent-bound receipt/runtime observation and
bounded native waits, plus durable candidate reconstruction/cache persistence.
Runtime is queried at the inclusion block; mismatched/empty code and failed,
locked, missing or reorganized execution remain explicit. The existing public
fixture deployment at block 6 passed the SDK observer and database reopen checks
on the isolated development node, with two sampled confirmations and no new
submission. Funded Orchard/unmodified-node qualification remains separate.


Solidity artifact import now accepts standard creation-bytecode string/object
and `evm.bytecode` forms, plus explicit source/contract selection from full
compiler output. It validates the ABI and constructor payload, preserves exact
leading-zero code and rejects duplicate keys, unresolved placeholders and
conflicting/empty code. Twelve factory cases and the offline deployment example
pass; the new artifact parser path completed 2,034,432 sanitizer fuzz executions
without failure. Library linking and runtime/immutable fixups remain explicit
application work, as in the pinned factory's bytecode requirements.


Extended private/public keys expose matching BIP32 metadata with redacted
diagnostics, and imported subtree keys support bounded relative paths. Forty
pinned JS cases cover chain codes/fingerprints/raw hardened child numbers and
import round trips; relative account derivation agrees with pinned leaf xpubs.
All 11 wallet reference tests and strict native/wasm clippy passed. Forty-four
HD declaration rows now link these APIs and explicit path/secret differences.


Signer-free replacement-family recovery now persists a bounded summary after
network, receipt, inclusion and head checks. SQLite compares both the complete
candidate list and old cache revision under its writer lock; a newly committed
candidate invalidates the prior summary atomically. Regression tests cover
failed canonical winners, conflicting winners, concurrent candidate/cache
writers, 33-member capacity, root receipt identity and confirmation-head changes.
The existing isolated block-7 replacement passed this API and database reopen
without another transaction submission. This closes observation integration,
not terminal claim release, irreversible finality or full ancestry replay.


Local Cargo archive qualification now packages all 12 crates, runs a separate
minimal/default/full consumer against extracted SDK sources, and checks every
packaged native test/example target with all features. Public fixture references
are crate-local and 56 mirrored fixture/license files are checked against their
origins. An isolated CI job repeats the offline rehearsal after dependency fetch.
No crate was published; registry bootstrap, release approval, docs.rs and
independent security qualification remain separate gates.


The account RPC discovery source now works with non-Send browser transports as
well as native providers. A wasm-specific SDK test target avoids native Tokio
runtime features and runs the complete account-xpub/Fetch/numbered-state scan in
a Chromium dedicated worker. Three actual worker tests passed, including a changed
checkpoint and explicit history/cancellation failures. The browser CI job now
runs these facade-level tests in addition to the transport and worker suites.


Portable Qi account discovery now needs only an account xpub and provider,
including Browser Fetch. It retains compact current outputs/locks, enforces
address and total-output budgets, checks duplicate references and before/after
network/head identity, and returns raw-index continuations. Native tests observed
exactly 50 empty matching addresses on each branch; dedicated-worker tests cover
a funded-address gap reset and lock boundary. `watch_qi` is a read-only native
example requiring no SQLite store. Fully spent history, atomic historical
snapshots and browser claim/state integration remain explicit separate limits.

Optional Qi address-use callbacks now match the pinned published wallet's
short-circuit/error behavior. Both portable discovery and native durable refresh
can continue past a known spent address without fabricating coins or requiring
an indexer. Eight differential cases, known-spent-gap and database-error
regressions pass; all four dedicated SDK worker tests pass with a non-Send async
callback. Default scans continue to use gap 50 without a callback.

Qi message signing now has an explicit Schnorr-over-Keccak API, distinct from
personal-message ECDSA. It supports owned native HD/imported/payment metadata
and portable local signers, validates full public-key/address ownership, and
rejects watch-only authorization. Sixteen published JS wallet cases verify in
Rust; pinned JS verifies the offline Rust example and rejects a modified message.
Actual Chromium worker signing obtains fresh auxiliary entropy and verifies the
signature. No account, RPC, transaction claim or broadcast is changed by signing.

Mnemonic entropy export now returns a redacted zeroizing guard and excludes
checksum bytes. The existing 50 JS mnemonic cases cover inverse export/import
across all ten languages and five entropy lengths. Generation is portable to
browser wasm through explicit Web Crypto support; worker coverage exercises all
five word counts and English/Japanese/Spanish round trips. Passphrases remain
caller-owned inputs and are not retained as mutable mnemonic properties.

Native payment-channel enumeration now recovers registered peers and validated
cursor generations after restart without a separately retained peer list. It is
bounded and scoped to owner/account plus chain/genesis. Tests cover other zones,
wrong owners/networks, unchanged database generations and corrupted metadata.
Qi balance regressions now cover HD receive/change, imported and payment receive
origins together, mutually exclusive claim/lock/expiry buckets, restart and
invalidated snapshots. Refresh remains an explicit separate operation.

WQUAI deposit/approval/transfer/withdrawal now pass on the isolated development
chain using deployed runtime bytes that exactly match the observed mainnet
contract. Five canonical executions verify exact native fees/backing, token
balances, finite allowance, overdraw rejection and database reopen. Signed bytes,
RPC transcripts and checksummed evidence are retained in
`test-infra/local-chain/wquai-evidence`. No verified source was available from the
explorer and no mainnet transaction was submitted; contract review, funded
Orchard and unmodified-node acceptance remain separate gates.

## WQI execution fixes and isolated acceptance (2026-09-13)

The [funded WQI flow](test-infra/local-chain/wqi-evidence/README.md) now passes
native wrap, backing claim, token mint/burn and attributed locked Qi redemption.
It exposed four fixed defects: exact absent-backing handling, specialized Qi
origin validation in both recovery paths, mandatory lockup access declarations,
and protocol receipt logs at Qi addresses. Captured failures and successful
retries remain separate evidence. Native/browser callers can inspect and preserve
`ContractCall::access_list`; `Log::address` and `LogFilter::addresses` now use
`Address` so both ledgers are represented. Contract ABI event matching remains exact.
The output lock at 241945 is observed; a mature redemption spend, modern fork
qualification, unmodified-node/Orchard acceptance and contract audit remain open.

Native account preparation now offers explicit bounded access-list discovery for
ordinary calls and deployments. It rechecks mandatory entries, repeats discovery
when nonce allocation advances the estimate nonce, and freezes the result before
signing. Reopened broadcast does not rediscover. Isolated generic WQUAI approval
acceptance passed at block 26 with exact fee/state/custody checks.

## Browser WebSocket completion

The browser crate now supplies an explicit `BrowserWebSocketTransport` and owned
`BrowserSubscription` without Window or Tokio dependencies. Requests, subscriptions,
queued bytes and outgoing buffers have separate limits; cancelled registrations
close the connection, and lost/overflowed notifications mark the stream lagged.
No requests are automatically replayed. The latest window suite passes 16 tests,
the dedicated-worker suite 10, and the JS bridge suite 12; strict Wasm Clippy passes.
See [browser lifecycle documentation](crates/quai-browser/README.md#websocket) and
[retained evidence](test-infra/reports/browser-websocket-2026-09-13.json).
This closes the browser WebSocket transport gap; complete persistent browser wallet
integration, real extension testing and browser reconnect/head-following orchestration
remain separate work.

## Whole-sequence ABI defaults

`AbiCoder::default_values` completes the reference parameter-list default operation
with aggregate field, value-node, text and encoding limits checked before nested
allocation. All 107 pinned JS default sequences match after the documented integer
representation normalization, including their exact encoded bytes. Empty tuples,
zero-length arrays and aggregate limits have separate regressions. The single-value
constructor uses the same bounded implementation.

## Atomic wallet rollback from canonical replay

`recovery::reconcile_head_replay` now applies reorganized head suffixes to native
wallet state before advancing the in-memory cursor. One SQLite transaction removes
affected inclusion observations, invalidates current coins and tombstones caches
while retaining signed bytes, candidates, claims and allocation cursors. Scope
generation checks fence delayed built-in RPC observers, including first writes to
empty cache slots. Tests cover rollback faults, revision exhaustion, concurrent
writers, missing history, replay paging, wrong scope and restart custody.

This delivers atomic reorg invalidation. Current discovery/settlement refresh is
still explicit; header replay alone cannot reconstruct historical Qi balances,
and the later persistent replay API below supplies bounded ancestry restoration.

## Verified injected transaction operations

The browser adapter now requests exact `quai_signTransaction` signing and verifies
the returned canonical protobuf, recovered sender and every unsigned field.
`InjectedSubmissionTransport` is an explicit capability for already-signed Quai,
ordinary Qi, conversion and wrapping bytes; the typed Provider retains exact hash
acknowledgement checks and ambiguous-send semantics. No draft transaction is
silently populated or retried. Seven shared runtime tests cover three captured JS
request shapes, field/order changes, permissions/context, malformed bytes,
cancellation, all Qi variants and failed/wrong acknowledgements. Actual Chromium
passes 23 window tests and 17 dedicated-worker tests. Real extension support and
full persistent browser wallet reservations remain separate gates.

Extracted package verification now compiles all Wasm browser and portable SDK
test/example targets as well as the native consumer matrix. It caught and fixed
Tokio-only attributes on otherwise portable contract/event tests: five contract
and two event tests now also run in a real worker.
[Retained evidence](test-infra/reports/injected-transactions-2026-09-13.json).
The later wallet-mediated send API below supplies a dedicated request and recovery
contract for extensions which decline offline signing.

## Wallet-mediated sends and independent transaction verification

`InjectedProvider::send_quai_transaction` now supports the extension's direct
`quai_sendTransaction` approval flow without requiring offline signing. It returns
a typed acknowledgement preserving the original request; explicit readonly
observation reconstructs verified signed bytes and exposes whether all requested
fields matched. Preflight failures and ambiguous post-dispatch outcomes are
separate, and original account/nonce/digest metadata survives errors without a
returned hash. No send is automatically retried. `Transaction::verified_quai`
also verifies a retained actual mainnet transaction and rejects mutated fields.
Browser state persistence and real extension qualification remain separate gates.


Canonical ancestry now survives native wallet restart. Bounded `HeadTracker`
state encoding/decoding and `recovery::reconcile_persisted_head_replay` atomically
save the cursor with conservative reorg invalidation, fenced by scope generation
and cursor revision. SQLite v5 migrates validated v1–v4; backup restore tombstones
old ancestry while preserving signed custody. Current-state refresh remains
explicit, and a fork deeper than retained history still requires recovery from
an older trusted checkpoint. This does not supply unavailable historical UTXOs
or terminal signed-claim release.


Packed ABI encoding and Keccak/SHA-256 helpers now cover 513 pinned JS observations:
341 exact byte/hash matches and 172 explicit rejections, including 66 inputs that
JS accepts through coercion or widened array integer ranges. Shared resource
limits, signed array padding, fixed-byte right padding, dynamic-field ambiguity
and reference-only nested/dynamic array extensions have dedicated tests. Canonical
ABI and EIP-712 paths remain the structured contract/signing APIs.


The text/crypto utility expansion adds explicit bounded Unicode normalization,
strict UTF-8 decoding/code points, immutable UUID-v4 formatting, RIPEMD-160,
shared OS/Web Crypto entropy filling and exact checksum-only address import.
Published-reference fixtures cover 75 normalization cases, 21 decoding cases,
five surrogate cases, four UUIDs, 19 RIPEMD inputs and 15 UTF-8 identifier hashes.
44 related utility/address/constant declarations now map to concrete APIs or
explicit typed-Rust differences; no malformed-address heuristics, global crypto
backend overrides or silent lossy decoding were introduced.


Typed ABI event filters now encode exact values or bounded OR alternatives in
full declaration order, and Contract queries bind them to the emitter. Interface
call/log/revert parsing uses canonical decoding and explicit unknown/collision
errors, including builtin Error/Panic. Tests cover 246 pinned filter cases,
call/log/revert fixtures, aggregate bounds, anonymous selection and actual worker
queries. 92 related declarations now map to tested APIs or explicit Rust differences.
[Retained evidence](test-infra/reports/abi-workflows-2026-09-13.json).


Public key-origin metadata and local Qi key resolution now run without SQLite.
Existing native storage paths re-export the same types; database and full-backup
encodings are unchanged. Four actual worker tests cover both-ledger metadata,
HD/imported/BIP47 ownership, ordered multi-input signing and IndexedDB reopen/CAS.
The new public metadata encoding is bounded and does not authenticate ancestry or
grant signing authority. Full persistent browser wallet lifecycle remains open.
[Retained evidence](test-infra/reports/portable-key-origins-2026-09-13.json).


Authenticated full-wallet backup handling is now portable through `backup`.
Shared public records, candidate validation and BIP47 exposure proofs no longer
depend on SQLite; native capture/monotonic restore keep their transaction boundary.
Seven actual worker tests cover QUAIWALT v1–v5, native-to-worker custody and cursor
inspection, payment-account ownership, tamper rejection and fresh encryption.
This closes envelope/inspection portability; live browser state merge, reservations
and complete recovery orchestration remain open.
[Retained evidence](test-infra/reports/portable-full-backups-2026-09-13.json).


Browser HD allocation now has durable range-before-search and address-before-return
commits. Scoped public journals preserve raw receive/change floors and all request
IDs across completion, abandonment, lost responses and restart. Actual worker tests
cover competing connections, cancellation after write dispatch, corrupt/tombstoned
state, idempotent resume and initialization from authenticated backup floors.
The 4,096-ID bound fails closed; this is address allocation, with UTXO/nonce claims
still separate browser work.


Browser payment-code allocation now commits consumed raw ranges before search and
verified destinations before return. Owner/account/peer/direction/network/zone
identities prevent context substitution; completed imports re-derive exact public
keys. Send destinations remain recipient owned. Worker tests cover competing
connections, cancelled search, dropped write futures, restart, tombstones and
authenticated backup floors. The public 1,024-ID journal retains abandoned IDs;
full browser wallet state, nonce/UTXO claims and transaction orchestration remain
open. [Retained evidence](test-infra/reports/browser-payment-allocation-2026-09-13.json).


ABI interfaces now import bounded readable declarations and format full/minimal
fragments in declaration order. JSON export preserves validated parameter/tuple
names, internal types and legacy flags; invalid fragments fail the whole import.
180 pinned formatting/selector cases and 31 rejection cases cover this addition,
with shared actual-worker tests and exact contract-facade calldata/block checks.
Gas annotations, Solidity source parsing and implicit JavaScript coercions remain
explicit exclusions. [Retained evidence](test-infra/reports/readable-abi-2026-09-13.json).


Receipt confirmation checks now run portably without Tokio. The native waiter
shares the same canonical block/receipt/head rechecks, while browser/worker waiting
adds owned timers, overall observable monotonic deadlines and explicit poll limits.
Cancellation drops active reads and clears timers; actual Fetch permit cleanup,
non-Send worker transports, reorgs and stalled reads are covered by nine worker
tests. Confirmation depth remains an observation, not finality or authorization
to release signed claims. [Retained evidence](test-infra/reports/browser-receipt-wait-2026-09-13.json).


A [provider declaration review](docs/history/PROVIDER_PARITY_REVIEW_2026-09-13.md) reconciles
144 inherited rows against existing typed APIs, explicit
normalization/routing differences and two pinned unsupported operations. The
inventory retains all 3,928 rows. Offline prototype probes and Rust comparisons
cover RPC mappings without constructing or connecting JavaScript providers;
existing fee/submission tests retain their separate scope. This is evidence-based
mapping work, not 144 newly implemented features or overall completeness.


Portable account transaction custody now retains nonce gaps, exact signed roots
and fee-only candidate families without SQLite. The browser adapter commits nonces
before returning and local signatures before exposure, with cross-tab CAS and
explicit revision fencing around canonical observations. Backup initialization
retains authenticated native account custody. Account-only capture and live merge
are also portable; Qi claims and complete browser session orchestration remain open.
[Workflow and bounds](docs/BROWSER_ACCOUNT_CUSTODY.md).

The Orchard read-only tests passed again on 2026-09-13. Its gateway reports
`go-quai/v0.34.0-pre-b07bc521`; configured WQI has code, while the configured
WQUAI address returned empty code. The faucet hostname still failed DNS resolution
from this environment. These observations do not qualify funded operations or
the pinned v0.56 protocol profile.
[Retained read-only evidence](test-infra/reports/orchard-read-recheck-2026-09-13.json).


Account-only backup capture and conservative live restore are now portable.
Exact owned metadata/private origins are proved before encryption; root and
replacement bytes restore into browser journals and native SQLite. Browser merges
retain live IDs, nonce floors and all signed candidates, reject conflicting claims
atomically, and invalidate stale inclusion. Full browser wallet capture across
account, HD/payment allocation and Qi state remains separate work.

[Account backup validation](test-infra/reports/browser-account-backup-2026-09-13.json)
retains six native tests including SQLite restore, eight actual worker tests and
a bounded sanitizer campaign, with the account-only capture boundary explicit.


Contract runtime preflight now pins code reads to rechecked canonical block and
genesis identities. Checked WQI/WQUAI bindings reject missing runtime, mismatched
trusted genesis and optional expected-code mismatches before returning a wrapper.
The offline constructors retain their intent-only behavior. Native and actual
worker tests cover error/cancellation boundaries; read-only checks on Orchard and
LAN mainnet retain exact block/code observations. Orchard WQUAI returned empty code
and was explicitly rejected by the checked binding.
[Workflow](docs/CONTRACT_CODE_PREFLIGHT.md).

## Browser Qi custody (2026-09-13)

Portable and IndexedDB journals now retain exact Qi inputs, public HD/imported/payment
origins and transfer/conversion/wrapping candidate bytes. Discovery reservation checks
locks, trusted derivation and the pre-discovery revision. Local signing persists bytes
before returning; concurrent/cancelled writes and reorgs retain custody. Native v5
backup initialization drops observations. See [workflow](docs/BROWSER_QI_CUSTODY.md)
and [validation](test-infra/reports/browser-qi-custody-2026-09-13.json). Qi backup
capture/live merge and complete multi-journal browser wallet capture remain open.

## Qi custody backup capture and live merge (2026-09-13)

`WalletBackup::capture_qi_custody` proves retained public origins before encryption.
Portable/browser live merges preserve claims and signed candidate branches, reject
conflicting or oversized unions atomically, and discard old observations. Tests
restore all Qi forms to native SQLite and exercise actual worker CAS merge races,
mixed-origin capture and stale revision rejection. See [validation](test-infra/reports/browser-qi-backup-2026-09-13.json).
Complete multi-journal browser wallet capture remains open.

## Combined portable wallet recovery capture (2026-09-13)

`WalletBackup::capture_portable` combines frozen HD/account/Qi/payment journals and
retained address inventory with explicit private ownership proofs. It preserves
burned cursors, both-ledger claims/candidates and channel exposures; native SQLite
restore/recapture and browser initialization across all journal types are tested.
See [workflow and format boundaries](docs/PORTABLE_WALLET_CAPTURE.md) and
[validation](test-infra/reports/portable-wallet-capture-2026-09-13.json). Automated
consistent browser collection and coordinated live restore remain open.

## Portable recovery inventory carry-forward (2026-09-13)

Combined capture now accepts authenticated `previous_inventory`: owned addresses,
HD burned floors, payment channels and earlier exposures survive successor-journal
capture. It preserves maximum cursors, rejects conflicting ancestry/ranges, and
re-proves ownership. Current account/Qi journals still supply transaction custody.
See [workflow](docs/PORTABLE_WALLET_CAPTURE.md) and
[validation](test-infra/reports/portable-inventory-recapture-2026-09-13.json).

## Consistent browser wallet backup collection (2026-09-13)

`browser_backups::capture_wallet_backup` collects explicitly selected HD/account/Qi/
payment stores, then rechecks every monotonic revision before ownership-proved
backup construction. Mutations, tombstones and bounds reject without retry; no
storage writes occur. Actual worker tests cover every journal type, first/last
revision races and cancellation. See [workflow](docs/PORTABLE_WALLET_CAPTURE.md)
and [validation](test-infra/reports/browser-wallet-collection-2026-09-13.json).

## Live HD/payment allocation restore (2026-09-13)

Portable/browser allocation merges preserve completed exposures and consumed IDs,
abandon pending work, and advance to maximum authenticated cursor floors. New
QADDRBK2/QPAYABK2 sealed-history journals retain nonoverlapping past records while
keeping strict new-range coverage; v1 parsing is unchanged. Independent fixtures
and native/worker race tests are retained in [validation](test-infra/reports/browser-allocation-restore-2026-09-13.json).
See [workflow](docs/BROWSER_ALLOCATION_RESTORE.md). Atomic multi-journal restoration
remains separate work.

## Atomic browser recovery and consolidated documentation

`browser_backups::merge_wallet_backup` now validates and atomically merges selected
initialized HD/payment allocation and account/Qi custody journals in one IndexedDB
transaction. Concurrent changes, duplicate/cross-database targets or invalid
merges reject without partial state. The shared storage path now enforces the
previously documented 2,048-namespace limit, including tombstones.

The consolidated SDK guide and parity analysis cover every crate and workflow
family, with a checked appendix of all pending/partial export families. This
does not finish the 2,639 pending declaration reviews or browser session/network
qualification gaps. See [atomic restore](docs/BROWSER_ATOMIC_RESTORE.md).

## Portable account quotation and browser transaction workflow

The shared `account_preflight` module now provides exact-nonce fee quotation with
explicit limits, pending/pinned observations and surrounding network checks.
`BrowserAccountSession` composes quotation with revision-fenced nonce reservation,
fixed payload review/signing and explicit persisted-root broadcast/restart.
Ordinary same/cross-zone calls and account-side wrapper operations are supported;
full Qi and specialized account browser workflows remain in progress. See
[browser account workflow](docs/BROWSER_ACCOUNT_WORKFLOW.md).

## Browser account conversion, deployment and access discovery

Portable quotation and the durable browser session now support exact Quai-to-Qi
value/destination/slippage and nonce-bound deployment preparation. Optional access
list discovery is shared with the native session and cannot discard mandatory
addresses or storage keys. Conversion fee arithmetic, leading-zero CREATE bytes,
reserved nonce mismatch, ambiguous-send replay and discovery rejection are covered
by seven portable native tests and twelve actual Chromium worker tests. All twenty
native account regression tests pass. Native/Wasm strict Clippy and rustdoc pass.
The prior account-workflow commit `13bcd00` passed all eight CI jobs in run
`34775551910`. Full browser Qi and account replacement/recovery integration remain
in progress; synthetic fixtures do not establish funded node acceptance.
All twelve source archives, three fresh extracted consumers, twelve native and two
Wasm packaged-target checks passed. Evidence is retained in
[test-infra/reports/browser-account-extensions-2026-09-13.json](test-infra/reports/browser-account-extensions-2026-09-13.json).

## Browser signed-family reconciliation

The portable candidate observer now powers native SQLite family tracking and
browser account/Qi recovery. Browser callers can inspect or explicitly submit any
persisted candidate, reconcile one canonical winner, retain prior inclusion when
receipt indexing disappears, and invalidate a lost inclusion without releasing
signed claims. Two portable tests, five Chromium worker tests and thirty native Qi
regression tests pass. The worker suite covers cancellation and concurrent writes,
all three Qi wire forms, replacement replay and reorganization. See
[the workflow](docs/BROWSER_CANDIDATE_RECOVERY.md). Browser replacement preparation,
destination settlement and full Qi preparation remain in progress.

## Browser Qi discovery and exact preparation

Portable `quote_qi` and `BrowserQiSession` now compose current gap/deep discovery,
all persisted owners/allocation completions, fixed-denomination selection, bounded
fee convergence and exact input custody/signing. Transfers, explicit cross-zone
payments, conversion, wrapping and sweep policies retain their actual wire shape.
Integrated discovery fences input and allocation journals atomically; concurrent
writers and cancellation cannot commit stale claims. Five portable tests, ten
Chromium worker tests and thirty native Qi regression tests pass. See
[the workflow](docs/BROWSER_QI_WORKFLOW.md). This is synthetic-RPC/browser runtime
evidence; unmodified funded/mature-settlement qualification remains open.
Native/Wasm strict Clippy and rustdoc pass. All twelve archives, three extracted
consumers, twelve native and two Wasm packaged-target checks passed. Evidence is
retained in [the report](test-infra/reports/browser-qi-workflow-2026-09-13.json).
The preceding candidate-recovery commit `63d4c63` passed all eight CI jobs in run
`34776798892`; its ancestor `18ee50e` run was cancelled by the newer push.

## Reviewed browser replacements

Account replacement quotation is shared between native/browser sessions and changes
only gas price under explicit bump/debit bounds and confirmed-nonce admission.
Browser Qi replacement planning preserves inputs, recipients and specialized data,
reducing only explicitly selected owned change. Signing persists the candidate edge
before returning; repeated Qi signing returns existing randomized-signature bytes.
Eight account and six Qi portable tests, twenty native account and thirty native Qi
regression tests pass; actual Chromium suites cover fourteen account and twelve Qi
cases. See [reviewed replacements](docs/BROWSER_REPLACEMENTS.md). Destination
settlement and the declaration-level semantic parity audit remain open.
Native/Wasm strict Clippy and rustdoc pass. Evidence is retained in
[the report](test-infra/reports/browser-replacements-2026-09-13.json). The preceding
Qi-workflow commit `df40f9b` passed CI run `34777796915`.

## Portable/browser signed settlement — September 13, 2026

Native settlement now delegates exact signed conversion, wrapping, WQI redemption
and cross-zone interpretation to a portable observer. Browser recovery selects
persisted candidate bytes and fences the destination result against its pre-RPC
IndexedDB revision. Custody bytes and claims remain unchanged. Destination views
and cursors are in memory on browser; native resume additionally persists cursors.

Three portable tests, eight Chromium worker tests (including a writer race),
20 account and 30 Qi native regressions, and 15 provider tracking tests passed;
one live provider test remained explicitly ignored. Strict native/Wasm SDK Clippy
and warnings-denied rustdoc passed. These fixture tests do not qualify funded
settlement. See `test-infra/reports/browser-settlement-2026-09-13.json`.
Prior replacement commit `2cb10d1` passed GitHub run 34778526388.

## Unknown account nonce competitors — September 13, 2026

Added portable bounded discovery of mined same-sender/nonce transactions absent
from registered candidate families, plus native successive-page waiting under
an overall deadline. Matching signatures and hashes, receipts, page ancestry,
canonical anchors and trusted genesis are checked. Results classify original,
repriced, cancelled and replaced without adopting candidates or releasing claims.

Six new Rust tests and seven receipt-wait regressions passed, plus five executable
published-JS checks. Native provider and Wasm SDK strict Clippy and provider rustdoc
passed. The 12-crate archive rehearsal passed all three extracted consumers, all
12 native target checks and both Wasm target checks. Twelve response declaration
rows now describe explicit API mappings and differences; unreviewed rows remain
pending. See `test-infra/reports/account-nonce-replacements-2026-09-13.json`.

## Local signer parity and browser keystore export — September 13, 2026

Legacy keystore export now shares the OS/Web Crypto entropy backend on native and
Wasm, preserving fixed strong KDF defaults and mnemonic ownership validation.
The local/watch-only signer review reconciles 175 pending/partial declaration
rows with explicit APIs, tests and differences. Rust rejects consumed Exact(0)
rather than reproducing the reference's pending-nonce substitution.

Eight native keystore tests, 24 Chromium worker tests (including production-cost
mnemonic export/import), eight native account preflight tests, 14 Chromium account
preflight tests and three JS signer checks passed. Strict Clippy and keystore
rustdoc passed. The preceding implementation's full-workspace rerun reported
477 passes and five ignored live checks; later changes received targeted tests.
Both refreshed lockfile advisory scans report no matched vulnerabilities and the
same two unsuppressed maintenance warnings.

Settlement CI run 34779281316 failed only the standalone IndexedDB fixture's
40-second timeout; native and package jobs passed. The harness now has progress
and browser-exit/stderr diagnostics plus a finite 120-second startup-inclusive
deadline. Local reproduction passed; CI must still qualify this change. See
`test-infra/reports/signer-keystore-parity-2026-09-13.json`.

## Selection reconciliation and publication metadata — September 13, 2026

Reconciled 38 selection/UTXO/denomination declarations. Three executable published
JS regressions demonstrate underfunded fee increases, reversed fee decreases and
lossy lock metadata. Seven Rust selection tests, including 69 fixed-fee cases,
pass and retain exact fees/targets/locks; wallet strict Clippy passes.

All twelve crates now declare publication metadata and docs.rs targets, with
repository links usable outside the checkout. The archive rehearsal passed twelve
packages, three extracted consumers, twelve native target checks and two Wasm
checks. All proposed crate names returned 404 on the dated read-only lookup; no
names were reserved or uploaded. Private vulnerability reporting is enabled.
`docs/PUBLISHING.md` records the dependency order, authorization boundary and
remaining qualification. See the retained selection/publication preflight report.

## 2026-09-13: portable and browser unknown account replacement waiting

Both native and browser waiters use `AccountReplacementTracker` to follow bounded
verified pages for an original or unknown same-sender/nonce competitor. Browser
waiting has window/worker deadlines, poll/page budgets and cancellation cleanup.
The tracker rechecks the prior page even if the reported head retreats; unsuccessful
polls do not advance. No custody adoption, claim release or submission occurs.

Seven native replacement tests, seven native confirmation regressions, four
Chromium worker replacement tests and nine Chromium receipt regressions pass.
Native/Wasm strict Clippy, warnings-denied rustdoc and extracted-package checks
pass. See [the retained report](test-infra/reports/browser-account-replacement-wait-2026-09-13.json)
and [the API guide](docs/ACCOUNT_NONCE_REPLACEMENTS.md). The previous main commit
`936fce0b054cc34de5ef0bdae3d88a0fb6655fb6` passed GitHub CI run `34781269431`,
including the standalone IndexedDB harness fix.

## 2026-09-13: specialized HD wallet declaration reconciliation

Reviewed 86 pending/partial QuaiHDWallet and QiHDWallet rows against the published
artifact and existing native/browser compositions. Added three executable source
checks for channel cursor reset, address getter/derivation behavior and provider
propagation. Native channel, HD reference, discovery, key-origin and payment
allocation regressions pass. Whole legacy wallet-JSON migration and cached
address-status convenience views remain partial, with concrete differences in
[the review](docs/HD_WALLET_PARITY_REVIEW.md).

The ledger now has 70 implemented, 1,344 deviation, 144 partial and 2,370 pending
rows. These are declaration counts, not a feature-completion percentage. This
batch changes audit/test documentation only; [the report](test-infra/reports/hd-wallet-parity-review-2026-09-13.json)
does not claim new package or runtime qualification.

## 2026-09-13: scoped Qi address inventory and cached gap views

Added `QiAddressBook` with exact HD/imported/payment receive origins, branch/account/
peer lookups, four-state cached usage and external/change/channel gap views.
Portable refresh queries every registered origin, checks resource bounds and
network/head identity, then commits one complete usage batch. Reorgs, errors,
partial failures and cancellation preserve the old cache. The cache has no
allocation or custody authority.

Six native and six Chromium worker tests pass, along with strict native/Wasm
Clippy, warnings-denied rustdoc, the minimal wallet feature check and all extracted
package checks. See [the guide](docs/QI_ADDRESS_VIEWS.md) and
[report](test-infra/reports/qi-address-views-2026-09-13.json). Six cached-gap ledger
rows move from partial to documented Rust behavior; legacy wallet-JSON migration
remains a separate gap. Ledger counts: 70 implemented, 1,350 deviation, 138 partial,
2,370 pending.

## 2026-09-13: verified legacy HD wallet migration

Added bounded quais.js version-1 wallet JSON import and explicit guarded plaintext
export. Import proves the mnemonic identity and every HD, imported and payment
address; it retains known allocation floors and discards stale usage/checkpoints.
Export rejects custody and allocation state the legacy format cannot preserve.
Four actual JavaScript fixture wallets cover both ledgers, payment channels,
imported keys, a mnemonic passphrase and French words.

Five native and five Chromium worker tests, strict Clippy, warnings-denied docs,
all extracted packages and a 42,643-execution sanitizer fuzz run pass. The audit
also corrected a prior mapping: the published HD `xPub()` returns a private xprv;
Rust keeps its public-only API. See [migration documentation](docs/LEGACY_WALLET_MIGRATION.md)
and [retained evidence](test-infra/reports/legacy-wallet-migration-2026-09-13.json).
Ledger counts are 66 implemented, 1,366 deviation, 126 partial and 2,370 pending.
This closes the specialized HD wallet migration gap; the broader parity audit
and release qualification remain in progress.

## 2026-09-13: portable remote account signer and external custody

Added `RpcAccountSigner` for native/Wasm remote signing, verified messages and
exact transaction responses, finite remote unlock and independently observed send
acknowledgements. Requests are bounded, network/account checked and never retried;
dispatched failures retain ambiguity. Prepared native/browser account operations
can persist externally signed exact bytes against their live reservation.

Seven shared protocol tests pass natively and in Chromium workers, three published
JavaScript request tests pass, and custody regressions pass (21 native account
and 15 browser preflight tests). Strict native/Wasm Clippy and warnings-denied
rustdoc pass. [The guide](docs/RPC_SIGNER.md) records API and behavior differences.
Forty declaration rows are reconciled; counts are 66 implemented, 1,400 deviation,
126 partial and 2,336 pending. Broader parity and release qualification continue.

## 2026-09-13: typed response views and normalized node JSON

Added exact block transaction lookup, bounded block metadata and distinct outbound
ETX views. Transactions, receipts, logs and both block representations export
normalized node JSON with revalidated fields and retained top-level extensions.
Receipt fees use checked U256 arithmetic. ABI receipt views preserve decoded,
unknown and malformed logs, with optional contract-address binding.

Six native and six Chromium response tests, four native and four Chromium event
tests, four published-source regressions and 29 provider block/receipt/confirmation
regressions pass. Response parser/export fuzzing completed 1,093,559 executions
with AddressSanitizer active and no failure. See [the response guide](docs/RESPONSE_PARITY.md).
The review reconciles 272 declarations; counts are 66 implemented, 1,670 deviation,
126 partial and 2,066 pending. The broader behavioral audit remains unfinished.

## 2026-09-13: provider accounts, bounded local events and lifecycle review

Added passive ordered remote-account discovery, native WebSocket observed open
state and a portable `EventHub` with typed keys, one-shot listeners, atomic capacity
checks, explicit buffered/dropping pause, listener removal and terminal close.
Transport subscriptions and executor scheduling remain explicit application choices.

Five event tests and eight account/signer tests pass on native and Chromium workers;
13 native WebSocket regressions pass with one existing live test ignored. Four
published-source lifecycle tests, strict native/Wasm Clippy, warnings-denied docs
and all extracted package checks pass. See [the provider guide](docs/PROVIDER_PARITY.md).
702 declarations are reconciled; counts are 66 implemented, 2,372 deviation,
64 partial and 1,426 pending. ABI/reflection and remaining response/utility family
review continue; these counts are not a feature-completion percentage.


## ABI reflection and named results — 2026-09-13

Added `AbiParameter` metadata/formatting/traversal, `AbiResult` named decoded values
and exact function/constructor gas annotations. All four compiled declaration
kinds expose named parameter trees and individual formats; interfaces expose
immutable original declaration metadata. Async walks support non-Send callbacks
and enforce aggregate input/output budgets. Gas remains descriptive metadata.

Seven new ABI tests and the existing readable-ABI suite pass natively and as ten
shared Chromium-worker tests. The full native ABI suite has 45 passing tests.
Four published-source regressions document exact encodings and formatter/result
defects. ABI AddressSanitizer fuzzing completed 552,498 executions over 120 seconds
without failure; this bounded smoke run is not sustained security qualification.
Strict native/Wasm Clippy and warnings-denied documentation checks pass. Detailed
API differences and limits are in [ABI reflection parity](docs/ABI_REFLECTION_PARITY.md).

The 360 reviewed declarations close 338 pending and four partial rows. The ledger
now has 66 implemented, 2,714 deviation, 60 partial and 1,088 pending entries;
typed-data, transaction/contract response and remaining utility reviews continue.

The ABI batch also passes extracted-source package rehearsal for all twelve crate
archives, three consumer profiles, twelve native target compilations and two Wasm
target compilations. This does not publish packages or certify docs.rs execution.


## Typed-data utility completion — 2026-09-13

Added reusable `TypedValueEncoder`, arbitrary primitive/array/struct root encoding,
borrowed schema metadata and shape-checked `visit`/`visit_type`. Encoders match
107 published EIP-712 vectors. Visitors preserve named objects and array order,
reject missing/extra fields before callbacks, and bound aggregate callback output.
Normal domain/signature validation remains required after transformations.

All 49 native ABI tests and 14 shared worker tests pass. Three published-source
regressions cover encoding, visitation order and upstream permissive behavior.
Strict native/Wasm Clippy and warnings-denied docs pass. ABI AddressSanitizer
fuzzing completed 485,158 executions over 120 seconds without failure; this is
bounded smoke coverage, not an independent security audit. See
[typed-data parity](docs/TYPED_DATA_PARITY.md) for the complete API mapping.

The 36 reviewed declarations close 14 pending and 20 partial rows. The ledger now
has 66 implemented, 2,748 deviation, 40 partial and 1,074 pending entries.

The typed-data batch passes extracted-source rehearsal for all twelve crate
archives, three consumer profiles, twelve native target compilations and two Wasm
target compilations. No registry upload or docs.rs release certification is claimed.


## Crypto metadata and curve utilities — 2026-09-13

Added EIP-2098 parsing/serialization, `SignatureMetadata`, checked U256 legacy
chain/V helpers, guarded full SEC1 ECDH output and validated public-point addition.
The default 32-byte `SecretBytes` now supports fixed-size guarded output including
65-byte shared points. Private/public scalar binding, nonzero low-S signatures
and identity rejection remain explicit. Legacy metadata does not alter Quai
protobuf signing or domain policy.

All 23 native crypto tests and eight shared worker/key-origin tests pass. Four
published-source regressions cover exact bytes and permissive constructor defects.
Strict native/Wasm Clippy and warnings-denied docs pass. AddressSanitizer wallet
import/crypto fuzzing completed 48,121 executions over 120 seconds without failure;
this is bounded smoke coverage, not an independent security audit.

An existing ruint dependency gained a direct crypto edge; registry versions and
checksums did not change. Both lockfile audits using the retained advisory database
report zero vulnerabilities and the same two unmaintained optional-graph warnings
(derivative/paste), absent from the all-features normal active workspace tree.
See [crypto parity](docs/CRYPTO_PARITY.md) for APIs and explicit behavior differences.

The review closes 46 pending and two partial rows. Current counts are 66
implemented, 2,796 deviation, 38 partial and 1,028 pending declarations.

The crypto batch passes extracted-source rehearsal for all twelve crate archives,
three consumer profiles, twelve native target compilations and two Wasm target
compilations. No public-registry upload or release certification is claimed.

## Wordlist and mnemonic parity — September 13

`quai_wallet::wordlist` now provides checked public dictionaries, exact built-in
index order, explicit phrase conventions, bounded OWL/OWL-A import with eager
checksums, and custom-dictionary BIP39 phrase/entropy/seed conversion. Explicit
Chinese entropy export avoids the dependency's ambiguous-language panic.
The [wordlist review](docs/WORDLIST_PARITY.md) records limits and differences.
Five native and five Chromium worker tests pass, including 150 entropy roundtrips;
23 SQLite tests and 11 existing derivation reference tests also pass.

The eight-writer Windows test failed in CI run 34790097774 with a sanitized
SQLite database error while the other seven jobs passed. A bounded configurable
lock-wait API retains the five-second runtime default; the concurrency test now
allows 30 seconds while retaining eight simultaneous writers. A deterministic
held-lock test verifies timeout rollback and exact nonce allocation after an
explicit retry. Lock contention is the inferred cause of the prior CI failure;
local checks alone do not establish the Windows fix.

## Transaction interchange parity — September 13

`quai_consensus::document::TransactionDocument` adds bounded JSON/protobuf/byte
interchange across unsigned and verified signed account/Qi operations. It checks
supplied sender/hash identities, handles conversion/wrapping data exactly, retains
full u64/U256 quantities and exposes all-output zone metadata. Raw public protobuf
DTOs preserve field presence; bounded wire helpers remain distinct from semantic
and signature validation. Access-list arrays preserve order; explicit map input
sorts/deduplicates decoded bytes. See [interchange parity](docs/TRANSACTION_INTERCHANGE_PARITY.md).

The 19 native consensus tests and five Chromium worker interchange tests pass.
The latter cover 24 supported independent signing fixtures and reject two
unsupported Qi data-length fixtures. Four published-source tests identify raw
protobuf default insertion, ignored hash claims, unchecked Qi signature strings,
JSON data coercion, discarded locks and hex-case access-map deduplication.

Wordlist commit `63d6f9ca8342cb0618ed7885eb5dfa58c3027543` subsequently passed
all eight jobs in CI run 34791584063, including Windows. This verifies the
contention regression change on the supported Windows runner.

## Transaction response parity — September 13

Receipt-independent `observe_transaction_confirmation` and native/browser
`wait_for_transaction` now check indexed Qi/Quai/ETX inclusion against exact block
position, refreshed fields and rechecked head. Explicit deadlines, cancellation
and browser completed-poll limits retain custody on uncertain results.
`Transaction::verified_qi` reconstructs and verifies supported signed Qi operations
separately from those observations. See [response review](docs/TRANSACTION_RESPONSE_PARITY.md).

Six native and six Chromium worker tests pass; four portable tests also pass
without default SDK features. They cover fourteen valid Qi signing fixtures,
wrong block positions, changed fields/head, stalled-read deadlines, drop,
provider errors and browser poll exhaustion. Eleven existing provider block and
receipt confirmation regressions pass. Four published-source tests identify
Promise-refresh/unchecked-confirmation behavior and document response differences.

Transaction interchange commit `1e24a5f` passed all eight jobs in CI run
34792470499, including Windows, macOS, browser and package checks.

## Contract operations parity — September 13

Raw fallback/receive preparation, simulation and estimation now preserve exact
data/value/access lists through account-intent conversion. Bounded wildcard
queries retain unknown/malformed/removed logs, share event declarations and cap
aggregate decoded nodes/string bytes. Explicit native/browser code waits support
existing addresses without deployment hashes and enforce genesis/runtime checks,
deadlines, poll budgets and cancellation. Four contract-I/O and six contract-code
tests pass natively and in a real Chromium worker; one live code test stays
ignored offline. Five published-source tests document behavior and source defects.
Strict native/Wasm Clippy passes. The [contract review](docs/CONTRACT_PARITY.md)
reconciles 212 rows, closing 174 pending and all 16 partial rows; 505 declarations
remain pending. This is not a claim of complete parity or funded qualification.

Qi response commit `ca5c563` passed all eight jobs in CI run `34793413427`,
including Windows, macOS, Linux, browser, archive consumers, reference checks,
advisories and sanitizer smoke.

The contract batch also passed strict rustdoc and a 120-second ASAN smoke
(458,952 executions). All twelve extracted archives passed three consumer tests,
twelve native and two Wasm target compilations. This rehearsal did not upload or
execute the packaged test targets. Evidence: [contract report](test-infra/reports/contract-io-2026-09-13.json).

## General resource fetching — September 13

`quai_rpc::fetch` now supplies explicit request/response models, validated headers
and bodies, native gzip/streaming limits, browser resource Fetch, per-client hooks,
data/custom/IPFS gateways and bounded retry/redirect policy. Cancellation covers
hooks, network reads and delays. Source credential forwarding and Retry-After unit
handling are corrected; browser opaque redirects remain unsupported. Twelve
reference fixtures and five source tests cover the mapping. Eight native shared
tests, nine worker tests and three native HTTP server tests cover behavior.
The [fetch review](docs/FETCH_PARITY.md) closes 116 pending rows; 389 remain.

Fuzzing found one-ULP drift when a large JSON integer had already become a binary
float. The resource dependency now enables serde_json float round-trip parsing,
and the public failing input is a native/worker regression and corpus seed. This
does not make large JSON integers exact; chain quantities remain strings.

Contract commit `df6967e` passed all eight jobs in CI run `34795218246`.
The fetch batch passed 200 RPC/provider/ABI/consensus regression tests (four live
checks ignored), eight shared native and nine worker tests, and a fresh 120-second
ASAN run with 392,388 executions after the float regression fix. Strict rustdoc
and native/Wasm Clippy pass; the worker fixture environment expectation is
explicitly exempted from the compile-time-env lint so extracted targets compile
without a running fixture server. All 19 reference test files pass.

The final resource archive rehearsal passed all twelve packages, three consumer
tests, twelve native and two Wasm target compilations, with 102 public file mirrors.
The rehearsal did not upload packages or execute packaged test targets.
Evidence: [fetch report](test-infra/reports/fetch-2026-09-13.json),
[corrected fuzz regression](test-infra/reports/fetch-fuzz-regression-2026-09-13.json),
[final sanitizer run](test-infra/reports/fetch-fuzz-2026-09-13.json) and
[dependency snapshot audit](test-infra/reports/fetch-dependencies-2026-09-13.json).

## Metadata and standalone key utilities — September 13

Added bounded immutable network descriptors/registries and exact fee views, raw
public-key/chain-code derivation roots with optional checked BIP32 metadata, and
standalone PBKDF2/scrypt with guarded output and total output-block work limits.
Six native and six real-worker tests pass against four network, four fee, eight
public-child and sixteen KDF cases. Five utility and three subscriber source tests
document semantics, including xPub private export and BaseWallet override defects.
KDF progress has explicit Started/Completed checkpoints rather than fine-grained
loop callbacks. The [utility review](docs/UTILITY_PARITY.md) reconciles 142 rows;
247 declarations remain pending.

Fetch commit `bfdd25f` passed all eight jobs in CI run `34796838633`. The utility
batch also passed 21 signer/HD/event regression tests, strict native/Wasm Clippy
and rustdoc, plus 120-second ASAN runs with 17,266 wallet-import executions and
335,556 encoding executions. No dependency lockfiles changed. All twelve package
archives passed three consumer tests, twelve native and two Wasm target
compilations with 103 public file mirrors. Packaged test targets were compiled,
not executed; no packages were uploaded. Evidence: [utility report](test-infra/reports/utilities-2026-09-13.json).

## Curve adapters and threshold aggregation — September 13

Added guarded canonical scalar math, validated point multiplication/lifting,
public field helpers and bounded multipart/tagged hashes. Both the reference's
non-ASCII tag encoding and UTF-8 are explicit. Threshold aggregation now selects
small coins and fee inputs separately, conserves value exactly, respects wallet
spendability and flows through sweep/quote fee convergence. The six-Qit source
fee-shortfall case is corrected. See the [review](docs/CURVE_AND_AGGREGATION_PARITY.md).
The ledger closes twelve rows; 235 remain pending.

Utility commit `2305842` passed all eight jobs in CI run `34798647241`. This batch
passed six native/worker curve tests, five native/worker aggregation tests, seven
native and thirteen worker Qi preflight tests, and 102 crypto/wallet regression
tests. All 23 source test files and strict native/Wasm Clippy and rustdoc pass.
The 120-second ASAN campaign completed 12,742 executions without findings.
All twelve archives passed three consumer tests, twelve native and two Wasm
target compilations with 105 public file mirrors. No dependency lockfiles changed;
no package upload or funded network write occurred. Evidence: [batch report](test-infra/reports/curve-aggregation-2026-09-13.json).

## Final declaration reconciliation — September 13

Added bounded numeric interchange, safe-number bridges, hex predicates, exact
signed constants, complete shard metadata and explicit recovered-signer helpers.
Eight native/Chromium utility tests and seven source semantics tests qualify the
new helpers. The [declaration review](docs/DECLARATION_PARITY.md) reconciles the
remaining 235 rows: 74 implemented, 3854 explicit deviations, no pending/partial.
This closes inventory review; the documented platform and qualification limits
still require separate interpretation rather than a blanket parity percentage.

Curve/aggregation commit `e7422f0` passed all eight CI jobs in run `34799724756`.
This batch passed 24 primitive and 77 crypto/ABI/signer regression tests, strict
native/Wasm Clippy and rustdoc, all 24 source test files, and a 120-second ASAN
run with 248,101 executions. Twelve archives passed three consumer tests, twelve
native and two Wasm target compilations with 106 public mirrors. All lockfiles
are unchanged. Packaged test targets were compiled, not executed; no upload or
funded network transaction occurred. Evidence: [utility/declaration report](test-infra/reports/final-utilities-2026-09-13.json).

The subsequent deviation audit identified explicit wrapping fixed-point arithmetic
and lossy floating-point conversion as useful remaining functionality, despite
their earlier intentional-omission mapping. These are being implemented before
the final release review; zero unreviewed declarations is not itself completion.

## Fixed-point deviation closure — September 13

The five omitted reference wrapping/float operations now have explicit
`FixedPoint::wrapping_*` and `to_f64_lossy` APIs. Five native/Chromium tests cover
632 source operations, 26 signed-minimum corrections, 131,072 eight-bit operand
pairs and explicit lossy conversion. Published placeholder/brand/tuple-name rows
have also been checked; see the [fixed-point review](docs/FIXED_POINT_PARITY.md).

## Alpha candidate closeout — September 13, 2026

The final omission audit added explicit wrapping fixed-point arithmetic and lossy
float conversion, then verified 632 differential operations and all 131072 signed/
unsigned eight-bit operand pairs. Five native and five worker tests, four existing
fixed-point regressions and three source tests passed. A 120-second ASAN run
completed 189960 executions without a reported failure. A new tuple metadata
assertion was corrected to the published source's `null` value before the full
25-file JavaScript suite passed. Fixture regeneration is unchanged.

The final local workspace checks passed: 617 all-feature tests/doctests and 300
no-default-feature tests/doctests (overlapping feature runs), strict native/Wasm
Clippy, warnings-denied rustdoc, 18 harness and three isolated-harness unit tests.
Read-only mainnet checks passed two provider tests and one wrapper-code test.
All twelve archives passed three consumer configurations, twelve native target
compilations and two Wasm target compilations; no upload occurred.

The [alpha manifest](test-infra/reports/alpha-release-2026-09-13.json) binds source,
archives and verification. The [dependency inventory](test-infra/reports/alpha-dependencies-2026-09-13.json)
records the locked resolution, including target/dev dependencies. Crates.io-only
metadata and release instructions are prepared. Actual publication, hosted docs
and the production qualification limits in the current comparison remain separate.

The preceding commit's first CI attempt had seven successful jobs and a Chromium
startup crash before IndexedDB fixture progress. The same fixture passed locally;
the failed browser job was rerun without changing assertions. Final committed
source must pass its own complete eight-job CI matrix before handoff.

## Funded mainnet and Pelagus qualification — September 14, 2026

Funded **mainnet** qualification began on 2026-09-14 through `https://rpc.quai.network`
([record](test-infra/orchard/mainnet-2026-09-14.json), [checks](test-infra/orchard/mainnet-checks-2026-09-14.json),
[Pelagus](test-infra/orchard/mainnet-pelagus-2026-09-14.json)). It covers QUAI transfers,
WQUAI deposit/withdraw and approve/transferFrom/transfer, a fee-only replacement,
reconciliation after a withheld acknowledgement, discovery of an unregistered nonce
cancellation, a ground deployment with code wait, seed-only recovery of locked
conversion Qi, automatic specialized fee quotes (36 Qits for a 1 Qi conversion, 33
for a wrap), authenticated backup restore and six Quai-to-Qi conversions. Batch-wide
conversion discounts from concurrent third-party flow refunded three conversions;
three credited 3,416 Qits. Pelagus interoperability passed in both directions: the
SDK discovered a Pelagus sender through its mailbox contract, recovered 15 Qi, sent a
Pelagus-compatible `notify`, and returned 1 Qi from the received output. Qi spending
of conversion outputs, WQI and Qi-to-Quai on mainnet await lock expiry or activation.

New APIs from this work: `conversion_batch_discount_bps` (pinned batch discount),
`payment_mailbox::PaymentMailbox` and `payment_channels::discover_mailbox_channels`
(Pelagus-compatible announcements), and `AccountSession::observe_nonce` (registered
and unregistered nonce outcomes). A detached WebSocket soak recorded real depth-1
reorgs handled by canonical replay. Open items: WQI after the unwrap-lock activation,
conversion-output spending after lock expiry, Qi-to-Quai on mainnet, header-level
rather than funded-transaction reorgs, and verification that the quais package's
compiled `lib/` (reporting 1.0.0-alpha.52) matches its alpha.57 source.
