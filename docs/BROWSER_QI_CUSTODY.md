# Browser Qi transaction custody

Enable SDK `backup,browser` for `browser_qi::BrowserQiBook`; the portable
`wallet::qi_custody::QiOperationBook` requires `backup`, without SQLite or Tokio.
The journal retains exact input claims and public key origins for ordinary Qi
transfers, Qi-to-Quai conversions and Qi-to-WQI wrapping transactions.

Use the same database name, trusted chain ID/genesis/zone and stable public wallet
identity in every tab and worker. A separate namespace cannot see another
namespace's claims. Initialize only previously unused custody, or initialize from
an authenticated wallet backup. Existing data and tombstones reject initialization.
An empty current UTXO response is insufficient evidence that old custody is absent.

1. Capture `snapshot().revision` before discovery. Use portable `discover_qi` with
   its default gap of 50 matching addresses on each receive/change branch, or
   explicit deeper ranges. Preserve the discovery checkpoint and canonical result.
2. Select fixed-denomination outputs, excluding `book.claimed_outpoints()` and
   observing unlock/expiry bounds. `reserve_discovery` re-derives selected HD
   metadata against the trusted account xpub and commits claims under the captured
   revision. `reserve` also supports explicit imported/payment-origin observations.
   A changed revision rejects the write; it does not retry stale discovery.
3. Prepare and review the complete `QiTransaction`, including outputs, fee and
   conversion/wrapping data. `sign` resolves the exact input public keys using an
   HD wallet or mixed `QiKeyring`, signs locally in input order, and stores canonical
   bytes before returning them. Keys are dropped before the storage await. This
   step does not quote node fees or establish aggregation/trim acceptance.
4. Retain explicitly approved same-input/data replacement candidates with
   `commit_replacement`. Candidates must reduce total output value; the root and
   every original input claim remain held. Node replacement acceptance is separate.
5. Mark submitted before explicitly broadcasting with the provider. Capture a new
   revision before canonical receipt/header checks and supply it to inclusion or
   reorg updates. Neither confirmation nor a reorg releases signed claims.

If a write is cancelled or its response lost, reopen and inspect the same operation
ID. A dispatched IndexedDB transaction may have committed. Only never-signed
operations can be released. Released IDs remain consumed; a new Qi request uses a
new ID and fresh observations. Local signing never retries automatically.

The deterministic `QQICUBK1` encoding binds the namespace and network, retains
public origin metadata, sorted owner/outpoint claims, exact signed roots,
replacement edges and optional observations. Import checks strict framing, scope,
public points, unique input claims, signatures and candidate ancestry. Limits are
256 retained IDs, 4,096 public origins, 4,096 claims per operation, 32 replacement
edges, 1 MiB per signed payload and 16 MiB per journal. Exhaustion preserves existing
state. This is public custody state, not encrypted private-key storage or a
quais.js serialization format.

Authenticated native v5 backup initialization preserves owned public origins,
claims, hash-only roots and signed candidates while dropping historical inclusion
and reservation checkpoints. Qi capture into a new encrypted backup and merge
into existing browser custody remain separate work, as does complete wallet
capture across allocation/account/payment journals. Discovery uses latest-only
outpoint RPC: a head recheck cannot make those calls an atomic historical snapshot.

Tests include 28 independently encoded Node states using pinned quais.js signed
transactions; native and actual Chromium worker tests cover all three transaction
forms, mixed HD/imported/BIP47 key resolution, Fetch discovery and locks, duplicate
claims, concurrent tabs, cancelled writes, restart, reorg fencing and authenticated
backup initialization. The shared account/Qi framing's 16 MiB aggregate boundary
is tested by the native account suite. Qi count and malformed-state bounds are
covered directly; fuzz imports retain the 65,536-byte input limit. Public toy keys
used by tests must never be funded on a public network.
