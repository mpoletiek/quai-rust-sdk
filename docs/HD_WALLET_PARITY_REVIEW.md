# HD wallet workflow review

This review covers the published `quais@1.0.0-alpha.57` `QuaiHDWallet` and
`QiHDWallet` declarations, including their inherited construction methods.
Rust separates deterministic keys, public observations, durable allocation,
signing and submission. The mappings below are functional compositions, not
source-compatible JavaScript objects or a claim of production qualification.

## Identity and exact address derivation

| Reference behavior | Rust composition | Difference |
| --- | --- | --- |
| Construct either HD wallet; `xPub()` | `HdWallet` with `CoinType::Quai` or `Qi`; `root_public_key().export()` | Coin-level `m/44'/994'` or `m/44'/969'` xpub; hardened account roots require the private parent or an explicit `AccountPublic` |
| `addAddress(account,index)`; Qi `addChangeAddress` | `PublicAddress::derive(&account_public, change, index)`; explicitly import metadata into storage | Exact raw BIP32 child, validated ledger/zone, no implicit search or private key in metadata; importing metadata invalidates the current snapshot |
| Get next receive/change address | `AccountPublic::search` plus native `allocate_address` or `BrowserAddressBook` | Commit a bounded raw range before search and a result before exposure; cancellation burns ranges |
| `connect(provider)` | Pass a provider to the native/browser session or discovery function | No mutable provider propagation through a secret-bearing wallet tree; each operation checks its explicit scope |
| `getPrivateKey(address)` | Verify retained origin with `QiKeyResolver`, or derive the exact Quai child and verify its public address before constructing `LocalSigner` | Returns a guarded key; public metadata alone cannot authorize signing |

The [HD reference tests](../crates/quai-wallet/tests/reference.rs) compare both
coin roots, account xpubs, exact keys, all ten mnemonic languages, passphrases and
bounded zone searches. [Portable origin tests](../crates/quai-sdk/tests/key_origins.rs)
check forged ancestry and mixed HD/imported/payment ownership. There is no Rust
counterpart to the reference's internal constructor guard token.

## Looking up wallet addresses

The reference `getAddressesForZone` for Qi returns **external BIP44 addresses**,
not all addresses in that zone. Change, imported and channel addresses have
separate getters; `getAddressesForAccount` unions all four origins. Imported
reference keys are assigned account zero even though they have no HD ancestry.

Rust exposes `PublicAddress::address`, `public_key` and `origin`. Use
`SqliteStore::addresses`, `QiOperationBook::addresses/address`, discovery reports,
allocation results and authenticated backup views as appropriate. Filter exact
`KeyOrigin::Bip44 { account, change, .. }` values with Rust iterators. For a
channel, `payment_addresses` or completed `PaymentAllocationBook` records retain
peer, account, direction, zone and raw child index. Payment receive keys appear
as imported public keys in the general address list; retain the channel records
to distinguish them from direct imports. Rust does not invent an HD account for
an imported key or put a secret key in a `derivationPath` string.

A browser custody book holds owners known to that book, not every address ever
derived. Compose it with the address/payment journals and current scan report.
Lookup absence does not establish that a key is unowned or safe to reuse.

### Empty-address and gap views

The reference gap getters filter its mutable `AddressStatus.UNUSED` cache;
new unobserved BIP44 addresses are `UNKNOWN` and do not appear there. This is not
proof of never-used history. Rust keeps observations and durable exposures
separate. In `CurrentQiDiscovery`, an address contributes to the empty gap when
`outputs.is_empty() && !use_hint`. Filter by `derived.change` for external/change
views. `ObservedAddress` also retains explicit history capability and `ever_used`
when using a history-capable source. Channel exposures can be queried through
`Provider::outpoints` and retained in an application-owned observation view.

`QiAddressBook` now supplies the four-state in-memory cache and external/change/
channel gap views. Its portable refresh helper queries all registered origins and
commits only after final network/head checks. See [Qi address views](QI_ADDRESS_VIEWS.md).
New imports start Unknown; empty observations preserve prior Used/AttemptedUse
until explicit invalidation. These statuses cannot rewind durable allocations.
The cache is rebuilt after restart, independently of authenticated custody.

## Coins, scanning and payment channels

| Reference behavior | Rust composition | Difference |
| --- | --- | --- |
| `getOutpoints` / `importOutpoints` | Native `snapshot` / `replace_snapshot`; portable `CurrentQiDiscovery`, `QiSource` and typed `CandidateCoin` collections | Native imports require known owners, scope, checkpoint, generation, unique outpoints and fixed denominations; durable claims override caller flags. Browser current coin views are caller-owned and separate from custody |
| `scan` | `discover_qi` / native `scan_and_refresh_qi`, plus explicit registered-channel receive scans and known-origin refresh | Gap 50 per matching receive/change branch; bounded deep ranges; reports retain coverage. Empty latest-only outpoints do not establish complete history |
| `sync` | Native `refresh_qi` over every persisted origin; portable discovery plus `include_known_qi_addresses` | Explicit current refresh instead of the reference's mutable incremental cache and creation/deletion callbacks; applications may compare typed snapshots for UI events |
| `getPaymentCode(account)` | `PrivatePaymentCode::from_seed` or explicit master/account xprv, then `public_code().to_base58()` | BIP47 account derivation is separate from BIP44 wallet state; no implicit notification or counterparty exchange |
| `openChannel` / `channelIsOpen` | Native owner-verified `import_payment_channel` / `payment_channel`; scoped `BrowserPaymentBook` initialization and snapshot | Existing native channels require the current generation and reject cursor rewind; browser initialization rejects overwriting an existing journal |
| `importPrivateKey` | `QiKeyring::import`, public metadata persistence and authenticated backup origin | Bounded memory-only keyring; duplicates reject; public records carry no secret material |

The [reference workflow tests](../compatibility/scripts/hd-wallet-workflows.test.mjs)
confirm that reopening a published channel repeats its first receive address.
Rust deliberately prevents this reset. The [native channel regression](../crates/quai-wallet/src/storage/payment.rs)
checks restart, cancelled allocations, stale import rejection and all-zone floors;
[browser payment tests](../crates/quai-sdk/tests/payment_allocation.rs) check durable
allocation and restore. The [Qi session tests](../crates/quai-sdk/tests/qi.rs) cover
current scans, payment receive scans, idempotent imports and mixed-origin refresh.

## Signing, sending and recovery

Quai HD signing composes an exact child `LocalSigner` with `Signer::sign_quai`,
`sign_message` or chain-bound `sign_typed_data`. `AccountSession` and
`BrowserAccountSession` prepare, review, sign, persist and submit exact bytes.
Quai message signing uses the account message prefix; Qi messages use the
reference Qi Schnorr construction. They are separate operations.

Qi signing uses validated input ownership with single-input Schnorr or ordered
multi-input MuSig. `QiSession` and `BrowserQiSession` perform bounded fee
convergence, input claims and exact signed-byte persistence. Sending to a payment
code first allocates enough peer destinations for denomination outputs using
`payment_intent` or the portable/browser payment allocation book. Cross-zone
sending is explicit. `convertToQuai` maps to typed conversion preparation;
`aggregate` maps to `SweepMode::Aggregate`. Specialized fees require an explicit
fee or asserted compatible node/fork profile. Destination observations and
operation-specific maturity have their own limits; no aggregation preparation
can guarantee the node's required block placement.

Rust does not reproduce signing-progress percentage callbacks or a send method
that conceals the preparation/signing/submission boundaries. Cancellation,
resource bounds and persistence are explicit. See [browser Qi workflows](BROWSER_QI_WORKFLOW.md),
[account workflows](BROWSER_ACCOUNT_WORKFLOW.md), [settlement observations](BROWSER_CANDIDATE_RECOVERY.md)
and [selection review](QI_SELECTION_PARITY_REVIEW.md).

## Serialization and restoration

Use `WalletBackup` with authenticated `EncryptedWalletBackup`, explicit private
origins and monotonic native/browser restoration. Public metadata and allocation
journals remain separately exportable. This format is not the plaintext
`Serialized*HDWallet` JSON schema. Rust does not currently import/export that
legacy schema as a whole wallet; recovery requires the actual seed, passphrase,
master/private origins and validated public allocation context.

The [reference regressions](../compatibility/scripts/wallet-regressions.test.mjs)
show that legacy serialization loses a BIP39 passphrase, cannot restore a French
mnemonic through its English-only path, cannot export a seed-only wallet, puts
an imported private key in nominal address metadata and can retain a checkpoint
without its outpoint snapshot. Rust's authenticated backups preserve effective
identity and claims while discarding stale chain observations. See
[backup tests](../crates/quai-wallet/src/full_backup/tests.rs) and
[portable backup tests](../crates/quai-sdk/tests/portable_backups.rs).
