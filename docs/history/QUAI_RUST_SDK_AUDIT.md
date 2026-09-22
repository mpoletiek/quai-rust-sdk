# Quai Rust SDK plan audit

This is the historical pre-implementation plan audit from 2026-09-11. Its
statements about no Rust implementation describe that point in time. For the
current code review and remaining features, see the
[2026-09-12 completeness review](FEATURE_COMPLETENESS_REVIEW_2026-09-12.md)
and [implementation status](../../IMPLEMENTATION_STATUS.md).

## Verdict

The original plan had a sound architecture and release structure, but it was not sufficiently specific about several behaviors that can cause incorrect signing, lost wallet recovery state, secret disclosure, or misleading test qualification. The audit identified **nine high-priority planning gaps and five medium-priority gaps**. The [revised plan](QUAI_RUST_SDK_PLAN.md) addresses all fourteen as requirements, tests or feasibility gates.

The revised plan is suitable for starting Phase 0. Full buildout estimates and compatibility claims remain conditional on a working mature local harness, candidate-node transaction acceptance, and the signing/conversion feasibility work. No Rust implementation exists yet, and revising a requirement does not fix the upstream SDK or establish that a future Rust implementation is secure.

The assessment combines three parallel reviews—protocol/RPC, wallet/security, and architecture/delivery—with direct source checks and offline execution of the published JS package. Severity describes the consequences of following the incomplete plan; it is not an assigned CVE severity or a claim that these behaviors were exploited.

## Evidence and limits

The original plan was audited on 2026-09-11. Its SHA-256 before revision was `833cf58d1800d5fc9685fd9c89310d2783c8497affa606c7925cbb63ccf13fab`. Original line references below identify that version; revised section references identify where corrections now reside.

| Reference | Evidence |
|---|---|
| quais.js source | Commit `94e32c7eb9960de36054135c40a341c44c84f922` |
| Published JS | npm `quais@1.0.0-alpha.57`; downloaded tarball verified against the registry's SHA-512 integrity |
| Source/artifact comparison | All **153 shipped source files** equal the checkout byte-for-byte; no differing or missing shipped source files |
| Tarball SHA-256 | `d45104f6db1b185cecb6765560c79e7dd924a835d2c7420049af53bd79030bde` |
| Node source | go-quai `v0.56.0`, commit `f3f345c877300c044e3e0081a48bf3cf786fb9cc` |
| Runtime tests | Published ESM SDK, Node `v26.8.1`, temporary isolated installation with package lifecycle scripts disabled |

Only public, deterministic offline test material was used. No wallets were funded, transactions broadcast, local chains started, Rust tests run, or package releases published during this audit. Source assertions about go-quai are not live Orchard acceptance results. The earlier read-only Orchard observations remain limited to what the plan records.

The npm `gitHead` mismatch is no longer a shipped-source uncertainty: the direct comparison resolves that narrower issue. It does **not** establish equivalence of every generated artifact to a rebuild, pin all oracle dependency resolutions, or prove JS/node interoperability.[^1]

## High-priority findings

### A01. Imported Qi address metadata contains a private key

**Original plan:** lines 137, 181, 330 and 340–342. **Correction:** revised §§3.3, 7.3 and 9.4.

The plan separated wallet serialization from public discovery state but did not recognize that the reference's “address metadata” itself can be secret. `PrivatekeyQiWallet.importPrivateKey` stores the raw key in `QiAddressInfo.derivationPath` and returns that record. Serialization includes it, and outpoint callback records can copy it. Invalid-format errors can also interpolate the submitted key.[^2]

The offline test confirmed the returned metadata and serialized address entry both contain the public test key. Rust must replace this overloaded field with a typed public origin and opaque key ID. Only an explicit secret interchange adapter may expose the legacy representation. Canary-secret tests must cover normal returns, callbacks, errors, Debug and tracing.

### A02. Legacy HD export is not a lossless backup

**Original plan:** lines 177, 184, 336–354 and 421. **Correction:** revised §§4 and 7.3.

Legacy serialization retains a phrase but omits its BIP39 passphrase and wordlist. Deserialization defaults those parameters. Seed-only construction is also incompatible with a serializer that unconditionally accesses mnemonic data.[^3]

Offline execution confirmed: a passphrase-bearing Quai wallet restored to a **different xpub**; a French mnemonic failed with `INVALID_ARGUMENT`; seed-only export threw `TypeError`. The revised plan requires a lossless native backup format, a legacy capability matrix, rejection of lossy exports, explicit recovery parameters where needed and verification of derived wallet identity. Unqualified JS↔Rust round-trip promises have been removed.

### A03. Payment-channel imports require derivation verification

**Original plan:** lines 182–184, 336–354 and 427. **Correction:** revised §7.3 and the tampered-import acceptance case.

The reference verifies BIP44-derived records more strongly than imported payment-channel records. Channel records can pass structural validation without re-deriving the supplied address/public key from the channel/account/index. Records marked available are later reused as outgoing payment destinations.[^4]

This establishes a source path by which tampered legacy state could substitute a valid-format destination under an existing payment-code label; no live payment was attempted. Rust import must re-derive every address/public key, validate origin/ledger/zone, reject conflicting records and commit atomically. A format-valid destination-substitution test is required; malformed-JSON fuzzing alone would miss it.

### A04. Checkpoints without their UTXO snapshot can hide recoverable balances

**Original plan:** lines 181, 184, 342–354 and 427. **Correction:** revised §7.3.

Qi serialization emits address checkpoints but no matching outpoint snapshot. A deserialized wallet starts with an empty outpoint map, while the synchronization path can retain those checkpoints and request only later deltas. An unchanged output created before the checkpoint would not be reconstructed by those deltas.[^5]

An offline constructed-checkpoint test confirmed that restore retains the checkpoint with zero outpoints. This demonstrates the inconsistent restored state; the missed-balance consequence is supported by the synchronization source, not a live-node reproduction. Rust must persist checkpoint and snapshot atomically, or invalidate metadata-only checkpoints and perform a full scan before reporting spendability.

### A05. A current-UTXO gap scan is not complete cold recovery

**Original plan:** lines 181, 336, 354 and 427. **Correction:** revised §7.3.

The inspected scan classifies use from current outpoints in a relevant path. Previously used addresses that have been fully spent can therefore look unused. A gap limit can stop before a later funded address, and a fixed extra deep-scan range does not prove recovery completeness.[^5]

The revised design separates historical use, unspent status, reservation and unknown state. It requires a supported historical-use capability or explicit user-controlled search ranges with an incomplete-coverage result. Tests must place a funded address after more than a gap limit of fully spent receive/change/payment-code addresses. Full backups must retain exposed/reserved derivation bounds.

### A06. Default Qi-to-Quai conversion conflicts with the candidate node

**Original plan:** lines 183, 289, 358–372 and 428. **Correction:** revised §§2, 5.4, 8, 9.4 and Phase 0.

The JS helper defaults to empty conversion options; preparation adds data only when explicitly supplied. The pinned Go validator recognizes ordinary same-zone Qi-to-Quai conversion using **22 bytes** of data: two slippage bytes and a 20-byte Qi refund address. A distinct 20-byte format represents wrapping. JS fee-estimation construction also omits conversion data supplied for the signed transaction.[^6]

This is a source-confirmed incompatibility with the candidate node profile, not a verified failure on deployed Orchard. The revised plan requires a typed conversion builder with tested encoding/bounds, refund ownership/recovery, identical estimated and signed payloads, and success/slippage/refund/maturity/rounding/gas-limit cases. It no longer treats the default JS example as an established node-valid fixture.

### A07. Denomination trimming is missing from wallet lifecycle accounting

**Original plan:** §§4, 7 and 9.4; no trimming requirement appeared. **Correction:** revised §§4, 7.4 and 9.4.

The node defines trim depths for small denominations 0–5 and removes qualifying historical outputs from the UTXO set. An SDK can therefore lose an output from current state without an ordinary spending transaction. Cached balances, recovery and “conservation” checks need to account for that behavior.[^7]

The revised plan requires creation metadata, trimming/removal classification, expiry-aware policy, and boundary, same-block spend/trim, restart and reorg tests. It distinguishes known trimming from an unclassified removal rather than inventing a spender, and limits value-conservation claims to account for protocol removal and rounding.

### A08. A fresh local chain is not immediately a funded write-test environment

**Original plan:** lines 397, 453–464, 526 and 631. **Correction:** revised §10 and Phase 0.

The pinned gas/state-limit logic keeps those limits at zero before the transaction-start threshold, which is **259,200 zone blocks**. Startup also verifies allocation content against a fixed genesis allocation hash and loads allocations for Cyprus-1. Editing a JSON funding fixture and starting containers cannot be assumed sufficient.[^8]

Phase 0 now requires a measured mature-chain strategy: a reproducible snapshot, supported initialized chain, or explicitly accelerated/modified test profile. It must prove known funding keys, spendable Qi, an accepted transfer and deployment, and record preparation/reset times. Accelerated-profile results must remain distinct from unmodified-node qualification. This is a schedule dependency, not just a Docker task.

### A09. Node commit alone does not identify the rules being tested

**Original plan:** lines 36, 434, 453–462 and 578. **Correction:** revised §10 and the release manifest.

SDK-observable behavior branches on active fork state and hierarchy heights. For example, `ConversionLockChangeForkBlock` is 2,237,000 in Prime context, and the unwrap lock helper changes across it. Ordinary conversion locks remain separate. A fresh local chain and mature public chain using the same binary can exercise different behavior.[^8]

Every profile/run manifest now requires relevant fork constants, zone and Prime-terminus context, active rules, and snapshot/patch provenance. Before/at/after tests must cover supported rules. Altering activation heights for fast tests cannot silently count as unmodified-network acceptance.

## Medium-priority findings

### A10. Qi maturity, legal output fields and wire keys need precise rules

**Original plan:** lines 180–183, 300, 362 and 426. **Correction:** revised §§6.1, 7.4 and 9.4.

At lock-height equality, the JS balance predicates overlap while its send filter is stricter; the node permits equality at the candidate block height. User Qi outputs cannot have nonzero locks, but the JS encoder silently writes an empty lock. The node additionally enforces ordinary output/input-address uniqueness and normalizes wire public keys differently from the pass-through JS encoder.[^9]

The revised plan defines one maturity policy with explicit tip/inclusion context, separates observed UTXO locks from user output fields, rejects unsupported intent rather than discarding it, and requires address-uniqueness and compressed/uncompressed key vectors. Conversion exceptions must follow the actual profile.

### A11. Aggregation depends on position within a block

**Original plan:** P12 and acceptance scenario 6. **Correction:** revised §8 and named aggregation-position tests.

The node exempts the first Qi transaction in a block from denomination-preservation checks. Later Qi transactions may not combine smaller denominations into larger ones. That matters directly to the reference aggregation operation.[^9]

Qualification now includes first/later positions, competing aggregation attempts, delayed inclusion and reservation/retry behavior. Mempool acceptance is not evidence of aggregation execution, and ordinary sends must not rely on the exception.

### A12. JS topology helpers disagree with Go expansion dimensions

**Original plan:** P01, route discovery and the discrepancy backlog. **Correction:** revised §§2, 5.3 and 9.4.

For expansions 0–4, Go calculates dimensions `1×1`, `1×2`, `2×2`, `2×3`, and `3×3`; the inspected JS region/zone helpers use different thresholds. Rust cannot blindly copy that schedule while claiming candidate-node correctness.[^10]

The revised plan requires disagreement vectors and uses actual running locations plus the selected protocol schedule. It also names `--node.starting-expansion-num` as a source-backed local feasibility candidate. Successful mining/funding/ETX delivery with that flag remains unverified.

### A13. Conversion estimates do not all accept a historical block selector

**Original plan:** conversion RPC row and line 384. **Correction:** revised §5.4 and §9.1.

`quai_calculateConversionAmount` accepts transaction arguments and uses the current node head. It differs from conversion-rate methods that accept block selection. Fee estimation also has current-head dependencies.[^6]

The method matrix now records that distinction. Exact comparison uses a paused node or captured oracle state for current-head-only calls; public quotes have freshness and revalidation semantics. The SDK must not fabricate a historical RPC parameter or an atomic block association it did not receive.

### A14. Browser feasibility must constrain architecture before Phase 6

**Original plan:** lines 24, 136, 149–164, 532 and 538. **Correction:** revised §3.4 and Phase 0/2.

The original plan recognized non-`Send` browser objects but deferred the runtime boundary for timers, task spawning, CPU work and durable storage. Tokio and Reqwest browser capabilities differ from their native forms, and Cargo features unify across consumers.[^11]

A minimal browser read/cancel/authorize/sign/CPU slice is now an early gate. Target-specific adapters or separate types must establish executor, timer and storage contracts before core interfaces stabilize. Full browser implementation can remain later; the architecture it constrains cannot.

## Offline reproduction results

The published package was installed in an isolated temporary directory with lifecycle scripts disabled. Public mnemonic fixtures were used; no network provider was constructed. The observed output was:

```json
{
  "passphrase_restored_xpub_matches": false,
  "french_legacy_restore": "INVALID_ARGUMENT",
  "seed_only_legacy_export": "TypeError",
  "imported_key_in_returned_metadata": true,
  "imported_key_in_export": true,
  "metadata_only_restore_preserves_checkpoint": true,
  "metadata_only_restore_outpoints": 0
}
```

The key reproduction sequence for wallet-identity loss is:

```javascript
// Run against the pinned quais@1.0.0-alpha.57 artifact, offline.
import { QuaiHDWallet } from 'quais';
const phrase = 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about';
const original = QuaiHDWallet.fromPhrase(phrase, 'audit-public-passphrase');
const restored = await QuaiHDWallet.deserialize(original.serialize());
console.log(original.xPub() === restored.xPub()); // observed: false
```

The checkpoint test inserted a synthetic block-100 checkpoint into an otherwise valid exported address record, then deserialized it. It verified retention with an empty outpoint map; it did not pretend to have funded an actual pre-checkpoint UTXO. The revised plan specifies that necessary live integration test.

## Decisions retained after audit

- Direct `usePathing: false` preserving the supplied endpoint is supported by the reference source. The boolean/string distinction remains correct.
- Protobuf transaction encoding, separate signing digests/transaction IDs, and ordered MuSig key aggregation remain appropriate requirements.
- The proposed package graph is acyclic and its publication order is valid.
- Phase ranges sum correctly to 71–105 engineer-weeks. The calendar estimate is now more clearly labeled provisional because the critical infrastructure work has not been measured.
- Clean consumer/package tests, staged unpublished dependencies, forward fixes/yanks and partial-publication recovery are appropriate.
- Current Trusted Publishing-only and blocked-trigger claims were checked against the official Rust announcement and retained.[^12]
- The original licensing language was appropriately provisional; this audit establishes no additional legal conclusion.
- The existing single-use MuSig nonce requirements were strong. The revised plan clarifies that local multi-input aggregation does not imply a distributed multi-party coordinator.

## Remaining qualification work

All fourteen findings have been addressed **in the plan**. Closure of the underlying engineering risks requires:

1. A pinned, activation-aware funded local harness with actual transfer/deployment and HTTP/WS evidence.
2. Candidate-node acceptance of Rust/JS Quai and Qi bytes, including valid conversion data and fee agreement.
3. The lossless-backup, tampered-import, checkpoint and spent-gap recovery regression suite.
4. Trimming, maturity, aggregation-position, topology and fork-boundary tests.
5. An early browser runtime proof, followed by a measured delivery schedule and independent security review.

The public node's deployed version, supported active fork profile and live acceptance of these edge cases remain unknown. No audited requirement should be marked “implemented” based on this document alone.

## Sources

All repository evidence uses the inspected commits; access date is 2026-09-11. Runtime observations above are direct offline measurements of the verified npm artifact.

[^1]: npm, [quais 1.0.0-alpha.57 metadata](https://registry.npmjs.org/quais/1.0.0-alpha.57); Dominant Strategies, [source package metadata](https://github.com/dominant-strategies/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/package.json). Artifact digest/equality measurements are recorded above.

[^2]: Dominant Strategies, [imported-key metadata, lines 23–65](https://github.com/dominant-strategies/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/src/wallet/qi-wallets/privkey-qi-wallet.ts#L23); [outpoint callback records](https://github.com/dominant-strategies/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/src/wallet/qi-wallets/abstract-qi-wallet.ts#L735).

[^3]: Dominant Strategies, [legacy HD serializer](https://github.com/dominant-strategies/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/src/wallet/abstract-hdwallet.ts#L312), [Quai deserializer](https://github.com/dominant-strategies/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/src/wallet/quai-hdwallet.ts#L181), [Qi deserializer](https://github.com/dominant-strategies/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/src/wallet/qi-hdwallet.ts#L1913).

[^4]: Dominant Strategies, [channel import validation](https://github.com/dominant-strategies/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/src/wallet/qi-hdwallet.ts#L1923), [base address validation](https://github.com/dominant-strategies/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/src/wallet/abstract-hdwallet.ts#L358), [channel destination reuse](https://github.com/dominant-strategies/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/src/wallet/qi-wallets/bip47-paymentchannel.ts#L72), [send destination selection](https://github.com/dominant-strategies/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/src/wallet/qi-hdwallet.ts#L627).

[^5]: Dominant Strategies, [Qi exported fields](https://github.com/dominant-strategies/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/src/wallet/qi-hdwallet.ts#L1870), [checkpoint-based sync](https://github.com/dominant-strategies/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/src/wallet/qi-wallets/abstract-qi-wallet.ts#L607), [delta application](https://github.com/dominant-strategies/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/src/wallet/qi-wallets/abstract-qi-wallet.ts#L691), [outpoint-only scan use](https://github.com/dominant-strategies/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/src/wallet/qi-hdwallet.ts#L1668).

[^6]: Dominant Strategies, [JS conversion defaults](https://github.com/dominant-strategies/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/src/wallet/qi-hdwallet.ts#L564), [preparation and fee estimation](https://github.com/dominant-strategies/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/src/wallet/qi-hdwallet.ts#L1182), [Go data validation](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/core/state_processor.go#L1590), [conversion recognition](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/core/state_processor.go#L1715), [current-head estimate](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/internal/quaiapi/quai_api.go#L1685).

[^7]: Dominant Strategies, [denomination trim depths](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/core/types/utxo.go#L55), [UTXO trimming](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/core/headerchain_validation.go#L1032).

[^8]: Dominant Strategies, [activation/fork constants](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/params/protocol_params.go), [gas activation](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/core/block_validator.go#L437), [state activation](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/consensus/misc/statefee.go#L9), [allocation configuration](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/cmd/utils/flags.go#L1703), [allocation integrity check](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/params/genesis_alloc.go#L95).

[^9]: Dominant Strategies, [JS locked-balance predicate](https://github.com/dominant-strategies/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/src/wallet/qi-wallets/abstract-qi-wallet.ts#L360), [JS spendable predicate](https://github.com/dominant-strategies/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/src/wallet/qi-wallets/abstract-qi-wallet.ts#L410), [JS send filter](https://github.com/dominant-strategies/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/src/wallet/qi-hdwallet.ts#L878), [JS wire output/key handling](https://github.com/dominant-strategies/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/src/transaction/qi-transaction.ts#L249), [Go validation](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/core/state_processor.go#L1612), [Go denomination exception/check](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/core/state_processor.go#L2139), [Go public-key normalization](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/core/types/utxo.go#L108).

[^10]: Dominant Strategies, [Go hierarchy dimensions](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/common/types.go#L749), [JS expansion helpers](https://github.com/dominant-strategies/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/src/providers/abstract-provider.ts#L1144), [starting-expansion flag](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/cmd/utils/flags.go#L623).

[^11]: Maintainer documentation: [Tokio WASM support](https://docs.rs/tokio/latest/tokio/#wasm-support), [Reqwest WASM support](https://docs.rs/reqwest/latest/reqwest/#wasm), [Cargo feature unification](https://doc.rust-lang.org/cargo/reference/features.html#feature-unification).

[^12]: crates.io team, [development update, 2026-01-21](https://blog.rust-lang.org/2026/01/21/crates-io-development-update/). Trusted Publishing-only mode and prohibited workflow triggers.
