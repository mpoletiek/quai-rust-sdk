use crate::receipt_wait::{Timer, now};
use quai_rpc::fetch::{
    FetchBackend, FetchError, FetchHeaders, FetchRequest, FetchResponse, MAX_FETCH_BYTES,
};
use wasm_bindgen::prelude::*;
#[wasm_bindgen(module = "/src/bridge.js")]
extern "C" {
    #[wasm_bindgen(catch,js_name=newAbort)]
    fn new_abort() -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_name=abort)]
    fn abort(controller: &JsValue);
    #[wasm_bindgen(js_name=errorKind)]
    fn error_kind(error: &JsValue) -> String;
}
#[wasm_bindgen(module = "/src/resource.js")]
extern "C" {
    #[wasm_bindgen(catch,js_name=resourceFetch)]
    async fn resource_fetch(
        url: &str,
        method: &str,
        headers: &str,
        body: &JsValue,
        controller: &JsValue,
        max_bytes: usize,
    ) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_name=resourceMeta)]
    fn resource_meta(result: &JsValue) -> String;
    #[wasm_bindgen(js_name=resourceBytes)]
    fn resource_bytes(result: &JsValue) -> js_sys::Uint8Array;
}
struct Abort(JsValue);
impl Drop for Abort {
    fn drop(&mut self) {
        abort(&self.0);
    }
}
/// Window/worker resource backend with streamed decoded-body limits and abort on
/// drop. Browser CORS/header/compression policies apply. Opaque redirects reject;
/// callers must explicitly supply a resolved URL rather than requesting auto-follow.
#[derive(Clone, Copy, Debug, Default)]
pub struct BrowserResourceFetch;
impl FetchBackend for BrowserResourceFetch {
    fn now(&self) -> Result<f64, FetchError> {
        now().map_err(|_| FetchError::Transport)
    }
    async fn sleep(&self, milliseconds: u32) -> Result<(), FetchError> {
        if milliseconds == 0 {
            return Ok(());
        }
        Timer::new(milliseconds)
            .map_err(|_| FetchError::Invalid)?
            .wait()
            .await
            .map_err(|_| FetchError::Transport)
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
        let guard = Abort(new_abort().map_err(|_| FetchError::Transport)?);
        let headers = request.headers()?;
        let headers =
            serde_json::to_string(&headers.iter().collect::<std::collections::BTreeMap<_, _>>())
                .map_err(|_| FetchError::Invalid)?;
        let body = request
            .body()
            .map(|b| JsValue::from(js_sys::Uint8Array::from(b)))
            .unwrap_or(JsValue::NULL);
        let result = resource_fetch(
            request.url(),
            request.method(),
            &headers,
            &body,
            &guard.0,
            max_bytes,
        )
        .await
        .map_err(|e| match error_kind(&e).as_str() {
            "response_size" => FetchError::Limit,
            "unsupported" => FetchError::Unsupported,
            "cancelled" => FetchError::Cancelled,
            _ => FetchError::Transport,
        })?;
        let meta = resource_meta(&result);
        if meta.len() > MAX_FETCH_BYTES {
            return Err(FetchError::Limit);
        }
        #[derive(serde::Deserialize)]
        struct Meta {
            status: u16,
            message: String,
            headers: std::collections::BTreeMap<String, String>,
        }
        let meta: Meta = serde_json::from_str(&meta).map_err(|_| FetchError::Invalid)?;
        let mut headers = FetchHeaders::default();
        for (k, v) in meta.headers {
            headers.set(&k, &v)?;
        }
        let body = resource_bytes(&result);
        if body.length() as usize > max_bytes {
            return Err(FetchError::Limit);
        }
        FetchResponse::new(meta.status, &meta.message, headers, Some(body.to_vec()))
    }
}
