# Implementation status

Updated 2026-09-12. The [audited plan](QUAI_RUST_SDK_PLAN.md) remains the full
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
| Encrypted backup | Authenticated seed and full native wallet envelopes, bounded Argon2id/XChaCha20-Poly1305, guarded key origins, monotonic reservation restore, signed-claim retention and checkpoint invalidation; independent format vectors. QUAIWALT v2 includes durable channels/exposures and preserves v1 compatibility |
| Selection | Full input-denomination capacity across recipient/change, fee convergence and eligibility checks; explicit all-eligible sweep/aggregation policies. Existing 69 JS vectors plus capacity/aggregation regressions pass |
| Signers | Local chain-bound and watch-only adapters, offline Quai/single-input Qi signing and personal-message signing; consensus supports ordered local multi-input Qi signing |
| HTTP/provider | Exact direct URLs and gateway routes, strict envelopes and U256 quantities, bounded native transport, typed headers/account calls/transactions/ETXs/receipts/outpoints, account broadcast ambiguity and canonicality-checked receipt polling |
| WebSocket | Native bounded session and subscription implementation; deterministic loopback tests and a real LAN mainnet head notification passed; reconnect/backfill remains separate work |
| Durable state | SQLite claims/cursors, specialized signed-byte custody, canonical inclusion reconciliation and conservative reorg invalidation, unsigned nonce recovery and authenticated backup |
| ABI/typed data | Bounded canonical ABI/interface/EIP-712 implementation, typed contract calls/ERC-20 helpers, bounded event queries retaining reorg metadata, same-zone CREATE grinding with required access list |
| Account workflow | Durable prepare/sign/submit, exact conversion simulation, nonce claims, unsigned restart preparation and explicit nonce-gap repair; signed replacements remain open |
| Legacy keystores | Bounded v3 AES-CTR/scrypt/PBKDF2 import/export, NFKC/byte passwords, all-language mnemonic derivation checks; eight tests cover 15 JS vectors, hostile inputs and fresh production-cost exports. Upstream scrypt workspace wiping remains a security-review limitation |
| Qi workflow | Current gap-50 discovery, mixed HD/imported/BIP47 signing, ordinary/cross-zone preparation, sweep/explicit aggregation, durable signed transfers/conversions/wrapping and restart broadcast |
| Payment codes | BIP47 seed/master/account-xprv derivation, registered send destinations and receive gap/deep scanning, verified receive key resolution, monotonic exposure imports and authenticated seed/master channel backup |
| Discovery | Both the history-capable abstract scanner and supplied current-state Qi scan/refresh; gap 50, explicit deep ranges, all stored origins, fixed denominations/locks and balance buckets; latest-only consistency limits remain explicit |
| Conversions | Typed rates/calculation, durable Quai-to-Qi and Qi-to-Quai preparation, explicit specialized Qit fees, signed backup/recovery and existing bounded ETX correlation; automatic specialized fee and maturity qualification remain open |
| Browser | Real Chromium Fetch/injected-provider tests, recovered personal/typed-data signatures, worker Fetch/HD Qi derivation/OS entropy/signing; browser persistence and real extension interoperability remain open |
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
Clippy and rustdoc checks passed. The local wasm rerun is
blocked: the target is absent and this environment has no `rustup`; existing CI
still installs the target and checks the browser feature combination.

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
Remaining work includes production Qi discovery, imported/BIP47 Qi spending,
additional payment key-origin support, indexed history/reorg recovery,
conversion quoting/fees and durable settlement reconciliation/claim policy,
Qi wrapping/redemption, wrapped Quai contract workflows, consolidation/cross-zone
orchestration, full wallet lifecycle, high-level account/deployment live qualification, remaining provider methods, browser
persistence and injected-wallet lifecycle/interoperability.

Also outstanding: unmodified/funded Orchard acceptance, full fault/reorg/soak
suites, sustained fuzzing and performance baselines, external specialist security review,
license/SBOM review and advisory warning resolution, macOS/Windows/extension coverage and package-contained
tests. See [local-chain evidence](test-infra/local-chain/README.md) for the exact
patched acceptance boundary. The advisory report records its own lock hash and
is historical whenever the workspace lock changes. No stable API or efficiency
target is yet certified.

## Security review closeout

The [internal review](docs/SECURITY_REVIEW_2026-09-11.md) fixed four medium findings
and hardened password normalization. All three bounded sanitizer fuzz targets passed
875,310 executions. Final advisory scans found no known vulnerabilities in the SDK
and separate fuzz lockfiles; two inactive optional unmaintained packages remain.

The [high-level live wallet evidence](test-infra/local-chain/HIGHLEVEL.md) verifies
Qi preparation, fee convergence, signed-byte persistence, process restart and exact
inclusion/output accounting. Pending-state account RPCs crash on the pinned
disposable node: preparation correctly stopped before reservations or signatures.
This blocks high-level account/deployment live qualification. All owned disposable
nodes were stopped after testing; mainnet remained read-only. Upstream scrypt
workspace wiping and the other documented release gates remain unresolved.
