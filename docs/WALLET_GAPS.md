# Wallet SDK gaps and completion criteria

Updated 2026-09-12 after reviewing public APIs, tests and pinned reference source.
See the [feature completeness review](FEATURE_COMPLETENESS_REVIEW_2026-09-12.md)
for evidence, priorities and acceptance criteria FC01–FC12. Node observations
below retain their original 2026-09-11 qualification boundary.

**The target is a feature-complete SDK for both Quai and Qi wallet applications.**
Keys, HD derivation, current gap-50 Qi discovery, mixed-origin signing, payment
channels, conversion sessions, wrapping adapters and durable state are implemented. End-to-end recovery, remaining
protocol operations and release qualification are unfinished.

The [audited plan](../QUAI_RUST_SDK_PLAN.md) remains the complete specification.
The 3,928-row [parity tracker](../compatibility/parity.json) retains every reference
declaration. It distinguishes implementation from qualification; declaration
counts are not a defensible percentage of engineering or security completion.

## Capability gaps

| Area | Available now | Missing before wallet completion |
|---|---|---|
| Addresses/amounts | Checksums, public-key address/recovery, exact decimals and contract prediction | Remaining utility mappings; checked fixed-point arithmetic, bounded byte encodings and signed-width conversions are implemented |
| Keys/identity | OS key entropy, ECDSA/Schnorr/ordered local aggregation, all BIP39 wordlists/passphrases, BIP32 | Broader browser/platform qualification and independent security review |
| Quai wallets | Local/watch-only and HD signing, durable account/conversion sessions, master-xprv restore and unsigned nonce-gap repair | Terminal lifecycle and funded Orchard workflows; both-ledger candidate families are durable |
| Quai transactions | Canonical protobuf, nonce claims, offline signing, exact stored broadcast, confirmation polling | Terminal/reorg policy qualification and full supported extensions; candidate families and bounded ETX tracking are implemented |
| Qi wallets | HD/watch-only derivation, current gap-50 receive/change scan, deep ranges, imported/channel refresh, exact balance buckets and atomic exposure allocation | Fully historical recovery and ancestry replay; latest-only RPC limits remain explicit (FC01/03) |
| Qi transactions | Mixed HD/imported/BIP47 signing, exact fee preparation, full denomination capacity, sweep/explicit aggregation, cross-zone Qi preparation and durable recovery | Qualified block placement for aggregation, trim profiles and funded unmodified/testnet acceptance (FC02/08) |
| Payment codes | BIP47 seed/master/account-xprv identities, registered send-to-code and receive gap/deep scan, mixed-origin spending, authenticated seed/master/account-xprv channel backup | Public code exchange remains explicit and out of band (FC07) |
| Quai↔Qi conversions | Explicit types, typed quotes/calculation, durable prepared sessions, signed backup/recovery and bounded conversion/refund ETX correlation | Per-operation Quai maturity and funded qualification; current Qi credit/refund locks and durable observations are implemented (FC04) |
| Qi wrapping/redemption | Native 20-byte wire/signing, durable sessions, WQI backing/claim/ERC-20/redemption adapter and dust/gas guards | Funded mature output acceptance; isolated wrap/claim/redemption through locked wallet credit now passes, including subtype-4/6 attribution and durable observations (FC05) |
| Wrapped Quai | Confirmed address constants, deposit/withdraw adapter, ERC-20 operations and runnable offline example; mainnet code presence observed | Funded Orchard and unmodified-node acceptance plus contract review (FC06); mainnet-bytecode-matched isolated deposit/withdraw, approval and transfer pass |
| Backup/persistence | Authenticated native seed/master/imported-key backups, seed/master payment channels, exact signed conversion/wrapping bytes, monotonic claims/cursors and inclusion invalidation | Complete browser wallet integration and terminal/reorg lifecycle; imported payment-account origins, candidate backups and scoped IndexedDB snapshots are implemented |
| Wallet RPC | Existing typed reads plus conversion rates/calculation, specialized account estimation, wrapped deposits, bounded multi-address outpoints and strict inclusive delta queries | Remaining RPC mapping and verified historical index profiles |
| Live wallet state | Native WS tested on LAN/Orchard, bounded subscriptions, canonical head replay with reconnect, receipt polling | Complete pending/replaced/dropped/reorg wallet state machine and cross-zone tracking |
| Contracts/dapps | Bounded ABI/EIP-712, ERC-20 calls/intents, events, durable account deployment preparation with CREATE grinding/access list, verified injected message/typed signatures | Funded Orchard acceptance, real injected-extension qualification and broader dapp workflows; signed-intent deployment/code observation, bounded waits and durable caches are implemented |
| Platforms/release | Linux native tests, JS+Go differential oracles, real reads/subscriptions | Real Chromium Fetch/injected/HD-signing worker tests pass; Windows/macOS, unmodified/testnet acceptance, fuzz/fault/reorg/soak, benchmarks, security review and packaging |

History must report the limits of the connected node or required indexer. A
current outpoint query cannot establish historical address use or recover fully
spent gaps. Missing history/index capabilities must not look like full recovery.

See [wallet workflows](WALLET_WORKFLOWS.md) for public APIs and runnable examples.

## Wallet acceptance gates

1. **Create/import/restore identity:** JS/Rust vectors agree on seed, extended
   keys, coin type, account, receive/change branch, requested zone, skipped child
   indices, public key and address. Native restore preserves passphrase/wordlist;
   unsupported lossy legacy exports fail explicitly.
2. **Receive and observe:** discover balances/outpoints with capability-aware
   history; classify pending, confirmed, locked, spendable and trimmed values.
   Watch-only workflows cannot accidentally sign.
3. **Authorize exact transactions:** destinations, network, ledger, amounts,
   fees, inputs/change and conversion constraints shown to the application come
   from the same final payload that is signed. Re-estimation cannot silently
   alter an already authorized payload.
4. **Sign and submit:** independent Quai protobuf/ECDSA and Qi Schnorr/ordered
   local MuSig vectors match the supported references and obtain node acceptance.
   Offline signing and broadcasting are separately supported operations.
5. **Track completion:** transfers, replacements, deployments, token operations,
   ETXs and conversions reach correct states; dropped, ambiguous and reorganized
   transactions reconcile without duplicate payment.
6. **Survive restarts/concurrency:** nonce/outpoint reservations, change indices
   and atomic storage survive interruptions. Keys do not appear in ordinary
   metadata/callbacks/logs; Qi checkpoints match their UTXO snapshots; channel
   imports prove ownership through derivation.
7. **Recover backups:** test supported key origins, languages, passphrases,
   imported keys and channels, including interrupted writes, stale checkpoints,
   fully spent address gaps and reorgs.
8. **Ship complete behavior:** map each in-scope reference behavior to a Rust API
   or justified deviation, named tests and runnable examples. Finish platform,
   security and reliability qualification before an unqualified complete/stable claim.

Hardware-device integrations beyond the pinned reference and a distributed
MuSig coordinator are separate extensions. The required local-key, HD,
watch-only and injected-provider workflows remain in scope.

## Verified LAN mainnet node

The first successful check at **2026-09-11 20:40:22 UTC** reported:

| Check | Result |
|---|---|
| Rust HTTP | `http://10.0.0.12:9200`, pathing disabled, chain ID 9 |
| Cyprus-1 header | Height 10,047,036; zone location validated |
| Execution limits | Gas/state both 50,000,000 |
| Prime terminus / expansion | 2,221,544 / 0 |
| Genesis | `0xac81c28f1a72591b87b5f16c9793cdc0e87c45c6d426d1a364c3b8f6386b5b8b`, matching pinned `ProgpowColosseumGenesisHash` |
| Port 9001 reads | Chain ID 9 and running locations `[[0,0]]` |
| WS | Direct Rust WS chain read, real `newHeads` notification and unsubscribe passed on port 8200 |
| Diagnostics | `quai_clientVersion` reports `go-quai/v0.56.0-f3f345c8`; sync/build attestation remains unverified |

The height-zero genesis response has root location `0x`, so it must not be fed
through the ordinary Cyprus-1 execution-header validator. Source comparison:
[pinned genesis configuration](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/params/config.go).

The Rust WS probe establishes subscription compatibility, not reconnect/backfill
or reorg recovery. Reported chain ID, genesis and client version are not software,
fork or synchronization attestations. Orchard reports the older
`go-quai/v0.34.0-pre-82368aff`; keep its acceptance evidence separate.

This node enables mainnet read fixtures, wallet-query/index capability checks
and real WS qualification. Orchard is the intended funded testnet acceptance environment;
funding and transaction acceptance have not yet been completed. An
isolated resettable development network now accepts real Quai/Qi/deployment
transactions with documented patches; it still needs broader double-spend,
fault, reorg and crash-recovery qualification. See [harness evidence](../test-infra/local-chain/README.md). A mature mainnet node does not replace that
environment. No mainnet transaction or wallet mutation was performed.

## Remaining implementation order

1. Supply production Qi discovery and common durable transaction/reorg
   reconciliation (FC01/03).
2. Extend Qi signing origins and complete payment-channel receive/send/spend
   workflows (FC02/07).
3. Complete qualified conversion quotes/fees, durable sessions and settlement
   tracking (FC04).
4. Add Qi wrapping/redemption and supported wrapped Quai contract workflows
   (FC05/06).
5. Close consolidation/cross-zone, account, provider/WS/browser, examples and
   declaration mapping gaps; complete independent release qualification
   (FC08–12).

Existing passing tests cover the implemented foundation, not missing wallet
behavior. See [implementation status](../IMPLEMENTATION_STATUS.md) and the
[dependency audit](dependency-audit.md) for current evidence and open warnings.
