use super::*;
use std::{io::Read, time::Duration};
/// Pooled rustls resource backend. No implicit redirects, retries, environment
/// proxies or cookie jar; compressed and decoded body limits are both enforced.
#[derive(Clone)]
pub struct NativeFetch {
    client: reqwest::Client,
    start: std::time::Instant,
}
impl NativeFetch {
    /// Create a backend with an explicit positive connection timeout (<= one day).
    pub fn new(connect_timeout_ms: u32) -> Result<Self, FetchError> {
        if !(1..=86_400_000).contains(&connect_timeout_ms) {
            return Err(FetchError::Invalid);
        }
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(connect_timeout_ms.into()))
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .build()
            .map_err(|_| FetchError::Invalid)?;
        Ok(Self {
            client,
            start: std::time::Instant::now(),
        })
    }
}
impl FetchBackend for NativeFetch {
    fn now(&self) -> Result<f64, FetchError> {
        Ok(self.start.elapsed().as_secs_f64() * 1000.0)
    }
    async fn sleep(&self, milliseconds: u32) -> Result<(), FetchError> {
        tokio::time::sleep(Duration::from_millis(milliseconds.into())).await;
        Ok(())
    }
    async fn execute(
        &self,
        request: &FetchRequest,
        max_bytes: usize,
    ) -> Result<FetchResponse, FetchError> {
        request.validate()?;
        if !matches!(request.scheme(), "http" | "https")
            || !(1..=MAX_FETCH_BYTES).contains(&max_bytes)
        {
            return Err(FetchError::Invalid);
        }
        let mut req = self.client.request(
            reqwest::Method::from_bytes(request.method().as_bytes())
                .map_err(|_| FetchError::Invalid)?,
            request.url(),
        );
        for (k, v) in request.headers()?.iter() {
            req = req.header(k, v);
        }
        if let Some(body) = request.body() {
            req = req.body(body.to_vec());
        }
        let mut resp = req.send().await.map_err(|_| FetchError::Transport)?;
        let status = resp.status();
        let mut headers = FetchHeaders::default();
        for (k, v) in resp.headers() {
            headers.set(k.as_str(), v.to_str().map_err(|_| FetchError::Invalid)?)?;
        }
        if request.method() != "HEAD" && resp.content_length().is_some_and(|n| n > max_bytes as u64)
        {
            return Err(FetchError::Limit);
        }
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|_| FetchError::Transport)? {
            if chunk.len() > max_bytes - body.len() {
                return Err(FetchError::Limit);
            }
            body.extend_from_slice(&chunk);
        }
        if request.method() != "HEAD"
            && headers
                .get("content-encoding")
                .is_some_and(|e| e.eq_ignore_ascii_case("gzip"))
        {
            let mut decoded = Vec::new();
            flate2::read::MultiGzDecoder::new(body.as_slice())
                .take(max_bytes as u64 + 1)
                .read_to_end(&mut decoded)
                .map_err(|_| FetchError::Invalid)?;
            if decoded.len() > max_bytes {
                return Err(FetchError::Limit);
            }
            body = decoded;
            // Retain original content-encoding/length metadata, as published fetch does.
        } else if request.method() != "HEAD"
            && headers
                .get("content-encoding")
                .is_some_and(|e| !e.eq_ignore_ascii_case("identity"))
        {
            return Err(FetchError::Unsupported);
        }
        FetchResponse::new(
            status.as_u16(),
            status.canonical_reason().unwrap_or_default(),
            headers,
            Some(body),
        )
    }
}
