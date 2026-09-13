# Disposable local consensus harness

This directory now contains an executable, funded development-chain harness. Real SDK-signed Quai transfer, contract creation and single-input Qi transfer were included and validated on a local go-quai process. A stopped-node snapshot was restored and its original head, balance, nonce and indexed Qi inventory matched exactly.

**The successful lane uses explicitly patched development consensus and public allocations. It does not qualify an unmodified mature node, Orchard, production fork transitions or production wallet readiness.** The pinned source checkout used by the independent oracle was never modified. The node, source copy, compiler caches, config, ephemeral P2P key and database were isolated under `/tmp/quai-sdk-local-chain`; no LAN node or existing user node was changed.

## Exact profile and evidence

Source: go-quai `f3f345c877300c044e3e0081a48bf3cf786fb9cc` (v0.56.0), compiled with Go `go1.26.5-X:nodwarf5 linux/amd64`, `go build -buildvcs=false -p 4`. `-buildvcs=false` is necessary for the clean `git archive` copy; the commit and binary SHA-256 are recorded separately.

The CLI was taken from this built binary's `start --help`, not Ethereum/geth flags. It runs the hierarchy with `--node.environment local --node.consensus-engine blake3 --node.solo`, binds all network listeners to `127.0.0.1`, disables NAT mapping/telemetry, sets peer limits to zero and enables `--node.index-address-utxos`. The node has no continuous miner. `tools.go` mines a bounded count of real Blake3 proof-of-work headers and submits through the actual `quai_receiveMinedHeader` RPC; it verifies chain 1337 and the exact expected genesis first and refuses non-loopback endpoints.

| Identity/measurement | Observed value |
| --- | --- |
| Development chain ID | 1337 |
| Development genesis | `0x654e7a894d57de62ec19b9c161cb1c647466278e0565d3e1ba5d806ae6af0aee` |
| Verified allocation Blake3 digest | `0x11035b9cfa8bdea8f0274f9d39723f337941659d95972a303384258fe7a8988a` |
| Direct Cyprus-1 HTTP | `http://127.0.0.1:19200` |
| Direct Cyprus-1 WS | `ws://127.0.0.1:18200` |
| Pathing | `false`; exact URLs, no appended shard path |
| Parent-based block gas/state limits | 12,000,000 each on mined development blocks |
| Funded snapshot | Height 3, 883,828 bytes; stopped-node copy 0.272 seconds |
| First clean restart | 4.642 seconds |
| Verified reset copy | 0.111 seconds |
| Restart after reset | 4.640 seconds |

These are one-machine measurements from 2026-09-11, not performance guarantees. The custom HTTP/WS base ports 19001/18001 use the pinned hierarchy's +199 Cyprus-1 rule; default base ports 9001/8001 would yield 9200/8200. Custom loopback ports avoid colliding with unrelated local services.

Machine-readable evidence:

- `unmodified-observation.json`: actual unmodified startup/mining attempt and its concrete failure.
- `development-observation.json`: binary/patch/allocation identities, timings, before/after reset inventory, successful transactions and the failed deployment regression.
- `acceptance-fixtures.json`: captured public node transaction/receipt responses, spent/received Qi outpoint index results, deployed bytecode and contract call result.
- `public-keys.json`: **already-public fixture scalars** 805 (Quai) and 130 (Qi). Never fund these keys on any public network.

## Why the development patch exists

`patch_profile.py` applies reviewed textual changes only to an isolated `/tmp/.../source` copy and refuses an unexpected baseline:

1. Set `TimeToStartTx` from 259,200 zone blocks to zero. Other fork thresholds remain pinned. Genesis gas/state values alone are misleading: the unmodified node's first pending header actually had both limits zero.
2. Replace the shipped allocation JSON with a single public Quai account funded with `10^27` base units at block 1, retain normal allocation-file Blake3 verification, and derive the configured local genesis from this allocation. No unknown allocation account is assumed to have a known key.
3. Add an explicit denomination-14 Qi bootstrap UTXO at block 1, owned by public scalar 130, unlocked at height zero. Its leaf participates in the actual MuHash/root/UTXO count and its entry/address index is committed during real block processing. It is a synthetic consensus allocation, not a historical transaction or mocked RPC output.
4. Guard a concrete upstream Blake3/index bug: `core/bodydb.go:148` selects `engine[types.Kawpow]` even when the engine list has only one Blake3 entry. The development lane skips that donor-workshare index when its engine is unavailable; normal address-UTXO indexing remains enabled.
5. Make the patched binary refuse startup unless environment is local, solo mode is enabled and the P2P bind address is loopback.

The first unmodified run produced a real mined height-1 header with gas/state zero. Its submission returned JSON `null`, then block processing logged a recovered index-out-of-range panic and the header did not become canonical within the bounded wait. The report therefore marks acceptance false. This distinction is why an RPC acknowledgement alone cannot qualify a local harness.

The synthetic Qi bootstrap is deliberately narrow: reorging through its block-1 allocation and rebuilding all bootstrap history/index metadata have not been qualified. All snapshot reset tests restore the complete stopped database. This profile must remain separate from an unmodified snapshot acceptance lane.

## Actual SDK acceptance

The Rust `client.rs` uses the repository's `quai-sdk` facade, typed consensus signing, strict provider broadcast and typed receipt parsing. It hardcodes only the disposable loopback URL and refuses any other genesis.

| Operation | Result |
| --- | --- |
| Quai transfer | Included at height 4; status 1; gas 22,250 |
| Ordinary single-input Qi spend | Included at height 5; status 1; gas 12,800; source outpoint absent and recipient denomination-13 outpoint present |
| Initial contract deployment without access list | Included at height 5; status 0; consumed its 1,000,000 gas limit |
| Corrected deployment with predicted-address access tuple | Included at height 6; status 1; gas 59,737; stored code and call result verified |

The Qi fee estimator returned 4 Qits for the input/output shape. This narrow acceptance fixture intentionally leaves 900,000,000 Qits as its fee to avoid introducing a change/address-selection dependency into this first consensus check. These are synthetic funds. **It does not validate minimal fees, quote sufficiency or efficient Qi wallet selection.** Those require a separate representative funding/selection lane.

The deployment failure established a Quai-specific requirement: the predicted contract address must be present in the access list (`core/vm/evm.go`, `EVM.create`). The corrected transaction used nonce 2, constructor `600a600c600039600a6000f3602a60005260206000f3`, and appended the unused four-byte grind suffix `00000037` (counter 55). Its computed and receipted address was `0x0078dfCDd5E5Ed63cCF8c7C72156EFD47C1E2d4D`, with one access tuple for that address and no storage keys. Stored runtime was `602a60005260206000f3`; actual `quai_call` returned the 32-byte value 42. This uses CREATE; there is no CREATE2 salt. The failed nonce-1 attempt used suffix `0000012d` (counter 301) and an empty access list. Both attempts remain in the evidence.

## Reproduce

Prerequisites are Go, Rust, Python 3.12+ and the pinned source checkout. Go module downloads and Cargo dependency caches are needed for an initial build. No Docker is required. The preparation command refuses to overwrite an existing work directory.

```sh
python3 test-infra/local-chain/harness.py prepare \
  --source /tmp/quai-sdk-research-go --work /tmp/quai-sdk-local-chain-new
python3 test-infra/local-chain/harness.py start --work /tmp/quai-sdk-local-chain-new
```

Keep `start` in its terminal. It writes the exact owned PID, logs and measured startup inventory in the work directory and handles Ctrl-C/SIGTERM with graceful shutdown. In another terminal:

```sh
python3 test-infra/local-chain/harness.py mine --work /tmp/quai-sdk-local-chain-new --count 3
python3 test-infra/local-chain/harness.py probe --work /tmp/quai-sdk-local-chain-new
python3 test-infra/local-chain/harness.py client --work /tmp/quai-sdk-local-chain-new --mode inventory
```

Stop the node with Ctrl-C, capture a clean snapshot, then restart:

```sh
python3 test-infra/local-chain/harness.py snapshot --work /tmp/quai-sdk-local-chain-new
python3 test-infra/local-chain/harness.py start --work /tmp/quai-sdk-local-chain-new
```

With the node running, submit a transaction, mine at most one block and query the printed ID. `deploy` automatically includes the required access tuple; `qi` spends the bootstrap fixture once.

```sh
python3 test-infra/local-chain/harness.py client --work /tmp/quai-sdk-local-chain-new --mode transfer
python3 test-infra/local-chain/harness.py mine --work /tmp/quai-sdk-local-chain-new --count 1
python3 test-infra/local-chain/harness.py client --work /tmp/quai-sdk-local-chain-new --mode receipt --hash TRANSACTION_HASH
```

Repeat with `--mode deploy` and `--mode qi`. Live node processing and mining times can change the resulting hashes. The checked-in report records one exact run, including its failed deployment; it is not a claim that every regenerated run has identical block hashes.

For reset, stop the owned node, run `reset`, then `start` and `probe`. Reset verifies the per-file snapshot manifest, retains the previous data directory, and restores the full snapshot; it never deletes unrelated directories or signals a PID read from an untrusted file.

```sh
python3 test-infra/local-chain/harness.py reset --work /tmp/quai-sdk-local-chain-new
python3 test-infra/local-chain/harness.py start --work /tmp/quai-sdk-local-chain-new
```

The tested run restored exact height-3 head `0x00000bf254b3d8ffc3f40be90ad32ac68802fa218ac5fe54b60b2b6f49bd73df`, nonce 0, `10^27` Quai base units and the original Qi outpoint after progressing to height 6/nonce 3 and spending that outpoint. Snapshot files stay under `/tmp`; only public manifests and responses are checked in.

Three Python harness tests cover owned-directory guards, isolation/port flags and snapshot digest/symlink validation:

```sh
python3 -m unittest discover -s test-infra/local-chain -p 'test_*.py' -v
```

Remaining qualification includes an unmodified mature funded chain, fork boundaries, realistic fee/change selection, broader reset/reorg/soak coverage, complete bootstrap history/index rebuild and profile-specific WebSocket subscription tests. The live LAN and Orchard read/WS checks recorded elsewhere supplement this lane; they do not replace it.

## Multi-input, replacement and abrupt-exit qualification

A second run from the same funded snapshot qualified actual ordered MuSig signing with **three Qi inputs and repeated signer keys**. The public key scalar order was `[2285, 2285, 2209]`; the two scalar-2285 outpoints came from separate real funding transactions, satisfying the node's prohibition on repeated output addresses within one transaction. The result was canonical at height 6, hash `0x00ac00d6b6431c129a36b23686242f5bf7aee7dffeb919507a14ab5349641e18`, status 1, gas 23,400. Its 120,000,000 Qits of inputs became a 100,000,000-Qit recipient output, a 10,000,000-Qit change output and a deliberately generous 10,000,000-Qit fixture fee. The node's shape estimate was 7 Qits; this remains inclusion/signature qualification, not efficient fee selection.

All three inputs and the intermediate funding outputs were absent from the final address index. Recipient scalar 3123/address `0x009cFEbAB1cC20D08e23eb7eA89DcEfAB3349E45` held output index 0/denomination 13; change scalar 3811/address `0x00916958977b54C8B4403DBca74dC434ccd49d28` held index 1/denomination 12. Both outputs were unlocked. The complete receipt/transaction/index assertions are captured in `multi-input-replacement-fixtures.json`.

The same run submitted two explicit account transfers at nonce 0, with values 111 and 222 and gas prices 1,189,822,500,397,110 and 2,379,645,000,794,220. The original hash `0x00080076c0c13ca38be2cf14ca5deb92ac6e5eed15195ede57319b2ad599b142` had no receipt. Its replacement `0x0039002fd6c63ba0384af517d70ca4b361862ade008fe7d852993e3f14c58327` was canonical at height 6/status 1/gas 22,250; the recipient gained exactly 222 base units and the sender nonce became 1. This validates this explicit 2x replacement case, not every txpool bump boundary or persistent-wallet replacement bookkeeping. The earlier failed contract deployment remains the actual reverted-receipt regression.

To repeat from the stopped height-3 snapshot, reset and start the node, then execute these modes in order (mine one block after each group):

```sh
python3 test-infra/local-chain/harness.py client --mode qi-split
python3 test-infra/local-chain/harness.py mine --count 1
python3 test-infra/local-chain/harness.py client --mode qi-duplicate-fund
python3 test-infra/local-chain/harness.py mine --count 1
python3 test-infra/local-chain/harness.py client --mode qi-multi
python3 test-infra/local-chain/harness.py client --mode replacement
python3 test-infra/local-chain/harness.py mine --count 1
python3 test-infra/local-chain/harness.py client --mode qi-inventory
```

Supply the six printed transaction hashes to `capture_qualification.py --split HASH --fund-a HASH --fund-b HASH --multi HASH --original HASH --replacement HASH --output REPORT.json`. This read-only script verifies successful receipts against canonical headers, exact ordered duplicate input keys/outpoints, recipient/change denominations and indices, absent spent inputs, and account replacement balances/nonce. Use `--work` consistently if using a nondefault disposable directory; client execution uses Cargo's offline cache (`CARGO_HOME=/tmp/quai-rust-cargo-home` in this session).

An explicit fault check subsequently restarted the isolated node, verified readiness, then sent SIGKILL to the exact child created by that supervisor. `start --simulate-crash-after-ready` is the reproducible opt-in mode; it never signals a PID loaded from a file. Reopening completed in 4.739 seconds and every captured receipt, transaction, balance, nonce, head and Qi index entry matched the pre-crash capture exactly. See `crash-recovery-observation.json`. This was an abrupt exit of a ready, idle node; interruption during block import, disk faults, reorganization and persistent-wallet reconciliation remain unqualified. The funded height-3 snapshot was restored again afterward.

The separate conversion lane is documented in [CONVERSIONS.md](CONVERSIONS.md), including its distinct genesis, failed initialization attempt, actual conversion/refund/maturity outcomes, changed ETX IDs and receipt-status limitation.

The bounded high-level wallet run is documented in [HIGHLEVEL.md](HIGHLEVEL.md): real Qi fee convergence, process-restart signed-byte custody and exact output accounting passed; live high-level account preparation stopped safely on a pending-state node RPC crash and remains a release blocker. All owned disposable nodes were stopped after that run.

The owned development harness enables `quai,txpool` on its loopback HTTP/WS
endpoints for the read-only `inspect_pool` SDK example. This does not change
consensus or fund a transaction; the node remains an isolated patched profile.
The 2026-09-12 read reported height 7, empty account/Qi pools, a 1018-byte pending
header and a 21000-gas zero-value access-list simulation.

The [WQI flow](wqi-evidence/README.md) records native wrap, claim, redemption and
locked wallet credit, including failed empty-access-list controls and SDK fixes.
