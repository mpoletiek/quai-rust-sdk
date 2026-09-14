# Funded Orchard qualification

This harness uses real development wallets on the public Orchard network. It is
separate from the isolated-chain toy-key clients. The endpoint is fixed to
`https://orchard.rpc.quai.network/cyprus1`, chain ID 15000 and genesis
`0x663a73416275109a01aad3a4c29ea9e310aded63c5eea491243b7312ad8cd16b`.
No mainnet writes or automatic broadcast retries are provided.

## Local inputs and stages

Keep `/tmp/quai-sdk-orchard-secrets` private (mode 0700). Put two owned, funded
Cyprus-1 QUAI accounts in `wallets.json` (mode 0600), with this shape:

```json
{"network":"orchard","wallets":[{"address":"ACCOUNT_A","private_key":"LOCAL_KEY_A"},{"address":"ACCOUNT_B","private_key":"LOCAL_KEY_B"}]}
```

`generate` creates a fresh 24-word Qi wallet once in `qi-wallet.json` (0600),
restores its payment code, and prints only public metadata. Never commit or share
the mnemonic, private keys, or local custody state. The supplied qualification
directory, including SQLite state and public allocation metadata, must be retained
to spend or recover the recorded outputs. Qualification allocations deliberately
burn bounded ranges; early 100,000-child receive allocations can exceed a normal
50-address recovery gap. Seed-only default-gap recovery is not sufficient for
this test inventory: retain the stored addresses/cursors or use explicit deep
ranges. None of those burned ranges should be reset after a failed preparation.

Build from the repository root:

```sh
CARGO_TARGET_DIR="$PWD/target" cargo build --manifest-path test-infra/orchard/Cargo.toml --locked
```

For extended gap scans, add `--release` and use `target/release/quai-orchard-setup`.
Optimized builds substantially reduce local payment-key derivation time; both
builds use the same private wallet files and durable stores.

Invoke `target/debug/quai-orchard-setup` with exactly one stage. `inspect` performs
account ownership and public RPC checks. `prepare OP` reviews and durably signs
without sending; `broadcast OP` submits those exact bytes once; `observe OP`
checks two confirmations and reconstructs the mined signed account bytes.
Operations `transfer-a-b` and `transfer-b-a` each send 0.01 QUAI. The conversion
operations each send the pinned 10 QUAI minimum with 1% slippage. Account fees
are capped at 500,000 gas, 100 gwei/gas and 0.01 QUAI total; actual reviewed limits
are lower. `PinnedLatest` is explicit because Orchard pending balance reads fail.

`convert-quai-to-qi` uses the generated receive address. Before preparing
`convert-quai-to-qi-v2`, run `qi-allocate-conversion` once; it durably allocates a
fresh address and writes public `conversion-v2-recipient.json`. V2 is a distinct
operation/reservation, not a retry of the first signed transaction. `credit OP`
scans a bounded 16-block destination range and attributes current output locks.
It saves a continuation cursor only when no execution was found. A caught-up
scan is not proof that no later execution will occur.

`qi-prepare` allocates a fresh self-payment recipient and change pool, refreshes
known addresses (up to three attempts if the head changes), and prepares/signs 1 Qi with a maximum 0.1 Qi fee. Each allocation
burns a bounded HD range even if later preparation fails. `qi-broadcast` submits
once; `qi-observe` checks receipt confirmations and current output indexing.
These commands are explicit funded actions. Inspect the prepared public output
before running any broadcast stage. Failed or interrupted sends retain signed
claims; inspect the hash before taking any further action. Confirmed operations
and consumed reservation IDs cannot be reused.

`prepare/broadcast/observe wquai-deposit` and `wquai-withdraw` exercise a 0.01
QUAI round trip using distinct account-A reservation IDs 4 and 5. Each prepare
checks code/genesis and the expected token balance before reserving/signing.
Withdrawal attaches zero native value. Observations check the minted/burned
amount at the receipt block. `target/debug/wquai_probe` performs only public
code/metadata reads at the configured Orchard deployment.

The harness is Unix-specific and intentionally fixed to this qualification
sequence. `diagnostic` reads the original conversion's public destination receipt,
outpoints and code at both confirmed wrapper addresses. It does not accept
arbitrary RPC methods, external recipients, amounts or network overrides.

## WQI and local payment-channel stages

`generate-peer` and `generate-redemption` each create a distinct private wallet
once, using the same private directory and restore check as `generate`.
`qi-extended OP STAGE` supports `wrap`, `payment-send`, `payment-return` and
`redemption-spend`, with separate `prepare`, `broadcast` and `observe` stages.
Each broadcast stage submits exact persisted bytes and refuses a mined hash.
No automatic submission retries occur; inspect the saved hash after any error.

- `wrap` deposits 1 Qi backing for account A at WQI, with an explicit 0.1 Qi
  qualification fee. Orchard's observed prime terminus does not meet the
  specialized estimator's pinned activation profile. `credit` checks the
  destination execution and unclaimed backing before `prepare/broadcast/observe
  wqi-claim` claims exactly 1 WQI.
- `prepare/broadcast/observe wqi-unwrap` redeems 1 WQI to a fresh address in
  `qi-redemption.sqlite`, with 30,000 destination gas. `credit wqi-unwrap`
  reads the actual lock. After maturity, `qi-extended redemption-spend` sends
  0.5 Qi from this separately funded wallet through a payment-code address.
- `payment-send` sends 5 Qi from the original wallet to the peer code.
  `qi-extended payment-return scan` recovers the receiving channel with the
  default 50-address gap, using the receiver seed and sender public code.
  `payment-return` then sends 1 Qi back using the recovered BIP47 input key;
  `qi-extended payment-send scan` discovers it in the original wallet.
- `channels` prints only public code/address/index information for comparison
  with the pinned JavaScript SDK. Local two-wallet acceptance is separate from
  testing a real Pelagus extension.

Original-wallet Qi reservation IDs 2 and 3 belong to wrap and payment-send.
Peer and redemption wallets each use ID 1 in their separate stores. Account-A
IDs 6 and 7 belong to WQI claim and redemption. All are one-use. Payment fees
are capped at 0.1 Qi; the 5 Qi send allocates up to 24 change addresses because
fixed-denomination change can require more than 16. Failed allocations and
preparations retain burned ranges. Never reset these stores or reuse their IDs.

## Observed results

See [funded account evidence](funded-accounts-2026-09-14.json) and
[conversion evidence](conversions-2026-09-14.json) and
[mature Qi spend](qi-spend-2026-09-14.json). These are trusted-node
observations, not independent consensus proofs or general network qualification.

- Two 0.01 QUAI transfers executed successfully in opposite directions, with
  exact mined signed bytes matching SQLite custody. Combined fees: 0.0000504 QUAI.
- The original 10 QUAI conversion used the former raw-estimate-plus-10% path.
  Origin execution succeeded, but the destination failed after creating 385,000
  Qits in eight outputs, locked until height 7,791,644. The final ETX value was
  386,286 Qits; 1,286 Qits were not observed in outputs.
- Its gas limit was 141,900; origin processing consumed 42,020 before forwarding
  99,880. The raw nominal quote was 387,060 Qits, whose output decomposition differs
  from the discounted settlement's 17 outputs. Destination execution needed
  174,000 gas under the pinned schedule. The receipt consumed all forwarded gas
  and logged the partial 385,000-Qit credit. This matches pinned go-quai's
  gas-exhaustion branch; it does not establish the live node's source revision.
- The SDK now budgets the greater of the raw estimate and a denomination-count
  bound below the nominal quote, then adds origin costs and applies the caller's
  margin. For this observation the final limit is 346,522. Raw RPC estimation
  remains available unchanged. Missing quotes and insufficient caps fail before
  nonce reservation. Later quote increases and different gas schedules remain
  risks; fee estimates cannot guarantee full settlement.
- The corrected conversion executed at destination block 7,791,652 and created
  all 386,286 settled Qits in 17 outputs, with zero unobserved Qits. Destination
  hash: `0x000c00f4a354df58b18bee045356b0e175f7d8151092b259a458eb52ad04c9a7`.
  Those outputs have reported lock height 7,791,752. The [unlock recheck](conversion-unlocks-2026-09-14.json)
  at height 7,794,731 found all 17 outputs present and unlocked (386.286 Qi).
  The original conversion retained seven unlocked outputs totaling 380 Qi after
  its recorded 5 Qi spend. Five self-transfer outputs totaling 4.5 Qi remained
  indexed; eleven small outputs totaling 0.492 Qi were no longer returned.
  Latest-index absence alone does not distinguish spending, trimming or index
  behavior. These observations do not claim a new spend of the corrected outputs.
- A matured 5 Qi output from the original conversion was spent in a successful
  1 Qi self-transfer at block 7,791,681. Its other outputs return 3.992 Qi change;
  the exact fee is 0.008 Qi. Four confirmations and all 16 fresh output addresses
  were checked. Hash: `0x00ff00d2c61e20af1f794827be205828e982eed048e02bbacb2ffd9a427421e5`.
  This proves spendability of that converted output, including after the original
  destination failure. It does not recover the original uncreated 1,286 Qits.
- The earlier shared-address configuration queried mainnet WQUAI on Orchard,
  returning empty code at height 7,791,593. The user subsequently supplied the
  correct Orchard address: `0x005c46f661Baef20671943f2b4c087Df3E7CEb13`.
  [Deployment evidence](wquai-deployment-2026-09-14.json) records 2,029 runtime
  bytes, name `Wrapped Quai`, symbol `WQUAI`, 18 decimals and a checked binding.
  Both WQI and corrected WQUAI passed the SDK's live deployment preflight.
- A [funded WQUAI round trip](wquai-roundtrip-2026-09-14.json) deposited 0.01 QUAI,
  minted exactly 0.01 WQUAI, then withdrew the same amount. Both receipts succeeded
  and mined signed bytes matched custody. WQUAI balance returned to zero; the
  native balance decreased only by combined fees of 0.0000987372 QUAI.
  This qualifies the observed Orchard deposit/withdraw flow, not an independent
  contract audit or a mainnet funded test.
- The [funded WQI round trip](wqi-roundtrip-2026-09-14.json) deposited 1 Qi
  backing, claimed 1 WQI, and redeemed it to an isolated owned Qi wallet. The
  destination produced exactly 1 Qi locked until height 7,794,808; the SDK observed
  its transition from locked to unlocked. A confirmed payment spent that exact
  output, sending 0.5 Qi with 0.493 Qi change and a 0.007 Qi fee. Each mined
  transaction's signed bytes matched custody. The wrap used an explicit 0.1 Qi
  fee; automatic specialized fee estimation rejected Orchard's activation profile.
  A 429 during redemption submission was handled by a hash lookup and one explicit
  submission of the same saved bytes; no replacement was created.

Public RPC rate limiting and block-boundary refresh failures prompted bounded
HTTP batching in the SDK. `outpoints_many` now uses transport batches when
available, with checked IDs and per-batch chain identity; other transports fall
back to four concurrent reads. Native wallet refresh retains its head guard.
The large known-address refresh then succeeded on Orchard. This is additional
evidence for explicit grouped reads, not for hidden retries or historical recovery.

Source references: pinned [gas estimator](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/internal/quaiapi/quai_api.go),
[origin ETX creation](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/core/vm/evm.go),
and [destination processing](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/core/state_processor.go).

## Payment-code interoperability

Cross-zone qualification is deferred by user instruction while only Cyprus-1 is
available. Same-zone conversion, wrapping and payment-channel testing continues.

Rust generation/restoration and pinned `quais@1.0.0-alpha.57` agreed on the
116-character payment code and first Qi address. The rejected pasted code was
115 characters: it omitted the `9` in `Hiw9Knwnx`. A successful Pelagus-funded
receive still requires the sender's public payment code and transaction hash.
No sender seed or private key is required. Real Pelagus extension interoperability
remains separate from the local two-wallet qualification below.

Local two-wallet qualification now uses a second generated wallet. The failed
preparations burned four 6,000-child send intervals, putting the eventual payment
at raw child 24,322. An initial gap-50 receive scan correctly stopped at 22,627
without finding it. `qi-extended payment-return scan 22627` explicitly continues
from that cursor. This illustrates why retained allocation metadata and deeper
ranges matter after failed preparations; a gap stop is not proof of no funds.
The [funded payment-code report](payment-codes-2026-09-14.json) records successful
continuation recovery of all 5 Qi, spending that exact BIP47 output in a 1 Qi
return payment, and default gap-50 discovery of the return in the original wallet.
The respective fees were 0.01 Qi and 0.008 Qi. Mined signed bytes matched durable
custody in both directions; pinned quais.js independently matched the actual
funded receive key/address and the return address. Each stage reopened the
private wallet and durable store, so return spending exercised key recovery.

## Verification

[Verification record](verification-2026-09-14.json): 620 workspace tests passed
(5 ignored live tests), 15 Chromium worker account/custody tests passed, strict
Clippy and rustdoc passed, reference/guide/107 package mirror checks passed. The
[package rehearsal](packages-2026-09-14.json) built all twelve archives, ran three
consumer tests and compiled native and Wasm consumers without registry upload.
The CI job compiles this funded harness without keys or live execution.

[Extended verification](extended-verification-2026-09-14.json) records the final
batching/refresh tree: 626 workspace tests passed (5 ignored live tests), all 8
Chromium worker tests passed, strict workspace/harness Clippy and rustdoc passed,
and native/Wasm package rehearsals passed for all twelve archives. The additional
batch tests cover reordered replies, duplicate/missing/foreign IDs, remote errors,
request/response limits, chain changes, bounded concurrency and no automatic replay.

## September 14: reverse conversion, aggregation, sweep and recovery

[Qi-to-Quai evidence](qi-to-quai-2026-09-14.json) records a 1 Qi conversion with
explicit 0.1 Qi fee and 100-basis-point slippage. Origin
`0x009b008ee78133025674ecfe8ecb9be8426f1d89738dfa1132591c1db9223717`
succeeded; the matched destination ETX executed at height 7,795,064 with value
25,761,653,122,291,725 Its. That exact amount was initially locked. It was still
locked at head 7,795,157 and unlocked by 7,795,168; the account balance increased
by exactly that amount and locked balance became zero.

The recipient then sent 0.01 QUAI to the other supplied account. Both original
and fee-only 2× gas-price replacement were durably signed before submission.
The replacement `0x0031005bf5ac208e367dc4ab04b2fb86db4d7e3317438efba65bcea8ea2c2531`
mined at height 7,795,176; the original had no receipt. Mined bytes matched the
saved replacement and the sender's debit equaled value plus 50,400,000,000,000 Its
in fees. This combines exact conversion maturity accounting with a subsequent
spend; fungible account units are not individually traced.

[Transaction scenario evidence](transaction-scenarios-2026-09-14.json) records:

- Aggregation `0x00a900d8acea31b6ff86fda673e51bdfc6d1bb768b617fb0bd9c7af34873496c`:
  46 inputs from 38 distinct addresses, including BIP47 and repeated HD keys;
  24,576 Qits became 26 outputs totaling 24,561 Qits with a 15-Qit fee. It was
  verified as the first Qi transaction in its canonical block; all outputs matched.
- Sweep `0x008f00de8d397aef23cd32e94d7eb3c70ccf0ac5ea1c8a1a7899a7ac9a0c4ca1`:
  15 inputs totaling 3,992 Qits became 16 denomination-preserving outputs totaling
  3,984 Qits. The explicit `broadcast-lost-ack` harness stage forwarded one request,
  withheld its successful response, and returned timeout. A new process reconciled
  the successful inclusion without rebroadcast; all 15 claims and exact bytes remained.

New explicit operations are `qi-extended convert-qi-to-quai`, `aggregate`, and
`sweep`, with `prepare`, `broadcast`, `observe`, `inventory`, and `reconcile`
stages as applicable; conversion also supports `credit`. Account operation
`conversion-credit-spend` adds `prepare-replacement`, `broadcast-family`, and
`observe-family`. Every recorded reservation has been consumed; rerunning
preparation against these stores must fail. Do not delete state to repeat tests.
Use fresh operation IDs and fresh destinations for a subsequent authorized run.

Native compact allocation was used for the new change pools. Earlier burned
ranges were preserved. Tests cover cancellation before exposure, restart,
concurrent allocators, and continuation beyond a persisted empty gap. This reduces
avoidable gaps without claiming that seed-only gap-50 scans recover every wallet.

[Isolated refund](../local-chain/refund-2026-09-14.json) and
[rollback](../local-chain/recovery-2026-09-14.json) checks supplement Orchard.
Automatic specialized Orchard fee estimation, competing-peer reorgs, sustained
fault/load testing, actual Pelagus interoperability and cross-zone execution are
not established by these runs. Cross-zone qualification remains deferred.

## Mainnet qualification (from September 14)

`QUAI_QUALIFICATION_NETWORK=mainnet` selects chain 9, its genesis,
`https://rpc.quai.network/cyprus1` and mainnet WQUAI. Custody lives in the 0700
directory `~/.local/share/quai-sdk-mainnet-qualification`, never `/tmp`. Do not
submit through a node without hashrate. Run stages through bash with `pipefail`
so a failed preparation stops a chain of stages.

[Mainnet record](mainnet-2026-09-14.json) and [checks](mainnet-checks-2026-09-14.json):
transfers, WQUAI deposit/withdraw and token approve/transferFrom/transfer, fee-only
replacement, withheld acknowledgement, unregistered cancellation discovery, a
ground deployment, and six Quai-to-Qi conversions. Batch-wide conversion discounts
from third-party flow refunded three conversions; three credited 3,416 Qits,
locked until blocks 10,338,942, 10,339,066 and 10,339,111. Read-only checks
(`mainnet-check recovery|locked-spend|special-fee|backup|events|interchange`)
verified seed-only recovery, locked exclusion, automatic specialized fees (36 Qits
for a 1 Qi conversion, 33 for a wrap), backup restore and interchange.

`target/release/head_soak` is a detached read-only WebSocket head follower writing
`soak/head-soak.jsonl`; `fork_watch.py` records prime blocks around 2,237,000.
Stop them with `pkill -x head_soak` and `pkill -f fork_watch.py`.

### Unlock-day sequence

Run only after every lock height has passed. Each funded stage is one-use.

1. `qi-extended aggregate inventory`: confirm spendable Qits; 1-Qit outputs may
   be trimmed as they unlock.
2. `qi-extended self-transfer prepare|broadcast|observe` (0.1 Qi).
3. `qi-extended payment-send prepare|broadcast|observe` (0.5 Qi to the peer code),
   `payment-return scan`, `payment-return prepare|broadcast|observe` (0.25 Qi),
   then `payment-send scan`.
4. `qi-extended wrap prepare|broadcast|observe|credit` (1 Qi, estimated fee),
   `prepare|broadcast|observe wqi-claim`, `prepare|broadcast|observe|credit wqi-unwrap`,
   then `qi-extended redemption-spend prepare|broadcast|observe` (0.25 Qi) once the
   10-block post-fork unwrap lock expires.
5. `qi-extended convert-qi-to-quai prepare|broadcast|observe|credit` (1 Qi, 2000
   basis points, estimated fee). Account B's credit is locked for two weeks.
6. `qi-extended aggregate prepare|broadcast|observe`; first-Qi block placement is
   recorded, not guaranteed. `qi-extended sweep` sweeps the peer wallet.

A Pelagus-funded payment-code receive needs the sender's public payment code.
