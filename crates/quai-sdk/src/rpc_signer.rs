//! Explicit remote account signing with verified responses and no automatic retries.
//!
//! A dropped or failed dispatched request may still have signed or sent remotely.
//! Persist reservations before dispatch; never release them solely because this
//! adapter returned an error. Transport implementations own deadlines and bounds.
use quai_consensus::{QuaiTransaction, SignedQuaiTransaction};
use quai_crypto::{RecoverableSignature, hash_message};
use quai_primitives::{Hash32, QuaiAddress, hexlify};
use quai_provider::{Inclusion, Provider, ProviderError};
use quai_rpc::{Endpoint, Routing, RpcError, Transport, U256};
use quai_signer::{DomainPolicy, TypedData};
use quai_wallet::discovery::NetworkScope;
use serde_json::{Value, json};
use std::fmt;

/// Maximum encoded signing parameter bytes (transport envelopes have their own bound).
pub const MAX_SIGNER_PARAMS: usize = 1024 * 1024;
/// Maximum number of remotely exposed accounts accepted in one response.
pub const MAX_SIGNER_ACCOUNTS: usize = 1024;

/// Sanitized validation or transport cause; payloads and passwords are not displayed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RpcSignerFailure {
    /// Invalid configuration, transaction, size or domain policy.
    #[error("invalid remote signing request")]
    InvalidRequest,
    /// Remote account is not exposed, or account response is malformed.
    #[error("remote signing account unavailable")]
    AccountUnavailable,
    /// Genesis or chain differs from the trusted scope.
    #[error("remote signing network changed")]
    NetworkMismatch,
    /// Signature, transaction or acknowledgement failed validation.
    #[error("invalid remote signing response")]
    InvalidResponse,
    /// A keystore password would have crossed an unencrypted transport.
    #[error("remote unlock refused over an unencrypted transport")]
    InsecureTransport,
    /// A typed provider check failed.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// Transport/protocol error retains sanitized numeric remote error information.
    #[error(transparent)]
    Rpc(#[from] RpcError),
}
/// Phase-aware failure. Dispatch means signing/submission may have occurred even
/// on rejection or timeout. Dropping a future cannot return this phase to callers.
#[derive(Debug, thiserror::Error)]
#[error("remote signer request failed (dispatched: {dispatched}): {source}")]
#[non_exhaustive]
pub struct RpcSignerError {
    /// True once the state-changing RPC may have been dispatched.
    pub dispatched: bool,
    /// Unverified hash returned before a post-dispatch failure, if parseable.
    pub reported_hash: Option<Hash32>,
    /// Sanitized cause.
    #[source]
    pub source: RpcSignerFailure,
}
impl RpcSignerError {
    /// How to react to this failure; see [`quai_primitives::ErrorClass`]. Once
    /// the request may have been dispatched, the outcome is ambiguous whatever
    /// the cause: never resubmit, reconcile instead.
    pub fn class(&self) -> quai_primitives::ErrorClass {
        use quai_primitives::ErrorClass;
        if self.dispatched {
            return ErrorClass::Ambiguous;
        }
        match &self.source {
            RpcSignerFailure::Provider(error) => error.class(),
            RpcSignerFailure::Rpc(error) => error.class(),
            RpcSignerFailure::NetworkMismatch => ErrorClass::NetworkMismatch,
            _ => ErrorClass::Invalid,
        }
    }
}
impl RpcSignerError {
    fn before(source: RpcSignerFailure) -> Self {
        Self {
            dispatched: false,
            reported_hash: None,
            source,
        }
    }
    fn after(source: RpcSignerFailure) -> Self {
        Self {
            dispatched: true,
            reported_hash: None,
            source,
        }
    }
}

/// An immutable remote account and network binding. Construction is offline.
///
/// Cloned transports must address the same service; HTTP, WebSocket or browser
/// Fetch transports are supported. Injected wallets have a separate adapter with
/// wallet event tracking. Passive checks cannot authenticate a malicious server.
#[derive(Clone)]
pub struct RpcAccountSigner<T> {
    transport: T,
    provider: Provider<T>,
    endpoint: Endpoint,
    address: QuaiAddress,
    scope: NetworkScope,
    allow_insecure_unlock: bool,
}
impl<T> fmt::Debug for RpcAccountSigner<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RpcAccountSigner")
            .field("address", &self.address)
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}
impl<T: Transport + Clone> RpcAccountSigner<T> {
    /// Bind an existing remote account to a trusted chain, genesis and zone.
    pub fn new(
        transport: T,
        routing: Routing,
        scope: NetworkScope,
        address: QuaiAddress,
    ) -> Result<Self, RpcSignerFailure> {
        if scope.chain_id == U256::ZERO
            || scope.genesis == Hash32::ZERO
            || scope.zone != address.zone()
        {
            return Err(RpcSignerFailure::InvalidRequest);
        }
        let endpoint = routing
            .endpoint(scope.zone.into())
            .map_err(|_| RpcSignerFailure::InvalidRequest)?
            .clone();
        Ok(Self {
            provider: Provider::new(transport.clone(), routing, scope.chain_id),
            transport,
            endpoint,
            address,
            scope,
            allow_insecure_unlock: false,
        })
    }
    /// Permit [`unlock`](Self::unlock) to send a keystore password over `http`
    /// or `ws`.
    ///
    /// Every other call here carries no long-lived secret, so only `unlock` is
    /// gated. Opt in for a loopback or private-network node whose traffic cannot
    /// be observed, which is what the local harnesses use; never for a remote
    /// endpoint. The password unlocks every account in that node's keystore for
    /// the requested duration, and the transport performs no proxying, so
    /// nothing else terminates TLS on its behalf.
    #[must_use]
    pub fn allow_insecure_unlock(mut self) -> Self {
        self.allow_insecure_unlock = true;
        self
    }
    /// Bound account; this does not assert current remote availability.
    pub fn address(&self) -> QuaiAddress {
        self.address
    }
    /// Trusted immutable network identity.
    pub fn scope(&self) -> NetworkScope {
        self.scope
    }
    /// Read/simulation provider using the same transport and routes.
    pub fn provider(&self) -> &Provider<T> {
        &self.provider
    }
    async fn check_network(&self) -> Result<(), RpcSignerFailure> {
        if self.provider.genesis_hash(self.scope.zone).await? != self.scope.genesis {
            return Err(RpcSignerFailure::NetworkMismatch);
        }
        Ok(())
    }
    /// Check network and passive account exposure without requesting permission.
    pub async fn check_account(&self) -> Result<(), RpcSignerFailure> {
        self.check_network().await?;
        self.check_exposed().await?;
        self.check_network().await
    }
    /// The bound account is among those the remote passively exposes.
    async fn check_exposed(&self) -> Result<(), RpcSignerFailure> {
        let accounts = self
            .transport
            .request(&self.endpoint, "quai_accounts", json!([]))
            .await?;
        let rows = accounts
            .as_array()
            .filter(|a| a.len() <= MAX_SIGNER_ACCOUNTS)
            .ok_or(RpcSignerFailure::AccountUnavailable)?;
        let mut unique = std::collections::BTreeSet::new();
        for row in rows {
            let address = row
                .as_str()
                .filter(|s| s.len() == 42)
                .and_then(|s| s.parse::<QuaiAddress>().ok())
                .ok_or(RpcSignerFailure::AccountUnavailable)?;
            if !unique.insert(address) {
                return Err(RpcSignerFailure::AccountUnavailable);
            }
        }
        if !unique.contains(&self.address) {
            return Err(RpcSignerFailure::AccountUnavailable);
        }
        Ok(())
    }
    /// Exposure, then network, immediately before a state-changing request.
    ///
    /// The network read sits next to the request on each side, so the pair
    /// brackets it exactly as `check_account` twice did, in four reads rather
    /// than six: the accounts list carries no address, so nothing is disclosed
    /// by reading it before the network is confirmed.
    async fn check_before(&self) -> Result<(), RpcSignerFailure> {
        self.check_exposed().await?;
        self.check_network().await
    }
    /// Network, then exposure, immediately after a state-changing request.
    async fn check_after(&self) -> Result<(), RpcSignerFailure> {
        self.check_network().await?;
        self.check_exposed().await
    }
    async fn dispatch(&self, method: &str, params: Value) -> Result<Value, RpcSignerError> {
        bounded(&params).map_err(RpcSignerError::before)?;
        self.check_before().await.map_err(RpcSignerError::before)?;
        let value = self
            .transport
            .request(&self.endpoint, method, params)
            .await
            .map_err(|e| RpcSignerError::after(e.into()))?;
        self.check_after().await.map_err(RpcSignerError::after)?;
        Ok(value)
    }
    /// Sign exact bytes using personal_sign; recover and verify the bound account.
    pub async fn sign_message(&self, message: &[u8]) -> Result<[u8; 65], RpcSignerError> {
        if message.len() > MAX_SIGNER_PARAMS / 2 {
            return Err(RpcSignerError::before(RpcSignerFailure::InvalidRequest));
        }
        let message_hex = hexlify(message)
            .map_err(|_| RpcSignerError::before(RpcSignerFailure::InvalidRequest))?;
        let value = self
            .dispatch("personal_sign", json!([message_hex, self.lower_address()]))
            .await?;
        verify_signature(value, self.address, &hash_message(message)).map_err(RpcSignerError::after)
    }
    /// Sign EIP-712 v4 using the immutable validated document and explicit domain policy.
    pub async fn sign_typed_data(
        &self,
        data: &TypedData,
        policy: DomainPolicy,
    ) -> Result<[u8; 65], RpcSignerError> {
        policy
            .validate(data, self.scope.chain_id)
            .map_err(|_| RpcSignerError::before(RpcSignerFailure::InvalidRequest))?;
        let document = data
            .to_rpc_json()
            .map_err(|_| RpcSignerError::before(RpcSignerFailure::InvalidRequest))?;
        let value = self
            .dispatch(
                "quai_signTypedData_v4",
                json!([self.lower_address(), document]),
            )
            .await?;
        verify_signature(value, self.address, data.signing_hash().bytes())
            .map_err(RpcSignerError::after)
    }
    /// Legacy quai_sign with an explicitly supplied expected digest. Servers vary
    /// in prefix/hash semantics; this API never guesses how the server hashes bytes.
    pub async fn legacy_sign(
        &self,
        message: &[u8],
        expected_digest: Hash32,
    ) -> Result<[u8; 65], RpcSignerError> {
        if message.len() > MAX_SIGNER_PARAMS / 2 {
            return Err(RpcSignerError::before(RpcSignerFailure::InvalidRequest));
        }
        let encoded = hexlify(message)
            .map_err(|_| RpcSignerError::before(RpcSignerFailure::InvalidRequest))?;
        let value = self
            .dispatch("quai_sign", json!([self.lower_address(), encoded]))
            .await?;
        verify_signature(value, self.address, expected_digest.bytes())
            .map_err(RpcSignerError::after)
    }
    /// Request exact type-0 signing without population or broadcast. The canonical
    /// protobuf must recover the bound account and preserve every unsigned field.
    pub async fn sign_transaction(
        &self,
        transaction: &QuaiTransaction,
    ) -> Result<SignedQuaiTransaction, RpcSignerError> {
        let params = self
            .transaction_params(transaction)
            .map_err(RpcSignerError::before)?;
        let value = self.dispatch("quai_signTransaction", params).await?;
        let text = value
            .as_str()
            .filter(|s| s.len() <= 2 * MAX_SIGNER_PARAMS)
            .ok_or_else(|| RpcSignerError::after(RpcSignerFailure::InvalidResponse))?;
        let bytes = quai_primitives::get_bytes(text)
            .map_err(|_| RpcSignerError::after(RpcSignerFailure::InvalidResponse))?;
        let signed = SignedQuaiTransaction::decode(&bytes)
            .map_err(|_| RpcSignerError::after(RpcSignerFailure::InvalidResponse))?;
        if signed.from() != self.address || signed.transaction() != transaction {
            return Err(RpcSignerError::after(RpcSignerFailure::InvalidResponse));
        }
        Ok(signed)
    }
    /// Explicit server-side unlock with a finite duration of 1..=3600 seconds.
    /// The password crosses the configured transport; it is never stored here or
    /// included in adapter diagnostics. Transport implementations may copy params.
    /// False is a valid server refusal; no automatic retry or later locking occurs.
    pub async fn unlock(
        &self,
        password: &str,
        duration_seconds: u32,
    ) -> Result<bool, RpcSignerError> {
        if password.len() > 1024 || !(1..=3600).contains(&duration_seconds) {
            return Err(RpcSignerError::before(RpcSignerFailure::InvalidRequest));
        }
        // This is the one call that puts a durable secret on the wire, so it is
        // the one that checks the transport. `Endpoint::parse` accepts `http`
        // and `ws`, and over either the password is readable by anything on the
        // path and unlocks every account in that node's keystore.
        if !matches!(self.endpoint.scheme(), "https" | "wss") && !self.allow_insecure_unlock {
            return Err(RpcSignerError::before(RpcSignerFailure::InsecureTransport));
        }
        let value = self
            .dispatch(
                "personal_unlockAccount",
                json!([self.lower_address(), password, duration_seconds]),
            )
            .await?;
        value
            .as_bool()
            .ok_or_else(|| RpcSignerError::after(RpcSignerFailure::InvalidResponse))
    }
    /// Ask the remote wallet to sign and send an exact populated request. Wallets
    /// may change fields: the returned hash is only an acknowledgement. Retain
    /// `RemoteSendIdentity::new` and the request before awaiting, including on drop.
    pub async fn send_transaction(
        &self,
        transaction: &QuaiTransaction,
    ) -> Result<RemoteSendAcknowledgement, RpcSignerError> {
        let params = self
            .transaction_params(transaction)
            .map_err(RpcSignerError::before)?;
        self.check_before().await.map_err(RpcSignerError::before)?;
        let value = self
            .transport
            .request(&self.endpoint, "quai_sendTransaction", params)
            .await
            .map_err(|e| RpcSignerError::after(e.into()))?;
        let hash = value
            .as_str()
            .filter(|s| s.len() == 66)
            .and_then(|s| s.parse::<Hash32>().ok());
        let after = |source| RpcSignerError {
            dispatched: true,
            reported_hash: hash,
            source,
        };
        self.check_after().await.map_err(after)?;
        RemoteSendAcknowledgement::from_reported_hash(
            self.scope,
            self.address,
            transaction.clone(),
            hash.ok_or_else(|| after(RpcSignerFailure::InvalidResponse))?,
        )
        .map_err(after)
    }
    fn lower_address(&self) -> String {
        self.address.to_string().to_ascii_lowercase()
    }
    fn transaction_params(&self, tx: &QuaiTransaction) -> Result<Value, RpcSignerFailure> {
        if tx.chain_id != self.scope.chain_id
            || tx.data.len() > MAX_SIGNER_PARAMS / 2
            || tx.access_list.len() > 1024
            || tx
                .access_list
                .iter()
                .try_fold(0usize, |n, a| n.checked_add(a.storage_keys.len()))
                .is_none_or(|n| n > 8192)
        {
            return Err(RpcSignerFailure::InvalidRequest);
        }
        tx.unsigned_bytes()
            .map_err(|_| RpcSignerFailure::InvalidRequest)?;
        let access: Vec<_> = tx.access_list.iter().map(|a| json!({"address":a.address.to_string(),"storageKeys":a.storage_keys.iter().map(ToString::to_string).collect::<Vec<_>>()})).collect();
        let mut dto = json!({"type":"0x0","chainId":format!("{:#x}",tx.chain_id),"from":self.lower_address(),"nonce":format!("{:#x}",tx.nonce),"value":format!("{:#x}",tx.value),"gas":format!("{:#x}",tx.gas_limit),"gasPrice":format!("{:#x}",tx.gas_price),"data":hexlify(&tx.data).map_err(|_|RpcSignerFailure::InvalidRequest)?,"accessList":access});
        if let Some(to) = tx.to {
            dto["to"] = json!(to.to_string().to_ascii_lowercase());
        }
        let params = json!([dto]);
        bounded(&params)?;
        Ok(params)
    }
}
fn bounded(value: &Value) -> Result<(), RpcSignerFailure> {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len())
                .filter(|n| *n <= MAX_SIGNER_PARAMS)
                .ok_or_else(|| std::io::Error::other("signing parameter limit"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Count(0), value).map_err(|_| RpcSignerFailure::InvalidRequest)
}
fn verify_signature(
    value: Value,
    address: QuaiAddress,
    digest: &[u8; 32],
) -> Result<[u8; 65], RpcSignerFailure> {
    let text = value
        .as_str()
        .filter(|s| s.len() == 132)
        .ok_or(RpcSignerFailure::InvalidResponse)?;
    let bytes = quai_primitives::get_bytes(text).map_err(|_| RpcSignerFailure::InvalidResponse)?;
    let signature: [u8; 65] = bytes
        .try_into()
        .map_err(|_| RpcSignerFailure::InvalidResponse)?;
    let recovered = RecoverableSignature::from_quais_bytes(&signature)
        .and_then(|s| s.recover_prehash(digest))
        .map_err(|_| RpcSignerFailure::InvalidResponse)?;
    if recovered.address() != address.address() {
        return Err(RpcSignerFailure::InvalidResponse);
    }
    Ok(signature)
}

/// Public request identity to persist before remote dispatch; digest is not a tx ID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct RemoteSendIdentity {
    /// Trusted network binding.
    pub scope: NetworkScope,
    /// Requested account.
    pub from: QuaiAddress,
    /// Requested nonce, which the remote wallet may change.
    pub nonce: u64,
    /// Complete unsigned signing digest.
    pub signing_digest: Hash32,
}
impl RemoteSendIdentity {
    /// Derive recovery metadata without network access or signing.
    pub fn new(
        scope: NetworkScope,
        from: QuaiAddress,
        transaction: &QuaiTransaction,
    ) -> Result<Self, RpcSignerFailure> {
        if scope.chain_id == U256::ZERO
            || scope.chain_id != transaction.chain_id
            || scope.genesis == Hash32::ZERO
            || scope.zone != from.zone()
        {
            return Err(RpcSignerFailure::InvalidRequest);
        }
        Ok(Self {
            scope,
            from,
            nonce: transaction.nonce,
            signing_digest: transaction
                .signing_digest()
                .map_err(|_| RpcSignerFailure::InvalidRequest)?,
        })
    }
}
/// Unverified remote submission acknowledgement with the exact original request.
#[derive(Clone)]
pub struct RemoteSendAcknowledgement {
    identity: RemoteSendIdentity,
    reported_hash: Hash32,
    requested: QuaiTransaction,
}
impl fmt::Debug for RemoteSendAcknowledgement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RemoteSendAcknowledgement")
            .field("identity", &self.identity)
            .field("reported_hash", &self.reported_hash)
            .finish_non_exhaustive()
    }
}
impl RemoteSendAcknowledgement {
    /// Reconstruct a structurally validated acknowledgement after restart. This
    /// does not assert acceptance; `observe` verifies the actual signed identity.
    pub fn from_reported_hash(
        scope: NetworkScope,
        from: QuaiAddress,
        requested: QuaiTransaction,
        reported_hash: Hash32,
    ) -> Result<Self, RpcSignerFailure> {
        let identity = RemoteSendIdentity::new(scope, from, &requested)?;
        let b = reported_hash.bytes();
        if reported_hash == Hash32::ZERO
            || b[0] != scope.zone.byte()
            || b[2] != scope.zone.byte()
            || b[1] & 0x80 != 0
            || b[3] & 0x80 != 0
        {
            return Err(RpcSignerFailure::InvalidResponse);
        }
        Ok(Self {
            identity,
            reported_hash,
            requested,
        })
    }
    /// Public original request identity.
    pub fn identity(&self) -> RemoteSendIdentity {
        self.identity
    }
    /// Unverified wallet hash, suitable for read-only lookup.
    pub fn reported_hash(&self) -> Hash32 {
        self.reported_hash
    }
    /// Exact request to retain for restart and comparison.
    pub fn requested_transaction(&self) -> &QuaiTransaction {
        &self.requested
    }
    /// One lookup bracketed by trusted network checks. None means not indexed,
    /// never evidence that the request was dropped or that its nonce is reusable.
    pub async fn observe<T: Transport>(
        &self,
        provider: &Provider<T>,
    ) -> Result<Option<RemoteSendObservation>, RpcSignerFailure> {
        let scope = self.identity.scope;
        let check = async || {
            if !crate::network::on_network(provider, scope, scope.zone).await? {
                return Err(RpcSignerFailure::NetworkMismatch);
            }
            Ok(())
        };
        check().await?;
        let tx = provider.transaction(scope.zone, self.reported_hash).await?;
        let observation = if let Some(tx) = tx {
            let signed = tx.verified_quai()?;
            Some(RemoteSendObservation {
                matches_request: signed.from() == self.identity.from
                    && signed.transaction() == &self.requested,
                signed,
                inclusion: tx.inclusion,
            })
        } else {
            None
        };
        check().await?;
        Ok(observation)
    }
}
/// Cryptographically verified transaction and explicit comparison with the request.
#[derive(Debug)]
#[non_exhaustive]
pub struct RemoteSendObservation {
    /// Canonical signed transaction reconstructed from the provider response.
    pub signed: SignedQuaiTransaction,
    /// Whether every requested field and the sender were preserved.
    pub matches_request: bool,
    /// Node-claimed location; canonical receipt/finality checks remain separate.
    pub inclusion: Option<Inclusion>,
}
