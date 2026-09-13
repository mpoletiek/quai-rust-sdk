use crate::{BrowserConfig, BrowserError, decode_response, encode, read_method};
use quai_crypto::{RecoverableSignature, hash_message};
use quai_primitives::{QuaiAddress, Shard};
use quai_rpc::{Endpoint, RemoteError, RpcError, Transport, U256, parse_quantity};
use serde_json::{Value, json};
use std::{cell::Cell, fmt, rc::Rc};
use wasm_bindgen::prelude::*;
mod submission;
pub use submission::InjectedSubmissionTransport;

#[wasm_bindgen(module = "/src/bridge.js")]
extern "C" {
    #[wasm_bindgen(catch, js_name = watchProvider)]
    fn watch_provider(provider: &JsValue) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_name = providerRevision)]
    fn provider_revision(watch: &JsValue) -> u32;
    #[wasm_bindgen(js_name = providerEventsSupported)]
    fn provider_events_supported(watch: &JsValue) -> bool;
    #[wasm_bindgen(js_name = providerChanged)]
    fn provider_changed(watch: &JsValue, revision: u32) -> bool;
    #[wasm_bindgen(js_name = closeProviderWatch)]
    fn close_provider_watch(watch: &JsValue);
    #[wasm_bindgen(catch, js_name = newAbort)]
    fn new_abort() -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_name = abort)]
    fn abort_js(controller: &JsValue);
    #[wasm_bindgen(js_name = errorKind)]
    fn error_kind(error: &JsValue) -> String;
    #[wasm_bindgen(js_name = errorCode)]
    fn error_code(error: &JsValue) -> f64;
    #[wasm_bindgen(catch, js_name = validateProvider)]
    fn validate_provider(provider: &JsValue) -> Result<bool, JsValue>;
    #[wasm_bindgen(catch, js_name = fetchJson)]
    async fn fetch_json(
        url: &str,
        body: &str,
        controller: &JsValue,
        timeout: u32,
        max_bytes: usize,
    ) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch, js_name = injectedJson)]
    async fn injected_json(
        provider: &JsValue,
        payload: &str,
        controller: &JsValue,
        timeout: u32,
        max_bytes: usize,
    ) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch, js_name = randomBytes)]
    fn random_bytes(length: usize) -> Result<js_sys::Uint8Array, JsValue>;
}
struct ProviderWatch(JsValue);
impl Drop for ProviderWatch {
    fn drop(&mut self) {
        close_provider_watch(&self.0);
    }
}
struct AbortGuard(JsValue);
impl AbortGuard {
    fn new() -> Result<Self, BrowserError> {
        new_abort()
            .map(Self)
            .map_err(|_| BrowserError::InvalidConfig)
    }
}
impl Drop for AbortGuard {
    fn drop(&mut self) {
        abort_js(&self.0);
    }
}
struct Permit(Rc<Cell<usize>>);
impl Drop for Permit {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}
fn permit(active: &Rc<Cell<usize>>, max: usize) -> Result<Permit, BrowserError> {
    if active.get() >= max {
        return Err(BrowserError::Busy);
    }
    active.set(active.get() + 1);
    Ok(Permit(active.clone()))
}
fn convert(error: JsValue) -> BrowserError {
    let code = error_code(&error) as i64;
    match error_kind(&error).as_str() {
        "timeout" => RpcError::Timeout.into(),
        "response_size" => RpcError::ResponseTooLarge.into(),
        "invalid" => BrowserError::InvalidResult,
        "http" => RpcError::HttpStatus(code as u16).into(),
        "provider" => BrowserError::Provider(code),
        "entropy" => BrowserError::EntropyUnavailable,
        _ => RpcError::Transport.into(),
    }
}
fn rpc_error(error: BrowserError) -> RpcError {
    match error {
        BrowserError::Rpc(error) => error,
        BrowserError::Provider(code) => RpcError::Remote(RemoteError {
            code,
            message: String::new(),
            data: None,
        }),
        BrowserError::InvalidResult => RpcError::InvalidResponse("invalid browser result"),
        BrowserError::ChainMismatch => RpcError::InvalidResponse("injected chain ID mismatch"),
        BrowserError::Busy => RpcError::AtCapacity,
        BrowserError::ContextChanged => {
            RpcError::InvalidResponse("injected provider context changed")
        }
        _ => RpcError::InvalidConfig,
    }
}
fn result_string(value: JsValue) -> Result<String, BrowserError> {
    value.as_string().ok_or(BrowserError::InvalidResult)
}
fn request_valid(method: &str, params: &Value) -> bool {
    !method.is_empty()
        && method.len() <= 128
        && method
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.'))
        && (params.is_array() || params.is_object())
}

/// Browser Fetch transport with streaming response limits, CORS and credential omission.
///
/// Clones share concurrency/ID limits. Dropping the request aborts its Fetch controller;
/// completion or cancellation does not prove whether a remote write was accepted.
#[derive(Clone)]
pub struct BrowserFetchTransport {
    config: BrowserConfig,
    active: Rc<Cell<usize>>,
    next_id: Rc<Cell<u64>>,
}
impl fmt::Debug for BrowserFetchTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BrowserFetchTransport")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}
impl BrowserFetchTransport {
    /// Construct without network access. Available in windows and dedicated workers.
    pub fn new(config: BrowserConfig) -> Result<Self, BrowserError> {
        config.validate()?;
        Ok(Self {
            config,
            active: Rc::new(Cell::new(0)),
            next_id: Rc::new(Cell::new(1)),
        })
    }
    /// Execute one explicit RPC call; never retries or follows redirects.
    pub async fn send(
        &self,
        endpoint: &Endpoint,
        method: &str,
        params: Value,
    ) -> Result<Value, BrowserError> {
        if !matches!(endpoint.scheme(), "http" | "https") || !request_valid(method, &params) {
            return Err(BrowserError::InvalidConfig);
        }
        let _permit = permit(&self.active, self.config.max_in_flight)?;
        let id = self.next_id.get();
        if id > 9_007_199_254_740_991 {
            return Err(RpcError::RequestIdExhausted.into());
        }
        self.next_id.set(id + 1);
        let body = encode(
            &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
            self.config.max_request_bytes,
        )?;
        let guard = AbortGuard::new()?;
        let result = fetch_json(
            endpoint.as_str(),
            &body,
            &guard.0,
            self.config.request_timeout_ms,
            self.config.max_response_bytes,
        )
        .await
        .map_err(convert)?;
        let result = result_string(result)?;
        Ok(decode_response(result.as_bytes(), id)?)
    }
}
impl Transport for BrowserFetchTransport {
    async fn request(
        &self,
        endpoint: &Endpoint,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        self.send(endpoint, method, params).await.map_err(rpc_error)
    }
}

/// Explicit adapter for an application-selected Quai EIP-1193 provider object.
///
/// The configured endpoint is an exact routing identity, not a fetched URL. The
/// encoded shard is included in every request as in pinned quais.js. Construction
/// never requests accounts or signatures. Generic Transport is read-only; explicit
/// methods are the only route to supported permission-sensitive wallet requests.
#[derive(Clone)]
pub struct InjectedProvider {
    provider: JsValue,
    watch: Rc<ProviderWatch>,
    endpoint: Endpoint,
    shard: Shard,
    expected_chain: U256,
    config: BrowserConfig,
    active: Rc<Cell<usize>>,
}
impl fmt::Debug for InjectedProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InjectedProvider")
            .field("endpoint", &self.endpoint)
            .field("shard", &self.shard)
            .field("expected_chain", &self.expected_chain)
            .finish_non_exhaustive()
    }
}
impl InjectedProvider {
    /// Bind an explicitly selected JS provider, endpoint identity, shard and expected chain.
    /// No global provider discovery or wallet prompt occurs.
    pub fn new(
        provider: JsValue,
        endpoint: Endpoint,
        shard: Shard,
        expected_chain: U256,
        config: BrowserConfig,
    ) -> Result<Self, BrowserError> {
        config.validate()?;
        if !validate_provider(&provider).map_err(|_| BrowserError::InvalidConfig)? {
            return Err(BrowserError::InvalidConfig);
        }
        let watch = Rc::new(ProviderWatch(watch_provider(&provider).map_err(convert)?));
        Ok(Self {
            provider,
            watch,
            endpoint,
            shard,
            expected_chain,
            config,
            active: Rc::new(Cell::new(0)),
        })
    }
    /// Monotonic account/chain/disconnect revision shared by adapter clones.
    /// Applications can invalidate displayed balances and cached authorization on change.
    pub fn context_revision(&self) -> u32 {
        provider_revision(&self.watch.0)
    }
    /// Whether this provider exposes removable EIP-1193 event listeners.
    pub fn monitors_context_events(&self) -> bool {
        provider_events_supported(&self.watch.0)
    }
    async fn verify_context(
        &self,
        revision: u32,
        account: Option<QuaiAddress>,
    ) -> Result<(), BrowserError> {
        if provider_changed(&self.watch.0, revision) {
            return Err(BrowserError::ContextChanged);
        }
        if !self.monitors_context_events() {
            // Request-only providers cannot report transient changes. Recheck their
            // current chain and exposed account before releasing a result.
            self.check_chain().await?;
            if let Some(account) = account
                && !Self::accounts_from(self.raw("quai_accounts", json!([])).await?)?
                    .contains(&account)
            {
                return Err(BrowserError::AccountUnavailable);
            }
        }
        if provider_changed(&self.watch.0, revision) {
            return Err(BrowserError::ContextChanged);
        }
        Ok(())
    }
    async fn raw(&self, method: &str, params: Value) -> Result<Value, BrowserError> {
        let _permit = permit(&self.active, self.config.max_in_flight)?;
        let payload = encode(
            &json!({"method":method,"params":params,"shard":self.shard.encoded()}),
            self.config.max_request_bytes,
        )?;
        let guard = AbortGuard::new()?;
        let result = injected_json(
            &self.provider,
            &payload,
            &guard.0,
            self.config.request_timeout_ms,
            self.config.max_response_bytes,
        )
        .await
        .map_err(convert)?;
        serde_json::from_str(&result_string(result)?).map_err(|_| BrowserError::InvalidResult)
    }
    async fn check_chain(&self) -> Result<Value, BrowserError> {
        let value = self.raw("quai_chainId", json!([])).await?;
        let chain = value
            .as_str()
            .ok_or(BrowserError::InvalidResult)
            .and_then(|value| parse_quantity(value).map_err(|_| BrowserError::InvalidResult))?;
        if chain != self.expected_chain {
            return Err(BrowserError::ChainMismatch);
        }
        Ok(value)
    }
    /// Read through the fixed injected wallet after a fresh chain check.
    /// Account/signing/sending permission methods cannot pass this allowlist.
    pub async fn read(
        &self,
        endpoint: &Endpoint,
        method: &str,
        params: Value,
    ) -> Result<Value, BrowserError> {
        if endpoint != &self.endpoint || !request_valid(method, &params) {
            return Err(BrowserError::InvalidConfig);
        }
        if !read_method(method) {
            return Err(BrowserError::AuthorizationRequired);
        }
        let revision = self.context_revision();
        let chain = self.check_chain().await?;
        if method == "quai_chainId" {
            if provider_changed(&self.watch.0, revision) {
                return Err(BrowserError::ContextChanged);
            }
            return Ok(chain);
        }
        let result = self.raw(method, params).await?;
        self.verify_context(revision, None).await?;
        Ok(result)
    }
    fn accounts_from(value: Value) -> Result<Vec<QuaiAddress>, BrowserError> {
        let values = value
            .as_array()
            .filter(|values| values.len() <= 1024)
            .ok_or(BrowserError::InvalidResult)?;
        let mut accounts = Vec::with_capacity(values.len());
        for value in values {
            let address = value
                .as_str()
                .ok_or(BrowserError::InvalidResult)?
                .parse::<QuaiAddress>()
                .map_err(|_| BrowserError::InvalidResult)?;
            if accounts.contains(&address) {
                return Err(BrowserError::InvalidResult);
            }
            accounts.push(address)
        }
        Ok(accounts)
    }
    /// Request wallet account access explicitly. Call only in the application's user-approved flow.
    /// User rejection preserves numeric EIP-1193 code 4001. No retry occurs.
    pub async fn request_accounts(&self) -> Result<Vec<QuaiAddress>, BrowserError> {
        self.check_chain().await?;
        let accounts = Self::accounts_from(self.raw("quai_requestAccounts", json!([])).await?)?;
        self.check_chain().await?;
        Ok(accounts)
    }
    /// Read accounts already exposed by the wallet; never requests additional permission.
    pub async fn accounts(&self) -> Result<Vec<QuaiAddress>, BrowserError> {
        let revision = self.context_revision();
        self.check_chain().await?;
        let accounts = Self::accounts_from(self.raw("quai_accounts", json!([])).await?)?;
        self.verify_context(revision, None).await?;
        Ok(accounts)
    }
    /// Explicitly ask the wallet to sign these bytes with personal_sign.
    ///
    /// Checks the configured chain, account exposure and sender zone before calling.
    /// Recovers the exact message signature and requires the requested account.
    /// Dropping this future cannot dismiss or revoke
    /// a wallet approval already presented by the injected provider.
    pub async fn personal_sign(
        &self,
        address: QuaiAddress,
        message: &[u8],
    ) -> Result<[u8; 65], BrowserError> {
        if self.shard != Shard::Zone(address.zone())
            || message.len() > self.config.max_request_bytes / 2
        {
            return Err(BrowserError::InvalidConfig);
        }
        let revision = self.context_revision();
        self.check_chain().await?;
        if !Self::accounts_from(self.raw("quai_accounts", json!([])).await?)?.contains(&address) {
            return Err(BrowserError::AccountUnavailable);
        }
        let mut hex = String::with_capacity(2 + message.len() * 2);
        hex.push_str("0x");
        const DIGITS: &[u8] = b"0123456789abcdef";
        for byte in message {
            hex.push(DIGITS[(byte >> 4) as usize] as char);
            hex.push(DIGITS[(byte & 15) as usize] as char)
        }
        if provider_changed(&self.watch.0, revision) {
            return Err(BrowserError::ContextChanged);
        }
        let value = self
            .raw(
                "personal_sign",
                json!([hex, address.to_string().to_ascii_lowercase()]),
            )
            .await?;
        self.verify_context(revision, Some(address)).await?;
        Self::verified_signature(value, address, &hash_message(message))
    }
    /// Explicitly request EIP-712 v4 signing after domain, chain and account checks.
    /// The immutable document determines both the serialized request and recovered
    /// signature hash. No implicit permission request or automatic retry occurs.
    pub async fn sign_typed_data(
        &self,
        address: QuaiAddress,
        data: &quai_signer::TypedData,
        policy: quai_signer::DomainPolicy,
    ) -> Result<[u8; 65], BrowserError> {
        if self.shard != Shard::Zone(address.zone()) {
            return Err(BrowserError::InvalidConfig);
        }
        policy
            .validate(data, self.expected_chain)
            .map_err(|_| BrowserError::ChainMismatch)?;
        let document = data
            .to_rpc_json()
            .map_err(|_| BrowserError::InvalidResult)?;
        let revision = self.context_revision();
        self.check_chain().await?;
        if !Self::accounts_from(self.raw("quai_accounts", json!([])).await?)?.contains(&address) {
            return Err(BrowserError::AccountUnavailable);
        }
        if provider_changed(&self.watch.0, revision) {
            return Err(BrowserError::ContextChanged);
        }
        let result = self
            .raw(
                "quai_signTypedData_v4",
                json!([address.to_string().to_ascii_lowercase(), document]),
            )
            .await?;
        self.verify_context(revision, Some(address)).await?;
        Self::verified_signature(result, address, data.signing_hash().bytes())
    }
    fn verified_signature(
        value: Value,
        address: QuaiAddress,
        digest: &[u8; 32],
    ) -> Result<[u8; 65], BrowserError> {
        let encoded = value
            .as_str()
            .filter(|s| {
                s.len() == 132
                    && s.starts_with("0x")
                    && s.as_bytes()[2..].iter().all(u8::is_ascii_hexdigit)
            })
            .ok_or(BrowserError::InvalidResult)?;
        let mut signature = [0; 65];
        for (index, byte) in signature.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&encoded[2 + index * 2..4 + index * 2], 16)
                .map_err(|_| BrowserError::InvalidResult)?;
        }
        let parsed = RecoverableSignature::from_quais_bytes(&signature)
            .map_err(|_| BrowserError::InvalidResult)?;
        let recovered = parsed
            .recover_prehash(digest)
            .map_err(|_| BrowserError::InvalidResult)?;
        if recovered.address() != address.address() {
            return Err(BrowserError::InvalidResult);
        }
        Ok(signature)
    }

    /// Explicitly request signing of an exact, fully populated type-0 Quai
    /// transaction. Does not estimate, fill fields, request accounts or broadcast.
    /// Canonical returned protobuf must recover the requested exposed account
    /// and match every unsigned field, including ordered access-list entries.
    /// Dropping the future cannot retract an already displayed wallet approval.
    pub async fn sign_quai_transaction(
        &self,
        address: QuaiAddress,
        transaction: &quai_consensus::QuaiTransaction,
    ) -> Result<quai_consensus::SignedQuaiTransaction, BrowserError> {
        if transaction.chain_id != self.expected_chain || transaction.chain_id == U256::ZERO {
            return Err(BrowserError::ChainMismatch);
        }
        if self.shard != Shard::Zone(address.zone()) {
            return Err(BrowserError::InvalidConfig);
        }
        transaction
            .unsigned_bytes()
            .map_err(|_| BrowserError::InvalidConfig)?;
        let access_list: Vec<_> = transaction.access_list.iter().map(|entry| {
            json!({"address":entry.address.to_string(),"storageKeys":entry.storage_keys.iter().map(ToString::to_string).collect::<Vec<_>>()})
        }).collect();
        let mut rpc = json!({
            "type":"0x0", "chainId":format!("{:#x}",transaction.chain_id),
            "from":address.to_string().to_ascii_lowercase(),
            "nonce":format!("{:#x}",transaction.nonce),
            "value":format!("{:#x}",transaction.value),
            "gas":format!("{:#x}",transaction.gas_limit),
            "gasPrice":format!("{:#x}",transaction.gas_price),
            "data":quai_primitives::hexlify(&transaction.data).map_err(|_|BrowserError::InvalidConfig)?,
            "accessList":access_list,
        });
        if let Some(to) = transaction.to {
            rpc["to"] = json!(to.to_string().to_ascii_lowercase());
        }
        let params = json!([rpc]);
        // Check the exact outgoing envelope before even passive wallet reads.
        encode(
            &json!({"method":"quai_signTransaction","params":params,"shard":self.shard.encoded()}),
            self.config.max_request_bytes,
        )?;
        let revision = self.context_revision();
        self.check_chain().await?;
        if !Self::accounts_from(self.raw("quai_accounts", json!([])).await?)?.contains(&address) {
            return Err(BrowserError::AccountUnavailable);
        }
        if provider_changed(&self.watch.0, revision) {
            return Err(BrowserError::ContextChanged);
        }
        let value = self.raw("quai_signTransaction", params).await?;
        self.verify_context(revision, Some(address)).await?;
        let bytes = value
            .as_str()
            .ok_or(BrowserError::InvalidResult)
            .and_then(|text| {
                quai_primitives::get_bytes(text).map_err(|_| BrowserError::InvalidResult)
            })?;
        let signed = quai_consensus::SignedQuaiTransaction::decode(&bytes)
            .map_err(|_| BrowserError::InvalidResult)?;
        if signed.from() != address || signed.transaction() != transaction {
            return Err(BrowserError::InvalidResult);
        }
        Ok(signed)
    }
}
impl Transport for InjectedProvider {
    async fn request(
        &self,
        endpoint: &Endpoint,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        self.read(endpoint, method, params).await.map_err(rpc_error)
    }
}

/// Fill at most 65,536 bytes using secure-context Web Crypto, with no fallback PRNG.
/// No wallet key storage or generation is performed automatically.
pub fn fill_random(destination: &mut [u8]) -> Result<(), BrowserError> {
    if destination.len() > 65_536 {
        return Err(BrowserError::EntropyUnavailable);
    }
    let bytes = random_bytes(destination.len()).map_err(|_| BrowserError::EntropyUnavailable)?;
    if bytes.length() as usize != destination.len() {
        return Err(BrowserError::EntropyUnavailable);
    }
    bytes.copy_to(destination);
    Ok(())
}
