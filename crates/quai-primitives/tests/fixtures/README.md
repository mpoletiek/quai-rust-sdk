`accounts.txt` contains only the 259 public checksummed address values from
`quai-network/quais.js` `testcases/accounts.json.gz`, revision
`94e32c7eb9960de36054135c40a341c44c84f922` (MIT licensed). Keys and unrelated
wallet fields were intentionally excluded. This fixture verifies Rust parsing
and formatting against independent, pinned upstream expected outputs.

Source: https://github.com/quai-network/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/testcases/accounts.json.gz

Checksum behavior is specified by the same revision's `src/address/address.ts`.
Zone and shard aliases are from `src/constants/zones.ts` and
`src/constants/shards.ts`; the ledger scope is the high bit of the second byte,
matching `src/address/checks.ts`.
