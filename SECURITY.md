# Development security status

This workspace is not production-qualified. The `0.1.0-alpha.1` packages are published on crates.io as an alpha; publication is not a production-safety claim.
It contains wallet keys, encrypted seed/full-wallet backups, transaction signing,
legacy keystore import/export and account/Qi submission APIs. Public fixtures are reproducible and must never hold
real funds. Mainnet testing in this development session is read-only.

Implemented controls include explicit ledger/zone and chain checks, canonical
bounded transaction decoding, immutable signed payloads, redacted zeroizing key
wrappers, fresh OS randomness for keys/encryption/Schnorr signing, authenticated
memory-hard seed backups, conservative durable reservations, explicit routing,
TLS verification, bounded transport resources and redacted diagnostics.
The raw RPC transport can invoke caller-selected methods and is not a read-only
authorization boundary. A cancelled or failed send can have reached the node;
never automatically release a signed reservation or retry a payment with a new
nonce. Polling canonicality checks are not a finality proof.

Secret export methods are explicit. Callers remain responsible for buffers they
create, OS/process security and avoiding secrets in logs. Zeroization does not
promise protection from a compromised host or elimination of every compiler/
backend temporary. In particular, upstream scrypt 0.12.0 does not wipe its internal
B/V/T working arrays; the legacy keystore wrapper cannot erase these allocations.
Per-call KDF limits do not bound concurrent application work. Keep expensive KDF
operations off async/UI threads and limit the number of workers.

The native full-wallet format preserves supported seed, extended-key and imported
key origins, durable reservations/signed payloads, derivation metadata and payment
channels. Seed-only and legacy formats have narrower documented recovery scopes.
Restoration invalidates cached chain observations. Open-store identity prevents
unsigned workflow objects crossing handles, but does not protect against a copied
wallet, malicious database rollback or independently operating the same seed.
Current-outpoint scans cannot prove complete historical wallet recovery.
Node-reported balances, Qi denominations, parent links and receipts are observations,
not independently verified chain proofs. Conversion receipt success alone does not
establish maturity or spendability. Signed claims must not be released automatically
on missing receipts, incomplete scans, cancellation or transport errors.

See [architecture](docs/architecture.md), [current parity and qualification](SDK_PARITY_ANALYSIS.md),
[backup format](crates/quai-wallet/BACKUP_FORMAT.md),
[full-wallet format](crates/quai-wallet/FULL_BACKUP_FORMAT.md), and
[dependency advisory evidence](docs/dependency-audit.md). Advisory reports bind
an exact lock hash; additions require rescanning. No finding has been suppressed.

The [2026-09-11 internal security review](docs/SECURITY_REVIEW_2026-09-11.md)
records four fixed medium findings, password-buffer hardening, regression evidence
and unresolved limits. Parser sanitizer smoke runs are bounded checks, not sustained
fuzz qualification. Two inactive optional dependencies remain unmaintained.

Report suspected vulnerabilities privately through
[GitHub private vulnerability reporting](https://github.com/mpoletiek/quai-rust-sdk/security/advisories/new).
This route is enabled for this repository and reaches its maintainers. Include a
minimal reproduction using public toy keys, affected versions and expected impact;
never include credentials or funded wallet material. No response-time SLA is promised.

Before a production release, complete dependency/license review, sustained fuzzing,
platform checks, protocol-specific review, funded acceptance and recovery gates. No independent security audit has
occurred. Never include real private keys, mnemonics, wallet exports or
credential-bearing URLs in public bug reports; use public test fixtures.
