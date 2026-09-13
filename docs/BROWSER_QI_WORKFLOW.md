# Browser Qi discovery, preparation and signing

Enable `backup,browser` with native defaults disabled. The browser workflow uses
`BrowserAddressBook` for fresh HD allocation, `BrowserQiBook` for input custody,
and `BrowserQiSession` for discovery, denomination selection, fees and preparation.
`QiPolicy` and `QiIntent` are portable in `qi_preflight`; native `qi` re-exports
the same types. Construction never prompts or contacts a node.

1. Open and explicitly initialize the allocation/custody journals using trusted
   network identity, wallet namespace and prior cursor floors. Restore existing
   custody before discovery; an empty latest UTXO result does not establish a new
   wallet. Use the same named database for integrated HD discovery.
2. Create a `QiKeyResolver`, such as the Qi `HdWallet` or `QiKeyring` with HD,
   imported and registered payment receive keys. Public metadata alone cannot sign.
3. Allocate `BrowserQiChangePool::allocate(&addresses, &allocation_ids, attempts,
   cancelled)` before scanning. IDs must be fresh and unique; ranges are committed
   before search and completions before return. A pool has no clone or constructor
   from arbitrary addresses. It is consumed even on failed preparation; unused
   addresses and cancelled ranges remain burned. Limits are 1024 addresses,
   100,000 total raw derivation attempts, and the allocator's retained-ID capacity.
4. Construct `BrowserQiRequest { intent, policy, fees }`. Specify exact Qits,
   destination capacity and resource limits. Recipient denominations use distinct
   addresses, largest denomination first. Payment-code destinations should come
   from already committed `BrowserPaymentBook` send allocations; sender private
   payment keys do not belong to the recipient input resolver.
5. Call `prepare_discovered(id, &account, &options, request, pool, cancelled)` for
   current HD discovery. Default options use a gap of **50 matching addresses per
   receive/change branch**. Explicit ranges support deeper scans. The workflow also
   queries every persisted input owner and completed address in the supplied
   allocation journal beyond those ranges, including imported/payment owners
   retained by custody. This coverage is the configured journals and scan bounds,
   not a claim that every historical address has been found.
6. Alternatively, capture `book.snapshot().revision` before reading sources, build
   `QiSource { scope, checkpoint, coins, owners }`, and call `prepare_observed` with
   that revision. This supports explicitly refreshed imported/payment origins and
   custom history/use hints. `QiSource::from_discovery` validates a successful HD
   report against a trusted xpub; `include_known_qi_addresses` atomically extends an
   in-memory source with missing owners' current observations. It does not persist
   inputs or turn latest-only RPCs into a historical snapshot.
7. Preparation overlays every durable local input claim, selects fixed
   denominations, estimates the exact shape in a bounded monotonic loop, verifies
   keys, and rechecks network/checkpoint age, locks and explicit expiry. It then
   commits selected owners/input claims with the pre-read custody revision. The
   integrated discovery path also fences its allocation-journal revision in the
   same IndexedDB transaction. Any competing writer rejects without retry.
8. Review `PreparedBrowserQiTransaction::{transaction,fee,recipient_outputs,
   signing_digest}`. Call `sign(&resolver)` explicitly; it checks ordered keys and
   persists exact ordinary/conversion/wrapping signed bytes before returning them.
   Dropping an unsigned prepared result retains the claim until explicit unsigned
   release. Signed claims cannot be released merely because RPCs omit their inputs.
9. Call `session.broadcast(id)` explicitly. Ambiguous sends retain Submitted and
   exact bytes. Reopen the same journal to replay the same operation. Use
   [BrowserRecoverySession](BROWSER_CANDIDATE_RECOVERY.md) for explicit replacement
   candidate selection and canonical reconciliation without unlocking keys.

`QiOperationIntent::Transfer` requires same-zone recipients. `CrossZone` requires
one explicitly different recipient zone. `Conversion` carries exact Quai
beneficiary, Qi refund and two-byte slippage; `Wrapping` carries exact beneficiary
and owner contract. `Sweep` consumes every eligible source coin without change,
under `SweepMode::PreserveDenominations` or explicitly requested aggregation.
Sweep destinations must already be fresh and distinct; pass an empty change pool.
Aggregation's required block placement is an external qualification condition.

`QiFeeMode::Node` uses the ordinary exact-shape RPC. `Explicit(qits)` uses a
caller-authorized fee and is not advertised as an estimate. `Profile(profile)`
uses the specialized conversion/wrapping estimator under asserted compatible node
rules. Ordinary node estimation rejects special operations; specialized profiles
reject ordinary transfers. Fees cannot exceed `QiPolicy::max_fee`. Selection
limits are 1024 inputs/outputs and 32 rounds; sources are bounded to 100,000 coins
and owners. Address capacity must cover intermediate fee-convergence shapes.

`qi_preflight::quote_qi` exposes the same planning independently of browser storage.
Its immutable `QiQuote` includes exact fields, selected input observations/owners,
last candidate height and an optional specialized fee quote. Callers must overlay
claims and reserve the result themselves. Quotes are advisory: denominations,
locks, unspentness and balances come from the source, with no cryptographic proof
of the eventual on-chain debit. Origin inclusion does not establish destination
settlement or mature spendability.

[Tests](../crates/quai-sdk/tests/qi_preflight.rs) exercise native/Wasm planning,
actual Chromium allocations and IndexedDB claims, persisted addresses beyond scan
bounds, allocator/input races, cancellation, exact conversion/wrapping data and
restart replay after ambiguous submission. RPC responses are synthetic. They do
not establish funded unmodified-node acceptance, mature redemption spending or
aggregation placement.

[Reviewed replacements](BROWSER_REPLACEMENTS.md) preserve the complete original
input set, recipients and operation data while reducing only selected owned change.
They reuse the same family claim and persist signed candidates before submission.
