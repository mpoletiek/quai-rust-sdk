# Rust SDK versus quais.js: feature parity and differences

**Complete feature parity is not established.** The Rust SDK now implements the
core wallet features raised in the review: Qi gap discovery, fixed denominations,
HD and payment-code derivation, both conversion directions, native Qi wrapping,
WQI claim/redemption, WQUAI deposit/withdraw, and durable custody/recovery. Native
and browser orchestration differ, reference declaration review remains unfinished,
and several network/release qualification gates remain open.

This document compares the checked-in implementation, rather than treating an
API name, a passing synthetic test or a large declaration count as completion.
[SDK documentation](SDK_DOCUMENTATION.md) explains the supported workflows.

## Audit of this report

Audited against implementation commit `f2f2326fb527614940fe396f67f63ec000a49814`.
**The evidence does not support reading this SDK as mostly unimplemented.** There
is substantial native implementation across the core wallet feature families.
However, this report is not a completed behavioral audit and cannot substantiate
a percentage-complete claim in either direction.

The declaration ledger has fallen behind implementation. Specific examples:

| Ledger observation at the audited commit | Implementation evidence | Audit conclusion |
| --- | --- | --- |
| `denominations` is pending | `Denomination::VALUES` and validated indices in [consensus](crates/quai-consensus/src/qi.rs); [denomination tests](crates/quai-consensus/tests/qi_vectors.rs) | Fixed denominations are implemented; the export mapping is unfinished |
| `FewestCoinSelector.performSelection` is pending | `select_fewest` in [selection](crates/quai-wallet/src/selection.rs), with [69 reference selection vectors](crates/quai-wallet/tests/selection.rs) | Coin selection is implemented; class/configuration semantics still need explicit reconciliation |
| `Wallet.signTransaction` is pending | `QuaiTransaction::sign` in [consensus](crates/quai-consensus/src/quai.rs), [signed reference vectors](crates/quai-consensus/tests/quai_vectors.rs), and [account sessions](crates/quai-sdk/src/accounts.rs) | Transaction signing exists; the JS wallet method's complete population/provider behavior is a separate mapping question |
| `TransactionReceipt.hash`, `.status` and `.logs` are pending | `Receipt` has `transaction_hash`, typed `outcome` and `logs` in [provider types](crates/quai-provider/src/types.rs) | Receipt data is implemented with Rust representations; the entire JS receipt helper class is not thereby qualified |
| 16 transaction property rows say specialized Qi is awaiting typed builders | Public conversion/wrapping types in [consensus exports](crates/quai-consensus/src/lib.rs), with [conversion](crates/quai-consensus/tests/conversions.rs) and [wrapping](crates/quai-consensus/tests/wrapping.rs) vectors | That rationale is stale. It has been corrected without promoting the rows to completed parity |

Ignoring export subpaths, the 3,928 rows reduce to 2,023 distinct
`(symbol, behavior, signatures)` keys. That still counts inherited class members,
properties and aliases; it is not a count of independent application features.
Likewise, the 62 `implemented` rows exclude working APIs classified as deliberate
`deviation`, as well as working APIs whose mappings remain `pending` or `partial`.

Three different completion questions must be answered separately:

| Question | Supported assessment |
| --- | --- |
| Does useful core functionality exist? | Yes: native identity/derivation, current Qi discovery, selection/signing, payment channels, conversions, wrappers, contracts and durable recovery have implementations and tests |
| Does every required published quais.js behavior have a tested Rust equivalent or justified difference? | Not established. The semantic review is unfinished, and browser/native orchestration has explicit storage and lifecycle differences |
| Is the SDK production-qualified? | No. Funded unmodified-network acceptance and broader reliability/security/platform qualification remain open |

The browser gap is visible in the [facade feature gates](crates/quai-sdk/src/lib.rs):
`accounts`, `qi`, `recovery` and related full native sessions require native
SQLite, while browser sessions compose their own IndexedDB custody, preparation, signing,
submission and recovery APIs.
Existing [reconciliation APIs](crates/quai-sdk/src/recovery.rs) must not be
reported as wholly absent merely because automatic lifecycle application remains
unfinished. Similarly, an unrun funded acceptance test is a qualification gap,
not evidence that its transaction builder has not been implemented.

The tables below remain a backlog and evidence index. Notification automation,
permission conveniences and safe journal compaction are absent capabilities or
Rust lifecycle concerns, not proven omissions from quais.js unless corresponding
published reference behavior is identified. They should not silently become
requirements for declaring reference parity.

## Reference and method

The reference is the **published `quais@1.0.0-alpha.57` npm artifact**, locked in
[compatibility/package-lock.json](compatibility/package-lock.json). The
[September 13 registry recheck](test-infra/reports/reference-version-recheck-2026-09-13.json)
found the same latest version and integrity. That is a dated observation, not a
promise that the release remains latest.

The [reference lock](compatibility/reference-lock.json) and
[source manifest](compatibility/reference-source-manifest.json) retain provenance
and hashes for 153 published source files. The separately inspected quais.js
checkout is `94e32c7eb9960de36054135c40a341c44c84f922`; npm's recorded `gitHead` is
`3bd0bf5b077f4aa5fab480474e3982e50e1af506`. Those commit IDs are not interchangeable.
Published behavior is determined from the locked artifact, and source-only or
unpublished methods must not silently expand or shrink its denominator.

Selected wire/rule checks additionally use go-quai v0.56.0 at
`f3f345c877300c044e3e0081a48bf3cf786fb9cc`. A node's client-version string is not a
build attestation. Mainnet reads, patched isolated-chain writes and independent
JS/Go vectors establish different kinds of evidence.

The TypeScript inventory resolves all 12 published export roots/subpaths,
including aliases, overloads and inherited public class members. There are 581
export entries and 3347 member entries: **3928 tracking rows**, with repeated
exports and inherited definitions included. These are not 3928 independent
features. The [parity ledger](compatibility/parity.json) preserves the denominator
and per-row Rust APIs, test paths, documentation and deviation notes.

| Ledger status | Rows | Meaning |
| --- | ---: | --- |
| `implemented` | 66 | An explicitly mapped behavior, still subject to qualification |
| `deviation` | 1670 | Documented replacement, stricter behavior, correction or omission |
| `partial` | 126 | A mapping exists with unfinished behavior or scope |
| `pending` | 2066 | No completed row-level reconciliation; not proof of absence |

There is no defensible feature-completion percentage from these counts. A
`deviation` can be a deliberate Rust design choice or an absent convenience API;
its text must be inspected. A `pending` type alias may have a straightforward
Rust equivalent, while a pending wallet method can represent substantial work.
This analysis does not relabel unresolved rows to make the counts appear complete.

The [HD wallet review](docs/HD_WALLET_PARITY_REVIEW.md) maps exact derivation,
address collections, channels, scanning, signing and sending. Cached address-status
views and verified legacy wallet-JSON migration now have native/worker tests.
Both pinned HD `xPub()` methods return private xprv strings; Rust public-root
export deliberately corrects that exposure.

## Feature comparison

| Area | Rust implementation / reference relationship | Remaining difference or qualification |
| --- | --- | --- |
| Addresses and routing | Typed ledger/zone addresses, checksum parsing, shard routing and direct/gateway selection with pinned vectors | Explicit static routes and trusted identities replace mutable JS discovery/configuration; known zone does not imply activation |
| Units and numeric utilities | Exact Quai/Qi units, signed formatting, fixed point, hex/Base58/Base64, packed hashes and Unicode | No JS numeric coercion, unsafe wrapping or loose UTF-8/Base64; corrected rounding and range behavior |
| BIP39/BIP32/BIP44 | All ten wordlists, 16–64-byte seeds, xprv/xpub metadata, absolute/relative paths, both coin types, bounded zone search | Rare invalid children fail at their actual index; explicit master/subtree APIs and bounded cancellation replace convenience loops |
| Imported and watch-only keys | Validated ownership metadata and mixed-origin key resolver; watch-only types cannot sign | Private imports are guarded rather than serialized through general public objects |
| Qi discovery | Receive/change default gap 50 matching Qi addresses, deep ranges, every persisted origin refresh, lock/balance classes | Latest-only outpoints cannot prove historical address use; optional external use hints and explicit recovery ranges are needed for stronger recovery |
| Qi selection/signing | Fixed denomination inventory, exact output/address capacity, single/multi-input signing, sweep and explicit aggregation | No fee-shortfall success; aggregation block placement remains unqualified |
| Quai transactions | Native durable prepare/review/sign/broadcast, nonce reservation, access lists, deployment and cross-zone intents | Native SQLite sessions and the browser account session use explicit pending/latest policy; browser replacement preparation, candidate selection and reconciliation exist |
| Qi transactions | Native and browser durable selection/fee convergence/preparation/signing/submission; exact special-operation custody and candidate reconciliation | Browser signed-intent destination observations exist; only native resume persists destination cursors; observations remain source claims |
| Payment codes | Version-one BIP47 seed/master/account origins, matching send/receive keys, registered channels, gap/deep scans and mixed-origin spending | Peer-code exchange is out of band; automatic notification discovery/blinding is absent; bounded cursors deliberately avoid reuse |
| Quai → Qi | Native and browser conversion preparation, simulation/fees/slippage and signed custody; native/browser destination observations | Quote history depends on node behavior; per-operation maturity/production execution need stronger evidence |
| Qi → Quai | Exact 22-byte conversion form, refund/slippage, explicit or profiled fees and signed recovery | Specialized estimator requires asserted compatible fork/node profile; aggregate Quai balance is not operation-specific maturity proof |
| Qi → WQI / WQI → Qi | Native 20-byte wrap, WQI backing/claim, ERC-20 operations, redemption gas/dust planning, lock observations | Mature unmodified-network redemption spend and audited contract/profile qualification remain open |
| Quai ↔ WQUAI | Configured deployment, deposit/withdraw and ERC-20 intents | Mainnet code observed; last Orchard WQUAI code check was empty; funded unmodified/testnet acceptance remains open |
| ABI/interfaces | Canonical encoder/decoder, JSON/readable declarations, defaults, packed encoding, EIP-712, event filters and call/log/revert parsing | Positional values and explicit overloads replace JS Proxy/Result/Typed ergonomics; strict bounds and validation differ |
| Contracts/deployments | Explicit contract calls/intents, ERC-20 helpers, artifacts, constructor data, address grinding, canonical code observations | No dynamic JS method/property generation; input artifacts must have resolved creation bytecode; execution qualification remains separate |
| RPC and providers | Typed reads, block/pool/log/wallet APIs, exact signed broadcast, bounded receipts/head/ETX tracking | Unregistered account nonce competitors now have bounded signed discovery and native/browser waits; many inherited hooks/response helpers still need declaration reconciliation; no fabricated `safe`/`finalized` or historical return-data capability |
| Native WebSocket | Bounded subscriptions, lag/disconnect reporting, reconnect and canonical replay with persistence fences | Complete automatic wallet-state application and terminal claim policy are unfinished |
| Browser transport/signing | Actual Wasm Fetch/WS, workers, injected message/typed/transaction verification, explicit raw submission capability | No automatic wallet selection, chain switching or permission manager; real extensions/other engines need qualification |
| Browser persistence | Durable allocation, account/Qi custody, consistent backup capture and atomic multi-journal live restore | Explicit enumeration; initialized targets required by the coordinator; account call/conversion/deployment fee/prepare/sign/root-submit orchestration now exists; account/Qi candidate reconciliation exists; Qi preparation now includes current discovery and persisted owners; destination observations now exist; native persisted destination cursors and general history compaction differ |
| Backup/restore | Authenticated private origins, public ownership proofs, channels, burned floors, exact candidate families, monotonic merge | Portable backups retain floors/exposures rather than missing allocation request history; browser/native storage APIs differ |
| Platform and release | Linux/macOS/Windows CI, Chromium, native/Wasm extracted package checks and bounded sanitizer fuzzing | Broader engine/extension/fault/reorg/soak/performance, funded acceptance and independent security review remain open; crates.io publishing disabled |

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

## Concrete remaining gaps

These are distinct from unreviewed declaration bookkeeping. They include SDK
workflow and qualification work, not only omissions from the published reference.
Notification automation, permission management and journal compaction are listed
as absent capabilities; this review does not establish that the pinned JS package
implements those complete workflows. They must not be counted as proven parity
gaps merely because they appear here.

| Gap | Why it remains open | What would close it |
| --- | --- | --- |
| Browser/native storage differences | Browser sessions compose discovery, account/Qi preparation, reviewed replacements, durable signing/submission, canonical reconciliation and signed-intent settlement; destination cursor persistence remains native-only | Browser callers must recheck explicit destination ranges after restart; native persisted cursors are an additional recovery convenience, not a proven missing quais.js behavior |
| Automatic wallet reconciliation and terminal claim policy | Bounded canonical replay and conservative claims exist; applications still explicitly refresh/apply observations | Defined pending/replaced/dropped/reorg/terminal transitions with evidence-based claim handling and fault tests; disappearance alone must not permit reuse |
| Row-level semantic parity review | 2426 pending and 156 partial declarations, including inherited provider and response helpers | Review each unique behavior, overload and inherited binding; map it to tested Rust behavior or justify a specific omission |
| Injected-wallet convenience/interoperability | Provider selection is explicit; chain switching and permission management are absent | Implement selected supported extension operations with exact request/result/context tests, then qualify real extensions |
| Payment notification automation | Version-one keys/channels work with explicit code exchange | A specified supported notification/exchange protocol, parser/construction/discovery tests and appropriate node/peer qualification; do not infer it from payment-code key derivation |
| Browser journal lifetime management | IDs remain consumed under finite bounds; there is no general safe compaction/rotation workflow | A design retaining anti-reuse/claim guarantees across archival or successor journals, with restore/concurrency tests |
| Funded unmodified-node/testnet qualification | Orchard maintenance/faucet access and isolated-node patches limit available evidence | Disposable funded checks on the intended unmodified profiles, including mature redemption spend, cross-zone execution and aggregation placement |
| Broader reliability/security/release work | Existing deterministic tests, bounded fuzz and CI are limited evidence | Sustained fault/reorg/soak/performance and fuzz campaigns, additional browser engines, specialist review and release artifacts/registry preparation |

Two further limits cannot be solved by renaming SDK methods: latest-only
outpoints do not provide historically consistent recovery, and an aggregate
account balance does not attribute maturity to one conversion. Qualifying a
stronger node/index/history capability is necessary where the application needs
those guarantees. Mainnet read access alone does not authorize spending real
funds or provide those capabilities.

## Rust additions and intentional behavioral differences

These are implemented capabilities or policies beyond a direct transcription of
the pinned JavaScript convenience API. They are not a claim that no JavaScript
application could build equivalent behavior.

| Addition/difference | Practical effect | Evidence |
| --- | --- | --- |
| Typed ledger/zone/network boundaries | Earlier rejection of wrong-ledger destinations and scoped storage/claim confusion | [primitives](crates/quai-primitives/README.md), [storage](crates/quai-wallet/STORAGE.md) |
| Durable prepare/sign/submit separation | Exact authorized bytes and claims survive restart before network ambiguity | [account workflow](docs/account-workflow.md), [Qi workflow](docs/qi-transactions.md) |
| Conservative candidate-family custody | Fee replacements retain roots/claims and merge compatible branches without releasing funds on uncertain observations | [account custody](docs/BROWSER_ACCOUNT_CUSTODY.md), [Qi custody](docs/BROWSER_QI_CUSTODY.md) |
| Durable range-before-search allocation | Cancellation and failed searches cannot expose a reused HD/payment index | [allocation restore](docs/BROWSER_ALLOCATION_RESTORE.md), [discovery](crates/quai-wallet/DISCOVERY.md) |
| Atomic browser recovery | All selected live allocation/account/Qi/payment journals restore in one transaction; conflicts cannot partially update a wallet | [atomic restore](docs/BROWSER_ATOMIC_RESTORE.md) |
| Ownership-proved authenticated full backup | Effective origins, burned floors, channels and exact signed candidates restore without trusting stale checkpoints | [backup format](crates/quai-wallet/FULL_BACKUP_FORMAT.md), [capture](docs/PORTABLE_WALLET_CAPTURE.md) |
| Explicit imported payment-account backup | Retains guarded account xprv while documenting its unprovable omitted ancestry | [payment origins](crates/quai-payments/README.md), [backup format](crates/quai-wallet/FULL_BACKUP_FORMAT.md) |
| Canonical contract-code preflight | Binds deployment presence/code hash to genesis and rechecked numeric block observations | [code preflight](docs/CONTRACT_CODE_PREFLIGHT.md) |
| Strict canonical/bounded parsing | Rejects duplicate JSON fields, ambiguous ABI selectors, malformed encodings and resource overflow early | [ABI](crates/quai-abi/README.md), [RPC](crates/quai-rpc/README.md), [browser](crates/quai-browser/README.md) |
| Verified injected results | Recovers message/typed signatures and validates exact returned transaction fields and context | [browser](crates/quai-browser/README.md) |
| Guarded secret ownership | Redacted diagnostics and zeroizing owned buffers; explicit secret exports | [crypto](crates/quai-crypto/README.md), [keystore limits](crates/quai-keystore/README.md) |
| Exact numeric corrections | Mathematical fixed-point rounding, declared integer bounds and no fee shortfall acceptance | [fixed point](crates/quai-primitives/README.md), [ABI values](crates/quai-abi/README.md), [selection vectors](compatibility/fixtures/selection.json) |
| Exact CREATE init bytes | Preserves leading zeros that the pinned JS predictor strips | [deployment prediction](crates/quai-primitives/README.md) |
| Explicit protocol fee/redemption profiles | Refuses generic specialized estimates or silent redemption dust under an asserted profile | [conversions/wrappers](docs/WALLET_WORKFLOWS.md) |
| Finite resource policies | Bounds scans, queues, payloads, KDFs and retained journals instead of unbounded convenience behavior | [SDK limits](SDK_DOCUMENTATION.md#errors-and-operational-limits) |

JavaScript-specific constructors, branding, mutable singleton hooks, named
`Result` properties, Promise-like addresses and dynamic contract methods often
have no useful one-for-one Rust translation. They still need explicit ledger
entries rather than silently disappearing from the review. Corrections to
reference bugs are tested deviations, not incompatibility accidentally hidden
by changing expected vectors.

The reference also contains placeholders: `waitForBlock` throws
`NOT_IMPLEMENTED`, and its JSON-RPC transaction-result path does not supply
historical execution return data. Rust does not need a throwing placeholder to
claim that unsupported capability. The [provider review](docs/PROVIDER_PARITY_REVIEW_2026-09-13.md)
retains executable reference observations and the precise distinction.

## Evidence and completion criteria

The compatibility fixtures are generated from the locked published reference and
retain expected failures/divergences. Selected transaction/address behavior also
has independent Go vectors. Tests use public toy keys. Public mainnet reads
validated chain/genesis, ordinary wallet RPCs and nonempty WQI/WQUAI code.
Isolated-chain writes include documented consensus/controller/lock changes and
must not be represented as unmodified production acceptance.

To claim complete parity, finish the unique-behavior review behind pending and
partial rows, close required orchestration gaps, and retain reproducible tests
for each supported behavior or explicitly agreed deviation. To claim production
qualification, separately finish the network, reliability, security and release
gates. Publishing to crates.io would be a subsequent authorized release action;
repository visibility and package rehearsal do not publish a crate.

## Declaration-review appendix

The following table is generated from `compatibility/parity.json`, grouped by
exported symbol family across all subpaths. Every family with pending or partial
rows is shown. A count identifies **review work**, not a confirmed missing API.
Consult each row's `id`, `signatures`, `rust_api`, `test_ids` and `deviation` in the
JSON for exact overload/member detail. Inherited provider methods and duplicate
root/subpath exports account for much of the volume.

<!-- parity-family-table:start -->
| Export family | Pending | Partial | Implemented | Deviation |
| --- | ---: | ---: | ---: | ---: |
| `WebSocketProvider` | 118 | 10 | 0 | 74 |
| `SocketProvider` | 114 | 10 | 0 | 68 |
| `JsonRpcProvider` | 110 | 10 | 0 | 68 |
| `BrowserProvider` | 106 | 12 | 0 | 70 |
| `JsonRpcApiProvider` | 108 | 10 | 0 | 68 |
| `AbstractProvider` | 84 | 10 | 0 | 68 |
| `Result` | 88 | 0 | 0 | 0 |
| `FetchRequest` | 64 | 0 | 0 | 0 |
| `QuaiTransaction` | 46 | 10 | 0 | 0 |
| `BaseContract` | 50 | 4 | 0 | 2 |
| `ContractTransactionResponse` | 52 | 0 | 0 | 6 |
| `QuaiTransactionResponse` | 52 | 0 | 0 | 6 |
| `Contract` | 46 | 4 | 0 | 2 |
| `QiTransaction` | 36 | 10 | 0 | 0 |
| `QiTransactionResponse` | 40 | 0 | 0 | 0 |
| `FunctionFragment` | 38 | 0 | 0 | 0 |
| `TypedDataEncoder` | 14 | 20 | 0 | 2 |
| `FetchResponse` | 32 | 0 | 0 | 0 |
| `ParamType` | 32 | 0 | 0 | 0 |
| `Signature` | 32 | 0 | 0 | 0 |
| `EventFragment` | 30 | 0 | 0 | 0 |
| `ConstructorFragment` | 26 | 0 | 0 | 0 |
| `ErrorFragment` | 26 | 0 | 0 | 0 |
| `ContractEventPayload` | 24 | 0 | 0 | 0 |
| `FallbackFragment` | 24 | 0 | 0 | 0 |
| `StructFragment` | 24 | 0 | 0 | 0 |
| `NamedFragment` | 22 | 0 | 0 | 0 |
| `BaseWallet` | 20 | 0 | 0 | 0 |
| `Fragment` | 20 | 0 | 0 | 0 |
| `LangEs` | 20 | 0 | 0 | 0 |
| `LangEn` | 18 | 0 | 0 | 0 |
| `SocketEventSubscriber` | 18 | 0 | 0 | 0 |
| `WordlistOwlA` | 18 | 0 | 0 | 0 |
| `AbstractTransaction` | 17 | 0 | 0 | 0 |
| `ContractUnknownEventPayload` | 16 | 0 | 0 | 0 |
| `Network` | 16 | 0 | 0 | 0 |
| `SigningKey` | 14 | 2 | 2 | 0 |
| `SocketBlockSubscriber` | 16 | 0 | 0 | 0 |
| `SocketPendingSubscriber` | 16 | 0 | 0 | 0 |
| `SocketSubscriber` | 16 | 0 | 0 | 0 |
| `WordlistOwl` | 16 | 0 | 0 | 0 |
| `BIP44` | 12 | 0 | 0 | 0 |
| `UnmanagedSubscriber` | 12 | 0 | 0 | 0 |
| `Wordlist` | 12 | 0 | 0 | 0 |
| `AbiCoder` | 6 | 2 | 0 | 6 |
| `AggregateCoinSelector` | 8 | 0 | 0 | 0 |
| `ContractFactory` | 0 | 8 | 0 | 18 |
| `EventPayload` | 8 | 0 | 0 | 0 |
| `FetchCancelSignal` | 8 | 0 | 0 | 0 |
| `FeeData` | 6 | 0 | 0 | 0 |
| `Interface` | 2 | 2 | 0 | 68 |
| `AccessList` | 2 | 0 | 0 | 0 |
| `AccessListEntry` | 2 | 0 | 0 | 0 |
| `accessListify` | 2 | 0 | 0 | 0 |
| `AccessListish` | 2 | 0 | 0 | 0 |
| `ActionRejectedError` | 2 | 0 | 0 | 0 |
| `Addressable` | 2 | 0 | 0 | 0 |
| `AddressLike` | 2 | 0 | 0 | 0 |
| `AddressStatus` | 2 | 0 | 0 | 0 |
| `AllowedCoinType` | 2 | 0 | 0 | 0 |
| `BadDataError` | 2 | 0 | 0 | 0 |
| `BaseContractMethod` | 2 | 0 | 0 | 0 |
| `BigNumberish` | 2 | 0 | 0 | 0 |
| `BlockParams` | 2 | 0 | 0 | 0 |
| `BlockTag` | 2 | 0 | 0 | 0 |
| `BufferOverrunError` | 2 | 0 | 0 | 0 |
| `BytesLike` | 2 | 0 | 0 | 0 |
| `CallExceptionAction` | 2 | 0 | 0 | 0 |
| `CallExceptionError` | 2 | 0 | 0 | 0 |
| `CallExceptionTransaction` | 2 | 0 | 0 | 0 |
| `CancelledError` | 2 | 0 | 0 | 0 |
| `checkResultErrors` | 2 | 0 | 0 | 0 |
| `CodedquaisError` | 2 | 0 | 0 | 0 |
| `ConstantContractMethod` | 2 | 0 | 0 | 0 |
| `ContractDeployTransaction` | 2 | 0 | 0 | 0 |
| `ContractEvent` | 2 | 0 | 0 | 0 |
| `ContractEventArgs` | 2 | 0 | 0 | 0 |
| `ContractEventName` | 2 | 0 | 0 | 0 |
| `ContractInterface` | 2 | 0 | 0 | 0 |
| `ContractMethod` | 2 | 0 | 0 | 0 |
| `ContractMethodArgs` | 2 | 0 | 0 | 0 |
| `ContractRunner` | 2 | 0 | 0 | 0 |
| `ContractTransaction` | 2 | 0 | 0 | 0 |
| `copyRequest` | 2 | 0 | 0 | 0 |
| `DebugEventBrowserProvider` | 2 | 0 | 0 | 0 |
| `DeferredTopicFilter` | 2 | 0 | 0 | 0 |
| `Eip1193Provider` | 2 | 0 | 0 | 0 |
| `EncryptOptions` | 2 | 0 | 0 | 0 |
| `ErrorCode` | 2 | 0 | 0 | 0 |
| `EventEmitterable` | 2 | 0 | 0 | 0 |
| `EventFilter` | 2 | 0 | 0 | 0 |
| `FetchGatewayFunc` | 2 | 0 | 0 | 0 |
| `FetchGetUrlFunc` | 2 | 0 | 0 | 0 |
| `FetchPreflightFunc` | 2 | 0 | 0 | 0 |
| `FetchProcessFunc` | 2 | 0 | 0 | 0 |
| `FetchRetryFunc` | 2 | 0 | 0 | 0 |
| `Filter` | 2 | 0 | 0 | 0 |
| `FilterByBlockHash` | 2 | 0 | 0 | 0 |
| `FixedFormat` | 2 | 0 | 0 | 0 |
| `FormatType` | 2 | 0 | 0 | 0 |
| `FragmentType` | 2 | 0 | 0 | 0 |
| `getBigInt` | 2 | 0 | 0 | 0 |
| `getNumber` | 2 | 0 | 0 | 0 |
| `getUint` | 2 | 0 | 0 | 0 |
| `GetUrlResponse` | 2 | 0 | 0 | 0 |
| `InsufficientFundsError` | 2 | 0 | 0 | 0 |
| `InterfaceAbi` | 2 | 0 | 0 | 0 |
| `InvalidArgumentError` | 2 | 0 | 0 | 0 |
| `isAddressable` | 2 | 0 | 0 | 0 |
| `isBytesLike` | 2 | 0 | 0 | 0 |
| `isCallException` | 2 | 0 | 0 | 0 |
| `isError` | 2 | 0 | 0 | 0 |
| `isHexString` | 2 | 0 | 0 | 0 |
| `JsonFragment` | 2 | 0 | 0 | 0 |
| `JsonFragmentType` | 2 | 0 | 0 | 0 |
| `JsonRpcApiProviderOptions` | 2 | 0 | 0 | 0 |
| `JsonRpcError` | 2 | 0 | 0 | 0 |
| `JsonRpcPayload` | 2 | 0 | 0 | 0 |
| `JsonRpcResult` | 2 | 0 | 0 | 0 |
| `JsonRpcTransactionRequest` | 2 | 0 | 0 | 0 |
| `Ledger` | 2 | 0 | 0 | 0 |
| `Listener` | 2 | 0 | 0 | 0 |
| `lock` | 2 | 0 | 0 | 0 |
| `LogParams` | 2 | 0 | 0 | 0 |
| `makeError` | 2 | 0 | 0 | 0 |
| `MaxInt256` | 2 | 0 | 0 | 0 |
| `MinedBlock` | 2 | 0 | 0 | 0 |
| `MinedTransactionResponse` | 2 | 0 | 0 | 0 |
| `MinInt256` | 2 | 0 | 0 | 0 |
| `MissingArgumentError` | 2 | 0 | 0 | 0 |
| `Mnemonic` | 0 | 2 | 14 | 6 |
| `musigCrypto` | 2 | 0 | 0 | 0 |
| `N` | 2 | 0 | 0 | 0 |
| `NetworkError` | 2 | 0 | 0 | 0 |
| `Networkish` | 2 | 0 | 0 | 0 |
| `NeuteredAddressInfo` | 2 | 0 | 0 | 0 |
| `NonceExpiredError` | 2 | 0 | 0 | 0 |
| `NotImplementedError` | 2 | 0 | 0 | 0 |
| `Numeric` | 2 | 0 | 0 | 0 |
| `NumericFaultError` | 2 | 0 | 0 | 0 |
| `OrphanFilter` | 2 | 0 | 0 | 0 |
| `OutpointInfo` | 2 | 0 | 0 | 0 |
| `Overrides` | 2 | 0 | 0 | 0 |
| `ParamTypeWalkAsyncFunc` | 2 | 0 | 0 | 0 |
| `ParamTypeWalkFunc` | 2 | 0 | 0 | 0 |
| `pbkdf2` | 2 | 0 | 0 | 0 |
| `PerformActionFilter` | 2 | 0 | 0 | 0 |
| `PerformActionRequest` | 2 | 0 | 0 | 0 |
| `PerformActionTransaction` | 2 | 0 | 0 | 0 |
| `PostfixOverrides` | 2 | 0 | 0 | 0 |
| `PreparedTransactionRequest` | 2 | 0 | 0 | 0 |
| `ProgressCallback` | 2 | 0 | 0 | 0 |
| `ProtoTransaction` | 2 | 0 | 0 | 0 |
| `Provider` | 2 | 0 | 0 | 0 |
| `ProviderEvent` | 2 | 0 | 0 | 0 |
| `QiAddressInfo` | 2 | 0 | 0 | 0 |
| `quaisError` | 2 | 0 | 0 | 0 |
| `quaisymbol` | 2 | 0 | 0 | 0 |
| `ReplacementUnderpricedError` | 2 | 0 | 0 | 0 |
| `scrypt` | 2 | 0 | 0 | 0 |
| `scryptSync` | 2 | 0 | 0 | 0 |
| `SerializedHDWallet` | 2 | 0 | 0 | 0 |
| `SerializedQiHDWallet` | 2 | 0 | 0 | 0 |
| `ServerError` | 2 | 0 | 0 | 0 |
| `Shard` | 2 | 0 | 0 | 0 |
| `SignatureLike` | 2 | 0 | 0 | 0 |
| `Subscriber` | 2 | 0 | 0 | 0 |
| `Subscription` | 2 | 0 | 0 | 0 |
| `TimeoutError` | 2 | 0 | 0 | 0 |
| `toBeArray` | 2 | 0 | 0 | 0 |
| `toBeHex` | 2 | 0 | 0 | 0 |
| `toBigInt` | 2 | 0 | 0 | 0 |
| `toNumber` | 2 | 0 | 0 | 0 |
| `TopicFilter` | 2 | 0 | 0 | 0 |
| `toQuantity` | 2 | 0 | 0 | 0 |
| `toShard` | 2 | 0 | 0 | 0 |
| `toZone` | 2 | 0 | 0 | 0 |
| `TransactionLike` | 2 | 0 | 0 | 0 |
| `TransactionReceiptParams` | 2 | 0 | 0 | 0 |
| `TransactionReplacedError` | 2 | 0 | 0 | 0 |
| `TransactionRequest` | 2 | 0 | 0 | 0 |
| `TransactionResponse` | 2 | 0 | 0 | 0 |
| `TransactionResponseParams` | 2 | 0 | 0 | 0 |
| `TypedDataDomain` | 2 | 0 | 0 | 0 |
| `TypedDataField` | 2 | 0 | 0 | 0 |
| `UnexpectedArgumentError` | 2 | 0 | 0 | 0 |
| `UnknownError` | 2 | 0 | 0 | 0 |
| `UnsupportedOperationError` | 2 | 0 | 0 | 0 |
| `verifyMessage` | 2 | 0 | 0 | 0 |
| `verifyTypedData` | 2 | 0 | 0 | 0 |
| `WebSocketCreator` | 2 | 0 | 0 | 0 |
| `WebSocketLike` | 2 | 0 | 0 | 0 |
| `wordlists` | 2 | 0 | 0 | 0 |
| `WrappedFallback` | 2 | 0 | 0 | 0 |
| `Zone` | 2 | 0 | 0 | 0 |
| `assert` | 1 | 0 | 0 | 0 |
| `assertArgument` | 1 | 0 | 0 | 0 |
| `assertArgumentCount` | 1 | 0 | 0 | 0 |
| `assertNormalize` | 1 | 0 | 0 | 0 |
| `assertPrivate` | 1 | 0 | 0 | 0 |
| `DataHexString` | 1 | 0 | 0 | 0 |
| `decodeProtoTransaction` | 1 | 0 | 0 | 0 |
| `defineProperties` | 1 | 0 | 0 | 0 |
| `encodeProtoTransaction` | 1 | 0 | 0 | 0 |
| `HexString` | 1 | 0 | 0 | 0 |
| `QiJsonRpcTransactionRequest` | 1 | 0 | 0 | 0 |
| `QiPreparedTransactionRequest` | 1 | 0 | 0 | 0 |
| `QiTransactionLike` | 1 | 0 | 0 | 0 |
| `QiTransactionRequest` | 1 | 0 | 0 | 0 |
| `QuaiJsonRpcTransactionRequest` | 1 | 0 | 0 | 0 |
| `QuaiPreparedTransactionRequest` | 1 | 0 | 0 | 0 |
| `quais` | 1 | 0 | 0 | 0 |
| `QuaiTransactionLike` | 1 | 0 | 0 | 0 |
| `QuaiTransactionRequest` | 1 | 0 | 0 | 0 |
| `resolveProperties` | 1 | 0 | 0 | 0 |
| `ShardData` | 1 | 0 | 0 | 0 |
| `Signer` | 1 | 0 | 0 | 0 |
| `SpendTarget` | 1 | 0 | 0 | 0 |
| `TxInput` | 1 | 0 | 0 | 0 |
| `TxOutput` | 1 | 0 | 0 | 0 |
| `version` | 1 | 0 | 0 | 0 |
| `ZoneData` | 1 | 0 | 0 | 0 |
<!-- parity-family-table:end -->

## Remote JSON-RPC account signer reconciliation

All 40 `JsonRpcSigner` declaration rows now map to explicit Rust behavior in
[the remote signer guide](docs/RPC_SIGNER.md). Native and Wasm signing support
personal/typed/legacy message requests, exact type-0 signing, finite-duration
unlocking and wallet-mediated sends. Returned signatures are recovered and
transactions compared field by field. Send acknowledgements require separate
observation; requests never retry automatically. Native SQLite and browser
IndexedDB preparation can commit exact external signatures without losing their
nonce claims. Provider conveniences use typed requests and explicit quotation.

## Block, receipt and log response reconciliation

Reviewed 272 declaration rows across `Block`, `TransactionReceipt`,
`ContractTransactionReceipt`, `Log`, `EventLog` and `UndecodedEventLog`.
[The response review](docs/RESPONSE_PARITY.md) maps typed data, explicit provider
reads, canonical confirmation/replay composition and ABI log interpretation.
New APIs add exact block lookup, bounded metadata/outbound views, checked receipt
fees, normalized JSON exports and receipt log views that retain decoding errors.
The published prefetched async hash lookup skips matching entries; Rust does not
reproduce that defect. The published receipt-result helper has no JSON-RPC backend
implementation. No archive execution data or finality is inferred.
