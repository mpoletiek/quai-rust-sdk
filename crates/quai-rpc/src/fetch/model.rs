use serde_json::Value;
use std::{collections::BTreeMap, fmt};
use url::Url;
use zeroize::{Zeroize, Zeroizing};
/// Maximum request or response body accepted by resource models.
pub const MAX_FETCH_BYTES: usize = 1_048_576;
/// Maximum aggregate header name/value bytes.
pub const MAX_FETCH_HEADERS: usize = 16_384;
/// Redacted resource errors; arbitrary URLs, credentials and remote bodies are omitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum FetchError {
    /// Invalid URL, method, header, configuration or encoded data.
    #[error("invalid resource request or data")]
    Invalid,
    /// A declared body/header/attempt bound was exceeded.
    #[error("resource limit exceeded")]
    Limit,
    /// Unsupported protocol or redirect policy.
    #[error("unsupported resource operation")]
    Unsupported,
    /// Credentials require HTTPS unless the request explicitly opts in.
    #[error("resource authentication requires a secure endpoint")]
    InsecureAuthentication,
    /// Network or platform failure.
    #[error("resource transport failed")]
    Transport,
    /// Overall deadline expired, including hooks, retries and delays.
    #[error("resource request timed out")]
    Timeout,
    /// The caller cancelled the operation.
    #[error("resource request cancelled")]
    Cancelled,
    /// A cancellation token already has an active request.
    #[error("resource cancellation token already in use")]
    Active,
    /// Response status was not successful; body remains available on the response.
    #[error("resource HTTP status {0}")]
    Status(u16),
}
fn token(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}
/// Header names whose values carry a credential and are wiped on drop.
///
/// Stored lowercase, which is how `set` normalizes every name.
const SENSITIVE_HEADERS: [&str; 3] = ["authorization", "proxy-authorization", "cookie"];

/// Case-insensitive, bounded HTTP headers. Diagnostics redact names and values.
///
/// `authorization`, `proxy-authorization` and `cookie` values are zeroized on
/// drop, so the copies created by cloning a request (once per retry attempt) do
/// not outlive their owner. This cannot erase copies a caller made itself, or
/// transients created inside the transport that ultimately sends the request.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct FetchHeaders(BTreeMap<String, String>);
impl Drop for FetchHeaders {
    fn drop(&mut self) {
        for name in SENSITIVE_HEADERS {
            if let Some(value) = self.0.get_mut(name) {
                value.zeroize();
            }
        }
    }
}
impl fmt::Debug for FetchHeaders {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FetchHeaders")
            .field("count", &self.0.len())
            .finish_non_exhaustive()
    }
}
impl FetchHeaders {
    /// Validate then insert/replace a header atomically (128 fields, 16 KiB total).
    pub fn set(&mut self, name: &str, value: &str) -> Result<(), FetchError> {
        if name.len() > 128
            || !token(name)
            || value
                .bytes()
                .any(|b| b < 32 && b != b'\t' || b == 127 || !b.is_ascii())
        {
            return Err(FetchError::Invalid);
        }
        let name = name.to_ascii_lowercase();
        let old = self.0.get(&name).map_or(0, |v| name.len() + v.len());
        let used: usize = self.0.iter().map(|(k, v)| k.len() + v.len()).sum();
        if value.len() > MAX_FETCH_HEADERS
            || used - old + name.len() + value.len() > MAX_FETCH_HEADERS
            || (self.0.len() == 128 && !self.0.contains_key(&name))
        {
            return Err(FetchError::Limit);
        }
        self.0.insert(name, value.into());
        Ok(())
    }
    /// Borrow a value using a case-insensitive name.
    pub fn get(&self, name: &str) -> Option<&str> {
        if name.len() > 128 {
            return None;
        }
        self.0.get(&name.to_ascii_lowercase()).map(String::as_str)
    }
    /// Remove one field.
    pub fn remove(&mut self, name: &str) {
        if name.len() > 128 {
            return;
        }
        self.0.remove(&name.to_ascii_lowercase());
    }
    /// Remove all fields.
    pub fn clear(&mut self) {
        self.0.clear();
    }
    /// Explicit iteration exposes caller/server-supplied values.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
}
/// An owned resource request. Debug never prints URL, headers, or body.
#[derive(Clone)]
#[non_exhaustive]
pub struct FetchRequest {
    url: String,
    method: Option<String>,
    body: Option<Vec<u8>>,
    intrinsic_type: Option<&'static str>,
    headers: FetchHeaders,
    /// Request gzip on native; browser compression negotiation is platform-owned.
    pub allow_gzip: bool,
    /// Explicit permission to send authentication headers over HTTP.
    pub allow_insecure_authentication: bool,
}
impl fmt::Debug for FetchRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FetchRequest")
            .field("body_bytes", &self.body.as_ref().map(Vec::len))
            .finish_non_exhaustive()
    }
}
impl FetchRequest {
    /// Create a GET request. HTTP(S) URLs reject userinfo/fragments; custom schemes
    /// are resolved only by explicit client hooks. Data URLs retain exact bytes.
    pub fn new(url: &str) -> Result<Self, FetchError> {
        validate_url(url)?;
        Ok(Self {
            url: url.into(),
            method: None,
            body: None,
            intrinsic_type: None,
            headers: FetchHeaders::default(),
            allow_gzip: true,
            allow_insecure_authentication: false,
        })
    }
    /// Borrow the explicit URL; it may contain private query values.
    pub fn url(&self) -> &str {
        &self.url
    }
    /// Replace a validated URL without changing method/body. Authentication is
    /// rechecked at execution; use redirect for redirect credential isolation.
    pub fn set_url(&mut self, url: &str) -> Result<(), FetchError> {
        validate_url(url)?;
        self.url = url.into();
        Ok(())
    }
    /// GET without a body, POST with a body, unless explicitly overridden.
    pub fn method(&self) -> &str {
        self.method
            .as_deref()
            .unwrap_or(if self.body.is_some() { "POST" } else { "GET" })
    }
    /// Set an explicit method, or restore body-dependent selection with None.
    pub fn set_method(&mut self, method: Option<&str>) -> Result<(), FetchError> {
        if let Some(m) = method {
            if m.len() > 32 || !token(m) {
                return Err(FetchError::Invalid);
            }
            self.method = Some(m.to_ascii_uppercase());
        } else {
            self.method = None;
        }
        Ok(())
    }
    /// Borrow an optional body. Some(empty) differs from no body.
    pub fn body(&self) -> Option<&[u8]> {
        self.body.as_deref()
    }
    /// Replace with bounded bytes and intrinsic application/octet-stream type.
    pub fn set_bytes(&mut self, bytes: &[u8]) -> Result<(), FetchError> {
        self.set_body(bytes, "application/octet-stream")
    }
    /// Replace with bounded UTF-8 bytes and intrinsic text/plain type.
    pub fn set_text(&mut self, text: &str) -> Result<(), FetchError> {
        self.set_body(text.as_bytes(), "text/plain")
    }
    /// Encode a bounded JSON value (depth 64 and 65,536 nodes) before replacing body.
    pub fn set_json(&mut self, value: &Value) -> Result<(), FetchError> {
        fn count(v: &Value, d: usize, n: &mut usize, b: &mut usize) -> Result<(), FetchError> {
            if d > 64 || *n == 0 {
                return Err(FetchError::Limit);
            }
            *n -= 1;
            match v {
                Value::String(s) => *b = b.checked_sub(s.len()).ok_or(FetchError::Limit)?,
                Value::Array(a) => {
                    for v in a {
                        count(v, d + 1, n, b)?
                    }
                }
                Value::Object(o) => {
                    for (k, v) in o {
                        *b = b.checked_sub(k.len()).ok_or(FetchError::Limit)?;
                        count(v, d + 1, n, b)?
                    }
                }
                _ => {}
            }
            Ok(())
        }
        let mut nodes = 65_536;
        let mut bytes_left = MAX_FETCH_BYTES;
        count(value, 0, &mut nodes, &mut bytes_left)?;
        // Escaping may expand strings up to sixfold, still bounded by the preflight.
        let bytes = serde_json::to_vec(value).map_err(|_| FetchError::Invalid)?;
        self.set_body(&bytes, "application/json")
    }
    fn set_body(&mut self, bytes: &[u8], kind: &'static str) -> Result<(), FetchError> {
        if bytes.len() > MAX_FETCH_BYTES {
            return Err(FetchError::Limit);
        }
        self.body = Some(bytes.to_vec());
        self.intrinsic_type = Some(kind);
        Ok(())
    }
    /// Clear body and intrinsic type; explicitly supplied headers are retained.
    pub fn clear_body(&mut self) {
        self.body = None;
        self.intrinsic_type = None;
    }
    /// Mutate explicit headers through their validated API.
    pub fn headers_mut(&mut self) -> &mut FetchHeaders {
        &mut self.headers
    }
    /// Effective headers, including content type and compression policy.
    pub fn headers(&self) -> Result<FetchHeaders, FetchError> {
        let mut h = self.headers.clone();
        if h.get("content-type").is_none()
            && let Some(t) = self.intrinsic_type
        {
            h.set("content-type", t)?;
        }
        if h.get("accept-encoding").is_none() {
            h.set(
                "accept-encoding",
                if self.allow_gzip { "gzip" } else { "identity" },
            )?;
        }
        Ok(h)
    }
    /// Set an explicit Basic Authorization value. Username must not contain ':'.
    pub fn set_credentials(&mut self, username: &str, password: &str) -> Result<(), FetchError> {
        if username.contains(':') || username.len() > 4096 || password.len() > 4096 {
            return Err(FetchError::Invalid);
        }
        // Build the value through guards rather than `format!` temporaries.
        // A plain `format!` leaves the credential, and its trivially reversible
        // Base64, in heap buffers that are never wiped; `FetchRequest` is Clone
        // and the retry path clones it once per attempt, so each unguarded
        // temporary is multiplied across a request's lifetime.
        let mut joined = Zeroizing::new(Vec::with_capacity(username.len() + 1 + password.len()));
        joined.extend_from_slice(username.as_bytes());
        joined.push(b':');
        joined.extend_from_slice(password.as_bytes());
        let encoded =
            Zeroizing::new(quai_primitives::encode_base64(&joined).map_err(|_| FetchError::Limit)?);
        let mut value = Zeroizing::new(String::with_capacity(6 + encoded.len()));
        value.push_str("Basic ");
        value.push_str(&encoded);
        self.headers.set("authorization", &value)
    }
    /// Validate execution policy, including authentication and browser-compatible
    /// GET/HEAD body rules. Resource requests cannot override transport framing.
    pub fn validate(&self) -> Result<(), FetchError> {
        if matches!(self.method(), "GET" | "HEAD") && self.body.is_some() {
            return Err(FetchError::Invalid);
        }
        let h = self.headers()?;
        if ["host", "content-length", "transfer-encoding", "connection"]
            .iter()
            .any(|k| h.get(k).is_some())
        {
            return Err(FetchError::Invalid);
        }
        if self.scheme() == "http"
            && !self.allow_insecure_authentication
            && ["authorization", "proxy-authorization", "cookie"]
                .iter()
                .any(|k| h.get(k).is_some())
        {
            return Err(FetchError::InsecureAuthentication);
        }
        Ok(())
    }
    /// Create a GET/HEAD redirect, permitting relative locations but never HTTPS
    /// downgrade. Cross-origin redirects clear all explicit headers and opt-ins.
    pub fn redirect(&self, location: &str) -> Result<Self, FetchError> {
        if !matches!(self.method(), "GET" | "HEAD") || location.len() > 16_384 {
            return Err(FetchError::Unsupported);
        }
        let old = Url::parse(&self.url).map_err(|_| FetchError::Invalid)?;
        let target = old.join(location).map_err(|_| FetchError::Invalid)?;
        if !matches!(target.scheme(), "http" | "https")
            || old.scheme() == "https" && target.scheme() == "http"
        {
            return Err(FetchError::Unsupported);
        }
        let mut next = Self::new(target.as_str())?;
        next.set_method(Some(self.method()))?;
        next.allow_gzip = self.allow_gzip;
        if target.origin() == old.origin() {
            next.headers = self.headers.clone();
            next.allow_insecure_authentication = self.allow_insecure_authentication;
        }
        Ok(next)
    }
    /// Lowercase URL scheme (construction enforces lowercase scheme spelling).
    pub fn scheme(&self) -> &str {
        self.url.split_once(':').unwrap().0
    }
}
fn validate_url(url: &str) -> Result<(), FetchError> {
    if url.len() > MAX_FETCH_BYTES * 2 || url.bytes().any(|b| b < 32 || b == 127) {
        return Err(FetchError::Limit);
    }
    let (scheme, _) = url.split_once(':').ok_or(FetchError::Invalid)?;
    if scheme.is_empty()
        || scheme.len() > 32
        || !scheme.as_bytes()[0].is_ascii_lowercase()
        || !scheme
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"+-.".contains(&b))
    {
        return Err(FetchError::Invalid);
    }
    if matches!(scheme, "http" | "https") {
        if url.len() > 16_384 {
            return Err(FetchError::Limit);
        }
        let u = Url::parse(url).map_err(|_| FetchError::Invalid)?;
        if u.host_str().is_none()
            || !u.username().is_empty()
            || u.password().is_some()
            || u.fragment().is_some()
        {
            return Err(FetchError::Invalid);
        }
    }
    Ok(())
}
/// Immutable bounded response, with optional exact body and explicit request context.
#[derive(Clone)]
pub struct FetchResponse {
    status: u16,
    message: String,
    headers: FetchHeaders,
    body: Option<Vec<u8>>,
}
impl fmt::Debug for FetchResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FetchResponse")
            .field("status", &self.status)
            .field("body_bytes", &self.body.as_ref().map(Vec::len))
            .finish_non_exhaustive()
    }
}
impl FetchResponse {
    /// Construct a bounded response; status text is untrusted and never logged.
    pub fn new(
        status: u16,
        message: &str,
        headers: FetchHeaders,
        body: Option<Vec<u8>>,
    ) -> Result<Self, FetchError> {
        if !(100..=599).contains(&status)
            || message.len() > 1024
            || message.bytes().any(|b| b < 32 || b == 127)
        {
            return Err(FetchError::Invalid);
        }
        if body.as_ref().is_some_and(|b| b.len() > MAX_FETCH_BYTES) {
            return Err(FetchError::Limit);
        }
        Ok(Self {
            status,
            message: message.into(),
            headers,
            body,
        })
    }
    /// HTTP status code (including explicit synthetic 599).
    pub fn status(&self) -> u16 {
        self.status
    }
    /// Explicit untrusted status text.
    pub fn status_message(&self) -> &str {
        &self.message
    }
    /// Borrow validated response headers.
    pub fn headers(&self) -> &FetchHeaders {
        &self.headers
    }
    /// Borrow optional exact decoded body bytes.
    pub fn body(&self) -> Option<&[u8]> {
        self.body.as_deref()
    }
    /// Strict UTF-8 text; absence yields the empty string.
    pub fn text(&self) -> Result<&str, FetchError> {
        std::str::from_utf8(self.body().unwrap_or_default()).map_err(|_| FetchError::Invalid)
    }
    /// Bounded JSON parsing; absent body and invalid/deep JSON reject.
    pub fn json(&self) -> Result<Value, FetchError> {
        serde_json::from_slice(self.body().ok_or(FetchError::Invalid)?)
            .map_err(|_| FetchError::Invalid)
    }
    /// True only for status 200..299.
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
    /// Return a structured status error without discarding this response's body.
    pub fn assert_ok(&self) -> Result<(), FetchError> {
        if self.ok() {
            Ok(())
        } else {
            Err(FetchError::Status(self.status))
        }
    }
    /// Clone body/headers under explicit synthetic server-error status 599.
    pub fn server_error(&self) -> Self {
        Self {
            status: 599,
            message: "resource processing failed".into(),
            headers: self.headers.clone(),
            body: self.body.clone(),
        }
    }
}
/// Decode a strict data URI without network I/O (base64 or percent-encoded bytes).
pub fn data_resource(uri: &str) -> Result<FetchResponse, FetchError> {
    if uri.len() > MAX_FETCH_BYTES * 2 {
        return Err(FetchError::Limit);
    }
    let (meta, raw) = uri
        .strip_prefix("data:")
        .and_then(|s| s.split_once(','))
        .ok_or(FetchError::Invalid)?;
    let (kind, base64) = meta
        .strip_suffix(";base64")
        .map_or((meta, false), |s| (s, true));
    if kind.len() > 256 || kind.contains(';') {
        return Err(FetchError::Invalid);
    }
    let body = if base64 {
        quai_primitives::decode_base64(raw).map_err(|_| FetchError::Invalid)?
    } else {
        let mut out = Vec::new();
        let mut i = 0;
        let b = raw.as_bytes();
        while i < b.len() {
            if out.len() == MAX_FETCH_BYTES {
                return Err(FetchError::Limit);
            }
            if b[i] == b'%' {
                let pair = b.get(i + 1..i + 3).ok_or(FetchError::Invalid)?;
                let text = std::str::from_utf8(pair).map_err(|_| FetchError::Invalid)?;
                out.push(u8::from_str_radix(text, 16).map_err(|_| FetchError::Invalid)?);
                i += 3;
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        out
    };
    let mut h = FetchHeaders::default();
    h.set(
        "content-type",
        if kind.is_empty() { "text/plain" } else { kind },
    )?;
    FetchResponse::new(200, "OK", h, Some(body))
}
/// Resolve an unverified IPFS URI through a caller-selected HTTP(S) gateway.
/// Reject traversal and query/fragment injection; this does not verify CID content.
pub fn ipfs_resource(uri: &str, gateway: &str) -> Result<FetchRequest, FetchError> {
    if uri.len() > 16_384 || gateway.len() > 16_384 {
        return Err(FetchError::Limit);
    }
    let path = uri.strip_prefix("ipfs://").ok_or(FetchError::Invalid)?;
    let path = path.strip_prefix("ipfs/").unwrap_or(path);
    if path.is_empty()
        || path.split('/').any(|s| {
            s.is_empty()
                || s == "."
                || s == ".."
                || !s
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
    {
        return Err(FetchError::Invalid);
    }
    let base = FetchRequest::new(gateway)?;
    let u = Url::parse(base.url()).map_err(|_| FetchError::Invalid)?;
    if !matches!(base.scheme(), "http" | "https") || !gateway.ends_with('/') || u.query().is_some()
    {
        return Err(FetchError::Invalid);
    }
    FetchRequest::new(&format!("{gateway}{path}"))
}
