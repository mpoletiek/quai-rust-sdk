# Using payment codes for anything but payments

BIP47 payment codes let two parties derive a fresh address for every payment
from each other's public code. Because a code identifies a counterparty and
comes with key material, builders reach for it for other jobs too: messaging,
pairwise identifiers, authentication. This note explains what that key
material actually is, why reusing it is riskier than it looks, and what to use
instead.

Status: the SDK exposes payment-code keys only for payments. A general pairwise
key between codes was requested on 2026-09-19 ([asks](WALLET_ASKS_2026-09-19.md),
ask 4) and is deferred. See [what the SDK may add](#what-the-sdk-may-add).

## What the keys are

A payment account lives at `m/47'/969'/account'`. Its public code publishes that
node's public key and chain code, so anyone holding the code can derive every
non-hardened public child `B_i`.

- The **notification key** is child `/0`: private `a₀`, public `A₀`
  (`PrivatePaymentCode::notification_key`).
- To pay a receiver at index `i`, the sender computes the shared secret
  `s_i = x(a₀ · B_i)`. The receiver computes the same value as `x(b_i · A₀)`.
  The payment key is `B_i + sha256(s_i)·G`, and only the receiver, holding `b_i`,
  can spend it.

Two consequences follow from this, and neither is obvious from the API.

1. **The notification-to-notification ECDH is a payment secret.** At `i = 0`,
   `s_0 = x(a₀ · B₀)`. Here `B₀` is the receiver's own notification key, so
   `x(a₀ · B₀)` is exactly what a builder gets by "doing ECDH between the two
   payment codes". ECDH is symmetric, so it is the index-0 secret for payments
   in *both* directions between the pair.
2. **The sender side of every payment uses the notification key.** `a₀` enters
   every payment secret a code ever sends, to anyone.

## What goes wrong when it is reused

**Leaking the raw ECDH links payments.** If a messaging component logs, stores,
transmits or weakly hashes `x(a₀ · B₀)`, whoever obtains it can compute
`sha256(s_0)`. With the public codes, that identifies the pair's index-0 payment
addresses in both directions. Spending still needs `b₀`, so funds are not at
risk, but the payments are tied to the two identities. Hashing the value with
SHA-256 as a "message key" is worse: `sha256(s_0)` *is* the payment tweak.

**A static key has no forward secrecy.** Any key derived only from the two
wallets' long-term seeds decrypts everything it ever protected once either seed
leaks. Wallet seeds are the most handled secrets a user has: written down,
backed up, restored on new devices. There is no recovery after compromise
either; a leak exposes future messages until both parties change seeds.

**On a chain, the ciphertext is forever.** Messages sent through a contract are
public and permanent. An attacker can collect them today and decrypt them
whenever a seed leaks, years later. Forward secrecy matters more here than in
ordinary messaging, not less.

**It ties two security domains together.** Compromising the messaging
application compromises payment privacy, and a wallet compromise exposes every
conversation. Neither side can rotate its keys without the other.

**Code linkage is public already.** A Pelagus-style mailbox announcement links
two codes on-chain for anyone to read. A protocol that adds more activity keyed
to the same codes adds to that linkage.

## What to use instead

- **Treat the payment code as an identity, not an encryption key.** Generate
  separate messaging keys. Publish a signature, made with a key derived from the
  payment code, that vouches for them, so a peer can check the binding. Messaging
  keys can then rotate or be revoked without touching funds.
- **Use an established protocol for the conversation.**
  - One-to-one: X3DH for first contact with published one-time prekeys, then the
    Double Ratchet (the Signal design). This gives forward secrecy, recovery
    after compromise and deniability. It works asynchronously, with a contract as
    the mailbox. Maintained Rust implementations exist.
  - Groups: MLS.
  - At the very least, an ephemeral key per message (ECIES style). This protects
    old messages if the *sender's* long-term key leaks, though not the
    receiver's.
- **Remember metadata.** Encryption does not hide who wrote to a contract, or
  when. Consider sealed-sender designs or relaying if that matters for your use.

## If you must derive a static pairwise value

Some uses are legitimately static: a pairwise identifier, a shared tag, a
low-value lookup key. For those:

- Do not use the notification-to-notification ECDH, or any `x(a₀ · B_i)`, as
  input. Those are payment secrets.
- Do not reuse `sha256(s)` in any form.
- Derive from key material that payments never use, then apply HKDF-SHA256 with
  a fixed protocol label, a purpose string, and both codes in a fixed order. For
  example, children at a reserved index `R` on both codes give
  `x(a_R · B_R) = x(b_R · A_R)`. Payments use `a₀` on the sending side and never
  reach index `2³¹ − 1` in practice.
- Write the derivation down and publish test vectors, so every implementation
  derives the same value.
- Say plainly, wherever the value is used, that it has no forward secrecy.

## What the SDK may add

Two primitives are deferred, not rejected, and would ship with a specification
and JavaScript test vectors:

1. **Key attestation (the recommended path).** Sign an external public key with a
   payment-code-derived key under a fixed domain, and verify such a signature
   against a public code. This is what a proper messaging design needs, and it
   keeps encryption keys out of the wallet.
2. **A static pairwise key**, perhaps named `static_shared_key`, built as in the
   previous section. Its documentation would state that it has no forward secrecy
   and no recovery after compromise, that anyone who later obtains either seed can
   read everything it protected, and that it must not be used for messaging.

Neither changes payment derivation, notification keys, mailbox announcements or
any wire format. Existing payments, and interoperability with Pelagus, are
unaffected.

## For Quai Terminal specifically

The wallet's sealed messages currently derive their key with HKDF over the
notification-to-notification ECDH. The HKDF label separates the *key* from the
payment tweak, so the scheme is not broken. But it carries every limitation
above: it depends on the index-0 payment secret, it has no forward secrecy, and
it lives on a permanent public contract. We recommend that `quai-messages` move
to payment-code-attested messaging keys with a ratchet. That is a protocol
change with an epoch or version bump, and it belongs in that project's
specification.
