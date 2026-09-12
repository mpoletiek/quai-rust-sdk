# Wallet SDK gaps and completion criteria

Updated 2026-09-11 after validating the user-supplied LAN mainnet node.

**The target is a feature-complete SDK for both Quai and Qi wallet applications.**
Keys, HD derivation, offline signing, seed backup, ordinary Qi selection, typed
wallet reads and durable state are implemented. End-to-end recovery, remaining
protocol operations and release qualification are unfinished.

The [audited plan](../QUAI_RUST_SDK_PLAN.md) remains the complete specification.
The 3,928-row [parity tracker](../compatibility/parity.json) retains every reference
declaration. It distinguishes implementation from qualification; declaration
counts are not a defensible percentage of engineering or security completion.

## Capability gaps

| Area | Available now | Missing before wallet completion |
|---|---|---|
| Addresses/amounts | Checksums, public-key address/recovery, exact decimals and contract prediction | Remaining byte/encoding/fixed-number utilities; higher-level deployment lifecycle |
| Keys/identity | OS key entropy, ECDSA/Schnorr/ordered local aggregation, all BIP39 wordlists/passphrases, BIP32 | Broader browser/platform qualification and independent security review |
| Quai wallets | Local/watch-only signing, coin 994 HD/grinding, message/typed data; durable prepared account workflow | Imported-wallet lifecycle, gap repair/replacement reconciliation, funded Orchard workflows |
| Quai transactions | Canonical protobuf, nonce claims, offline signing, exact stored broadcast, confirmation polling | Stateful replacement/drop/reorg reconciliation, ETX lifecycle, full supported extensions |
| Qi wallets | Coin 969 HD/watch-only, receive/change derivation and explicit indices | History-aware discovery/deep scan/sync and atomic exposed-index allocation in progress; history and reorg recovery |
| Qi transactions | Outpoints/denominations, strict ordinary wire/signatures, multi-input local MuSig, selection/fee loop | Integrated selection/change/reservation/send in progress; trim profiles and unmodified/testnet acceptance |
| Payment codes/conversion | BIP47 codes, owned channel metadata and exact JS/independent derivation vectors | Seed-owned durable channels/backups pass; imported private payment origins, Quai↔Qi conversion, wrapping/redemption, slippage/refund/maturity/recovery |
| Backup/persistence | Authenticated seed and full-wallet native backups; seed/master/imported-key origins, monotonic claims/cursors, signed bytes and checkpoint invalidation | Expanded payment-key origins, broader JS keystore interchange, browser persistence and complete reorg recovery |
| Wallet RPC | Typed headers/genesis/transactions/ETXs/receipts/calls/outpoints, nonce/fees, exact broadcast | Remaining full RPCs, batches/custom authentication, conversion methods, capability/fork profiles |
| Live wallet state | Native WS tested on LAN/Orchard, bounded subscriptions and explicit termination, canonical receipt polling | Reconnect/backfill, complete pending/replaced/dropped/reorg state machine and cross-zone tracking |
| Contracts/dapps | Bounded ABI/EIP-712, ERC-20 calls/intents, events, CREATE grinding with required access list, verified injected message/typed signatures | Integrated deployment/send tracking, real injected-extension qualification and broader dapp workflows |
| Platforms/release | Linux native tests, JS+Go differential oracles, real reads/subscriptions | Real Chromium Fetch/injected/HD-signing worker tests pass; Windows/macOS, unmodified/testnet acceptance, fuzz/fault/reorg/soak, benchmarks, security review and packaging |

History must report the limits of the connected node or required indexer. A
current outpoint query cannot establish historical address use or recover fully
spent gaps. Missing history/index capabilities must not look like full recovery.

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

## Implementation order

1. Retain the LAN profile, expand identity/capability checks, and create the
   per-behavior tracker. Complete browser feasibility alongside native work.
2. Freeze consensus codecs/preimages and crypto vectors; implement safe keys,
   mnemonic/HD derivation and message signing for both ledgers.
3. Deliver an end-to-end Quai wallet with secure backup, nonce management,
   signing/send/tracking and contracts/tokens, qualified on testnet.
4. Deliver Qi signing/selection, durable UTXO recovery, payment codes and
   conversion, including multi-input and restart/reorg acceptance tests.
5. Complete provider/WS/browser parity, all reference examples, cross-platform
   and security/performance gates; reconcile every tracker row before beta.

Existing passing tests cover the implemented foundation, not missing wallet
behavior. See [implementation status](../IMPLEMENTATION_STATUS.md) and the
[dependency audit](dependency-audit.md) for current evidence and open warnings.
