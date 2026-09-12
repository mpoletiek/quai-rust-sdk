# Quai/Qi consensus values

This crate implements bounded canonical protobuf for unsigned and signed ordinary
Quai and Qi transactions. It uses the pinned Quai schema and crypto primitives;
it does not reuse Ethereum transaction envelopes or RLP signing rules.

`QuaiTransaction` stores the full u64 nonce/gas range and U256 chain/value/price.
`signing_digest()` hashes unsigned protobuf, including required empty fields.
`SignedQuaiTransaction` holds a verified immutable payload and recovered sender;
`hash()` applies the Quai location/ledger permutation to the signed-byte hash.

`QiTransaction` retains input order, validates outpoints/public keys and distinct
outputs, and supports single BIP340 or ordered local multi-key signing. Duplicate
keys in different inputs remain distinct ordered aggregation participants.
`sign_local` requires keys in input order and fresh OS signing randomness; no
networked/distributed MuSig coordinator is implemented. Output locks are zero.

Canonical decode rejects unknown/duplicate fields, nonminimal numeric bytes,
inapplicable transaction fields and re-encoding differences. A nonallocating
protobuf preflight bounds nested message count before prost allocates. Encoded
transactions are limited to 1 MiB and 16,384 nested messages. Cryptographic
verification does not prove an input exists, is spendable or is fee-sufficient.

Unsigned Qi wire fixtures may retain data, but ordinary sign/attach/decode reject
nonempty data until typed conversion/wrapping builders are qualified. In
particular, JS-generated data lengths one and two are rejected by pinned Go.
Special work-proof/ETX encodings, conversion/refund/aggregation consensus policy
and real node acceptance remain separate gates.

Tests compare 14 pinned JS transaction cases; `test-infra/go-oracle` independently
checks bytes, signing digests, IDs, ECDSA sender and Schnorr/ordered-key signatures.
Public fixture keys must never be funded. Agreement with both libraries does not
establish mainnet suitability or stateful transaction acceptance.
