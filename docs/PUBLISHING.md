# Publishing the Rust SDK

Rust's public package registry is **crates.io**. This workspace contains twelve
versioned crates, including the facade `quai-sdk`. The current version is
`0.1.0-alpha.1`; the workspace policy permits only `crates-io`. Metadata in the
repository does not by itself authorize an upload.

## Release record

| Version | Date | Source | Result |
| --- | --- | --- | --- |
| `0.1.0-alpha.1` | 2026-09-14 | `bf315ee`, tag `v0.1.0-alpha.1` | All twelve crates on crates.io; all twelve docs.rs builds succeeded |

The [release-candidate name lookup](../test-infra/reports/alpha-registry-check-2026-09-13.json)
returned HTTP 404 for each name before the first upload; the names are now owned
by the publishing account. Published versions are immutable: a mistake needs a
new version and, where appropriate, a yank. Crate READMEs are captured in each
archive, so README edits appear on crates.io only with the next published version.
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
   reports. Deliberate differences and unqualified funded behavior remain visible;
   passing an archive build does not establish complete reference parity.
2. Require the intended commit's complete CI matrix, including default/all-feature
   and feature-minimal tests, Chromium, package consumers and advisory checks.
3. Run `python3 test-infra/package_rehearsal.py --wasm --report /tmp/package.json`
   with the pinned toolchain and populated offline dependency cache. It checks all
   twelve extracted archives, three consumer feature configurations, every native
   test/example target and two Wasm target configurations. It does not upload.
4. Inspect package contents, sizes, licenses and metadata. Archive validation
   uses an isolated local registry to resolve new sibling versions; crates.io
   requires those versions to exist before dependents can publish.
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

The account must have a verified email address; crates.io rejects uploads from
an unverified account with HTTP 400. Scope API tokens to `publish-update` (and
`publish-new` only when adding a crate), restrict them to the `quai-*` pattern,
give them a short expiry and revoke them after the release.

After authorization, bump `version` in the root `Cargo.toml` (the workspace
dependency entries must match), rerun verification at that exact revision and run
`cargo publish --workspace --dry-run --locked`, which verifies every archive against
its new sibling versions. Upload in dependency order (including local test
dependencies):

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

`cargo publish -p <crate>` waits for each upload to reach the index before
returning, so a sequential loop over this list is sufficient. crates.io limits new
crate names to a short burst (five during the first release) followed by one
every ten minutes, returning HTTP 429 with a retry time; the first release took
about eighty minutes. New versions of existing crates have a separate, looser
limit. Before retrying after a failure, check
`https://crates.io/api/v1/crates/<crate>/<version>` and skip versions that already
exist. A partial upload cannot be rolled back by reusing a version; follow Cargo's
publication and yanking rules. Finally verify clean registry consumers and hosted
rustdoc, then create release notes tied to the published source commit. A local
archive rehearsal cannot prove registry credentials, upload acceptance or hosted
documentation success.

## Retained release inventory

The [alpha dependency inventory](../test-infra/reports/alpha-dependencies-2026-09-13.json)
records exact package versions, checksums, declared license expressions, resolved
features and dependency edges from locked Cargo metadata. It includes development
and target-specific dependencies, not just components in a particular binary.
Available root license/notice files have content hashes. It is a custom schema,
not an SPDX document or legal clearance; absent packaged notice files and upstream
license expressions remain visible for downstream distribution review.

Regenerate with `python3 test-infra/release_inventory.py --output /tmp/dependencies.json`
using the locked offline cache. The [alpha release manifest](../test-infra/reports/alpha-release-2026-09-13.json)
binds archive hashes, source identity, toolchain, reference and retained verification.
Archive hashes describe the tested dirty checkout and exclude their own later report;
use the final Git revision and CI run for the committed source identity.
