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
This crate is an alpha; see the repository's
[implementation status](https://github.com/mpoletiek/quai-rust-sdk/blob/main/IMPLEMENTATION_STATUS.md)
for compatibility evidence and open qualification gates.

`Signer::sign_qi_message` explicitly selects the pinned Qi wallet format: a
64-byte BIP340 signature over Keccak of the supplied bytes, with fresh auxiliary
entropy. It requires a Qi key and inserts no prefix or chain/application domain. Bytes that parse as a
transaction with inputs are refused with `SignerError::QiTransactionMessage`,
because a Qi message signature over them would also authorize that spend.
UTF-8 callers pass `text.as_bytes()`; hex-looking text is still text unless the
application decodes it first. `sign_message` retains its personal-message ECDSA
behavior. Watch-only signers reject both formats.

`verify_qi_message` requires the expected Qi address and its full SEC1 public key
because BIP340 signatures cannot recover a public key, and x-only keys do not
select address parity. `LocalSigner::public_key()` supplies this public identity.
Native, worker and bidirectional pinned-JS checks cover the format; the application
still owns message authorization and domain semantics.
