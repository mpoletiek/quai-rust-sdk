# Implementation status

Updated 2026-09-13. The [audited plan](QUAI_RUST_SDK_PLAN.md) remains the full
scope. **The SDK is under construction and has not passed the feature-complete,
production-security or release gates.** All packages remain unpublished.
The [feature completeness review](docs/FEATURE_COMPLETENESS_REVIEW_2026-09-12.md)
separates implemented primitives from missing workflows and records completion
criteria FC01–FC12. Historical test/node results below retain their recorded scope.

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
| Browser | Real Chromium Fetch/injected-provider tests, recovered personal/typed-data signatures, worker Fetch/HD Qi derivation/OS entropy/signing; scoped atomic IndexedDB snapshots; full browser wallet integration and real extension interoperability remain open |
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
and the [updated review](docs/FEATURE_COMPLETENESS_REVIEW_2026-09-12.md) for FC01–FC12
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
qualification. No mainnet transaction has been submitted. The isolated funded harness is available with documented development patches;
both conversion directions, refund and maturity/spend acceptance now also pass
on their separate controller/lock-adjusted profile. Unmodified mature-node writes
and disposable funded Orchard acceptance remain
required. The documented Orchard faucet hostname currently fails DNS here.

## Open gates

See [wallet gaps](docs/WALLET_GAPS.md) for the complete capability matrix.
Current implementation gaps are complete application of canonical replay to
wallet state, terminal signed-claim policy, per-operation Quai conversion maturity,
remaining provider/browser workflows and declaration/overload reconciliation.
Current gap-50 discovery, optional use hints, imported/BIP47 spending, payment
key origins, conversions/fees/settlement observations, wrappers, candidate
families, deployment observation and scoped IndexedDB snapshots are implemented.
Latest-only outpoint RPC still cannot prove spent history or an atomic historical
snapshot; default discovery does not require an indexer.

Qualification still requires unmodified/funded Orchard acceptance, broader
fault/reorg/soak and performance coverage, real injected-extension interoperability,
external specialist security review, and release/SBOM/advisory closeout. Sustained
bounded fuzz runs, macOS/Windows CI, and extracted-package consumer/test-target
checks have passed within their retained evidence boundaries. Browser snapshots
are an opaque atomic storage substrate; complete browser wallet reservations and
restore integration remain unfinished. No crate has been published.
See [local-chain evidence](test-infra/local-chain/README.md) for the patched
acceptance boundary and retained reports for exact source identities. No stable
API or efficiency target is yet certified.

## Historical internal security review closeout

The [internal review](docs/SECURITY_REVIEW_2026-09-11.md) fixed four medium findings
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
