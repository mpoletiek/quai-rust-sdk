# Account access discovery acceptance

The 2026-09-13 isolated run starts from an ordinary WQUAI `approve(spender,0)`
contract call with an empty access list. `AccountSession` uses explicit
`AccountAccessListPolicy::Discover` and `PinnedLatest` observation to obtain
access entries, re-estimate the exact call and freeze the final transaction.
A separate process broadcasts the stored bytes without another discovery call.

At block 26, the transaction succeeded: the previous 1000-atom allowance became
zero, the sender's native balance decreased by exactly the receipt gas fee, and
reopened SQLite state retained the same signed bytes and confirmed reservation.
The [report](../../reports/access-discovery-2026-09-13.json) hashes each transcript.
This uses public scalar 805 and synthetic funds on the patched isolated chain
1337, not an unmodified or funded Orchard/mainnet acceptance result.

With the earlier WQUAI fixture at the expected state, run `highlevel_harness.py
run --mode access-prepare`, then `access-broadcast`, mine one bounded block using
`harness.py mine --count 1`, and run `access-verify`. Reservation ID 37 is one-use.
The provider remains fixed to loopback and the known isolated genesis; no wallet
reset or implicit retry occurs. The mode expects the earlier allowance of 1000.

Deterministic account tests additionally cover CREATE identity, missing mandatory
entries, discovery execution errors, changed heads, a nonce advanced by another
reservation, rebuilding from original requirements at the actual nonce, and a
failure after nonce allocation retaining only an unsigned reservation. Explicit
`Preserve` remains the default, and signed replacements/broadcast never repopulate.
