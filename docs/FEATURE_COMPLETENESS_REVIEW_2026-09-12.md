# Feature completeness review and implementation — 2026-09-12

Historical review: its earlier implementation backlog is superseded by the
[current parity analysis](../SDK_PARITY_ANALYSIS.md) and
[SDK guide](../SDK_DOCUMENTATION.md). Retained qualification requirements still
need their own evidence.


The requested Qi discovery, mixed-origin spending, payment-code workflows,
conversion preparation and WQI/WQUAI operations now have public implementations.
The SDK is **not yet feature complete or release-qualified** across the entire
reference inventory. The table below separates work delivered in this change
from remaining work; no gate is closed merely by having a signing primitive.

Baseline: repository `74bb103929c83abf83445882ac939de2a70288b6`,
`quais@1.0.0-alpha.57` (inspected source
`94e32c7eb9960de36054135c40a341c44c84f922`), go-quai v0.56.0
`f3f345c877300c044e3e0081a48bf3cf786fb9cc`. Published source hashes and locked
reference identity remain the compatibility boundary. The declaration tracker
retains all 3,928 rows; repeated exports/inherited members make row counts an
unsuitable completion percentage.

## Findings and delivered changes

| ID | Delivered | Remaining acceptance or implementation |
| --- | --- | --- |
| FC01 Qi discovery | Node-backed receive/change scan; default gap 50 matching addresses; explicit deep ranges/continuations; all persisted HD/imported/payment addresses refreshed; locks, balances, canonicality sampling, generation checks and claim retention | Latest-only RPC cannot prove fully spent historical gaps or an atomic historical snapshot. Full historical ancestor replay and qualified history coverage remain separate |
| FC02 signing origins | `QiKeyResolver` and bounded `QiKeyring`; independently checked HD/imported/BIP47 receive keys; mixed-input native session signing and portable worker key resolution/ordered signing, including public-metadata reopen | Hardware/distributed custody remains outside current local-key scope; watch-only metadata never grants signing authority |
| FC03 durable lifecycle | Unified persisted signed Qi operation types, restart/rebroadcast, exact canonical inclusion reconciliation, atomic persisted-ancestry/reorg rollback and generation-fenced observations, unsigned nonce recovery and explicit gap repair | Qi same-input candidate graphs and v5 backups are implemented; terminal claim release and full wallet-state application of canonical replay and fault/soak qualification remain open; absence never authorizes claim reuse |
| FC04 conversions | Exact provider rates/calculation, dedicated Quai conversion simulation, durable Quai-to-Qi preparation, Qi-to-Quai preparation with explicit fee/slippage/refund; specialized payload backup and recovery; bounded automatic fees for the explicit SHA-anchored v0.56.0 profile | Persistent bounded destination/refund observations and attributed current Qi locks are implemented; Quai aggregate locked balances still cannot prove per-operation maturity. Explicit confirmed-state preparation is available; funded acceptance remains a gate |
| FC05 Qi wrapping/redemption | Typed 20-byte native wrapping; single/ordered multi-key signing; durable prepare/sign/recover/broadcast; WQI protocol backing, claim, ERC-20 adapter and redemption; exact atom/Qit scaling and dust/gas rejection; bounded profile-selected wrapping estimates | Signed-intent subtype-4/6 observers, current Qi credit/lock attribution and restart caches are implemented; isolated funded wrap/claim/redeem and locked-wallet exclusion now pass with retained failed controls; mature unlock/spend and unmodified/Orchard acceptance remain open |
| FC06 Wrapped Quai | Explicit user-confirmed deployment constants, payable deposit/withdraw and ERC-20 adapter; offline runnable example; mainnet runtime-code presence observed | Observed mainnet runtime bytes now pass isolated funded deposit/approval/transfer/withdraw, fee, revert and restart checks. Verified source/contract audit, unmodified-node and funded Orchard acceptance remain open |
| FC07 payment channels | Registered send-to-code allocation, receive gap/deep scanning and ownership import, keyring integration; seed/master/account-xprv constructors; full-backup seed/master/account-xprv channel ownership with authenticated v3; portable bounded exposure journals and browser atomic allocation/resume with authenticated cursor initialization | Public code exchange is explicit and out of band; no automatic notification discovery |
| FC08 sweep/cross-zone | Exact sweep, explicit denomination aggregation, durable owned output pools; explicit cross-zone Qi preparation with exact-shape fee request; account prepare/resume and both-ledger ETX correlation | Aggregation requires first-Qi block placement on pinned node. Funded node/destination/reorg qualification and funded destination qualification remains open |
| FC09 account/wallet lifecycle | Existing creation/import/backup/deployment APIs, portable authenticated v1–v5 backup decryption/encryption and custody/cursor inspection, plus master-xprv HD restore, unsigned restart preparation, nonce-gap repair and explicit confirmed-state preflight across transfers/conversions/deployments | Fee-only account replacement graphs, candidate observation, exact restart broadcast and v4 backup are implemented; Qi candidates are durable; production deployment/code acceptance remains open; isolated transfer/deployment/code acceptance now passes with confirmed-state preparation |
| FC10 provider/browser | Typed conversion/wrapper reads, bounded multi-address outpoints and inclusive delta queries, bounded WS reconnect/canonical header replay; browser Fetch and bounded window/worker WebSocket adapters; passive account/chain/disconnect revisions, verified exact transaction signing, explicit signed submission, observed wallet-mediated sends and bounded browser receipt waiting with portable canonicality checks; atomic scoped IndexedDB snapshots, durable HD/payment-code allocation, and account nonce/Qi input signed-candidate custody with backup initialization and explicit restart/cancellation recovery | Remaining reference RPC mapping, complete browser wallet-state integration and real extension interoperability remain open |
| FC11 usability/parity | Runnable examples, workflow guide, updated semantic mappings with tests/deviations; bounded readable ABI import/full/minimal formatting and lossless JSON metadata export; JS regeneration wired into CI | 144 inherited basic-provider rows now have explicit evidence and deviations; remaining declaration/overload reconciliation and additional end-to-end examples remain open; rows are not blanket marked complete |
| FC12 release qualification | Existing qualification gates retained; independent Go wrapping and special-fee evidence; Linux/macOS/Windows, browser, fuzz-smoke and advisory CI runs passed on baseline; checkout-independent inventory generation fixed | Funded testnet, unmodified-node operations, sustained fuzz/fault/soak, real extension interoperability, specialist review and release/package gates remain open |

Address derivation and payment-code cryptography were already implemented before
this change; the principal gaps were integration and recovery. Both coin types,
accounts, receive/change, zone grinding and explicit child derivation are covered.

The ordinary selector had an additional issue: restricting outputs to the largest
input denomination did not preserve the full input inventory. It now shares
available denomination capacity across recipient and change outputs, allowing
splitting without combining smaller inputs. All 69 existing JS selection cases
still pass; the added regression checks a 10+5+5 input set cannot become ordinary
10+10 outputs. Aggregation is an explicit different policy.

## User decisions reflected in the implementation

Ordinary Qi use does not require an external indexer. Current outpoint gap scanning
with default 50 is provided directly, with explicit deep-scan ranges and honest
history limitations. Allocation metadata and known addresses are retained because
wallets rarely reuse addresses and this SDK burns bounded raw ranges on allocation.

Both networks use the confirmed Cyprus-1 addresses:

- WQI: `0x002b2596EcF05C93a31ff916E8b456DF6C77c750`
- WQUAI: `0x006C3e2AaAE5DB1bCd11A1a097cE572312EADdBB`

The [workflow guide](WALLET_WORKFLOWS.md) documents public APIs, units, sequencing,
examples, supported origins and node-specific constraints. Historical
[local-chain evidence](../test-infra/local-chain/README.md) retains its original
patched/unmodified boundary; this change does not reinterpret it as qualification
for new wrapping or cross-zone workflows.

## Validation

- Native workspace all-features tests passed; exact summary retained in
  [implementation status](../IMPLEMENTATION_STATUS.md). Targeted SDK tests were
  rerun after the final balance/example additions.
- Four independent wrapping fixtures match JS and Rust; the pinned Go oracle
  verifies protobuf, signing digest, signed bytes, transaction hash and signature
  for all four. [Retained report](../test-infra/go-oracle/WRAPPING-RESULTS.json).
- Four wrapper ABI calls match JS, with separate native-value, atom/Qit rounding,
  overflow, zone, trim-loss and destination-gas regressions.
- New regressions cover mixed key origins, channel scans/idempotent recovery,
  conversion nonce preparation, specialized restart and full backup, reorg claim
  retention, unsigned nonce-gap repair, sweep and cross-zone payloads.
- [Read-only deployment observation](../test-infra/reports/wrapper-deployments-2026-09-12.json):
  chain ID 9 mainnet reports 7,255 bytes for WQI and 2,029 bytes for WQUAI;
  Orchard returned HTTP 403. No transaction was submitted during those probes.

The source-only JS `deepScan` and `setAddressStatus` remain absent from the pinned
compiled ESM/declarations. Rust deep scanning is an explicit supported operation;
source-only methods are not silently added to the published declaration baseline.

Feature completeness requires implementation plus demonstrated behavior for each
in-scope workflow. The outstanding items above remain tracked rather than being
hidden behind generic signing, ABI support or passing offline tests.


Account custody now has a portable journal and browser CAS adapter, with native
and actual worker tests for signature-before-return persistence, competing nonce
allocations, cancellation and stale canonical observations. Account-only backup capture and live merge now preserve custody;
Qi custody now covers durable input claims and all three signed Qi forms;
account/Qi backup capture and live merge preserve custody, while complete
browser wallet capture across all journals remains open. See [the account workflow](BROWSER_ACCOUNT_CUSTODY.md) and
[retained validation](../test-infra/reports/browser-account-custody-2026-09-13.json).

The latest [Orchard read-only recheck](../test-infra/reports/orchard-read-recheck-2026-09-13.json)
passed two SDK tests. It observed WQI code but empty code at the configured WQUAI
address; the faucet hostname still failed DNS resolution. These findings preserve
the separate funded wrapper and pinned-node qualification gates.

[Browser Qi custody](BROWSER_QI_CUSTODY.md) connects current gap/deep discovery
to revision-fenced input reservations and local mixed-origin signing. Its independent
codec and actual worker checks are retained in the [Qi custody report](../test-infra/reports/browser-qi-custody-2026-09-13.json).

Qi-only authenticated capture and live merge now retain exact claims and signed
candidate families, reject conflicting or over-capacity unions atomically, and
restore encrypted captures to native SQLite. See [retained backup validation](../test-infra/reports/browser-qi-backup-2026-09-13.json).

[Combined portable recovery capture](PORTABLE_WALLET_CAPTURE.md) now preserves
HD/account/Qi/payment cursor, custody and exposure state in one authenticated
backup. Detached input collection still requires caller-established consistency;
allocator request-ID history is outside the existing recovery format.

Portable capture now carries authenticated prior address/payment inventory forward,
preserving receive-key derivation proofs and maximum cursor floors after recovery.
Transaction custody remains explicitly supplied by all current account/Qi journals.

The browser backup collector now reads all explicitly selected HD/account/Qi/payment
journals under a common revision-checked snapshot. It rejects concurrent mutation
without retry and returns a guarded backup plus revision evidence. Coordinated live
restore remains separate from read-only collection.

HD/payment journals now support authenticated live floor merge with retained IDs,
completed addresses/exposures, abandoned pending work and strict versioned history.
Both browser adapters use CAS. See [allocation recovery](BROWSER_ALLOCATION_RESTORE.md).

September 13 follow-up: selected browser HD/payment/account/Qi live backup merges
now commit atomically across one named IndexedDB database, with cancellation,
conflict and bounded-capacity tests. See [atomic restore](BROWSER_ATOMIC_RESTORE.md).
The [consolidated SDK guide](../SDK_DOCUMENTATION.md) and
[parity analysis](../SDK_PARITY_ANALYSIS.md) retain explicit unfinished workflow,
declaration-review and qualification work.
