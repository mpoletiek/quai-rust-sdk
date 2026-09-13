# Signer-free family recovery on the isolated development node

The `verify-replacement` harness mode used `recovery::track_family` to reconstruct
the already-stored account candidate family, observe block 7, save its winner,
and reopen the database to check the same cache revision. No transaction was
submitted by this verification. The original nonce claim remains held.

The node uses the documented isolated development patches and public fixture
funds; this is not unmodified-node, Orchard, finality or production qualification.
See `verified.json` and the complete read-only `rpc.json` transcript.
