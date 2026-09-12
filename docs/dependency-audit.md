# Dependency advisory audit

Original scan **2026-09-11, 20:31–20:34 UTC**; final Rust scans at **2026-09-11T22:53:09.297428+00:00** (using the same refreshed advisory snapshot). The expanded wallet/browser/legacy-keystore lock was scanned; no dependency versions or
implementation files were changed by the scan, and no advisories were ignored.

| Scope | Tool | Result |
| --- | --- | --- |
| Rust workspace `Cargo.lock`, all 373 recorded packages | cargo-audit 0.22.2 | 0 known vulnerabilities; **2 unmaintained-package warnings** |
| Original Rust lock, `--deny warnings` | cargo-audit 0.22.2 | **Failed, exit 1**; the same two warnings persist in the refreshed lock |
| JavaScript reference `compatibility/package-lock.json`, 18 installed packages | npm 11.19.0 audit | 0 reported vulnerabilities at every severity; exit 0 |

The default Rust audit exited 0 because maintenance warnings are nonfatal by
default. This is not a warning-free dependency qualification. Advisory scans
cannot prove security, and the JS wallet defects recorded in the SDK audit
remain relevant even though npm reports no published vulnerability matches.

## Rust evidence and remaining warnings

[cargo-audit 0.22.2](https://github.com/rustsec/rustsec/releases/tag/cargo-audit%2Fv0.22.2)
was verified against the official release, then installed with `--locked` in
isolated temporary directories. Toolchain: rustc 1.97.1 and Cargo 1.97.1.
The scan refreshed the official RustSec database and checked registry yanks;
it applied no target, severity, or advisory exclusions.

- RustSec database: **1,243 advisories**, commit
  `b50980aad8b8f14f77e25a97b32dd94bf008b0af`, committed
  `2026-09-09T12:49:52+02:00`, fetched during this audit.
- Rust lock SHA-256:
  `a048d92a17149940efffa32926c33c525220902e3881a37d8fc6e6543680287b`.
- Report: zero vulnerability matches; no yanked, unsound, or notice warnings
  reported; the following two maintenance warnings remain.

| Package in lock | Advisory | Dependency path and current exposure |
| --- | --- | --- |
| `derivative 2.2.0` | [RUSTSEC-2024-0388](https://rustsec.org/advisories/RUSTSEC-2024-0388.html) | `ruint → ark-ff 0.3/0.4 → derivative`; unmaintained |
| `paste 1.0.15` | [RUSTSEC-2024-0436](https://rustsec.org/advisories/RUSTSEC-2024-0436.html) | `ruint → ark-ff 0.3/0.4/0.5 → paste`; unmaintained |

These paths exist in the full lock through `ruint`'s optional integrations.
Neither warning package appears in the current native `quai-sdk` default
normal/build graph or the workspace's default normal/build/dev graph, as checked
with `cargo tree`. They are **inactive optional dependencies**, not dependencies
currently compiled into the SDK under those feature selections. This does not
waive the warnings: feature unification in a different consumer can change that
assessment. Track upstream removal/replacement of these integrations, recheck
actual consumer features, and keep a strict all-lock warning gate marked failing
until the underlying warnings are resolved. No ignore list was added.

Selected production versions are `ruint 1.20.0`, `reqwest 0.12.28`,
`rustls 0.23.44`, `tokio 1.53.1`, `tiny-keccak 2.0.2`, `serde 1.0.229`,
`serde_json 1.0.151`, `url 2.5.8`, and `thiserror 2.0.20`. The advisory tool's
own dependency graph was installed separately and is not part of the SDK lock.

## JavaScript reference scope

Node 26.8.1 / npm 11.19.0 audited the private reference package containing
`quais 1.0.0-alpha.57` and `typescript 5.0.4`, with lifecycle scripts disabled.
The live npm advisory service returned an empty vulnerability map: info, low,
moderate, high, critical, and total counts were all zero. npm does not identify
an immutable advisory-database snapshot in its response.

Lock SHA-256:
`3deef1670658845ed8d73a83550a87b5eb810c9e7709f51b7dd7907df2be19c9`.

All 18 installed dependencies belong to the **development-only JS oracle**;
they are not Rust runtime dependencies. npm's metadata also counts the private
root package as `prod: 1`; that does not make the oracle a production SDK
dependency. No `npm audit fix`, version overrides, or suppression flags were
used, preserving the pinned reference artifact.

## Reproduction

From the repository root:

```sh
CARGO_HOME=/tmp/quai-rust-audit-cargo \
CARGO_TARGET_DIR=/tmp/quai-rust-audit-tools/target \
cargo install cargo-audit --version 0.22.2 --locked --root /tmp/quai-rust-audit-tools

CARGO_HOME=/tmp/quai-rust-audit-cargo \
/tmp/quai-rust-audit-tools/bin/cargo-audit audit \
  --db /tmp/quai-rust-rustsec-db --file Cargo.lock --json

CARGO_HOME=/tmp/quai-rust-audit-cargo \
/tmp/quai-rust-audit-tools/bin/cargo-audit audit \
  --db /tmp/quai-rust-rustsec-db --file Cargo.lock --deny warnings --json

cargo tree --locked -p quai-sdk --edges normal,build
cargo tree --locked --workspace --edges normal,build,dev

cd compatibility
npm audit --json --ignore-scripts --cache /tmp/quai-sdk-audit-npm-cache
```

The strict confirmation in this audit used `--no-fetch` immediately after the
refresh to hold the same RustSec snapshot; it retained registry-yank checking.
Reproduction commands above refresh the database, so findings may change over
time. Rerun after lock changes and before any release.

The refreshed scan includes crypto, backup, SQLite and browser development dependencies.
No known vulnerability matches were found in the 373-entry SDK lock. `cargo tree --workspace --all-features --edges normal,build,dev -i derivative -i paste`
also found neither warning package active in the expanded native graph. Other
consumer/target feature unification must still be assessed independently.

The separate development-only `fuzz/Cargo.lock` was also scanned: **259 packages,
zero known vulnerability matches, the same two unmaintained warnings**. Registry
yank checks completed using the populated SDK Cargo cache. Exact lock hashes and
scan metadata are retained in [SDK evidence](../test-infra/reports/dependency-audit.json)
and [fuzz evidence](../test-infra/reports/fuzz-dependency-audit.json). Neither report
suppresses advisory matches; the strict warning-free release gate remains open.
