# Publishing the Rust SDK

Rust's public package registry is **crates.io**. This workspace contains twelve
versioned crates and a facade, `quai-sdk`. The current version is
`0.1.0-alpha.1`; publication remains disabled by the workspace's `publish = false`.
No registry upload or crate-name reservation has occurred.

The [September 13 name lookup](../test-infra/reports/crates-name-check-2026-09-13.json)
returned HTTP 404 for each proposed name. That is a dated observation, not a
reservation or guarantee. Check again immediately before the first upload.
Names are allocated on a first-come basis, and published versions are immutable.
See the [Cargo publishing guide](https://doc.rust-lang.org/cargo/reference/publishing.html).

Each package declares its license, description, repository, homepage, README,
documentation URL, keywords and category. Crate README repository links are
absolute so they remain usable when rendered outside the checkout. docs.rs
metadata selects all features on native Linux, except `quai-browser`, which uses
Wasm. The facade's Wasm workflows are additionally described in the
[complete guide](../SDK_DOCUMENTATION.md); native facade rustdoc cannot expose
modules compiled only for Wasm.

## Candidate qualification

1. Review the [parity analysis](../SDK_PARITY_ANALYSIS.md) and retained qualification
   reports. Unreviewed declarations and unqualified funded behavior remain visible;
   passing an archive build does not establish complete reference parity.
2. Require the intended commit's complete CI matrix, including default/all-feature
   and feature-minimal tests, Chromium, package consumers and advisory checks.
3. Run `python3 test-infra/package_rehearsal.py --wasm --report /tmp/package.json`
   with the pinned toolchain and populated offline dependency cache. It checks all
   twelve extracted archives, three consumer feature configurations, every native
   test/example target and two Wasm target configurations. It does not upload.
4. Inspect package contents, sizes, licenses and metadata. Archive validation
   uses an isolated local registry to resolve unpublished sibling versions;
   crates.io will require those versions to exist before dependents can publish.
5. Select the release version, support expectations and changelog. Resolve any
   actual remaining feature gap or explicitly retain a documented difference;
   do not promote ledger entries merely to bypass a release gate.

Private security reports can go to
[the repository's private reporting form](https://github.com/mpoletiek/quai-rust-sdk/security/advisories/new).
The [security policy](../SECURITY.md) describes the current limits.

## Authorized registry upload

Actual publication requires separate authorization and an authenticated registry
account. Do not put a registry token in the repository or a command transcript.
Use Cargo's credential provider or a separately configured trusted-publishing
workflow. No automatic publish-on-push or publish-on-tag action is installed.

After authorization, change the publish policy to allow only `crates-io`, rerun
verification at that exact revision and use `cargo publish --dry-run --locked -p
<crate>` before each upload. For a first release, the dependency order (including
local test dependencies) is:

1. `quai-primitives`
2. `quai-crypto`
3. `quai-rpc`
4. `quai-abi`
5. `quai-consensus`
6. `quai-payments`
7. `quai-provider`
8. `quai-signer`
9. `quai-wallet`
10. `quai-keystore`
11. `quai-browser`
12. `quai-sdk`

Verify registry availability of each exact uploaded version before its dependents.
A partial upload cannot be rolled back by reusing a version; follow Cargo's
publication and yanking rules. Finally verify clean registry consumers and hosted
rustdoc, then create release notes tied to the published source commit. A local
archive rehearsal cannot prove registry credentials, upload acceptance or hosted
documentation success.
