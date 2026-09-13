# Quai signer interfaces

`Signer` provides synchronous local signing with an explicit chain identity.
`LocalSigner` owns a zeroizing secret key and supports exact Quai transactions,
ordinary single-input Qi transfers, personal messages and validated EIP-712 data.
`WatchOnlySigner` carries a public address and chain ID and rejects signing.
Neither implementation performs RPC calls or broadcasts a transaction.

Transaction signing rejects a chain mismatch. Typed-data signing requires an
explicit `DomainPolicy`: `RequireChainId` binds the document to the configured
chain, while `AllowUnbound` permits an omitted chain ID but still validates any
present one. Personal-message signatures do not bind a chain ID.

The key wrapper cannot be cloned and diagnostics omit its scalar. Mixed-input
Qi, conversions and wrapping use the consensus and SDK session APIs, which
validate each input origin and persist exact signed bytes before exposure.
Asynchronous injected-wallet signing lives in `quai-browser`.

The [SDK workflow guide](https://github.com/mpoletiek/quai-rust-sdk/blob/main/docs/WALLET_WORKFLOWS.md)
shows durable preparation, authorization, signing, broadcast and recovery.
This crate remains unpublished; see the repository's
[implementation status](https://github.com/mpoletiek/quai-rust-sdk/blob/main/IMPLEMENTATION_STATUS.md)
for compatibility evidence and open qualification gates.
