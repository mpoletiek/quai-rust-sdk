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
nonempty data; specialized operations use the explicit conversion/wrapping builders. In
particular, JS-generated data lengths one and two are rejected by pinned Go.
Special work-proof/ETX encodings, conversion/refund/aggregation consensus policy
and real node acceptance remain separate gates.

Tests compare 14 pinned JS transaction cases; `test-infra/go-oracle` independently
checks bytes, signing digests, IDs, ECDSA sender and Schnorr/ordered-key signatures.
Public fixture keys must never be funded. Agreement with both libraries does not
establish mainnet suitability or stateful transaction acceptance.


`document::TransactionDocument` imports/exports bounded published transaction
JSON, canonical bytes and raw protobuf views across account and Qi transfer,
conversion and wrapping states. Signed import verifies signatures, supplied hashes
and sender claims. Unsigned sender assertions remain explicitly unverified.
Full u64/U256 values survive interchange. `destination_zones` includes every Qi
output; first-output metadata remains separately available. Ordered access lists
preserve signing order; explicit map normalization sorts/deduplicates decoded bytes.
See [interchange parity](https://github.com/mpoletiek/quai-rust-sdk/blob/main/docs/TRANSACTION_INTERCHANGE_PARITY.md).

Public `proto` DTOs represent raw field presence without semantic validation.
Use bounded `encode_proto_transaction` / `decode_proto_transaction`, then a
concrete transaction decoder to verify semantics/signatures. Direct prost trait
decoding does not run the SDK's allocation preflight. JSON imports accept
caller-parsed values and cannot recover duplicate textual keys already discarded.
