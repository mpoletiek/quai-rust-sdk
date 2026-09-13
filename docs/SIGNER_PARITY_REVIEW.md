# Local and watch-only signer parity review

This review covers the published `quais@1.0.0-alpha.57` local signer families:
`AbstractSigner`, `VoidSigner`, `Wallet`, `HDNodeWallet` and `HDNodeVoidWallet`.
It does not establish parity for `JsonRpcSigner` or the specialized Qi/Quai HD
wallet orchestration. Those have separate ledger entries.

Rust separates key ownership, signing, provider reads, exact transaction quotation
and durable custody. The facade's native and browser sessions compose these APIs.
There is no mutable JS-style provider property on a secret key or watch-only xpub.
The [executable reference checks](../compatibility/scripts/signer-basics.test.mjs)
verify inherited provider-method bindings, sender mismatch rejection, void signing,
and the explicit-zero nonce population defect.

| Reference behavior | Rust API and behavior | Evidence |
| --- | --- | --- |
| Construct/import a local wallet; address/signing key/private key | `SecretKey::from_bytes`, `LocalSigner::new`, `Signer::address` and `LocalSigner::public_key`. The signer has an explicit nonzero chain and validates the address. Export key bytes deliberately through a guarded `SecretKey::export_bytes` before moving it into a signer; no secret getter on `LocalSigner` | [signers](../crates/quai-signer/tests/signers.rs), [crypto](../crates/quai-crypto/tests/crypto.rs) |
| Watch-only constructor and public identity | `WatchOnlySigner` for an address, `ExtendedPublicKey` for a subtree and `AccountPublic` for BIP44 discovery. Signing returns WatchOnly; public derivation rejects hardened steps | [signers](../crates/quai-signer/tests/signers.rs), [HD reference](../crates/quai-wallet/tests/reference.rs) |
| `connect` / `provider` | Pass an explicit provider to preflight or create a native/browser session. No shared mutable connection, implicit key cloning or wallet-controlled chain switching | [native account session](../crates/quai-sdk/tests/accounts.rs), [browser preflight](../crates/quai-sdk/tests/account_preflight.rs) |
| `_getAddress`, `getAddress`, `address`, `zoneFromAddress` | Typed address parsing and `.zone()` or `Signer::address`. Inputs are concrete; JS Promise/Addressable coercion is absent | [addresses](../crates/quai-primitives/tests/primitives.rs), [signers](../crates/quai-signer/tests/signers.rs) |
| `getNonce` | `Provider::transaction_count` with explicit pending/latest/numeric block selection; sessions combine this observation with durable nonce floors | [provider](../crates/quai-provider/tests/provider.rs), [preflight](../crates/quai-sdk/tests/account_preflight.rs) |
| `populateCall`, `call`, `estimateGas`, `createAccessList` | Construct a typed `CallRequest`, then use `Provider::call`, `estimate_gas` or `create_access_list`. Exact address/data/access-list ordering and explicit simulation block replace JS coercion/defaulting | [provider](../crates/quai-provider/tests/provider.rs), [account RPC](../crates/quai-provider/tests/wallet_reads.rs), [preflight](../crates/quai-sdk/tests/account_preflight.rs) |
| `populateQuaiTransaction` | `quote_account` / `quote_account_with_access` with explicit intent, nonce policy, state source and fee caps; typed conversion/deployment quotation is separate. No quote reserves or sends | [preflight](../crates/quai-sdk/tests/account_preflight.rs), [workflow](BROWSER_ACCOUNT_WORKFLOW.md) |
| `signTransaction` | `Signer::sign_quai` and canonical `QuaiTransaction::sign`, without provider population. Immutable signed bytes bind the configured chain; session signing also persists exact bytes before exposure | [signed reference vectors](../crates/quai-consensus/tests/quai_vectors.rs), [signers](../crates/quai-signer/tests/signers.rs), [native accounts](../crates/quai-sdk/tests/accounts.rs) |
| `sendTransaction` | Explicit prepare/review/sign/broadcast on native or browser account sessions, including calls, cross-zone transfers, conversions and deployments. Watch-only signing cannot authorize a send. Ambiguous submission retains the exact candidate and nonce claim | [native accounts](../crates/quai-sdk/tests/accounts.rs), [browser preflight](../crates/quai-sdk/tests/account_preflight.rs), [candidate recovery](../crates/quai-sdk/tests/browser_recovery.rs) |
| `signMessage`, `signMessageSync` | `Signer::sign_message` over caller-supplied bytes. Text callers use UTF-8 explicitly. Recoverable signatures expose exact quais wire bytes; no implicit hex-string decoding | [signers](../crates/quai-signer/tests/signers.rs), [crypto](../crates/quai-crypto/tests/crypto.rs) |
| `signTypedData` | Immutable validated `TypedData` plus `Signer::sign_typed_data` and explicit `DomainPolicy`. Chain-mismatched or unbound documents require the documented policy; dynamic JS value coercions are absent | [signers](../crates/quai-signer/tests/signers.rs), [ABI documentation](../crates/quai-abi/README.md) |
| HD constructors, `fromPhrase`, `fromSeed`, public key and mnemonic origin | `Mnemonic::parse/to_seed`, `ExtendedPrivateKey::from_seed/derive_path/public_key` and `ExtendedPublicKey` imports. BIP44 facade derivation uses `HdWallet`. Retain the original guarded mnemonic separately: imported extended keys cannot prove or reconstruct it | [HD vectors](../crates/quai-wallet/tests/reference.rs), [HD guide](../crates/quai-wallet/README.md) |
| `addressFromUncompressed` | Validate SEC1 point bytes with `PublicKey::from_sec1_bytes` then compute `.address()`. No optional React Native crypto bridge | [crypto](../crates/quai-crypto/tests/crypto.rs) |
| Legacy `encrypt` / `encryptSync`, `fromEncryptedJson` / sync form, global encrypt/decrypt helpers | `quai_keystore::encrypt`, `encrypt_with_mnemonic`, `Keystore::from_json/decrypt`, then explicit signer construction. Native and Wasm use OS/Web Crypto randomness. KDFs are synchronous, bounded and caller-scheduled; no progress callback or in-flight KDF cancellation. Export uses fixed strong scrypt defaults, and imported mnemonic ancestry is verified | [keystore tests](../crates/quai-keystore/tests/keystore.rs), [Chromium worker](../crates/quai-browser/tests/worker.rs), [limits](../crates/quai-keystore/README.md) |

The reference populates explicit `nonce: 0` from the pending nonce; Rust's
`AccountNonce::Exact(0)` never silently changes it and rejects if it is already
consumed. `AtLeast(0)` is the explicit allocation alternative. Rust estimates and
checks final fields, positive gas and maximum fees rather than silently inserting
zero-address estimates for special ledger operations. These differences are
intentional and are not evidence of missing transaction signing.

The mappings cover functional composition, not identical class layouts or every
JavaScript asynchronous scheduling behavior. UI progress reporting during a KDF,
React Native native-crypto bridges, copying secret-bearing wallet objects, and
arbitrary weak export KDF options are deliberately not exposed. Real extension
interoperability, node execution qualification and independent security review
remain separate from local signer behavior.
