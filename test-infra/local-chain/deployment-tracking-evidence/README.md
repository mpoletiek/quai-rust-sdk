# Deployment tracking observation

`highlevel_harness.py run --mode verify-deployment` read the existing public
fixture deployment on the documented isolated development profile. It did not
submit a transaction. The signed creation is retained in
`../confirmed-account-evidence/deployment-signed.json`; `rpc.json` contains only
public source responses and `verified.json` records the SDK assertions.

The SDK reconstructed the exact signed candidate from the existing SQLite
wallet, matched its predicted contract and canonical receipt, queried runtime
code at numeric inclusion block 6, compared the expected Keccak hash, rechecked
inclusion and sampled head, saved a compact observation, reopened the database,
and verified the nonce claim was retained. The profile includes documented node
patches and public fixture funding; this is not unmodified-node, Orchard or
mainnet deployment acceptance. Do not fund these fixture keys on public chains.
