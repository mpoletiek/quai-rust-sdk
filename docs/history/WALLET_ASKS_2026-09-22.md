# Asks from Quai Terminal, 2026-09-22

Quai Terminal (`~/Devspace/quai-rust-cli-wallet`) runs on `quai-sdk =0.1.0-alpha.9` and is upgrading to
`0.1.0-alpha.10`. Nearly all of the upgrade is done: the wallet's library, its examples and its live tests
build every input through the new constructors, and 541 of its tests pass on alpha.10. One gap in alpha.10
blocks the rest, and one earlier ask was only half answered. Both were checked against the published
`0.1.0-alpha.10` crates.

## Summary

| # | Ask | Priority | Why it matters |
|---|---|---|---|
| 1 | Constructors for the observation types consumers test against | High | Six wallet tests of settlement logic cannot be written against alpha.10 at all, so the wallet cannot upgrade without deleting them. |
| 2 | SOCKS proxies in `HttpProxy` | Medium | Node RPC still cannot go over Tor, the case the 2026-09-19 proxy ask was about. |

## 1. Constructors for the observation types consumers test against

**What happens.**
- alpha.10 made output types `#[non_exhaustive]`. The changelog says outputs "callers read but do not build, need no change".
- These seven now have no constructor, no `Default` and no `Deserialize`, so code outside the SDK cannot make one:

  | Type | Crate | Fields |
  |---|---|---|
  | `QiCreditObservation` | quai-provider `qi_credit.rs` | `beneficiary`, `transaction_hash`, `creating_hash`, `execution`, `head`, `outputs`, `locked_qits`, `unlocked_qits`, `unobserved_qits` |
  | `AddressOutpoint` | quai-provider `types.rs` | `outpoint`, `denomination`, `lock`, `extensions` |
  | `ConversionObservation` | quai-provider `conversion_tracking.rs` | `origin`, `scan`, `effect`, `spendability` |
  | `ExternalObservation` | quai-provider `external_tracking.rs` | `origin`, `scan`, `outcome` |
  | `EtxScanResult` | quai-provider `conversion_tracking.rs` | `coverage`, `last_block`, `transactions_examined`, `execution` |
  | `EtxExecutionObservation` | quai-provider `conversion_tracking.rs` | `transaction`, `receipt` |
  | `Log` | quai-provider `types.rs` | `address`, `topics`, `data`, `transaction_hash`, `inclusion`, `log_index`, `removed`, `extensions` |

- An application that interprets these, rather than only displaying them, has to test that interpretation against values it chooses. That is the use the changelog's premise misses.

**Evidence.**
- The wallet's `settle_result` (`crates/wallet-core/src/track.rs`) turns these observations into an operation's status: settled, locked, failed, or still waiting. It reads about twenty of their fields.
- Six tests in `crates/wallet-core/src/track_lifecycle_tests.rs` build the observations as fixtures and no longer compile:
  - `partial_delayed_and_previously_spent_outputs_never_become_complete_credit`
  - `account_conversion_effects_do_not_invent_operation_specific_maturity`
  - `a_reported_account_conversion_rests_locked_without_claiming_a_spendable_credit`
  - `failed_redemption_records_partial_credit_without_success_or_loss_claims`
  - and the fixtures `credit()` and `executed_destination()` behind them.
- The third of these pins the Qi→QUAI rule fixed on 2026-09-22: a reported conversion rests at `Locked`, not `Settled`, because its proceeds sit in the protocol's conversion lockup. Deleting it would unguard a rule that has already been wrong once.
- `Log` has a workaround: decode a receipt DTO that carries one log, then set its public fields. The wallet uses that now. The observations have none, because they come out of multi-call tracking, not one decoded response. Scripting that RPC sequence per scenario would break whenever the SDK changes its calls, as alpha.10's reduced round trips just did.

**Ask.**
- A `new` constructor per type, taking every field. No defaults: every field of these carries meaning, and a defaulted `unobserved_qits` or `coverage` would make a fixture claim something it never meant.
- This is additive for the semver gate. The types stay `#[non_exhaustive]`, and a field added later becomes a new constructor or a `with_*` method, as the inputs already do.
- If the team would rather keep them out of the stable surface, the same constructors behind a `fixtures` cargo feature serve just as well, since the wallet only needs them in tests.
- Please adjust the changelog's "need no change" to cover consumers who build outputs in tests, so the next consumer does not rediscover it.

**Wallet state meanwhile.** The alpha.10 migration sits unmerged on the wallet's `sdk-alpha10` branch, with the six items above parked. The wallet stays on alpha.9 until a release carries these constructors, and will then go straight to that release.

## 2. SOCKS proxies in `HttpProxy`

**What happens.**
- The 2026-09-19 asks (#5) asked for proxy support in the node transport, because every node request goes out direct and one IP links every wallet on a machine.
- alpha.10 added `HttpConfig::with_proxy` and `HttpProxy`. That covers most of it, thank you.
- `HttpProxy::parse` accepts only `http://` and `https://` URLs.
- The wallet's proxy setting exists for Tor: `socks5h://127.0.0.1:9050` (`config.rs`, `http.rs`). Its third-party lookups already go through it, and node RPC still cannot. The wallet's documentation tells users so: "Node RPC is not proxied."

**Ask.**
- Accept `socks5://` and `socks5h://` in `HttpProxy::parse`. reqwest supports both with its `socks` feature.
- `socks5h` resolves the host name through the proxy, which is what keeps a Tor user's DNS off the local resolver. So both schemes are wanted, not only `socks5`.
- Keep the existing guarantees: opt-in only, no environment variables read, credentials redacted.
- With that, the wallet would route node RPC through the same proxy as everything else. A local or LAN node would still be reached directly, as the wallet already does for its IPFS gateway.
