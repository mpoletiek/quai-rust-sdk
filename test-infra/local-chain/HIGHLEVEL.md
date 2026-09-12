# High-level wallet-session acceptance

The bounded high-level Qi test succeeded on the ordinary disposable profile. The high-level account test failed safely during a pending-state RPC preflight and remains a release blocker. This is development-chain evidence, not a claim of production wallet readiness.

## Qi session result

Public seed `07` repeated 32 times derives the test HD Qi account. The already-public bootstrap key funded two derived receive addresses with 10,000 Qits each. This funding transaction used synthetic funds and is separate from the wallet fee qualification.

`QiChangePool` allocated and durably burned change ranges. A harness-only observation source scanned every stored receive/change address at quiescent height 4, checking the identical head before and after each latest-only outpoint request. No other writer or miner ran during this scan. The discovery reports explicitly claim current state only; they do not provide historical coverage or a production block-pinned Qi RPC implementation.

`QiSession` selected one 10,000-Qit input for an exact 1,234-Qit payment. The initial payload had 21 outputs and received a 56-Qit quote. Reselecting with that fee yielded 18 outputs and a 48-Qit quote. The bounded monotonic loop retained the reviewed 56-Qit fee, giving exact change of 8,710 Qits. This demonstrates actual fee convergence and an eight-Qit conservative overpayment, not a promise of globally minimal fees.

The session froze and signed the payload, persisted its exact bytes and input claim, and exited. A separate process reopened SQLite, verified the saved bytes/hash and claim, and explicitly broadcast once. The transaction `0x00cf00f60caf9938d15cd5981a36b13a9a0a6bc392e452a6a942a9fa69888d4e` was canonical at height 5 with status 1 and gas 165,800. All 18 recipient/change outputs matched the signed plan. The other funded HD outpoint remained unspent.

A subsequent process recorded the verified inclusion and reopened SQLite again. Reservation state remained `Confirmed`; the exact input claim remained held, as required by the conservative storage model. No automatic claim release, replacement or replay occurred.

An initial discovery attempt scanned unnecessary burned raw-index gaps and was interrupted before reservations or signing. Its 32 allocated change addresses stayed burned. The final bounded scan used explicit known-address intervals and covered all 58 stored addresses, including those abandoned allocations. This interruption was not a transaction submission.

## Account-session blocker

`AccountSession::prepare` called `quai_estimateGas` with the explicit `pending` selector. The node returned -32000, `method handler crashed`; a separate read-only pending balance check also crashed, while pending nonce lookup succeeded. The captured stack reaches `core/state/statedb.go:804` and `internal/quaiapi/api.go:759`. The account database contained **zero reservations and zero signed payloads**, and the RPC trace contains no account submission.

The SDK did not silently replace pending observations with latest state. Therefore this run does not qualify high-level account preparation, deployment reservation/grinding/preparation, or account restart/broadcast against the live node. Earlier low-level deployment acceptance and deterministic high-level tests remain separate evidence. Resolve the pending-state compatibility failure or introduce an explicit, reviewed alternative observation policy before claiming the high-level account flow works on this profile.

## Evidence and reproduction

[highlevel-evidence/summary.json](highlevel-evidence/summary.json) summarizes exact accounting, quotes, durable states and the blocker. The directory also contains public RPC traces, signed-byte records, canonical receipt/transaction, output checks and node error evidence. SQLite databases remain under `/tmp/quai-sdk-highlevel-wallets`; no private key is stored there. Public fixture seeds must never receive public-network funds.

`highlevel_harness.py build` creates an isolated standalone Cargo client with `sqlite` and `abi` enabled. `run --mode MODE` requires an explicit operation. Use `CARGO_HOME=/tmp/quai-rust-cargo-home` in this session and the matching ordinary profile's clean height-3 snapshot. The `init-fund` operation refuses an existing wallet fixture directory; retain prior databases and evidence before arranging a fresh disposable run.

The tested sequence was `init-fund`, one mined block, `account-prepare` (failed safely), `qi-prepare`, process exit, `qi-broadcast`, one mined block, and `verify-qi`. `account-broadcast` and full `verify` were not run because account preparation failed. The source contains the intended account deployment workflow for review, without labeling it live-qualified.

All owned disposable node processes were gracefully stopped after the test. Ordinary and conversion data and funded snapshots remain preserved. Remaining limits include production discovery/index trust, independent security review, unmodified mature-node qualification, fee changes between signing and inclusion, crash during writes/import, and reorganization/reconciliation testing.
