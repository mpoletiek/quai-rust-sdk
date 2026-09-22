use super::*;
use std::{
    future::Future,
    sync::{Arc, Mutex},
    task::{Poll, Waker},
};
/// Native implementations support multithreaded executors.
#[cfg(not(target_arch = "wasm32"))]
pub trait FetchThreading: Send + Sync {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + Sync> FetchThreading for T {}
/// Browser implementations may own thread-local JavaScript handles.
#[cfg(target_arch = "wasm32")]
pub trait FetchThreading {}
#[cfg(target_arch = "wasm32")]
impl<T> FetchThreading for T {}
/// Native resource futures are Send.
#[cfg(not(target_arch = "wasm32"))]
pub trait FetchFuture: Future + Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Future + Send> FetchFuture for T {}
/// Browser resource futures do not require Send.
#[cfg(target_arch = "wasm32")]
pub trait FetchFuture: Future {}
#[cfg(target_arch = "wasm32")]
impl<T: Future> FetchFuture for T {}
/// Runtime adapter. Execute must make exactly one HTTP request without redirects
/// or retries, bound wire and decoded bodies, and cancel active I/O on future drop.
pub trait FetchBackend: FetchThreading {
    /// Monotonic milliseconds, finite and nonnegative.
    fn now(&self) -> Result<f64, FetchError>;
    /// An owned cancellable delay. Dropping the future must release timers.
    fn sleep(&self, milliseconds: u32) -> impl FetchFuture<Output = Result<(), FetchError>>;
    /// One HTTP(S) exchange. Non-success statuses are returned as response data.
    fn execute(
        &self,
        request: &FetchRequest,
        max_bytes: usize,
    ) -> impl FetchFuture<Output = Result<FetchResponse, FetchError>>;
}
/// Explicit processing result; retry is a decision, not a special thrown error.
pub enum FetchAction {
    /// Return this response to the caller.
    Return(FetchResponse),
    /// Request a bounded retry after this delay (subject to method and attempt policy).
    Retry {
        /// Response available to the retry hook or final caller.
        response: FetchResponse,
        /// Delay in milliseconds, at most the overall timeout.
        delay_ms: u32,
    },
}
/// A custom scheme resolves either to an HTTP request or an immediate response.
pub enum FetchGateway {
    /// Continue with exactly one HTTP(S) exchange.
    Request(FetchRequest),
    /// Return a locally resolved resource through the process hook.
    Response(FetchResponse),
}
/// Per-client immutable hook implementation; applications can hold their own
/// registry or middleware state. No mutable process-global registry is installed.
pub trait FetchHooks: FetchThreading {
    /// Resolve custom schemes. HTTP(S) cannot be replaced through a gateway hook.
    fn gateway(
        &self,
        _request: &FetchRequest,
    ) -> impl FetchFuture<Output = Result<Option<FetchGateway>, FetchError>> {
        async { Ok(None) }
    }
    /// Update headers/body before each actual exchange. Result is revalidated.
    fn preflight(
        &self,
        request: &FetchRequest,
    ) -> impl FetchFuture<Output = Result<FetchRequest, FetchError>> {
        async { Ok(request.clone()) }
    }
    /// Transform response or explicitly ask for a retry.
    fn process(
        &self,
        _request: &FetchRequest,
        response: FetchResponse,
    ) -> impl FetchFuture<Output = Result<FetchAction, FetchError>> {
        async { Ok(FetchAction::Return(response)) }
    }
    /// Permit/reject each proposed retry; attempt is the completed exchange count.
    fn retry(
        &self,
        _request: &FetchRequest,
        _response: &FetchResponse,
        _attempt: u32,
    ) -> impl FetchFuture<Output = Result<bool, FetchError>> {
        async { Ok(true) }
    }
}
/// Default hooks preserve all requests and responses.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoFetchHooks;
impl FetchHooks for NoFetchHooks {}
/// Independent resource policy; none of these settings change RPC transport behavior.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct FetchConfig {
    /// Overall timeout including hooks and delays, 1..=i32::MAX milliseconds.
    pub timeout_ms: u32,
    /// Maximum wire and decoded response bytes, 1..=1 MiB.
    pub max_response_bytes: usize,
    /// Maximum exchanges, including redirects/retries, 1..=12. Default one.
    pub max_attempts: u32,
    /// Maximum redirects, 0..=10. Default zero; only GET/HEAD may redirect.
    pub max_redirects: u32,
    /// Base deterministic exponential delay for 429, capped by the deadline.
    pub retry_delay_ms: u32,
    /// Explicit permission to retry methods other than GET/HEAD. Defaults false.
    pub retry_non_idempotent: bool,
}
impl FetchConfig {
    /// Replace `timeout_ms`.
    pub const fn with_timeout_ms(mut self, timeout_ms: u32) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }
    /// Replace `max_response_bytes`.
    pub const fn with_max_response_bytes(mut self, max_response_bytes: usize) -> Self {
        self.max_response_bytes = max_response_bytes;
        self
    }
    /// Replace `max_attempts`.
    pub const fn with_max_attempts(mut self, max_attempts: u32) -> Self {
        self.max_attempts = max_attempts;
        self
    }
    /// Replace `max_redirects`.
    pub const fn with_max_redirects(mut self, max_redirects: u32) -> Self {
        self.max_redirects = max_redirects;
        self
    }
    /// Replace `retry_delay_ms`.
    pub const fn with_retry_delay_ms(mut self, retry_delay_ms: u32) -> Self {
        self.retry_delay_ms = retry_delay_ms;
        self
    }
    /// Replace `retry_non_idempotent`.
    pub const fn with_retry_non_idempotent(mut self, retry_non_idempotent: bool) -> Self {
        self.retry_non_idempotent = retry_non_idempotent;
        self
    }
}
impl Default for FetchConfig {
    fn default() -> Self {
        Self {
            timeout_ms: 10_000,
            max_response_bytes: MAX_FETCH_BYTES,
            max_attempts: 1,
            max_redirects: 0,
            retry_delay_ms: 250,
            retry_non_idempotent: false,
        }
    }
}
impl FetchConfig {
    /// Check all bounds before hooks, timers or I/O.
    pub fn validate(self) -> Result<(), FetchError> {
        if self.timeout_ms == 0
            || self.timeout_ms > i32::MAX as u32
            || !(1..=MAX_FETCH_BYTES).contains(&self.max_response_bytes)
            || !(1..=12).contains(&self.max_attempts)
            || self.max_redirects > 10
            || self.retry_delay_ms > self.timeout_ms
        {
            return Err(FetchError::Invalid);
        }
        Ok(())
    }
}
#[derive(Default)]
struct CancelState {
    cancelled: bool,
    active: bool,
    waker: Option<Waker>,
}
/// Cloneable one-operation cancellation token. Cancel is permanent; a fresh token
/// is needed for another operation after cancellation. Never logs request data.
#[derive(Clone, Default)]
pub struct FetchCancellation(Arc<Mutex<CancelState>>);
impl FetchCancellation {
    /// Signal the active operation and wake it promptly. No remote rollback is implied.
    pub fn cancel(&self) {
        let wake = {
            let mut s = self.0.lock().unwrap();
            s.cancelled = true;
            s.waker.take()
        };
        if let Some(w) = wake {
            w.wake();
        }
    }
    /// True after cancellation, including before a send begins.
    pub fn cancelled(&self) -> bool {
        self.0.lock().unwrap().cancelled
    }
    fn begin(&self) -> Result<CancelGuard, FetchError> {
        let mut s = self.0.lock().unwrap();
        if s.cancelled {
            return Err(FetchError::Cancelled);
        }
        if s.active {
            return Err(FetchError::Active);
        }
        s.active = true;
        Ok(CancelGuard(self.clone()))
    }
    fn check(&self, waker: &Waker) -> Result<(), FetchError> {
        let mut s = self.0.lock().unwrap();
        if s.cancelled {
            return Err(FetchError::Cancelled);
        }
        if !s.waker.as_ref().is_some_and(|w| w.will_wake(waker)) {
            s.waker = Some(waker.clone());
        }
        Ok(())
    }
}
struct CancelGuard(FetchCancellation);
impl Drop for CancelGuard {
    fn drop(&mut self) {
        let mut s = self.0.0.lock().unwrap();
        s.active = false;
        s.waker = None;
    }
}
/// Result with explicit original and final request context; Debug stays redacted.
#[derive(Debug)]
#[non_exhaustive]
pub struct FetchedResource {
    /// Caller-supplied request before hooks/gateways/redirects.
    pub original: FetchRequest,
    /// Actual final request (or local resource request).
    pub request: FetchRequest,
    /// Exact bounded response after processing.
    pub response: FetchResponse,
    /// Number of completed HTTP exchanges (zero for a local data URI).
    pub attempts: u32,
}
/// Owned runtime and hooks with an immutable bounded policy.
pub struct FetchClient<B, H = NoFetchHooks> {
    backend: B,
    hooks: H,
    config: FetchConfig,
}
impl<B: FetchBackend> FetchClient<B> {
    /// Create a client using identity hooks.
    pub fn new(backend: B, config: FetchConfig) -> Result<Self, FetchError> {
        Self::with_hooks(backend, config, NoFetchHooks)
    }
}
impl<B: FetchBackend, H: FetchHooks> FetchClient<B, H> {
    /// Supply explicit hooks; no global registrations or locks are needed.
    pub fn with_hooks(backend: B, config: FetchConfig, hooks: H) -> Result<Self, FetchError> {
        config.validate()?;
        Ok(Self {
            backend,
            hooks,
            config,
        })
    }
    /// Borrow the fixed policy.
    pub fn config(&self) -> FetchConfig {
        self.config
    }
    /// Borrow the backend to share connection/runtime state explicitly.
    pub fn backend(&self) -> &B {
        &self.backend
    }
    /// Fetch under an overall deadline. Dropping this future cancels local I/O,
    /// hooks and delays; state-changing remote requests may already have executed.
    /// Transport failures are never automatically retried.
    pub async fn send(
        &self,
        request: &FetchRequest,
        cancel: &FetchCancellation,
    ) -> Result<FetchedResource, FetchError> {
        request.validate()?;
        let _guard = cancel.begin()?;
        let start = self.backend.now()?;
        if !start.is_finite() || start < 0.0 {
            return Err(FetchError::Transport);
        }
        let expired = || {
            let n = self.backend.now()?;
            if !n.is_finite() || n < start {
                return Err(FetchError::Transport);
            }
            Ok(n - start >= f64::from(self.config.timeout_ms))
        };
        let mut deadline = std::pin::pin!(self.backend.sleep(self.config.timeout_ms));
        let work = async {
            let mut req = request.clone();
            let mut attempts = 0;
            let mut redirects = 0;
            loop {
                let mut response = if !matches!(req.scheme(), "http" | "https") {
                    let gateway = self.hooks.gateway(&req).await?;
                    let gateway = match gateway {
                        Some(g) => g,
                        None if req.scheme() == "data" => {
                            FetchGateway::Response(data_resource(req.url())?)
                        }
                        _ => return Err(FetchError::Unsupported),
                    };
                    match gateway {
                        FetchGateway::Response(response) => Some(response),
                        FetchGateway::Request(next) => {
                            if !matches!(next.scheme(), "http" | "https") {
                                return Err(FetchError::Unsupported);
                            }
                            req = next;
                            None
                        }
                    }
                } else {
                    None
                };
                if response.is_none() {
                    req = self.hooks.preflight(&req).await?;
                    req.validate()?;
                    if !matches!(req.scheme(), "http" | "https") {
                        return Err(FetchError::Unsupported);
                    }
                    response = Some(
                        self.backend
                            .execute(&req, self.config.max_response_bytes)
                            .await?,
                    );
                    attempts += 1;
                }
                let response = response.unwrap();
                if response
                    .body()
                    .is_some_and(|b| b.len() > self.config.max_response_bytes)
                {
                    return Err(FetchError::Limit);
                }
                if matches!(response.status(), 301 | 302 | 303 | 307 | 308)
                    && redirects < self.config.max_redirects
                    && attempts < self.config.max_attempts
                    && let Some(location) = response.headers().get("location")
                {
                    // Rejected redirects are returned, never followed or treated as transport failure.
                    if let Ok(next) = req.redirect(location) {
                        req = next;
                        redirects += 1;
                        continue;
                    }
                }
                let action = self.hooks.process(&req, response).await?;
                let (response, delay) = match action {
                    FetchAction::Return(r) => {
                        let delay = if r.status() == 429 {
                            // HTTP Retry-After delta is seconds, correcting the source's millisecond interpretation.
                            let delay = match r.headers().get("retry-after") {
                                Some(s)
                                    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) =>
                                {
                                    s.parse::<u64>()
                                        .ok()
                                        .and_then(|n| n.checked_mul(1000))
                                        .unwrap_or(u64::MAX)
                                }
                                _ => {
                                    u64::from(self.config.retry_delay_ms)
                                        * (1u64 << attempts.saturating_sub(1).min(11))
                                }
                            };
                            Some(delay)
                        } else {
                            None
                        };
                        (r, delay)
                    }
                    FetchAction::Retry { response, delay_ms } => {
                        (response, Some(u64::from(delay_ms)))
                    }
                };
                if response
                    .body()
                    .is_some_and(|b| b.len() > self.config.max_response_bytes)
                {
                    return Err(FetchError::Limit);
                }
                if let Some(delay) = delay {
                    // Locally resolved resources are processed once; retry is for actual exchanges.
                    if attempts > 0
                        && attempts < self.config.max_attempts
                        && (matches!(req.method(), "GET" | "HEAD")
                            || self.config.retry_non_idempotent)
                        && self.hooks.retry(&req, &response, attempts).await?
                    {
                        let delay = delay.min(u64::from(self.config.timeout_ms));
                        if delay > 0 {
                            self.backend.sleep(delay as u32).await?;
                        }
                        continue;
                    }
                }
                return Ok(FetchedResource {
                    original: request.clone(),
                    request: req,
                    response,
                    attempts,
                });
            }
        };
        let mut work = std::pin::pin!(work);
        std::future::poll_fn(|cx| {
            if let Err(e) = cancel.check(cx.waker()) {
                return Poll::Ready(Err(e));
            }
            match expired() {
                Ok(true) => return Poll::Ready(Err(FetchError::Timeout)),
                Err(e) => return Poll::Ready(Err(e)),
                _ => {}
            }
            if let Poll::Ready(r) = deadline.as_mut().poll(cx) {
                return Poll::Ready(Err(r.err().unwrap_or(FetchError::Timeout)));
            }
            let result = work.as_mut().poll(cx);
            if result.is_ready() {
                if cancel.cancelled() {
                    return Poll::Ready(Err(FetchError::Cancelled));
                }
                match expired() {
                    Ok(true) => return Poll::Ready(Err(FetchError::Timeout)),
                    Err(e) => return Poll::Ready(Err(e)),
                    _ => {}
                }
            }
            result
        })
        .await
    }
}
