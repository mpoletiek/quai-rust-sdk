# Asks from Quai Terminal, 2026-09-24

Quai Terminal (`~/Devspace/quai-rust-cli-wallet`) is on `quai-sdk =0.1.0-alpha.12` and is evaluating
alpha.13's state proofs before adopting them. We reviewed `08cb72f` from four directions:

- an adversarial audit of `state_proof.rs`, `account_proof.rs` and `anchored.rs`;
- a map of every place the wallet trusts a node;
- live measurements against `rpc.quai.network/cyprus1` and a LAN full node;
- recomputing Quai header hashes from go-quai source and live headers.

**The trie verifier is sound**, and it accepts what honest go-quai sends. A differential test against
an independent geth-rules trie builder found no false accept, and neither did a probe of every splice
and truncation. The test exercised 708 inline and 739 extension nodes.

**The problem is where the root comes from.** Attachments are in
[`wallet-asks-2026-09-24/`](wallet-asks-2026-09-24/).

## Summary

| # | Ask | Priority | Why it matters |
|---|---|---|---|
| 1 | Say what a proof establishes today: the docs overstate it, and the "second endpoint" advice does not work | High | A wallet following STATE_PROOFS.md would relax defences on the strength of a check a lying node passes |
| 2 | `verify_header_hash`: bind `evmRoot` to the header's own hashes | High | Without it, no second source can confirm the root the proofs are checked against |
| 3 | Prove at a caller-supplied anchor (one round trip) | Medium | Lets the wallet cross-check the anchor with a second node before any proof, and halves the cost of proven reads |
| 4 | A freshness bound on `Latest` | Medium | A real but old canonical header passes today, and still would after ask 2 |
| 5 | `gzip` in `HttpTransport` | Medium | The gateway compresses; the SDK never asks. Code is 5.6× smaller and proofs 1.9× |
| 6 | Proven nonce and balance inside `AccountSession`'s pinned observation | Low | The one place a wallet would prove its own account, without another round trip |
| 7 | Small items: `requireCanonical`, the real batch ceiling, `MAX_PROOF_NODES`, fuzzing, privacy wording | Low | Correctness of docs and limits |

## 1. What a proof establishes today

**What happens.**
- `prove_accounts` checks every proof against `evmRoot`, and `state_root()` (account_proof.rs:166-175)
  reads that field straight from the serving node's `quai_getHeaderByNumber` JSON.
- `parse_zone_header` (lib.rs:435-452) checks only null, zone and height.
- `ZoneHeader` (types.rs:317-349) keeps `woHeader.hash` as reported and `evmRoot` as an opaque
  extension.
- Nothing ties `evmRoot` to either hash. The genesis check (anchored.rs:46-51) compares the node's
  own echo, and the recheck (anchored.rs:96-101) compares the node's own hash.

A node that returns the real genesis header, and the real header with only `evmRoot` changed, passes
every check. It can then prove whatever trie it built.

**Evidence.** [`forged_root.rs`](wallet-asks-2026-09-24/forged_root.rs) runs unmodified against
`08cb72f`. Drop it into `crates/quai-provider/tests/` and run
`cargo test -p quai-provider --test forged_root -- --nocapture`. It uses your
`state-proofs-mainnet.json` fixture: real genesis, real header hash `0x109fe335…baaf`. It gets
`prove_accounts` to return:

- a balance of 10²⁷ Its for an address in the fixture;
- 7.77×10²³ at that address's WQUAI `balanceOf` slot;
- an attacker-chosen code hash with `has_code()` true, so `prove_deployment`'s pin check passes for
  any expected hash;
- `account: None` for an existing contract, so `prove_deployment` says `MissingCode`.

The returned `block.hash` is the real one, and the stored proof re-verifies offline with
`verify_account_proof`.

```
FORGED: block 0x109fe33599f0dfa3acc16b175ff42ce41ed919816f4961a1d4196ec8ecedbaaf (real hash),
        balance 1000000000000000000000000 Its, token slot 777000000000000000000000,
        code hash 0x2d2aff97769c5aace892c72430a45c4ed14dc0aaa2ae432db6e143bc71b157d8
absence probe: account None, has_code false
test result: ok. 2 passed
```

This is no weaker than `quai_getBalance` or `verify_deployment`, which trust the node too. The
trouble is the wording. "Proven", "never a value", and "moves what a wallet trusts … to one block
header" (STATE_PROOFS.md:3-11, CHANGELOG, SDK_DOCUMENTATION) read as a guarantee against the node.

**The mitigation the docs offer does not work.** STATE_PROOFS.md:144-147 says reading "the same block
hash at that height from a second, independent endpoint" narrows trust. The forged result already
carries the real hash, so a second honest node agrees with it. Agreement binds the root only when it
is on `evmRoot` itself, or on a `headerHash` the client recomputed (ask 2).

**What would help.**
- Describe `ProvenAccount` as "consistent with the state root this node reported for this block"
  until a header check exists.
- Replace the second-endpoint paragraph: compare `evmRoot`, or a recomputed `headerHash`, at the same
  height, and never the hash alone.
- Say that "re-verifiable offline" means re-verifiable against the stored root, which was never
  authenticated (STATE_PROOFS.md:96-112).
- Keep the header fields in `ProvenAccount`, so a record can be re-bound once ask 2 exists.

## 2. `verify_header_hash`: bind `evmRoot` to the header's hashes

We traced go-quai (v0.52.0 source, checked against the v0.56.0 diff; mainnet blocks carry
`v0.54.0` in extraData) and recomputed the hashes on live blocks. Everything is protobuf and BLAKE3;
there is no RLP or keccak.

```
evmRoot (+ utxoRoot, etxSetRoot, receiptsRoot, manifestHash[3], parentHash[2], entropies, fees …)
  → headerHash = blake3(proto(Header.SealEncode()))                      core/types/block.go:903-914
  → sealHash   = blake3(blake3(proto(WorkObjectHeader seal fields)) ‖ primaryCoinbase[20])
                                                                         core/types/wo.go:1467-1545
  → bytes 4..36 of the 44-byte `fabe6d6d` push in the Ravencoin coinbase scriptSig
  → AuxPow (KawPoW, merge-mined with RVN; powId 1)
  → hash       = blake3(proto(AuxPow.ProtoEncode()))                     core/types/wo.go:1436-1459
```

Consensus rejects a block whose `woHeader.headerHash` differs from `Header.Hash()`
(headerchain_validation.go:464). It checks the coinbase commitment at headerchain_validation.go:645-652.

**What is recomputable from what an SDK already receives:**

- **`headerHash`: yes, from the v1 `quai_getHeaderByNumber` JSON.**
  - It is 840 bytes of protobuf and one BLAKE3 call.
  - It matched `woHeader.headerHash` on every block we tried: public RPC, LAN node, thirdweb, and
    `latest`.
  - Example: #10,259,520 recomputes to `0x3b302157…d6c2`, the value the node reports.
  - Changing only `evmRoot` gives `0xc319928a…c9bb`, while the reported `hash` stays
    `0x20b89862…72ca`. That is the forgery a hash-only comparison misses.
- **`hash` after the KawPoW fork (`primeTerminusNumber` ≥ 1,171,500): not from v1 JSON.**
  - v1 omits `auxpow`, `scryptDiffAndCount`, `shaDiffAndCount`, `shaShareTarget`,
    `scryptShareTarget` and `kawpowDifficulty`.
  - Only `RPCMarshalWorkObjectHeader("v2")` (wo.go:1411) emits them. Every public node we found runs
    the default `--rpc.version v1`.
  - The `newHeadsV2` subscription always sends v2 (quai/filters/api.go:506), and it works on
    `wss://rpc.quai.network/cyprus1`. So does any HTTP method on a node started with
    `--rpc.version v2`.
  - From v2 data we recomputed `headerHash`, `sealHash`, the coinbase commitment and `hash` for
    #10,259,605 and #10,259,606. All matched.
- **Pre-fork blocks: yes, fully.**
  - There `hash = blake3(mixHash ‖ sealHash ‖ nonce8)`, and #3,000,000 matched from v1 JSON.

**Reference implementation and vectors:**
- [`quai_hash.py`](wallet-asks-2026-09-24/quai_hash.py): about 200 lines, hand-rolled protobuf. Run it
  with `pip install blake3`, then `quai_hash.py header <file>` or `quai_hash.py wo <file>`.
- [`header-v1-10259520.json`](wallet-asks-2026-09-24/header-v1-10259520.json): gives `headerHash`.
- [`workobject-v2-10259606.json`](wallet-asks-2026-09-24/workobject-v2-10259606.json): gives
  `headerHash`, `sealHash` and `hash`. Its coinbase commits the seal hash.

**Encoding traps the reference handles:**
- `ProtoHash` is a nested `{1: 32 bytes}` and is always emitted.
- Optional `uint64` fields are emitted even when zero.
- Big integers are minimal big-endian. Zero is a present field of length zero.
- `PowShareDiffAndCount` encodes zero as `[0x00]`, not empty.
- Repeated fields have fixed lengths: `parentHash` 2, `manifestHash` 3, the three entropy lists 3
  each, `number` 2.
- Header fields 9, 18, 20 and 21 (difficulty, location, mixHash, nonce) are not in the seal, and
  `size` is never hashed.
- Seal field 13 (`data`) is omitted when empty; `auxpow2` (AuxPow field 7) is emitted even when `0x`.
- After the fork, seal field 12 (`primaryCoinbase`) is replaced by appending the 20 coinbase bytes
  to the inner hash.
- Hashing the AuxPow alone proves nothing about the header: the coinbase `sealHash` check is
  mandatory.

**What would help.** Something like:

```rust
pub struct VerifiedHeader {
    pub header_hash: Hash32,        // always: recomputed, and equal to woHeader.headerHash
    pub seal_hash: Option<Hash32>,  // pre-fork, or when v2 fields are present
    pub hash: Option<Hash32>,       // pre-fork, or post-fork with auxpow plus the coinbase commitment
    pub evm_root: Hash32,
    pub utxo_root: Hash32,
    // number, zone, prime_terminus_number, time …
}
pub fn verify_header_hash(header: &serde_json::Value) -> Result<VerifiedHeader, HeaderHashError>;
```

We would also like `prove_accounts` to refuse a header whose recomputed `headerHash` differs from
`woHeader.headerHash`. It should fail closed on unknown seal fields, because a fork that adds one
shows up as a mismatch.

A wallet can then check the root against a second node by comparing `headerHash`. That covers every
root in the header, `utxoRoot` included, which matters for Qi later. Checking proof-of-work is a
separate, later question. KawPoW light verification cost 3.3 s and 130 MB once per epoch, then 16 ms
per header. But forging one header at today's difficulty is about 11 GPU-hours, so PoW means little
without a checkpoint and depth. We are not asking for that.

## 3. Prove at a caller-supplied anchor

**What happens.**
- `Anchor`, `Provider::anchor` and `read_at_anchor` are `pub(crate)`.
- `prove_accounts` takes only `Latest` or a number, and both re-run round one.
- So every proven read costs two sequential round trips plus about 11.5 KB of repeated headers,
  whatever it proves.

**Evidence.** Medians, warm connection. The public RPC was at about 248 ms per round trip; LAN is a
full node on the same network.

| Read | Plain, public | `prove_accounts`, public | Plain, LAN | `prove_accounts`, LAN | Bytes plain → proof |
|---|---|---|---|---|---|
| Balance + nonce | 249 ms | 500 ms | 0.9 ms | 5.7 ms | 178 B → 15.9 KB |
| WQUAI balance + allowance (2 slots) | 254 ms | 509 ms | 1.7 ms | 7.4 ms | 286 B → 21.3 KB |
| UniV2 reserves (slot 8) | 253 ms | 509 ms | 1.6 ms | 6.8 ms | 271 B → 18.0 KB |
| Pin 1 router (20 KB runtime) | 747 ms, `verify_deployment` | 499 ms, `prove_deployment` | 10.8 ms | 6.1 ms | 52 KB → 16.4 KB |

For comparison, a one-round proof built by hand from the public `verify_account_proof` batched
`[chainId, header(latest), getProof(latest)]` and checked the proof against that header's `evmRoot`.
It took 249 ms public and 2.7 ms LAN, at 7.3 KB, and 0 of 40 attempts saw a block land between the
two reads. It gives up your genesis-before-address order and the recheck, which is why we would rather
you offer it.

**What would help, any one of these:**
- `prove_accounts_at(&VerifiedHeader or &ZoneHeader, targets)`: the caller has already read, and
  checked, the header; the SDK proves by its hash and rechecks it in the same batch.
- A public `Anchor` with a constructor that runs round one, so several proofs can share it.
- A provider mode that carries a verified genesis for its lifetime, so round one is only the header.

The first also lets a wallet compare the anchor with its second node before it sends any address,
which is the check that makes proofs mean something (ask 1).

## 4. A freshness bound

**What happens.**
- With `BlockTag::Latest`, any header the node returns is accepted (anchored.rs:37-51). The height
  is checked only for `BlockTag::Number` (lib.rs:445-450), and no timestamp is parsed.
- A real, older canonical block passes: its true `evmRoot` and valid proofs pass, and the recheck by
  number passes because the block is still canonical at its height.
- After ask 2 this becomes the easiest remaining attack: replay a pre-revocation allowance, an old
  balance, or a pre-upgrade implementation slot.
- An honest node behind a gateway gives the same stale values without any attack.

**What would help.** Parse `woHeader.timestamp`, and take a caller maximum age or a minimum height,
failing with a `Stale`-class error. The wallet already knows a second node's head, so a minimum
height is what it would pass.

## 5. `gzip` in `HttpTransport`

**What happens.**
- `rpc.quai.network` answers `Accept-Encoding: gzip` with `content-encoding: gzip` (checked with curl
  on 2026-09-24).
- The workspace `reqwest` is built with `["json", "rustls-tls", "socks"]`. `http.rs` posts plain, so
  every response arrives uncompressed.
- Only the `fetch` module decompresses, through `flate2`.

**Evidence** (curl, same requests):

| Response | Plain | gzip |
|---|---|---|
| `quai_getCode` (Quainance router) | 40,475 B | 7,229 B |
| `quai_getProof` (one EOA) | 4,338 B | 2,284 B |
| `quai_getHeaderByNumber` | 2,926 B | 1,296 B |

On the public RPC, responses over about 15 KB also paid extra round trips in TCP slow start. A
9-contract `verify_deployment` batch moved 380 KB.

**What would help.** Enable reqwest's `gzip` feature. We checked the response cap: with it on,
reqwest drops `content_length()` and yields decompressed chunks. So `max_response_bytes`
(http.rs:187-203) still bounds the decompressed size, and a compression bomb stops at the cap. It is
worth a test either way.

## 6. Proven nonce and balance inside `AccountSession`

The wallet signs with a nonce and checks a balance from the SDK's pinned observation. Proving them
there, at the block it already pins, costs no extra round trip, while proving them in the wallet costs
two. It is only worth doing together with asks 2 and 3; unanchored, it adds nothing. Low priority.

## 7. Small items

- **`requireCanonical: true`.** go-quai honors it on the by-hash block parameter
  (quai/api_backend.go:217), and it would be a free server-side check. `Anchor::block_param`
  (anchored.rs:21-23) does not send it.
- **The real batch ceiling.** Round two returns every proof in one response, and the default
  `max_response_bytes` is 2 MiB.
  - One WQUAI account with 64 slots was 147 KB. 32 accounts × 64 slots failed on both endpoints with
    `ResponseTooLarge`, while 32 × 16 was 707 KB.
  - The documented 32 × 64 is unreachable; about 32 × 40 is the practical limit.
  - Page round two by estimated size, or document that ceiling.
- **`MAX_PROOF_NODES = 64` is one short.** 64 branches plus a hashed leaf is 65 nodes. The auditor
  built that shape and got `TooLarge`. Real keys would need a 252-bit keccak prefix collision, so this
  is theoretical, but the comment at state_proof.rs:10 is wrong.
- **Fuzzing and fixtures.** The `state_proof` fuzz target cannot get past the root hash, since every
  later node hits `HashMismatch`, so it only checks determinism. The mainnet fixture has no inline or
  extension nodes. A target that builds a valid trie and then mutates it, plus fixture cases with
  those shapes, would cover what the differential test did.
- **`ObservationChanged`** is described as a reorg (STATE_PROOFS.md:119). In practice it also means a
  lagging backend behind a gateway.
- **Privacy wording.** "A node on the wrong network learns no address" holds only for an honest node.
  A node that echoes the right genesis receives, in one batch, every address and every mapping slot.
  Slots are `keccak(owner ‖ base)`, so it can confirm which tokens an owner holds.
- **Dates.** STATE_PROOFS.md:131 dates a measurement 2026-09-24, while the release commit is
  2026-09-23 local time. That is harmless.

## What the wallet will do regardless

- Anchor on its own: when a user has their own node configured, compare `evmRoot` at the proof block
  between that node and the public RPC.
- Switch pin checks to one `prove_accounts` per review, which is cheaper in bytes than
  `verify_deployment`.
- Label proofs without a second node as consistent, not verified.

Storage layouts we confirmed against `quai_call` at the same block, in case they are useful for docs
or examples:

- WQUAI: `balanceOf` at slot 3, `allowance` at slot 4.
- UniswapV2 pairs on four mainnet venues: `totalSupply` 0, `factory` 5, `token0`/`token1` 6/7,
  reserves at 8, packed 112/112/32.
- Factory `getPair` base slot: 2 on Quainance and the legacy AMM, but 4 on the launch AMM and the
  Hartii AMM.
