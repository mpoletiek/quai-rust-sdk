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
  Those outputs have reported lock height 7,791,752; full creation does not itself
  establish maturity or a successful spend.
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

Source references: pinned [gas estimator](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/internal/quaiapi/quai_api.go),
[origin ETX creation](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/core/vm/evm.go),
and [destination processing](https://github.com/dominant-strategies/go-quai/blob/f3f345c877300c044e3e0081a48bf3cf786fb9cc/core/state_processor.go).

## Payment-code interoperability

Rust generation/restoration and pinned `quais@1.0.0-alpha.57` agreed on the
116-character payment code and first Qi address. The rejected pasted code was
115 characters: it omitted the `9` in `Hiw9Knwnx`. A successful Pelagus-funded
receive still requires the sender's public payment code and transaction hash.
No sender seed or private key is required. This is not yet a live BIP47 receive
qualification.

## Verification

[Verification record](verification-2026-09-14.json): 620 workspace tests passed
(5 ignored live tests), 15 Chromium worker account/custody tests passed, strict
Clippy and rustdoc passed, reference/guide/107 package mirror checks passed. The
[package rehearsal](packages-2026-09-14.json) built all twelve archives, ran three
consumer tests and compiled native and Wasm consumers without registry upload.
The CI job compiles this funded harness without keys or live execution.
