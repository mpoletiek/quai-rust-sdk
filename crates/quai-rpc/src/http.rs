use crate::{Endpoint, RpcError, Transport, transport::decode_response};
use serde_json::{Value, json};
use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::Semaphore;

/// Limits applied to every HTTP request.
#[derive(Clone, Debug)]
pub struct HttpConfig {
    /// Entire request deadline, including concurrency-queue time.
    pub timeout: Duration,
    /// Connection establishment deadline, capped by the overall deadline.
    pub connect_timeout: Duration,
    /// Maximum decoded JSON body size in bytes.
    pub max_response_bytes: usize,
    /// Maximum simultaneous network requests per cloned transport group.
    pub max_in_flight: usize,
}
impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            connect_timeout: Duration::from_secs(3),
            max_response_bytes: 2 * 1024 * 1024,
            max_in_flight: 16,
        }
    }
}

/// Native pooled HTTP transport with no redirects or automatic retries.
#[derive(Clone)]
pub struct HttpTransport {
    inner: Arc<Inner>,
}
struct Inner {
    client: reqwest::Client,
    config: HttpConfig,
    permits: Semaphore,
    next_id: AtomicU64,
}
impl fmt::Debug for HttpTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpTransport")
            .field("config", &self.inner.config)
            .finish_non_exhaustive()
    }
}

impl HttpTransport {
    /// Create a rustls-backed client. System proxy environment variables are not used.
    pub fn new(config: HttpConfig) -> Result<Self, RpcError> {
        if config.timeout.is_zero()
            || config.connect_timeout.is_zero()
            || config.max_response_bytes == 0
            || config.max_in_flight == 0
            || config.max_in_flight > Semaphore::MAX_PERMITS
        {
            return Err(RpcError::InvalidConfig);
        }
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .connect_timeout(config.connect_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .user_agent(concat!("quai-rust-sdk/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| RpcError::InvalidConfig)?;
        Ok(Self {
            inner: Arc::new(Inner {
                client,
                permits: Semaphore::new(config.max_in_flight),
                config,
                next_id: AtomicU64::new(1),
            }),
        })
    }
    async fn post_json(&self, endpoint: &Endpoint, body: &Value) -> Result<Vec<u8>, RpcError> {
        let map_error = |e: reqwest::Error| {
            if e.is_timeout() {
                RpcError::Timeout
            } else {
                RpcError::Transport
            }
        };
        let mut response = self
            .inner
            .client
            .post(endpoint.as_str())
            .json(body)
            .send()
            .await
            .map_err(map_error)?;
        if !response.status().is_success() {
            return Err(RpcError::HttpStatus(response.status().as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|n| n > self.inner.config.max_response_bytes as u64)
        {
            return Err(RpcError::ResponseTooLarge);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(map_error)? {
            if chunk.len()
                > self
                    .inner
                    .config
                    .max_response_bytes
                    .saturating_sub(bytes.len())
            {
                return Err(RpcError::ResponseTooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
}

impl Transport for HttpTransport {
    async fn request_batch(
        &self,
        endpoint: &Endpoint,
        requests: Vec<(&str, Value)>,
    ) -> Option<crate::BatchResult> {
        Some(self.batch(endpoint, requests).await)
    }
    async fn request(
        &self,
        endpoint: &Endpoint,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        if !matches!(endpoint.scheme(), "http" | "https")
            || method.is_empty()
            || !(params.is_array() || params.is_object())
        {
            return Err(RpcError::InvalidConfig);
        }
        let work = async {
            let _permit = self
                .inner
                .permits
                .acquire()
                .await
                .map_err(|_| RpcError::Transport)?;
            let id = self
                .inner
                .next_id
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
                .map_err(|_| RpcError::RequestIdExhausted)?;
            let body = json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params});
            let bytes = self.post_json(endpoint, &body).await?;
            decode_response(&bytes, id)
        };
        tokio::time::timeout(self.inner.config.timeout, work)
            .await
            .map_err(|_| RpcError::Timeout)?
    }
}

/// Maximum calls accepted in one JSON-RPC batch.
///
/// Exported so a caller sizing its own pages can assert against it rather than
/// duplicating the number. `quai-provider` does exactly that.
pub const MAX_BATCH_CALLS: usize = 128;

impl HttpTransport {
    /// Submit one explicit JSON-RPC batch of 1..=128 calls, without retries.
    /// Results retain request order even when the server reorders responses.
    /// Malformed, duplicate, missing or foreign IDs reject the entire batch.
    /// The response-size, concurrency and total-deadline limits apply to the
    /// whole batch. A failed batch containing writes must not be replayed blindly.
    /// Encoded batch requests are limited to 2 MiB before network I/O.
    pub async fn batch(
        &self,
        endpoint: &Endpoint,
        requests: Vec<(&str, Value)>,
    ) -> crate::BatchResult {
        if !matches!(endpoint.scheme(), "http" | "https")
            || requests.is_empty()
            || requests.len() > MAX_BATCH_CALLS
            || requests.iter().any(|(method, params)| {
                method.is_empty() || !(params.is_array() || params.is_object())
            })
        {
            return Err(RpcError::InvalidConfig);
        }
        let work = async {
            let _permit = self
                .inner
                .permits
                .acquire()
                .await
                .map_err(|_| RpcError::Transport)?;
            let count = requests.len();
            let first = self
                .inner
                .next_id
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                    n.checked_add(count as u64)
                })
                .map_err(|_| RpcError::RequestIdExhausted)?;
            let body = Value::Array(requests.into_iter().enumerate().map(|(i,(method,params))|json!({"jsonrpc":"2.0","id":first+i as u64,"method":method,"params":params})).collect());
            if serde_json::to_vec(&body)
                .map_err(|_| RpcError::InvalidConfig)?
                .len()
                > 2 * 1024 * 1024
            {
                return Err(RpcError::RequestTooLarge);
            }
            let bytes = self.post_json(endpoint, &body).await?;
            decode_batch(&bytes, first, count)
        };
        tokio::time::timeout(self.inner.config.timeout, work)
            .await
            .map_err(|_| RpcError::Timeout)?
    }
}

pub(crate) fn decode_batch(bytes: &[u8], first: u64, count: usize) -> crate::BatchResult {
    let rows: Vec<Box<serde_json::value::RawValue>> =
        serde_json::from_slice(bytes).map_err(|_| RpcError::InvalidResponse("batch array"))?;
    if rows.len() != count {
        return Err(RpcError::InvalidResponse("batch response count"));
    }
    let mut ordered: Vec<Option<Result<Value, RpcError>>> = (0..count).map(|_| None).collect();
    for row in rows {
        let value: Value = serde_json::from_str(row.get())
            .map_err(|_| RpcError::InvalidResponse("batch response"))?;
        let id = value
            .get("id")
            .and_then(Value::as_u64)
            .ok_or(RpcError::InvalidResponse("batch ID"))?;
        let offset = id
            .checked_sub(first)
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| *n < count)
            .ok_or(RpcError::InvalidResponse("foreign batch ID"))?;
        if ordered[offset].is_some() {
            return Err(RpcError::InvalidResponse("duplicate batch ID"));
        }
        // Decode the original raw object so duplicate envelope fields are rejected.
        let result = decode_response(row.get().as_bytes(), id);
        match result {
            Ok(_) | Err(RpcError::Remote(_)) => ordered[offset] = Some(result),
            Err(error) => return Err(error),
        }
    }
    ordered
        .into_iter()
        .map(|row| row.ok_or(RpcError::InvalidResponse("missing batch ID")))
        .collect()
}

#[cfg(test)]
mod batch_tests {
    use super::*;
    #[test]
    fn batch_reorders_results_and_preserves_remote_errors() {
        let rows = decode_batch(br#"[{"jsonrpc":"2.0","id":8,"error":{"code":-32000,"message":"missing"}},{"jsonrpc":"2.0","id":7,"result":null}]"#,7,2).unwrap();
        assert!(matches!(&rows[0], Ok(Value::Null)));
        assert!(matches!(&rows[1], Err(RpcError::Remote(_))));
    }
    #[test]
    fn batch_rejects_duplicate_missing_foreign_and_ambiguous_envelopes() {
        for value in [
            r#"[{"jsonrpc":"2.0","id":7,"result":0},{"jsonrpc":"2.0","id":7,"result":0}]"#,
            r#"[{"jsonrpc":"2.0","id":7,"result":0}]"#,
            r#"[{"jsonrpc":"2.0","id":7,"result":0},{"jsonrpc":"2.0","id":9,"result":0}]"#,
            r#"[{"jsonrpc":"2.0","id":7,"result":0},{"jsonrpc":"2.0","id":8,"id":8,"result":0}]"#,
            r#"[{"jsonrpc":"2.0","id":7,"result":0},{"jsonrpc":"2.0","id":8,"result":0,"error":{"code":-1,"message":"bad"}}]"#,
        ] {
            assert!(decode_batch(value.as_bytes(), 7, 2).is_err());
        }
    }
}
