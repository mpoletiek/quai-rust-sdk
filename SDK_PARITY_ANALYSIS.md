# Rust SDK versus quais.js: current parity analysis

The Rust SDK implements the reviewed application capabilities of the pinned
**quais@1.0.0-alpha.57** package across native and browser targets. The declaration
review has no pending or partial rows. This is a feature-equivalence assessment
with explicit differences, **not identical JavaScript API/behavior compatibility
or production qualification**. No known working reference operation is left as an
unexplained omission in the reviewed inventory.

The SDK is an experimental `0.1.0-alpha.1` release candidate. It includes Qi
current-state discovery, fixed denominations, both conversion directions,
WQI/WQUAI workflows, HD/payment-code derivation, contract operations, native and
browser transaction custody and recovery. Read the
[complete SDK guide](SDK_DOCUMENTATION.md) for integration instructions and the
[release procedure](docs/PUBLISHING.md) for package preparation and upload limits.
The [historical analysis](SDK_PARITY_HISTORY.md) preserves the earlier audits;
its old backlog counts are not the current implementation status.

## Reference and audit method

The published artifact is pinned by [package lock](compatibility/package-lock.json),
[reference lock](compatibility/reference-lock.json) and hashes of 153 published
source files in the [source manifest](compatibility/reference-source-manifest.json).
The npm recorded gitHead is `3bd0bf5b077f4aa5fab480474e3982e50e1af506`; the separately
inspected checkout is `94e32c7eb9960de36054135c40a341c44c84f922`. They are not interchangeable.
The [registry recheck](test-infra/reports/reference-version-recheck-2026-09-13.json)
is a dated observation, not a claim about future releases. Selected wire/rule
checks also use go-quai v0.56.0 at `f3f345c877300c044e3e0081a48bf3cf786fb9cc`.

The [ledger](compatibility/parity.json) covers 12 export roots/subpaths, 581 exports
and 3347 member entries. Re-exports, inherited properties and overloads inflate
these **3928 tracking rows**; even the 2023 distinct symbol/behavior/signature
keys are not independent features. Each row records its Rust API, evidence and
specific deviation. A follow-up review of omission rationales found missing
wrapping fixed-point operations; those were implemented and tested before closing
this assessment. Merely changing a ledger status is insufficient evidence.

| Ledger status | Rows | Meaning |
| --- | ---: | --- |
| `implemented` | 74 | Direct mapping within the documented qualification scope |
| `deviation` | 3854 | Tested Rust replacement, explicit bound/runtime difference, source correction or justified omission |
| `partial` | 0 | Unfinished mapped behavior |
| `pending` | 0 | Unreviewed declaration |

These counts do not support a percentage-complete calculation. In particular,
3854 deviations do not mean 3854 missing features. Most inherited JavaScript class
members map to shared typed Rust operations. Conversely, a deviation is not a claim
of exact behavior: the restrictions below matter when porting an application.

## Feature comparison


| Area | Rust implementation / reference relationship | Remaining difference or qualification |
| --- | --- | --- |
| Addresses and routing | Typed ledger/zone addresses, checksum parsing, shard routing and direct/gateway selection with pinned vectors | Explicit static routes and trusted identities replace mutable JS discovery/configuration; known zone does not imply activation |
| Units and numeric utilities | Exact Quai/Qi units, signed formatting, fixed point, hex/Base58/Base64, packed hashes and Unicode | Explicit checked/wrapping arithmetic and lossy float conversion; strict UTF-8/Base64 and corrected rounding/range behavior |
| BIP39/BIP32/BIP44 | All ten wordlists, 16–64-byte seeds, xprv/xpub metadata, absolute/relative paths, both coin types, bounded zone search | Rare invalid children fail at their actual index; explicit master/subtree APIs and bounded cancellation replace convenience loops |
| Imported and watch-only keys | Validated ownership metadata and mixed-origin key resolver; watch-only types cannot sign | Private imports are guarded rather than serialized through general public objects |
| Qi discovery | Receive/change default gap 50 matching Qi addresses, deep ranges, every persisted origin refresh, lock/balance classes | Latest-only outpoints cannot prove historical address use; optional external use hints and explicit recovery ranges are needed for stronger recovery |
| Qi selection/signing | Fixed denomination inventory, exact output/address capacity, single/multi-input signing, sweep and explicit aggregation | No fee-shortfall success; funded Orchard 46-input mixed-key aggregation and first-Qi block placement verified, plus 15-input sweep |
| Quai transactions | Native durable prepare/review/sign/broadcast, nonce reservation, access lists, deployment and cross-zone intents | Native SQLite sessions and the browser account session use explicit pending/latest policy; browser replacement preparation, candidate selection and reconciliation exist |
| Qi transactions | Native and browser durable selection/fee convergence/preparation/signing/submission; exact special-operation custody and candidate reconciliation | Browser signed-intent destination observations exist; only native resume persists destination cursors; observations remain source claims |
| Payment codes | Version-one BIP47 seed/master/account origins, matching send/receive keys, registered channels, gap/deep scans and mixed-origin spending | BIP47 notification transactions/blinding are absent; optional Pelagus-compatible mailbox discovery and `notify` intents recovered a real Pelagus mainnet payment; bounded cursors deliberately avoid reuse |
| Quai → Qi | Native and browser conversion preparation, simulation/fees/slippage and signed custody; native/browser destination observations | Funded Orchard creation, unlocking and converted-output spending passed; quote/fork behavior and broader production execution require qualification |
| Qi → Quai | Exact 22-byte conversion form, refund/slippage, explicit or profiled fees and signed recovery | Funded Orchard conversion, exact maturity credit and subsequent spend passed; isolated strict-slippage refund and refund-output spend passed; automatic Orchard specialized estimation remains open |
| Qi → WQI / WQI → Qi | Native 20-byte wrap, WQI backing/claim, ERC-20 operations, redemption gas/dust planning, lock observations | Funded Orchard wrap/claim/redeem/mature-spend passed with explicit wrapping fee; automatic Orchard specialized fee estimation and audited contract/profile qualification remain open |
| Quai ↔ WQUAI | Configured deployment, deposit/withdraw and ERC-20 intents | Distinct mainnet/Orchard deployments; funded Orchard deposit/withdraw and exact balance round trip passed; mainnet funded execution and contract audit remain open |
| ABI/interfaces | Canonical encoder/decoder, JSON/readable declarations, defaults, packed encoding, EIP-712, event filters and call/log/revert parsing | Positional values and explicit overloads replace JS Proxy/Result/Typed ergonomics; strict bounds and validation differ |
| Contracts/deployments | Explicit contract calls/intents, ERC-20 helpers, artifacts, constructor data, address grinding, canonical code observations | No dynamic JS method/property generation; input artifacts must have resolved creation bytecode; execution qualification remains separate |
| RPC and providers | Typed reads, block/pool/log/wallet APIs, exact signed broadcast, bounded receipts/head/ETX tracking | Unregistered account nonce competitors now have bounded signed discovery and native/browser waits; no fabricated `safe`/`finalized` or historical return-data capability |
| Native WebSocket | Bounded subscriptions, lag/disconnect reporting, reconnect and canonical replay with persistence fences | Applications explicitly apply canonical observations; disappearance alone never releases signed claims |
| Browser transport/signing | Actual Wasm Fetch/WS, workers, injected message/typed/transaction verification, explicit raw submission capability | No automatic wallet selection, chain switching or permission manager; real extensions/other engines need qualification |
| Browser persistence | Durable allocation, account/Qi custody, consistent backup capture and atomic multi-journal live restore | Explicit enumeration; initialized targets required by the coordinator; account call/conversion/deployment fee/prepare/sign/root-submit orchestration now exists; account/Qi candidate reconciliation exists; Qi preparation now includes current discovery and persisted owners; destination observations now exist; native persisted destination cursors and general history compaction differ |
| Backup/restore | Authenticated private origins, public ownership proofs, channels, burned floors, exact candidate families, monotonic merge | Portable backups retain floors/exposures rather than missing allocation request history; browser/native storage APIs differ |
| Platform and release | Linux/macOS/Windows CI, Chromium, native/Wasm extracted package checks and bounded sanitizer fuzzing | Broader engine/extension/fault/reorg/soak/performance, funded acceptance and independent security review remain open; `0.1.0-alpha.1` published on crates.io with docs.rs builds |

Evidence for the wallet rows is linked from [wallet workflows](docs/WALLET_WORKFLOWS.md),
[browser custody](docs/BROWSER_QI_CUSTODY.md), [atomic restore](docs/BROWSER_ATOMIC_RESTORE.md),
[ABI](crates/quai-abi/README.md), [provider review](docs/PROVIDER_PARITY_REVIEW_2026-09-13.md)
and [retained test reports](test-infra/reports). Per-declaration evidence is in the
machine-readable ledger. [Unknown account replacements](docs/ACCOUNT_NONCE_REPLACEMENTS.md)
now have executable reference comparisons and typed observation/wait APIs.
The [local/watch-only signer review](docs/SIGNER_PARITY_REVIEW.md) reconciles
provider composition, signing, HD origins and native/Wasm legacy key interchange.
The [Qi selection review](docs/QI_SELECTION_PARITY_REVIEW.md) maps fixed
denominations, coin records, selection state and corrected fee adjustments.

## Material differences when porting

| Difference | Rust contract and consequence |
| --- | --- |
| Dynamic JavaScript objects | Typed values, traits, explicit overload selection and eager ABI results replace Proxy methods, coercion, mutable class properties and global hooks. No runtime crypto-backend replacement registry is provided. |
| Provider scheduling | Caller-owned futures, polling and bounded event queues replace hidden timers and callback lifecycle machinery. Explicit HTTP batches and grouped outpoint reads are supported; no general implicit request coalescing/cache is provided. Applications explicitly apply observations to custody state. |
| Resource limits | Parsing, derivation, queries, queues, backup collections and numeric intermediates are bounded. Larger work requires explicit pages or application coordination; malformed/noncanonical inputs are rejected. |
| Block selectors | Explicit latest, pending, numbered and separate hash reads replace overloaded relative/string aliases. Unsupported node tags and historical return-data requests are not fabricated. The reference's `waitForBlock`, `getTransactionResult` backend and named `Typed.tuple` factory are unfinished or unsupported source operations. |
| Arithmetic | Checked operations and explicit wrapping/lossy conversion cover both policies. Rust corrects signed-minimum wrap, rounding and fee-shortfall defects rather than reproducing invalid outputs. |
| Wallet recovery | Gap 50 counts matching addresses on each receive/change branch. Latest-only outpoints cannot identify every fully spent historical address; optional use hints and explicit deeper ranges are available. No mandatory indexer is imposed. |
| Key interchange | Public exports cannot contain xprv. Fresh salt/IV/UUID generation replaces caller-selected deterministic encryption entropy; secret buffers and KDF work have explicit limits. |
| Browser ownership | Browser sessions use IndexedDB and worker-local futures; native sessions use SQLite. Native destination cursors persist automatically, while browser callers retain/recheck explicit ranges. |
| Injected wallets | The application selects a provider and account. Supported signatures and transactions are independently verified. Automatic extension selection, chain switching and a general permission manager are outside the typed adapter. |
| Payment channels | Registered BIP47 channels are supported. The pinned package has no notification exchange; Pelagus adds a mailbox contract, which `payment_mailbox` reads and notifies explicitly. Announcements are unauthenticated public links between codes. |

Detailed mappings and executable source counterexamples are linked from the ledger
and guides for [declarations](docs/DECLARATION_PARITY.md),
[fixed point](docs/FIXED_POINT_PARITY.md), [curve/aggregation](docs/CURVE_AND_AGGREGATION_PARITY.md),
[utilities](docs/UTILITY_PARITY.md), [providers](docs/PROVIDER_PARITY.md),
[transactions](docs/TRANSACTION_INTERCHANGE_PARITY.md),
[contracts](docs/CONTRACT_PARITY.md) and [HD wallets](docs/HD_WALLET_PARITY_REVIEW.md).

## Additional functionality for Rust applications

The SDK adds immutable verified signed payloads; typed ledger/network identities;
checked exact arithmetic; redacted zeroizing secret wrappers; bounded parsers and
transports; caller-owned cancellation; and native/worker implementations sharing
protocol tests. Durable account/Qi reservations preserve exact candidate bytes
across ambiguous submission, replacement and restart. Authenticated full-wallet
backups, monotonic allocation floors and atomic multi-journal browser restore
preserve anti-reuse and signed-claim guarantees. These are additions to the
reference comparison, not evidence of consensus finality or a security audit.

## Qualification and release boundary

Native Linux/macOS/Windows CI, Chromium window/worker tests, pinned JS/Go vectors,
read-only mainnet checks, extracted-package builds/consumers and bounded ASAN fuzz
provide complementary evidence. Reports under [test-infra/reports](test-infra/reports)
bind their source and lock hashes; historical results do not automatically qualify
a later commit. CI on the release revision must pass before handoff.

The following remain production qualification work rather than hidden implemented
features: broader funded execution against unmodified target nodes; real extension and
additional browser-engine coverage; sustained fault/reorg/soak/performance and fuzz
campaigns; and independent security review. The owned isolated chain used documented
patches and toy keys. Its success does not establish unmodified-network acceptance.
Cross-zone qualification is deferred at the user's direction while only Cyprus-1
is available; it remains unqualified until another zone is active.
[Funded Orchard qualification](test-infra/orchard/README.md) now includes two
successful QUAI transfers and a corrected Quai-to-Qi conversion with all 386,286
settled Qits observed in 17 outputs. A [later live check](test-infra/orchard/conversion-unlocks-2026-09-14.json)
at height 7,794,731 found all 17 present and unlocked, with zero missing or locked
conversion Qits. The first conversion exposed an
underfunded destination despite origin success; the SDK now budgets origin costs
and denomination fragmentation after discounts. This is a useful Rust difference
from the reference's raw-estimate path, not a claim of universal fee sufficiency.
A matured output from the original conversion was subsequently spent in a
confirmed 1 Qi self-transfer with all resulting outputs indexed.
The corrected Orchard WQUAI deployment passed a funded 0.01 QUAI deposit/withdraw
round trip with exact token and native balance accounting. Earlier empty-code
reports queried the mainnet address on Orchard. Funded WQI qualification now also
includes a [1 Qi backing deposit](test-infra/orchard/wqi-roundtrip-2026-09-14.json), claiming 1 WQI, redemption to an observed locked
Qi output, expiry of its Orchard lock, and a confirmed spend of that exact output.
The wrap used an explicit 0.1 Qi fee because Orchard did not satisfy the pinned
specialized estimator's activation profile; the same estimator produced mainnet
quotes. Real Pelagus interoperability is recorded separately from [funded local payment-code testing](test-infra/orchard/payment-codes-2026-09-14.json).
That test sent 5 Qi, recovered it using the receiver seed and sender public code,
spent the recovered BIP47 output in a 1 Qi return payment, and discovered the
return with a default gap-50 scan. Failed preparations had burned enough send
ranges that the first receive scan needed explicit continuation past its initial
50 empty addresses. Compact native allocation now commits only examined children,
so failed preparations no longer burn thousands of unused trailing candidates.
Existing floors are preserved; explicit persisted-metadata continuation and deep
ranges remain necessary for old or fully spent gaps. Pinned quais.js matched both payment codes
and the actual funded receive key/address and return address.

Additional [Orchard qualification](test-infra/orchard/qi-to-quai-2026-09-14.json)
verified 1 Qi → 0.025761653122291725 QUAI, the exact locked-to-spendable balance
transition, and a 0.01 QUAI transfer whose 2× gas-price replacement mined.
[Aggregation and sweep evidence](test-infra/orchard/transaction-scenarios-2026-09-14.json)
records 46 inputs (HD plus BIP47, including repeated keys), verified first-Qi block
placement, exact 26-output accounting, and a 15-input sweep recovered after the
client deliberately withheld an accepted submission's acknowledgement.
A [copied-node rollback test](test-infra/local-chain/recovery-2026-09-14.json)
verified invalidation, reopening and reconfirmation without releasing claims.
This is controlled database rollback, not a competing-peer consensus reorg.

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

The [isolated refund test](test-infra/local-chain/refund-2026-09-14.json) exposed and
fixed rejection of valid Qi refund outpoints carrying a Quai-ledger creating hash.
The fix covers native/portable custody, discovery, selection, signing and backups.
It also established that sub-1-Qi refund amounts may create no outputs despite a
successful receipt under the pinned denomination cutoff. A real 5 Qi refund was
discovered, matured and spent through the SDK; controller and refund evidence is
from the documented synthetic profile, not Orchard.

A packageable alpha does not promise production custody safety, unrestricted
backward compatibility or independent chain verification. General journal
compaction, automatic peer notification protocols and hardware/distributed custody
are possible future additions, not features silently asserted by this comparison.
The [security policy](SECURITY.md) retains specific memory-erasure and trust limits.
The `0.1.0-alpha.1` crates are published on crates.io and documented on docs.rs;
publication does not change the qualification limits above.

## Unfinished declaration families

The generated table lists only families with pending or partial rows; it is empty.
Its absence of rows measures review bookkeeping, not production certification.

<!-- parity-family-table:start -->
| Export family | Pending | Partial | Implemented | Deviation |
| --- | ---: | ---: | ---: | ---: |
<!-- parity-family-table:end -->
