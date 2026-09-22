# Network, public-node, KDF and lifecycle parity

These mappings use the published `quais@1.0.0-alpha.57` implementation. They cover
network/fee metadata, the remaining BIP44 and BaseWallet surface, standalone key
derivation, and socket/unmanaged subscriber wrappers. Rust keeps provider, worker,
key and listener ownership explicit.

## Network and fee metadata

`quai_provider::Network` is an immutable label and exact unsigned chain ID, with
`new`, `name`, `chain_id`, `Clone`, `to_json` and `from_json`. Labels are 1–128 UTF-8
bytes without control characters. JSON contains exactly name and chainId; the ID
exports as a decimal string and imports from an unsigned integer or decimal/0x
string. Floating, negative, extra and missing fields reject. Zero is representable
as custom metadata even though signing policies may require a nonzero chain.

`NetworkMatch` distinguishes an exact name from a chain ID. Comparing descriptors
uses their chain IDs. A numeric-looking name is not silently coerced to a number.
Names and chain IDs do not authenticate genesis, select RPC endpoints, establish
activation or prove a provider's identity.

`NetworkRegistry` is caller-owned and limited to 1–256 combined name/chain keys.
`register` inserts a canonical name and chain atomically; either collision or
insufficient capacity leaves both maps unchanged. `register_alias` adds an explicit
name. `by_name` rejects unknown names; `by_chain_id` returns the label unknown for
an unregistered chain. Returned descriptors are owned copies. There are no mutable
global factories or implicit mainnet entries; immutable values replace source
property setters and global configuration side effects.

`FeeData` contains `Option<U256>`. `to_json` retains decimal-string or null gasPrice
and omits the JS display-only `_type: FeeData` marker. `Provider::fee_data(zone)`
uses the existing chain-checked gas-price read. RPC failure is returned rather than
being silently converted to absent metadata or a zero fee.

## Raw public derivation and BIP44

`ExtendedPublicKey::from_public_key_chain_code` accepts a validated curve point
and 32-byte chain code. It constructs an explicitly synthetic depth-zero root for
nonhardened derivation. The root is not evidence of a BIP32 master or an account
path. `from_components` preserves supplied `ExtendedKeyMetadata`, checking root
metadata constraints and this point's fingerprint. Parent fingerprints and depth
remain caller claims, not an authenticated ancestry proof.

Both constructors support the same child math as published
`BIP44.deriveChildFromPublic`. The source account argument is only a returned path
label: it does not derive a hardened account child. Eight fixture cases exercise
receive/change branches and indexes 0, 1, 31 and 2^31−1; reconstructed full metadata
also reproduces the source account xpub exactly. Invalid fingerprints, contradictory
master metadata, hardened public derivation and depth exhaustion reject.

The rest of BIP44 maps to existing `HdWallet::derive_key`, `HdWallet::search`,
`AccountPublic::derive_address` and `AccountPublic::search`. Explicit search budgets
and cancellation replace the source ten-million-attempt loop. Native and browser
workers use the same Rust implementation rather than a separate React Native
crypto backend. Deriving a point does not durably allocate an address; wallet
allocation and gap scanning remain their own workflows.

Published `BIP44.xPub` returns the stored root's extendedKey and therefore exposes
an xprv for a normal private coin root. `HdWallet::root_public_key().export()` returns
only xpub. Private export is a separate guarded operation, consistent with the
previous HD-wallet review.

## Standalone key derivation

`quai_keystore::derive` exposes `derive_key` and `derive_key_with_progress` with
`DeriveParams::Pbkdf2` (HMAC-SHA256/SHA512) or `DeriveParams::Scrypt`. Password and
salt are exact borrowed bytes, each at most 1,024 bytes; empty inputs are allowed
for interoperability. Unicode normalization is the caller's explicit decision.
The output is `DerivedKey`, with redacted diagnostics and zeroizing owned storage;
`as_bytes` exposes a borrowed slice. It has no Clone or general serialization.

`DeriveLimits` reuses existing KDF memory/parameter ceilings and adds a maximum
output length (default 64, hard ceiling 1,024 bytes) plus a total PBKDF2 work cap
(rounds × output blocks). The default work cap is 2^24, with hard ceiling 2^28.
The existing default scrypt memory limit is 256 MiB plus 64 KiB and its work cap is 2^24 N*r*p;
explicit hard ceilings are 1 GiB and 2^28 work. PBKDF2 rounds default to at most
2,000,000, with hard ceiling 10,000,000. Output-block work is checked separately so
large requested keys cannot bypass that policy. Scrypt also enforces N > 1 and
N < 2^(16*r), matching the published helper's RFC parameter constraint.

Validation occurs before progress callbacks or expensive allocation/work.
`DeriveProgress::Started` and `Completed` correspond to fractions zero and one.
Returning false cancels at that checkpoint; cancellation after derivation drops
and wipes the output before release. These are coarse lifecycle notifications,
not the source's fine-grained asynchronous progress callbacks. RustCrypto does
not expose inner-loop interruption/progress, and this wrapper does not invent an
intermediate percentage or promise mid-loop cancellation.

The KDF calls are synchronous. Use an application-owned bounded blocking worker
on native and a dedicated browser worker for a responsive UI. The shared worker
tests exercise these exact calls. This replaces JavaScript async-loop scheduling
with explicit Rust worker ownership; it does not imply that adding `async` around
CPU work makes a UI responsive. Existing upstream scrypt scratch-workspace wiping
limitations remain documented in the keystore security review. Caller-owned input
buffers and copies of exposed output remain the caller's responsibility.

Sixteen published vectors cover both PBKDF2 hashes, several iteration counts,
32/96-byte multiblock outputs and scrypt block/lane parameters. Bounds tests cover
output-work amplification, excessive inputs, invalid costs, and before/after
cancellation checkpoints.

## Minimal wallet and subscriber wrappers

BaseWallet's local identity/key/signing operations map to `LocalSigner`, `Signer`
and guarded `SecretKey` operations. Rust derives the address from the key and binds
an explicit nonzero chain; it does not accept an unrelated address override.
Published BaseWallet accepts such an override and then loses it on `connect`.
Public-key access is explicit; retain/export an owned guarded key before handing
it to a signer when backup access is needed. Plaintext private-key getters are
not reproduced.

Provider, nonce, call, estimate and access-list operations use explicit `Provider`
methods and `CallRequest`. Population uses typed intents and account preflight;
sending uses existing native/browser durable account sessions or deliberate
low-level signed broadcast. Message signing uses the same personal-message prefix;
typed signing additionally checks the selected domain policy. See the
[signer review](SIGNER_PARITY_REVIEW.md) for the existing tested workflow mapping.

SocketBlock/Pending/Event/Subscriber lifecycle uses native `WsTransport` /
`WsSubscription` and browser `BrowserWebSocketTransport` / `BrowserSubscription`.
Subscription and awaited unsubscribe are explicit; dropping or losing a session
has the documented terminal behavior. Caller-held typed operation/topic/zone
filters replace cloned JSON arrays. Native `recv` or browser `next`, typed
header/log decoding, and `EventHub::emit` replace subclass callbacks. Exact heights
and removed-log metadata are retained, and pending events can retain explicit zone
context that the source pending emitter omits.

Local delivery pause/resume uses `EventHub` with explicit Drop or bounded Buffer
policy while the application owns the subscription drain task. The source socket
subscriber throws for pause(false) and supports only dropping paused messages.
Pausing delivery does not stop the remote stream; overflow/continuity errors must
be handled explicitly. No unbounded Promise callback chain or invisible reconnect
pump is introduced. Published UnmanagedSubscriber stores a name with no-op
start/stop/pause/resume; typed local registration/context replaces that placeholder.

[Five utility source tests](../compatibility/scripts/utility-completion.test.mjs),
[three subscriber source tests](../compatibility/scripts/subscriber-lifecycle.test.mjs),
and [six shared native/worker tests](../crates/quai-sdk/tests/utility_completion.rs)
provide current evidence. Existing signer, HD reference, local event and native/
browser WebSocket tests cover the composed runtime behavior. These checks do not
establish funded-network acceptance or production security qualification.
