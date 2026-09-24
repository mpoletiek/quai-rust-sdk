# Response to the Quai Terminal asks of 2026-09-24

This answers [the asks](WALLET_ASKS_2026-09-24.md) about alpha.13's state
proofs. The central finding is right, and it was worse than a wording problem.
`forged_root.rs` ran unmodified against `08cb72f`, and both of its cases passed:
a node could get any balance, slot, code hash or absence "proven". Asks 1 to 5
and 7 ship in the next release. Ask 6 is deferred.

The proof of concept, `forged_root.rs`, works against the published alpha.13
and is not included in this repository. The ask's other attachments are.

## Outcome

| # | Ask | Verdict | What ships |
|---|---|---|---|
| 1 | Say what a proof establishes | Valid | Rewritten `STATE_PROOFS.md` and API docs; the header check below closes the hole the docs hid |
| 2 | `verify_header_hash` | Valid | `header_hash::verify_header_hash`, and `prove_accounts` refuses a header that does not recompute |
| 3 | Prove at a caller-supplied anchor | Valid | `StateAnchor`, `state_anchor`, `prove_accounts_at`, `prove_deployment_at`, plus `confirm_anchor` for the second node |
| 4 | Freshness bound | Valid, with a limit | `require_number_at_least` and `require_timestamp_at_least`, error `StaleAnchor` |
| 5 | gzip in `HttpTransport` | Valid | Requested and decoded, with no new dependency |
| 6 | Proven nonce and balance in `AccountSession` | Deferred | A wallet can do this with one `prove_accounts_at` on a shared anchor |
| 7 | Small items | All valid | `requireCanonical`, paged proof batches, `MAX_PROOF_NODES = 65`, trie tests and two fuzz targets, doc wording |

## How we checked

- `forged_root.rs` passed 2 of 2 against `08cb72f`. Against this change both
  cases fail with `ProviderError::Header(HeaderHashMismatch)`, before any
  address is sent.
- `quai_hash.py` matched your two vectors. We then recomputed live headers
  ourselves from `rpc.quai.network` (go-quai v0.56.0): the genesis, #1,048,576,
  #3,000,000 and the head. We also recomputed one live `newHeadsV2` work
  object. `headerHash` matched every time; the block hash matched before the
  fork, and after it through the auxpow, with the coinbase committing the seal.
  Those captures are the new test vectors
  (`test-infra/fixtures/header-hashes-mainnet.json`).
- The go-quai citations hold in the v0.52.0 source: the `headerHash` check
  (headerchain_validation.go:464), `requireCanonical` (api_backend.go:217),
  v2-only auxpow marshalling (wo.go:1411) and `newHeadsV2` (filters/api.go:506).
- End to end on mainnet, the `prove_state` example anchored #10,260,549 on the
  public RPC, confirmed its header hash on a LAN full node, and proved WQUAI
  and a token slot through gzip responses.

## 1 and 2: what a proof shows, and the header check

`state_anchor` recomputes `headerHash` from the header's JSON (protobuf and
BLAKE3, following go-quai's `SealEncode`) and requires it to equal
`woHeader.headerHash`. Since `headerHash` covers `evmRoot`, a node can no longer
keep the real header hash and change the root. It has to report a different
header hash, and a second node's header at that height then disagrees.

`VerifiedHeader` differs a little from your sketch:

- `hash` is the reported block hash. `hash_verified` says whether it was
  recomputed: before the fork always, after it only from a v2 work object with
  an auxpow whose coinbase commits the seal. A mismatch is an error either way.
- An unknown header or seal field fails closed as
  `HeaderHashError::UnknownField`, as you suggested. That error is distinct
  from `HeaderHashMismatch`, so a wallet can show "update the SDK" rather than
  "this node is lying" after a go-quai upgrade.
- An empty `extraData` is hashed both as present and as absent, because go-quai
  sends both as `"0x"`.
- Scrypt auxpow (merged-mining merkle root) is not checked; `hash_verified`
  stays false for it.

A header check alone does not stop a lying node, which can recompute a
consistent header of its own. What makes a proof independent is a second node,
so we added the step your ask 3 described:

```rust,ignore
let anchor = public.state_anchor(genesis, Zone::Cyprus1, BlockTag::Number(head - 4)).await?;
own_node.confirm_anchor(&anchor).await?;          // same headerHash at that height, no address sent
anchor.require_number_at_least(own_head - 8)?;    // ask 4
let proven = public.prove_accounts_at(&anchor, &targets).await?; // one round trip
```

`confirm_anchor` returns `AnchorDisputed` (class `Stale`) when the headers
differ. Near the head that is usually a race, so anchor a few blocks deep.
`ProvenAccount::header_hash` is kept, and `Provider::verified_header` lets a
stored record be checked against any node later.

## 3: prove at an anchor

`prove_accounts_at` is one round trip: the proofs by the anchor's hash, with
the header and genesis rechecked in the same batch. The genesis-before-address
order and the recheck are both kept. Your measured one-round shape was 249 ms
public against 500 ms. This gives the same round-trip count per proof once the
anchor is read, and the anchor is paid once per review rather than once per
proof.

## 4: freshness

`require_number_at_least` is the check to use, with a height taken from your
second node's head. The zone height and time are in the work-object seal, not
in `headerHash`, so against a dishonest node they hold only through
`confirm_anchor` at that height. `require_timestamp_at_least` catches a lagging
honest node, and a dishonest one only when `hash_verified` is true.

## 5: gzip

Rather than enable reqwest's `gzip` feature, which adds five crates, the
transport sends `Accept-Encoding: gzip` and decodes with `flate2`, already a
dependency. `max_response_bytes` bounds the compressed read and the decoded
body; a test decodes 8 MiB from about 8 KB and gets `ResponseTooLarge`. An
unknown encoding or a corrupt body is `InvalidResponse`. We measured the
gateway on 2026-09-24: an account proof was 1.9 times smaller, WQUAI's
bytecode 3.1 times.

## 6: deferred

`AccountSession` defaults to `Pending`, which cannot be proven, and a proven
nonce only matters with a confirmed anchor. The pinned path would need the
session to take an anchor from the caller and carry it into the preparation.
Until then, `prove_accounts_at(&anchor, &[(sender, &[])])` proves the sender at
the anchor the review already confirmed, in one round trip.

## 7: small items

- **`requireCanonical`** is sent on every by-hash read, `quai_getCode` and
  `quai_getProof`. Both work on the public gateway.
- **The batch ceiling.** `prove_accounts_at` splits requests into batches of
  about half a megabyte, each rechecking the block, and sends them
  concurrently. 32 accounts with 64 slots each now fit.
- **`MAX_PROOF_NODES`** is 65. A test builds a trie whose key branches at each
  of its 64 nibbles and proves it in 65 nodes.
- **Tests and fuzzing.** With `test-fixtures`, `state_proof::test_trie::TestTrie`
  builds tries with go-quai's node rules, so your tests can serve real proofs.
  A randomized test proves over 800 keys through several hundred embedded
  nodes and extensions each, and checks that byte changes, truncation and extra
  nodes fail. A public-API test uses slots mined to share a 9-nibble Keccak
  prefix, so real storage keys reach embedded leaves. New fuzz targets:
  `state_proof_trie` (valid tries, then mutated proofs) and `header_hash`.
- **`ObservationChanged`** and **privacy** wording are corrected in
  `STATE_PROOFS.md`.
- **The date** is left as it is: the measurement was taken on 2026-09-24 UTC.

## What changes for the wallet

- A mock transport that serves synthetic headers to `prove_accounts` must serve
  headers that recompute. Real captured headers do, as does anything
  `verify_header_hash` accepts.
- A mock that matches `quai_getCode` or `quai_getProof` parameters exactly must
  accept `"requireCanonical": true` beside `blockHash`.
- `ProviderError::Header` is class `Invalid`. Do not retry it against the same
  node, except `UnknownField`, which calls for an SDK update.
- `AnchorDisputed` and `StaleAnchor` are class `Stale`: anchor again, deeper.
