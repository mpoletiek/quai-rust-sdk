# Asks from Quai Terminal, 2026-09-19

Quai Terminal (`~/Devspace/quai-rust-cli-wallet`) runs on `quai-sdk =0.1.0-alpha.4`. A six-part review of the wallet on 2026-09-18 turned up problems whose fix belongs in the SDK, not the wallet. Each ask below was checked against this repository's `next-release` branch (`1155bc2`, alpha.5) before being written down. Asks that turned out to be already solved, or to belong somewhere else, are listed at the end with the reason.

The wallet's review is `docs/REVIEW_2026-09-18.md` in the wallet repo. Finding IDs (STAB-1, TX-5, …) refer to it.

## Summary

| # | Ask | Priority | Why it matters |
|---|---|---|---|
| 1 | Qi stores tolerate a second writer | High | Two processes on one wallet (terminal + daemon) fail each other's Qi refreshes and sends. |
| 2 | Allocated but unused change and payment addresses are not lost | High | Retries and rejected reviews can push change past the gap-50 scan a seed-only restore uses. |
| 3 | A lost observation race is Stale, not Invalid | Medium | Two trackers of one settlement report "conflict" as a hard error. |
| 4 | A domain-separated key between two payment codes | Medium | Apps now reuse the BIP47 notification key for other protocols. |
| 5 | Proxy support in the node transport | Medium (owner decision) | Every node request goes out direct; one IP links every wallet on a machine. |
| 6 | Qi refresh does not rewrite unchanged coins | Low–medium | Every refresh rewrites every coin row and bumps the generation. |
| 7 | Chain codes are wiped on drop | Low | Defence in depth for extended private keys. |
| 8 | One RustCrypto generation | Low | Duplicate crates in every downstream build. |

## 1. Qi stores tolerate a second writer

**What happens.**
- `refresh_qi` reads, checks the tip did not move, then commits with `replace_snapshot` against the generation it read (`qi_discovery.rs`, `refresh_once`).
- If another process committed to the same `qi.sqlite` in between, the generation compare-and-swap fails. The error is `StorageError::StaleSnapshot`, and it surfaces as `QiError::Storage(..)`.
- `REFRESH_ATTEMPTS` retries only a moving tip, not this. The same applies to everything that ends in a snapshot commit, `QiSession::prepare`'s reservation included.
- The two stale cases share one message ("stale or mismatched wallet snapshot") but are different variants, so an app matching `QiError::StaleSnapshot` never retries the concurrent-writer case.

**Evidence.**
- Three concurrent `qi refresh` runs against one wallet copy on mainnet: 18 of 36 failed.
- The wallet now retries on `class() == Stale` with jitter, five times, around every call (wallet commit of 2026-09-19):
  - Two concurrent writers: 24 of 24 succeed.
  - Three writers refreshing back to back: 3 of 36 still exhaust.
- The wallet's daemon log showed these errors whenever the daemon and the terminal refreshed the same wallet together.

**Ask.** When only the generation moved, the SDK should not fail the caller:
- If the stored checkpoint is the same block, the work is already done: return `Ok(stored checkpoint)`.
- If it is newer, return `Ok(stored)`: someone else did newer work.
- If it is older, re-read the generation and commit again. The reads are still valid, because the tip did not move across them.
- Reservation paths (`prepare`, `prepare_special`, `prepare_sweep`) could re-read the snapshot and retry once in the same way.
- Document that callers retry on `class()`, not on a variant.
- Optionally expose the attempt count (`RefreshOptions::attempts`), so an app need not wrap the SDK's three attempts in its own five.

## 2. Allocated but unused change and payment addresses are not lost

**What happens.**
- `QiChangePool` is one-use. A prepare given it by `&mut` takes only what it used, but "a dropped pool's unused addresses remain burned" (`qi.rs`).
- A wallet that sizes a pool, fails on `InsufficientChange`, allocates a bigger pool and retries burns every pool it drops. Doubling from 3 burns about 3 + 6 + 12 + 24 = 45 addresses before success.
- A rejected review burns its whole pool, up to 48 addresses.
- `allocate_payment_destinations` consumes the peer's send indexes when the review is built. A review the user rejects, or a batch dry run, burns them too.

**Why it matters.**
- A seed-only restore, and Pelagus, scan change with gap 50 (`DEFAULT_QI_GAP`). One or two such events leave later change outside that scan, so the funds look lost until a deep scan.
- On the receiving side, payments after more than 50 burned send indexes are not found by the peer's wallet.

**Ask.**
- Record change addresses that were allocated but never put in a signed payload. `QiChangePool::allocate` would hand those out first, before advancing the cursor.
- The same for payment-code destinations: release them with `release_unsigned`, or allocate them when signing rather than when preparing.

The wallet will reuse one pool by `&mut` across its retries in the meantime. That does not cover rejected reviews or process restarts.

## 3. A lost observation race is Stale, not Invalid

**What happens.**
- `compare_exchange_observation_scoped` (`quai-wallet/src/storage/observations.rs`) returns `StorageError::Conflict` when the cache revision moved.
- `StorageError::class()` puts `Conflict` in `Invalid`, deliberately, because `Conflict` also means a mismatched account xpub, which never succeeds on retry.
- So when two trackers follow one settlement (a terminal and a daemon), the one that commits second gets a hard "wallet metadata or reservation conflict". Its only problem was that the other got there first.

**Ask.** A separate error for a lost revision race, for example `StorageError::ObservationRaced`, classed `Stale`. `Conflict` stays for the xpub mismatch. `track_settlement` then reports "re-observe" rather than "invalid".

## 4. A domain-separated key between two payment codes

**What happens.**
- The wallet's sealed messages (the owner's `quai-messages` contract) derive their key as follows:
  1. ECDH between the two payment codes' notification keys, using `PrivatePaymentCode::notification_key` and `ecdh_shared_x`.
  2. HKDF-SHA256 with an app label.
- That reuses the BIP47 notification key, whose job is the mailbox notification, for a second protocol. Every app that wants a pairwise channel between payment codes will do the same.

**Ask.**
- A derivation in the SDK, which owns payment-code keys, for example `PrivatePaymentCode::shared_key(&self, peer: &PaymentCode, domain: &[u8]) -> Zeroizing<[u8; 32]>`.
- It should come from a dedicated branch or a domain-separated KDF, so it is never the notification key's ECDH as-is.
- Include test vectors, so the JS client and the Rust wallet agree.

The message format itself (tags, epochs, padding, and binding the sender address into the AAD) is the messaging protocol's business. It belongs in `quai-messages` with its own spec, not here.

## 5. Proxy support in the node transport

**What happens.** `HttpTransport::new` builds its client with `.no_proxy()`, and nothing in `HttpConfig` sets one. The wallet can route its third-party HTTP through a proxy, but not node RPC.

**New evidence.** The wallet now runs a daemon that polls every wallet on the machine, back to back, from one IP. The RPC operator can therefore link wallets the user keeps separate on purpose.

**Ask.**
- `HttpConfig::with_proxy(url)` (SOCKS5 and HTTP).
- Ideally a per-endpoint or per-caller credential, so Tor's stream isolation can give each wallet its own circuit.

The owner deferred this on 2026-09-17. It is listed so the evidence is recorded; the decision is theirs.

## 6. Qi refresh does not rewrite unchanged coins

**What happens.** Every successful `refresh_once` calls `replace_snapshot`, which deletes and reinserts every coin row and bumps the generation, even when nothing but the checkpoint changed.

**Evidence.**
- The owner's busiest wallet store is at generation 28,426.
- An idle wallet grows its `qi.sqlite` write-ahead log by 49–115 KB a minute.

**Ask.** When the coin set is unchanged, advance only the checkpoint, with the same generation compare-and-swap. This composes with ask 1: fewer commits, fewer races.

## 7. Chain codes are wiped on drop

**What happens.**
- `quai_wallet::ExtendedPrivateKey` is a newtype over `bip32::XPrv`, and `quai-payments` keeps an `XPrv` root.
- `bip32` 0.5.3 wipes the private scalar, but `ExtendedKeyAttrs::chain_code` has no zeroizing drop. The root and account chain codes therefore stay in freed memory after a wallet locks.
- A chain code plus any leaked non-hardened child private key recovers the parent.

**Ask.** A `Drop` on the newtype (and on the payment-code root) that wipes the chain code, or an upstream `ZeroizeOnDrop` for `ExtendedKeyAttrs`.

## 8. One RustCrypto generation

**What happens.**
- `quai-keystore` uses sha2 0.11, hmac 0.13, pbkdf2 0.13 and scrypt 0.12.
- `quai-crypto`, `bip32` and `elliptic-curve` pull in sha2 0.10 and hmac 0.12.
- ark-ff arrives in four versions (0.3–0.6) through the alloy/ruint features, and digest in three.
- The wallet's lockfile has 546 packages, and a cold build takes 9 minutes.

**Ask.** Move the crates onto one generation, and trim the ark-ff features. That should remove roughly 10–15 duplicate crates from every downstream build.

## Checked and dropped

- **Typed coin-selection errors.** Already there: `SelectionError::InsufficientFunds`, `FeeBudgetExceeded` and `LimitExceeded`. The wallet was matching error text instead of these; fixed in the wallet on 2026-09-19.
- **"Is this inclusion still canonical" helper.** `observe_candidates` already returns canonical inclusion with a confirmation depth. Holding an operation open until N confirmations is the wallet tracker's job.
- **Mailbox paging and spam resistance.** Delivered in alpha.5 (`MailboxSource::Logs`/`Senders`, `with_min_funded`). The wallet has to adopt it.
- **Replacement gas headroom.** Delivered in alpha.5 (`estimated_gas()`).
- **Sealed message format and sender binding.** A protocol of the owner's messaging contract; spec and vectors belong in `quai-messages`. Only the key derivation (ask 4) is SDK material.
