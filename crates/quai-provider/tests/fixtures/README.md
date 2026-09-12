# Read-only public RPC fixtures

Captured on 2026-09-11 at approximately 20:48 UTC. The JSON files contain public RPC results, observed chain/head/version metadata, and request method/parameters for small state reads. They contain no keys, account credentials or transaction submissions.

| File | Reported chain/version | Content |
|---|---|---|
| `orchard.json` | Chain 15000; `go-quai/v0.34.0-pre-82368aff` | Five state read results and three existing ETX transaction/receipt pairs |
| `lan-mainnet.json` | Chain 9; `go-quai/v0.56.0-f3f345c8` | Five state read results, four existing ETX pairs and one existing Quai account pair |

Orchard was queried at its resolved `/cyprus1` gateway endpoint. The authorized LAN endpoint used direct routing with pathing disabled. Chain identity was checked before the remaining reads. Version strings are server-reported evidence, not authenticated binary attestations. The LAN short commit matches the pinned candidate; the observed Orchard version is substantially older. Captured schema compatibility must not be described as consensus/signature acceptance across both versions.

Capture read `quai_chainId`, `quai_clientVersion`, `quai_blockNumber`, and bounded recent `quai_getBlockByNumber` results. Existing transactions and receipts were read by hash. State reads were `quai_gasPrice`, `quai_getTransactionCount`, `quai_getCode`, `quai_getStorageAt` and `quai_getOutpointsByAddress`; account reads used the captured explicit height. Gas-price and address-index reads are current-head-only and are not an atomic historical snapshot. The zero-address outpoint result is empty and proves no index completeness or funding.

A bounded search of 40 recent LAN blocks found an account transaction but no Qi transaction. The Qi transaction used by Rust tests is a synthetic schema example with placeholder signature/key material; it is deliberately not a cryptographic or node-acceptance vector. Tests mutate captured records to exercise malformed types, hashes, quantities, inclusion, blooms and logs. They preserve the originals in these files.

Both nodes returned 10,240-byte log blooms and ETX receipts whose cumulative gas can be zero while gas used is positive. The account fixture includes a real captured receipt and verifies its typed field handling. These observations establish only the recorded read behavior at capture time.

`lan-mainnet-genesis.json` and `orchard-genesis.json` were captured on 2026-09-11 with the read-only `quai_getHeaderByNumber(["0x0"])` method through the same zone endpoints. Both return a root `woHeader.location` of `0x`, a zero work-object height and zero parent hash. These fixtures validate `Provider::genesis_hash` independently of the ordinary zone-header parser. Their exact hashes are asserted in provider tests; applications must obtain and compare a trusted expected genesis themselves.
