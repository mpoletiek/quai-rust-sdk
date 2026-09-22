# Internal security review and validation — 2026-09-11

**Four medium findings were fixed, password handling was hardened, and the final
bounded checks passed. The project remains development software and is not yet
qualified for real-fund custody or public release.** No external audit has occurred.
Severity labels below describe this internal review; they are not independently
assigned CVSS scores. “No finding” means none established within this scope, not
proof that vulnerabilities are absent.

The final all-features suite passed **254 tests plus three subprocess contention
checks**, with four explicitly gated live tests omitted from that automated run.
The earlier progress count of 257 included those three subprocess executions.
The no-default-features suite passed 169 tests. Sanitizer smoke runs completed
875,310 executions without findings. These checks complement the code review and
retain the material limitations described below.


Reviewed 2026-09-11, completed against the shared working tree. This is an internal
implementation review and regression exercise, not an independent external audit
or a production-security certification. Scope: `quai-keystore`, SDK `accounts.rs`
(including reserved-nonce deployment) and `qi.rs`, plus directly relevant storage,
consensus, and installed cryptographic source.

## Verified findings and fixes

### Medium — ordinary preparation estimated a different nonce from the final transaction

Trigger: prepare two account operations while the node keeps reporting pending nonce
4. The persisted local cursor reserves 5 for the second operation, but its original
simulation still used nonce 4. The added test failed before the change, with the last
estimate request containing `0x4` while the prepared/signed transaction contained 5.
This violated the reviewed-payload simulation contract; it could invalidate estimation
assumptions and leave an unusable or misestimated operation. No key theft was demonstrated.

Fix: retain the initial fee/balance preflight before allocation. If `reserve_nonce`
returns a different nonce, re-estimate and recheck gas, total fee and balance with that
exact durable nonce. Any later rejection or cancellation leaves the unsigned reservation
held; signing still freezes the exact result. Explicit deployment preparation already
uses its reserved nonce and is covered by its existing regression.

Regressions: `ordinary_repeated_prepare_estimates_the_actual_reserved_nonce`,
`actual_nonce_reestimate_failure_retains_an_unsigned_reservation`, and the original
preflight-fee-failure and deployment tests.

### Medium — live workflow custody could cross distinct but publicly identical stores

Trigger: initialize two databases from the same public toy seed, allocate the same
change ranges, and give both equal cursor/metadata state. The previous checks accepted
a one-use change pool from database A in database B. Equivalent reservation IDs and
nonce/outpoint sets also allowed prepared account/Qi objects to cross stores. A baseline
regression demonstrated the pool was accepted. This bypassed the API's store-custody
boundary and could cause accidental operation confusion or address reuse across copies;
it is not a solution to intentional independent operation of copied wallets.

Fix: an opaque, privately constructed `StoreInstance` identifies each open storage
handle using a checked process-local counter that never wraps. Change pools and prepared
objects capture it; prepare/sign require the exact handle. Matching public state is
insufficient. Opening the same file again intentionally yields another instance, so
unsigned live objects cannot migrate to the reopened handle. This adds no persisted
schema field. Signed bytes still survive restart and support explicit `broadcast(id)`.

Regressions: identical-state pool rejection, account and Qi prepared-object rejection
with identical reservations, reopened-handle rejection, and existing durable restart
rebroadcast/ambiguous-submission tests.

### Medium, configuration-dependent — scrypt memory ceiling assumed sequential execution

The inspected `scrypt` 0.12.0 source allocates B plus V/T per parallel lane when its
optional `parallel` Cargo feature is enabled. A downstream dependency can enable that
additive feature despite this crate disabling defaults. The old accounting
`128*r*(N+p+1)` bounded only sequential execution. For example N=16,r=1,p=4 uses a
2688-byte sequential estimate but may need 9216 bytes with four active workspaces.
Larger allowed parameters could bypass the application's stated working-memory policy
and cause resource exhaustion. This workspace currently does not enable parallel scrypt.

Fix: budget the conservative `128*r*p*(N+2)` in both parsing and decryption. The fixed
new-export p=1 cost remains unchanged. Explicit p>1 import policies become stricter.
Regression verifies rejection below the parallel-capable bound and acceptance at it.

### Low hardening — normalized password output previously grew before becoming guarded

NFKC output was collected into an ordinary growing String and then wrapped in Zeroizing;
reallocation could leave earlier heap copies. The replacement writes into a single
4096-byte guarded allocation from the start and rejects expansion before overflow,
using a guarded UTF-8 scratch buffer. Regression checks U+FDFA expansion at 4092 bytes
and rejection past 4096. This does not prove erasure of Unicode iterator/compiler state.

## Review evidence beyond the fixes

Hostile keystore JSON is bounded to 64 KiB and 16 levels. Case-insensitive and escaped
aliases cannot overwrite another field; noninteger numeric tokens, trailing content,
unknown extensions, invalid cipher lengths and unbounded KDF settings fail. Additional
regressions exercise these parser boundaries. Private ciphertext MAC is checked in
constant time before decryption; a required derived address checks un-MACed IV changes.
Optional x-quais mnemonic data remains unauthenticated legacy metadata, but import
re-derives its exact key using its language/path and empty BIP39 passphrase. Changed
mnemonic entropy, IV, path or language fails ownership verification. Arbitrary original
seed and nonempty BIP39-passphrase recovery must use the native format.

Account and Qi signing verify exact prepared payloads/keys and commit canonical signed
bytes before returning them. Broadcast reloads validated bytes, checks network identity,
and records Submitted before transport submission. Existing timeout, malformed response,
cancellation and restart tests retain signed claims. Qi selection rechecks generation,
checkpoint canonicality and lock/expiry eligibility before claiming inputs. Fee amounts
remain contingent on trusted source-reported input denominations; no cryptographic
on-chain fee guarantee is inferred from unproven observations.

## Residual limitations and recommendation

RustCrypto scrypt 0.12.0, release VCS commit
`a795bf6f2dea6b9700fcc78f82a2ceaa19f36da7`, internally uses ordinary B/V/T vectors and
provides no caller-owned workspace hook. These buffers are not wiped after use. This
was already documented and remains unresolved; wrapper guards cannot erase them.
A narrowly scoped upstream workspace API or separately reviewed vendor patch replacing
all sequential/parallel B/V/T allocations with Zeroizing buffers is a reasonable next
step. Such a patch should preserve independent vectors and cover both feature paths;
it would still not certify SHA/Salsa/compiler temporary erasure. No vendor patch or
replacement cryptographic backend was introduced in this review.

Per-call KDF/selection limits do not cap total concurrent application work. Applications
must bound workers and keep synchronous KDF work off async/UI threads. StoreInstance
does not authenticate external databases or prevent rollback across devices or copied
seeds. Native chain-proof reconciliation, full reorg recovery and unproven-source fee
risks remain separately tracked.

## Validation

Using `CARGO_HOME=/tmp/quai-rust-cargo-home` and `--offline`:

- SDK `--features sqlite,abi --test accounts --test qi`: 8 account + 13 Qi tests passed.
- `quai-keystore`: 2 unit + 6 integration tests passed, including 15 pinned JS vectors
  and fresh production-cost export/import.
- `quai-wallet --features sqlite storage:: --lib`: 19 tests passed, including reopen,
  concurrent writers/processes and durable claim checks.
- Strict Clippy passed for keystore + wallet sqlite/all-targets, and for the final
  combined SDK sqlite+abi + keystore all-targets run after the provider owner fixed
  its in-progress warnings. No warnings were suppressed in reviewed code.

Primary source examined: installed crate source and release VCS metadata for scrypt
0.12.0; public upstream is https://github.com/RustCrypto/password-hashes/tree/master/scrypt.


## Follow-up read-only conversion-tracker review

Reviewed `quai-provider/src/conversion_tracking.rs` and relevant typed receipt/block
parsers against the captured isolated-chain fixtures and pinned Go state-processor
source. No provider implementation was edited by this reviewer.

**Medium data-association correctness, fixed by the provider owner:** the canonical
origin receipt failure branch returned `Failed` before comparing receipt sender and
destination with signed conversion intent. Thus status0 skipped an association check
that status1 enforced. The owner moved that existing check before failure handling and
added mismatched-from/to rejection tests plus a correctly associated failure case.
No release or balance mutation is performed by this observer, and no theft was shown.

No additional high/critical finding was established in this bounded review. Checked
boundaries include stable origin-hash/index identity; unchanged emitted sender,
destination and data; initial amount and subtype5 refund amount; final receipt hash,
kind, inclusion and subtype; duplicate execution rejection; linked scan blocks,
ending-anchor and origin rechecks; explicit incomplete-history coverage; refund
beneficiary derived from signed intent instead of the unchanged ETX `to`; and
spendability remaining unverified despite receipt status1. Pinned Go conversion-refund
processing confirms that recipient and refund beneficiary need not be the same field.

Residual limits remain material: node-reported hashes/parent links are not independently
verified chain proofs; a scan covers only its explicit requested page; receipts and
latest aggregate locked balance do not prove maturity or attributable spendability;
future subtype/profile rules remain unknown; this is not a durable reconciliation or
reservation-release engine. Correlation observations must not automatically unlock
signed wallet claims. No new feature work was started.

Final conversion regression command `cargo test -p quai-provider --offline --test
conversion_tracking` passed 10 tests. The explicitly gated disposable-node read test
was ignored in this run; its independent execution belongs to the live acceptance
agent's evidence.

## Final integrated validation

[Machine-readable evidence and retained logs](../../test-infra/reports/security-2026-09-11/summary.json)
bind the Rust source/manifests and final successful runs. Native tools were Rust/Cargo
1.97.1 on Linux. These are overlapping configurations, not additive unique coverage.

| Check | Final result and scope |
| --- | --- |
| Native all features | 254 tests, three extra subprocess contention checks; four gated live tests ignored |
| Native no default features | 169 tests; no failures |
| Strict Clippy / rustdoc / rustfmt | Workspace all targets/all features where applicable; no warnings or formatting failures |
| Wasm SDK composition | Browser, wallet, ABI, payments and keystore compile together |
| Actual Chromium dedicated worker | Five tests passed, including keystore mnemonic ownership after the security fixes |
| Harness regressions | 14 readiness/transport tests and three disposable-chain harness tests passed |
| Sanitizer fuzz smoke | Three targets × 30 seconds; 875,310 executions, all exit 0; canonical transaction, ABI/typed-data and wallet-import boundaries |
| SDK dependency audit | 373 lock entries; zero known vulnerability matches, two unmaintained optional-package warnings |
| Separate fuzz dependency audit | 259 development lock entries; zero known vulnerability matches, same two warnings |

The [fuzz evidence](../../fuzz/smoke-results.json) records binary/lock hashes and counters.
The initial sandboxed sanitizer run could not finish LeakSanitizer process inspection;
it was not counted as passing. The successful rerun permitted local process inspection
without disabling sanitizers. Initial Python mock-server execution likewise required
loopback permission, then all tests passed. No failing parser assertion was suppressed.
Short fuzz runs do not establish exhaustive coverage, upper-bound performance, leak
freedom or correctness of expensive KDF paths; those paths use separate regression tests.

The [dependency report](../dependency-audit.md) records the advisory snapshot and exact
lock hashes. `derivative` and `paste` are inactive in the checked native all-features
graph, but remain in both locks. A strict warning-free audit still fails. Consumer
feature unification can change which dependencies compile. CI now includes platform,
browser, fuzz and advisory jobs, but remote CI/macOS/Windows runs were not executed here.

## Live wallet evidence and unresolved qualification

The [high-level Qi acceptance run](../../test-infra/local-chain/HIGHLEVEL.md) verified
fee convergence, exact-byte persistence, process restart, explicit submission,
canonical inclusion, output accounting and retained claims. Its 10,000-Qit input
produced 1,234 to the recipient, 8,710 change and a 56-Qit fee. The final quote was
48 Qits; the conservative monotonic loop retained eight extra Qits. This is concrete
behavior, not a globally optimal fee claim.

The account workflow encountered a **release-blocking node compatibility failure**:
the pinned disposable node crashed on pending-state gas estimation and balance reads.
Preparation stopped before any nonce reservation, signature or account submission.
High-level account/deployment live qualification therefore remains open. Earlier
low-level deployment acceptance and deterministic account regressions do not close it.

All writes used isolated loopback nodes with explicit documented development patches
and public fixture keys. The LAN mainnet node at `10.0.0.12` was used read-only; no
mainnet transaction was submitted. All owned disposable nodes were gracefully stopped,
with snapshots/evidence preserved. Patched-node acceptance does not qualify unmodified
mainnet or funded Orchard writes, full historical recovery, or reorganization behavior.

## Priorities before real-fund use or publishing

1. Resolve upstream scrypt workspace wiping through a separately reviewed backend
   change or upstream fix; retain all independent vectors and feature-path checks.
2. Qualify an explicit account pending-state policy against supported unmodified
   nodes, and complete funded disposable testnet account/Qi/deployment acceptance.
3. Complete durable reconciliation, reorg and rollback/recovery testing, production
   discovery trust requirements, nonce-gap repair and ambiguous replacement handling.
   Never release signed claims based solely on absence or receipt success.
4. Expand sustained/stateful fuzzing, interruption-during-write tests, malformed
   remote-source tests, concurrency/resource limits, platform and actual wallet-extension
   interoperability. Measure time/memory/fees before making efficiency guarantees.
5. Obtain specialist external review of cryptography, transaction rules and custody;
   resolve dependency maintenance/license/SBOM and package-contained test gates;
   designate a security owner and private reporting channel before release.

All crates were `publish = false` when this review was written. That is no longer
true: `Cargo.toml` has carried `publish = ["crates-io"]` since the first alpha
upload, and releases through `0.1.0-alpha.8` are on crates.io. No security
certification was performed then or since. Full SDK feature parity also remains unfinished; see the
[implementation status](../../IMPLEMENTATION_STATUS.md) and [wallet gaps](../WALLET_GAPS.md).
