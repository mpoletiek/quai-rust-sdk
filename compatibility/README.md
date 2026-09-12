# JavaScript compatibility reference

This private development package pins the published `quais@1.0.0-alpha.57`
artifact and its complete npm dependency graph. It is not a runtime dependency
of the Rust SDK and must not be published. Installation disables lifecycle
scripts via `.npmrc` and the explicit command below. No upstream `.env`, wallet
files, example project configuration, or production credentials were copied.

From this directory, using Node 20 or newer:

```sh
npm ci --ignore-scripts --cache /tmp/quai-rust-reference-npm-cache
npm run verify
npm run generate
npm test
```

Generation and tests are offline after installation. The initial run used
Node 26.8.1 and TypeScript 5.0.4. Review generated-file diffs whenever changing
the reference; do not automatically accept regenerated expectations after a
failed compatibility test. With the original artifacts available, additionally
verify the tarball and checkout bytes:

```sh
npm run verify -- --tarball /tmp/quais-1.0.0-alpha.57.tgz --source /tmp/quai-sdk-research-quais
```

`reference-lock.json` records tarball SHA-256 and npm SRI, npm `gitHead`, the
separately inspected JS commit, Go candidate commit/version, compiler pin, and
dependency-lock digest. `reference-source-manifest.json` records all 153
published source-file hashes. The inspected checkout matches those bytes; its
commit differs from npm `gitHead`. This is not a claim that the complete npm
release can be rebuilt identically, or that the Go candidate accepts transactions.

`api-inventory.json` uses the TypeScript compiler checker to resolve all 12
shipped export-map roots/subpaths, including aliases, overloads, and inherited
public class members. It records 581 export entries, including 143 class entries
and 3,347 class-member entries. These counts include a symbol exported from more
than one root. The `quais` namespace's names are listed and the same definitions
are represented as root exports. Its scope/limitations are embedded in the file:
this inventories declarations, not behavioral parity, runtime/browser coverage,
or a finished Rust mapping.

`fixtures/primitives.json` contains 65 deterministic vectors for addresses,
Quai message hashing, CREATE addresses, Keccak and decimal-unit conversion.
JSON integer quantities are decimal strings. An expected `{ "error": { "code":
"..." } }` denotes a reference failure. Addresses outside active zones retain
a checksum and ledger but report a null zone. Negative unit values reflect JS
behavior; Rust unsigned amounts may intentionally reject them. Such differences
need explicit policy entries, not weakened assertions.

`fixtures/routing.json` contains 25 URL-construction observations using explicit
shard lists, which avoid automatic network discovery. Boolean `usePathing: false`
preserves the exact URL; `true` appends the shard path while retaining base paths
and query parameters. The string `'false'` is not the boolean configuration.
These fixtures do not qualify discovery, transport, authentication or nodes.

The JSONL oracle accepts only allowlisted public, deterministic operations:

```sh
printf '%s\n' '{"id":"example","operation":"hashMessage","input":{"encoding":"utf8","message":"Hello Quai"}}' | node scripts/oracle.mjs
```

It emits one `{ "id": ..., "result": ... }` response per nonempty line, reports
error codes without echoing input, and has no signing or network operation. Feed
it only public test data. It is development tooling, not a hostile-input service.

`wallet-regressions.test.mjs` reproduces audit A01/A02/A04: imported-key exposure,
passphrase identity loss, French mnemonic import failure, seed-only export
failure, and retained checkpoints with empty restored outpoints. Assertions
describe defects Rust must fix. All keys derive from public toy inputs; never
fund them. The checkpoint reproduction is synthetic and does not prove loss of
a funded output against a running node. Other audit findings still require their
planned Rust and node-backed regressions.
