# Changelog

## Unreleased

Hardening and speed from a full-workspace review.

Security:

- `sign_qi_message` refuses a message that decodes as a Qi transaction with
  inputs (`SignerError::QiTransactionMessage`). Qi messages and single-input
  spends are both BIP340 over Keccak of the raw bytes, so signing such a
  "message" produced a valid signature for that spend.
- Legacy `SqliteStore::allocate_address` gives back the unexamined part of its
  burned range on success. A large `max_attempts` used to skip about
  `max_attempts / 512` matching addresses per allocation (about 190 at
  100,000), so a gap-limited restore stopped before later addresses. Addresses
  already allocated that way are still found by an explicit deep scan.
- `discover_qi` rejects outpoints whose hash is from another zone.
- Secret-handling fixes in mnemonic, keystore and fetch credential paths.

Breaking:

- Every public error enum is `#[non_exhaustive]`; match with a wildcard arm.
  Outcome enums that gate a commit or signing decision (`ScanStop`,
  `WindowStop`, `CanonicalStatus`, candidate statuses) stay exhaustive; see
  `docs/architecture.md`.
- The nine `*Config` structs (`HttpConfig`, `WsConfig`, `FetchConfig`,
  `EventHubConfig`, `WaitConfig`, `CodeWaitConfig`, `BrowserConfig`,
  `BrowserSocketConfig`, `BrowserWaitConfig`), `KdfLimits`, `DeriveLimits`,
  `QiDiscoveryOptions` and `QiScanOptions` are `#[non_exhaustive]`. Build them
  with `Default::default()` or `new` and the `with_*` methods. The three wait
  configs have no default by design, so `new` takes every limit.
- Scan and restore reports (`DiscoveryReport`, `BranchCoverage`,
  `QiScanReport`, `CurrentQiDiscovery`, `ObservedQiBalance`,
  `PaymentScanReport`, `MailboxDiscoveryReport`, `RestoreReport`,
  `AccountMergeReport`, `QiMergeReport`) are `#[non_exhaustive]`.
- Internal `quai-*` dependencies are exact (`=`) requirements. A caret
  requirement on a pre-release also matches later pre-releases, so pinning
  `quai-sdk` exactly did not pin its siblings. Users of 0.1.0-alpha.1 through
  alpha.3 should pin every `quai-*` crate they depend on.
- `refresh_qi_address_book` reports a row missing from a batched read as
  `QiDiscoveryError::IncompleteObservation`, not `ObservationChanged`.

Added:

- `DynTransport`, so one `Provider<DynTransport>` type can hold any native
  transport chosen at runtime.
- `Provider::account_states`, the balance and nonce of up to
  `MAX_ACCOUNT_STATES` accounts in batched, chain-guarded pages.
- `AccountPublic::search_window`, several consecutive usable addresses with
  the branch derived once.
- `ObservationSource::observe_many` for batched sources. Its default calls
  `observe` per address, so existing sources compile unchanged.
- Opt-in `rayon` feature on `quai-wallet` for parallel address grinding.

Changed:

- `AccountSession` and `QiSession` futures are `Send`, so they can run on a
  multi-threaded runtime.
- `scan_qi`, `discover_qi` and the generic `discover` read addresses in
  gap-bounded windows. A completed scan queries exactly what a one-at-a-time
  scan would, so nothing past the gap stop is disclosed. An aborted scan may
  already have read the rest of its window, at most 63 addresses a retry would
  read anyway. `scan_payment_channel` still reads one address at a time.
- `refresh_qi` and `include_known_qi_addresses` read addresses through
  `outpoints_many`, up to 1,024 per call, instead of in pages of eight and one
  at a time respectively.
- Reads carry their chain guard in the same batch as the call. Network
  identity checks compare the chain ID locally, saving a round trip each.
  Batching is supported on HTTP and WebSocket.
- Faster address derivation and signing: k256 generator tables and a
  scan-specific derivation path.

## 0.1.0-alpha.3

Security, stability and wallet-usability fixes from a full project review.

- **Breaking:** `payment_channels::discover_mailbox_channels` now takes a `start`
  index and reports `next_start`. Previously every call processed the first
  `max_channels` announced senders again, so senders beyond that page were never
  scanned; because mailbox announcements are unauthenticated, anyone could hide a
  real sender's channel by announcing 64 codes first.
- Add `QiKeyring::load_payment_channels`, which loads receive keys for every channel
  registered to an owner. A channel left unloaded let coin selection choose its
  outputs and then fail the whole spend.
- Funded mainnet WQI round trip after prime block 2,237,000: wrap, claim, unwrap,
  a 10-block redemption lock, and a spend of the redeemed Qi
  (`test-infra/orchard/mainnet-wqi-roundtrip-2026-09-15.json`).
- Qualification harness: Qi stages load all registered payment channels, and mainnet
  payment amounts use a single Qi denomination.

## 0.1.0-alpha.2

Documentation-only release; no API or behavior changes from `0.1.0-alpha.1`.

- Crate READMEs no longer describe the packages as unpublished, and install
  instructions use the crates.io release with an exact alpha pin.
- Releases are now published by the tag-triggered `release` GitHub Actions
  workflow using crates.io Trusted Publishing.

## 0.1.0-alpha.1 — 2026-09-14

Initial modular Quai/Qi SDK alpha, published to crates.io from commit `bf315ee`
(tag `v0.1.0-alpha.1`) and compared against the published
`quais@1.0.0-alpha.57` artifact. APIs may change during alpha development.

- Add `AccountSession::observe_nonce`, combining durable candidate reconciliation
  with discovery of unregistered same-nonce transactions.
- Add Pelagus-compatible `payment_mailbox` announcements and
  `discover_mailbox_channels`; the SDK discovered and paid a real Pelagus wallet.
- Add `conversion_batch_discount_bps` for the pinned batch-wide conversion discount.
- Accept Qi refund outpoints whose creating ETX hash carries the Quai ledger bit.
- Allocate native change and payment destinations compactly, committing only
  examined children; add `continue_payment_channel`.
- Add explicit mainnet/Orchard WQUAI addresses and correct the Orchard example
  and funded harness to use the verified testnet deployment.
- Correct Quai-to-Qi preparation gas: include origin costs and conservative
  denomination fragmentation after discounts; preserve the raw estimate API.
- Twelve crates cover typed addresses/amounts, crypto, consensus serialization,
  RPC/providers, ABI/contracts, signers, HD wallets, payment codes, keystores,
  browser adapters and the `quai-sdk` facade.
- Qi discovery defaults to 50 matching receive/change addresses, with fixed
  denominations, explicit deep ranges and mixed HD/imported/BIP47 ownership.
- Native and browser workflows support account/Qi preparation, exact fee checks,
  signing, submission, replacements, conversion/wrapping and settlement observations.
- Durable reservations, immutable signed candidates, authenticated full-wallet
  backups and coordinated IndexedDB restore support restart and recovery.
- Utility parity includes ten BIP39 wordlists, public derivation, bounded KDFs,
  curve helpers, threshold aggregation, fixed-point checked/wrapping arithmetic,
  transaction interchange, ABI reflection, typed data and resource fetching.
- Explicit Rust differences correct reference bugs and replace dynamic JavaScript
  lifecycle/coercion with typed values, bounded resources and caller-owned futures.

The [complete SDK guide](SDK_DOCUMENTATION.md) describes usage. The
[current comparison](SDK_PARITY_ANALYSIS.md) records the audited scope, deliberate
omissions, source corrections and useful Rust additions. The
[publishing guide](docs/PUBLISHING.md) describes the registry metadata and ordered
upload procedure. All twelve crates are on crates.io, with API documentation
built on docs.rs.

This alpha is not production-qualified. Unmodified funded-node execution, mature
redemption spend, broader engines/extensions, sustained reliability/fuzz campaigns
and independent security review remain outside the established evidence. Read the
[security policy](SECURITY.md) before integration.
