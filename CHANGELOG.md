# Changelog

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
