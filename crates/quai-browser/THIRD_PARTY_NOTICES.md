# Third-party provenance

First-party code is MIT licensed. Dependencies retain their own licenses; a full
release license inventory and SBOM remain publishing gates.

- The public account checksum fixture in `crates/quai-primitives/tests/fixtures/`
  comes from the pinned quais.js checkout. Its upstream MIT notice and provenance
  are included in that directory.
- `compatibility/` uses `quais@1.0.0-alpha.57` as a development-only oracle, with
  its MIT license retained in the installed package. Generated API inventories
  and deterministic fixture results record that package's public API and behavior.
  The exact artifact, source hashes and dependency lock are recorded in
  `compatibility/reference-lock.json`. TypeScript is a development tool under
  Apache-2.0, retained with its installed package.
- go-quai is a source reference and the separately built local acceptance node.
  `test-infra/local-chain` and `test-infra/go-oracle` build/import pinned Go code
  only in explicitly isolated work directories. No node binary or complete node
  source tree is shipped in a Rust crate. The patch scripts retain short Go
  context fragments and development-only modifications; the source files retain
  their original notices (including LGPL-3.0-or-later core files). Copies of the
  [upstream notices and license texts](https://github.com/mpoletiek/quai-rust-sdk/blob/main/test-infra/licenses/README.md) accompany
  the embedded patch context. Review each
  modified file and the node's distribution obligations before shipping a harness
  image or node binary; the Rust workspace MIT label does not relicense Go code.
- The bounded OWL/OWL-A dictionary decoder in `quai-wallet` follows the published
  quais.js wordlist format and algorithm. Its MIT license is included as
  `LICENSE.quais-js` in packaged crates; the pinned artifact sources and generated
  dictionary vectors record provenance.
- Official BIP340 vectors are retained under the offered CC0-1.0 option, with
  authorship, source and checksum recorded in the crypto fixture README.
- Published BIP47 factual vectors retain attribution and an immutable source
  revision in the payment fixture README; no reference implementation/prose is
  copied into the Rust implementation.
- RustCrypto AES/CTR/scrypt/PBKDF2 dependencies retain their MIT-or-Apache-2.0
  licenses. Keystore fixtures are generated from pinned quais.js and Node crypto,
  with only already-public keys/passwords. Native backup vectors have separate
  independent Python/libsodium generator provenance.

Reference links: [quais.js pinned license](https://github.com/dominant-strategies/quais.js/blob/94e32c7eb9960de36054135c40a341c44c84f922/LICENSE.md),
[go-quai pinned source](https://github.com/dominant-strategies/go-quai/tree/f3f345c877300c044e3e0081a48bf3cf786fb9cc).
