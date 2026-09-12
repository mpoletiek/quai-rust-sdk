# Ordinary Qi transaction sessions

**September 12 update:** See [current wallet workflows](WALLET_WORKFLOWS.md) for
the added node-backed Qi scanner, mixed-origin sessions, conversion/wrapping
preparation and recovery APIs. Historical evidence and limitations below retain
their original scope; the newer guide describes the additional implementations.

`quai_sdk::qi` is available on native targets with the `sqlite` feature. It composes deterministic denomination selection, exact provider fee estimation, local BIP44 Qi keys, ordered multi-input Schnorr signing, and SQLite input claims. The caller selects a chain ID, trusted genesis hash and zone. The library does not hardcode a test network; this workflow's tests use a mock transport and never submit to a live node.

The supported operation is an ordinary same-zone Qi payment with empty transaction data. Imported private keys, BIP47-derived inputs, conversions, distributed signing, chain proofs and automatic discovery are outside this session. Base consensus and other crates expose separate capabilities; this session does not silently reinterpret those operations.

## Allocate, discover, prepare, review, sign, broadcast

1. Open `SqliteStore` with the explicit `NetworkScope`. Obtain the local Qi wallet's `AccountPublic` for its change account.
2. Call `QiChangePool::allocate`. It reserves raw derivation ranges, derives same-zone Qi addresses on the change branch and persists their public metadata **before returning addresses**. The opaque pool cannot be cloned, imported or reconstructed. Preparation requires its persisted address metadata and account derivation cursor in the receiving store; matching only the network scope is insufficient. Every reserved range remains consumed after failure, cancellation, process death or an unused pool.
3. Refresh the wallet's qualified discovery snapshot **after allocation**. Adding change metadata invalidates the previous checkpoint. A complete discovery commit must cover existing metadata, including the new addresses. `snapshot()` returning no checkpoint is not spendable. A manually constructed `Snapshot` remains a caller/source claim, not a proof.
4. Create `QiSession::new(&provider, &wallet, &mut store)`. Call `prepare(id, intent, policy, change_pool)`. It consumes the pool, checks chain/genesis, observes canonical identity at the stored checkpoint, enforces a caller-selected maximum checkpoint age and selects eligible inputs. It estimates the exact inputs, public keys, destination addresses, denominations and zero locks in a bounded monotonic fee loop. It checks checkpoint identity and candidate-height spend eligibility again after estimation, then atomically reserves the final inputs against the snapshot generation.
5. Review `PreparedQiTransaction::transaction()`, `fee()`, `recipient_outputs()` and `signing_digest()`. Recipient outputs appear first; change follows. No mutation API is exposed. Dropping a prepared transaction keeps its unsigned reservation until explicit `release_unsigned(id)`.
6. Call `sign(&prepared)`. It derives and verifies each local input key in transaction order, checks the durable claims, signs the frozen payload and commits verified canonical bytes before returning a signature. Secret key guards are dropped on every success/error path. There is no network access while signing.
7. Explicitly call `broadcast(id)`. It loads and validates the saved bytes, verifies local ownership and chain/genesis again, then records `Submitted` before awaiting submission. It sends once. A timeout, cancellation, malformed acknowledgement or other ambiguous result retains bytes and claims. A subsequent explicit call rebroadcasts those same bytes; it does not select replacement inputs or generate a replacement signature.

The call structure is:

```rust,ignore
let account = wallet.account_public(0)?;
let change = QiChangePool::allocate(&mut store, &account, 16, 2_000, || false)?;

// Refresh qualified discovery over all store.addresses() here.
// Do not label latest-only RPC outpoints as a pinned checkpoint snapshot.
refresh_from_your_qualified_source(&mut store).await?;

let policy = QiPolicy {
    initial_fee: U256::ZERO,
    max_fee: approved_fee_in_qits,
    max_inputs: 64,
    max_outputs: 64,
    max_fee_rounds: 8,
    max_snapshot_age: 2,
};
let intent = QiIntent {
    amount: amount_in_qits,
    destinations: fresh_recipient_addresses,
};
let mut session = QiSession::new(&provider, &wallet, &mut store)?;
let prepared = session.prepare(unique_reservation_id, intent, policy, change).await?;
// Present exact prepared.transaction() and prepared.fee() for authorization.
let signed = session.sign(&prepared)?; // canonical bytes are already durable
let result = session.broadcast(prepared.reservation_id()).await?;
```

`refresh_from_your_qualified_source` above is an application integration point, not an SDK function. A production live Qi discovery source still needs independent qualification for history, checkpoint consistency, lock/trim rules and reorg behavior. The session's header reads check source-reported canonicality; they do not authenticate the source, prove UTXO existence or make separate RPC calls atomic. Fee quotes and spendability can change after preparation. Node acceptance and mining remain separate from local signing.

## Address capacity and bounds

A Qi output has one denomination and needs a distinct address. `QiIntent::destinations` is an ordered capacity list: the exact amount is decomposed, largest denomination first, and the required prefix is used. Any extra destination addresses are unused. All supplied destinations must be distinct and same-zone; the final transaction also rejects input/output address reuse. Splitting an amount does not duplicate its value across the addresses.

Change capacity must cover every attempted fee round, including the initial fee. Lower fees may need more change outputs than the final fee. Plan capacity conservatively with the pure selector when useful, allocate it durably, then refresh discovery. An insufficient pool or destination list fails before claiming inputs. The pool remains consumed, including its unused addresses; allocation or preparation never silently retries with a new pool. A failed allocation may have persisted part of its requested capacity, so inspect storage and refresh before retrying.

The explicit limits are 1–1024 inputs, 1–1024 total outputs and 1–32 fee rounds. These preserve the local aggregation bound rather than exposing the selector's larger independent limit. Change allocation accepts at most 1024 addresses, 1–100,000 attempts each and at most 100,000 total raw attempts. Zero change capacity is supported for exact spends. A fee is accepted only when the quote for the final payload is covered; fees never decrease during convergence. The exposed planned fee can therefore exceed the last estimate, but cannot exceed `max_fee`. This calculation relies on the source-reported input denominations. A lying or incorrect source can understate input values and thereby understate the actual on-chain fee: neither a canonical-header observation nor a signature cryptographically enforces this planned fee cap. Qualify and trust the UTXO source before authorizing a payment.

`ReservationId` is application-generated and must never be reused. Durable cursors and signed claims do not roll back. Confirmations, reorg reconciliation and deliberate unsigned releases use the wallet storage APIs; a remote error never authorizes release of signed inputs.

## Verification

`cargo test -p quai-sdk --features sqlite --test qi` exercises the workflow against a deterministic transport and real on-disk WAL SQLite databases. Coverage includes changed-input fee convergence, the final estimation payload, multi-input signing, persisted bytes before exposure, checkpoint invalidation after change allocation, insufficient capacity, fee limits, bounded nonconvergence, wrong wallet/genesis, noncanonical snapshots, concurrent database invalidation, expired coins, expiry during estimation, tip age, cross-store pool misuse, allocation cancellation, duplicate/reused addresses, ambiguous acknowledgements, restart/rebroadcast and cancellation while submission is pending.
