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
}

impl Transport for HttpTransport {
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
                .json(&body)
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
            decode_response(&bytes, id)
        };
        tokio::time::timeout(self.inner.config.timeout, work)
            .await
            .map_err(|_| RpcError::Timeout)?
    }
}
