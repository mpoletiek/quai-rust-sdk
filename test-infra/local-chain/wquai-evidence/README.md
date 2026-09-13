# Isolated WQUAI acceptance

The durable SDK account and wrapper adapters deployed and exercised bytecode
observed at mainnet WQUAI `0x006C3e2AaAE5DB1bCd11A1a097cE572312EADdBB`.
The explorer returned creation/runtime bytes but **no verified source**. Runtime
bytes matched the read-only LAN mainnet node and the retained SHA-256 pin.
This establishes behavior of those bytes on the documented patched development
profile, not source verification, a contract audit or funded Orchard acceptance.

- Isolated chain ID 1337; genesis `0x654e7a894d57de62ec19b9c161cb1c647466278e0565d3e1ba5d806ae6af0aee`.
- Local deployed address `0x001fe06012Ab36517842EFb946cb5fDC655Eb15f`.
- Creation SHA-256 `e670f177b98bb8bd4653a78f295308e31fe70e9ec30b396fd4bbb7d1be1518a8`.
- Runtime SHA-256 `b55a87a8bbabbb5ffd44c6e17f7273c9c22fea6a47a03d58a49b45160bcbc3f6`.
- Public toy scalar 805, exclusively synthetic fixture funds. No mainnet writes.

| Step | Block | Owner WQUAI atoms | Recipient WQUAI atoms | Contract native backing |
| --- | --- | --- | --- | --- |
| Deploy | 8 | 0 | 0 | 0 |
| Deposit | 9 | 123456789 | 0 | 123456789 |
| Approve exactly 1000 | 10 | 123456789 | 0 | 123456789 |
| Transfer | 11 | 123433333 | 23456 | 123456789 |
| Withdraw owner remainder | 12 | 0 | 23456 | 23456 |

Every operation was prepared, persisted and signed before a separate process
broadcast the exact stored bytes. Each mined receipt was checked canonically,
then reconciled into durable state and checked after database reopen. Every step
checked the exact native sender balance change including the receipt gas fee,
allowance/token balances and native backing. Overdraw simulations returned EVM
revert code 3 without claiming another nonce or submitting a transaction.
The aggregate [report](../../reports/wquai-acceptance-2026-09-12.json) hashes every
retained signed record, read/write RPC transcript and verification result.

To repeat, first establish a fresh funded high-level fixture using the existing
harness instructions. These operation IDs are one-use; a database containing
them refuses another prepare. The provider is fixed to loopback and validates
the isolated genesis before any wallet operation.

```sh
python3 test-infra/local-chain/fetch_wrapper_bytecode.py
CARGO_HOME=/your/cached/cargo/home python3 test-infra/local-chain/highlevel_harness.py run --mode wquai-deploy-prepare
CARGO_HOME=/your/cached/cargo/home python3 test-infra/local-chain/highlevel_harness.py run --mode wquai-deploy-broadcast
python3 test-infra/local-chain/harness.py mine --count 1
CARGO_HOME=/your/cached/cargo/home python3 test-infra/local-chain/highlevel_harness.py run --mode wquai-deploy-verify
```

Repeat prepare, broadcast, one bounded mined block and verify for `deposit`,
`approve`, `transfer` and `withdraw`, in that order. The fetch helper only reads
public bytecode and rejects a pin mismatch; it never submits a transaction.
`wrapper_workflow.rs` independently checks both pins before each operation.
Verification can be repeated without resubmitting; snapshots and nonce claims
are not reset automatically. Reference runtime equality is checked at each
operation's numbered canonical inclusion block.
