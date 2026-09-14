//! Resource fetching models and lifecycle, shared by native and dedicated workers.
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);
use quai_sdk::rpc::fetch::*;
use serde_json::json;
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn models_preserve_exact_bodies_and_bound_and_redact_headers() {
    let mut req = FetchRequest::new("https://example.invalid/api?secret=PRIVATE").unwrap();
    assert_eq!(req.method(), "GET");
    assert!(req.body().is_none());
    req.set_text("hello π").unwrap();
    assert_eq!(req.method(), "POST");
    assert_eq!(req.body().unwrap(), "hello π".as_bytes());
    assert_eq!(
        req.headers().unwrap().get("CONTENT-TYPE"),
        Some("text/plain")
    );
    req.set_json(&json!({"n":"9007199254740993"})).unwrap();
    assert_eq!(
        req.headers().unwrap().get("content-type"),
        Some("application/json")
    );
    req.headers_mut()
        .set("Content-Type", "custom/value")
        .unwrap();
    req.set_bytes(&[]).unwrap();
    assert_eq!(
        req.headers().unwrap().get("content-type"),
        Some("custom/value")
    );
    assert_eq!(req.body(), Some(&[][..]));
    req.set_credentials("user", "PRIVATE").unwrap();
    assert_eq!(
        req.headers().unwrap().get("authorization"),
        Some("Basic dXNlcjpQUklWQVRF")
    );
    assert!(!format!("{req:?}").contains("PRIVATE"));
    let h = req.headers().unwrap();
    assert!(!format!("{h:?}").contains("dXNlcjp"));
    let mut h = FetchHeaders::default();
    h.set("x-public", "old").unwrap();
    let old = h.clone();
    assert!(h.set("x-public", "bad\r\ninjected").is_err());
    assert_eq!(h, old);
    assert!(h.set("bad name", "v").is_err());
    assert!(h.set("x-public", &"x".repeat(MAX_FETCH_HEADERS)).is_err());
    assert_eq!(h, old);
    req.clear_body();
    req.set_method(Some("GET")).unwrap();
    req.headers_mut().clear();
    assert!(req.validate().is_ok());
    req.set_text("no GET body").unwrap();
    assert_eq!(req.validate(), Err(FetchError::Invalid));
    let before = req.body().unwrap().to_vec();
    assert_eq!(
        req.set_bytes(&vec![0; MAX_FETCH_BYTES + 1]),
        Err(FetchError::Limit)
    );
    assert_eq!(req.body().unwrap(), before);
    // Fuzz regression: preserve the already-parsed binary float through encoding.
    // Exact large chain quantities must still be represented as strings.
    let large: serde_json::Value =
        serde_json::from_str("266666666666666666666666666666666626666666666666222242").unwrap();
    req.set_method(None).unwrap();
    req.set_json(&large).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(req.body().unwrap()).unwrap(),
        large
    );
    let mut deep = json!(0);
    for _ in 0..66 {
        deep = json!([deep]);
    }
    assert_eq!(req.set_json(&deep), Err(FetchError::Limit));
    for url in [
        "https://user:pw@example.invalid/",
        "https://example.invalid/#fragment",
        "http://",
        "https://a/\r\n",
    ] {
        assert!(FetchRequest::new(url).is_err());
    }
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn redirect_and_authentication_policy_prevents_credential_forwarding() {
    let mut req = FetchRequest::new("https://example.invalid/a").unwrap();
    req.set_credentials("public", "toy").unwrap();
    req.headers_mut().set("x-api-key", "PRIVATE").unwrap();
    let same = req.redirect("/b").unwrap();
    assert_eq!(same.url(), "https://example.invalid/b");
    assert_eq!(same.headers().unwrap().get("x-api-key"), Some("PRIVATE"));
    let cross = req.redirect("https://other.invalid/b").unwrap();
    assert_eq!(cross.headers().unwrap().get("x-api-key"), None);
    assert_eq!(cross.headers().unwrap().get("authorization"), None);
    assert!(req.redirect("http://example.invalid/b").is_err());
    assert!(req.redirect("data:,no").is_err());
    req.set_method(Some("POST")).unwrap();
    assert!(req.redirect("/b").is_err());
    req.set_url("http://example.invalid/").unwrap();
    assert_eq!(req.validate(), Err(FetchError::InsecureAuthentication));
    req.allow_insecure_authentication = true;
    assert!(req.validate().is_ok());
    req.headers_mut().set("content-length", "1").unwrap();
    assert_eq!(req.validate(), Err(FetchError::Invalid));
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn data_ipfs_and_response_views_preserve_bytes_and_reject_hostile_inputs() {
    let response = data_resource("data:application/json,%7B%22a%22%3A1%7D").unwrap();
    assert_eq!(response.json().unwrap(), json!({"a":1}));
    assert!(response.ok());
    assert_eq!(
        data_resource("data:;base64,AAH/").unwrap().body().unwrap(),
        &[0, 1, 255]
    );
    assert_eq!(
        data_resource("data:,a+b%20c").unwrap().text().unwrap(),
        "a+b c"
    );
    for uri in [
        "data:,%",
        "data:,%gg",
        "data:;base64,?",
        "data:;base64,AB==",
    ] {
        assert!(data_resource(uri).is_err());
    }
    let invalid =
        FetchResponse::new(404, "PRIVATE", FetchHeaders::default(), Some(vec![255])).unwrap();
    assert_eq!(invalid.text(), Err(FetchError::Invalid));
    assert_eq!(invalid.assert_ok(), Err(FetchError::Status(404)));
    assert!(!format!("{invalid:?}").contains("PRIVATE"));
    assert_eq!(invalid.server_error().status(), 599);
    assert_eq!(
        ipfs_resource(
            "ipfs://ipfs/QmPublic/file.json",
            "https://gateway.invalid/ipfs/"
        )
        .unwrap()
        .url(),
        "https://gateway.invalid/ipfs/QmPublic/file.json"
    );
    for path in [
        "ipfs://../evil",
        "ipfs://QmPublic/%2e%2e/evil",
        "ipfs://QmPublic/a?key=1",
        "ipfs://QmPublic//a",
    ] {
        assert!(ipfs_resource(path, "https://gateway.invalid/ipfs/").is_err());
    }
}
#[cfg_attr(not(target_arch = "wasm32"), test)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
fn published_body_and_data_fixtures_match_without_numeric_coercion() {
    let f: serde_json::Value = serde_json::from_str(include_str!(
        "fixtures/shared/compatibility/fixtures/fetch.json"
    ))
    .unwrap();
    for row in f["bodies"].as_array().unwrap() {
        let mut req = FetchRequest::new("https://example.invalid/").unwrap();
        match row["kind"].as_str().unwrap() {
            "none" => {}
            "text" => req.set_text(row["input"].as_str().unwrap()).unwrap(),
            "bytes" => req
                .set_bytes(
                    &quai_sdk::primitives::get_bytes(row["input"].as_str().unwrap()).unwrap(),
                )
                .unwrap(),
            "json" => req.set_json(&row["input"]).unwrap(),
            _ => panic!(),
        };
        assert_eq!(req.method(), row["method"]);
        assert_eq!(
            req.body()
                .map(|b| quai_sdk::primitives::hexlify(b).unwrap()),
            row["body"].as_str().map(str::to_owned)
        );
        assert_eq!(
            req.headers().unwrap().get("content-type"),
            row["contentType"].as_str().filter(|s| !s.is_empty())
        );
    }
    for row in f["data"].as_array().unwrap() {
        let result = data_resource(row["uri"].as_str().unwrap());
        if row["status"] == 599 || row["rustReject"] == true {
            assert!(result.is_err());
        } else {
            let r = result.unwrap();
            assert_eq!(r.status(), row["status"].as_u64().unwrap() as u16);
            assert_eq!(
                quai_sdk::primitives::hexlify(r.body().unwrap()).unwrap(),
                row["body"]
            );
            assert_eq!(r.headers().get("content-type"), row["contentType"].as_str());
        }
    }
}
#[cfg(any(target_arch = "wasm32", feature = "http"))]
mod lifecycle {
    use super::*;
    use std::{
        collections::VecDeque,
        future::Future,
        sync::{Arc, Mutex},
        task::Poll,
    };
    #[cfg(not(target_arch = "wasm32"))]
    type Clock = NativeFetch;
    #[cfg(target_arch = "wasm32")]
    type Clock = quai_sdk::browser::BrowserResourceFetch;
    fn clock() -> Clock {
        #[cfg(not(target_arch = "wasm32"))]
        {
            NativeFetch::new(1000).unwrap()
        }
        #[cfg(target_arch = "wasm32")]
        {
            quai_sdk::browser::BrowserResourceFetch
        }
    }
    #[derive(Default)]
    struct State {
        responses: VecDeque<Result<FetchResponse, FetchError>>,
        requests: Vec<FetchRequest>,
        stall: bool,
        active: bool,
    }
    struct Active(Arc<Mutex<State>>);
    impl Drop for Active {
        fn drop(&mut self) {
            self.0.lock().unwrap().active = false;
        }
    }
    struct Mock {
        clock: Clock,
        state: Arc<Mutex<State>>,
    }
    impl FetchBackend for Mock {
        fn now(&self) -> Result<f64, FetchError> {
            self.clock.now()
        }
        async fn sleep(&self, n: u32) -> Result<(), FetchError> {
            self.clock.sleep(n).await
        }
        async fn execute(&self, r: &FetchRequest, _: usize) -> Result<FetchResponse, FetchError> {
            let stall = {
                let mut s = self.state.lock().unwrap();
                s.requests.push(r.clone());
                s.active = true;
                s.stall
            };
            let _guard = Active(self.state.clone());
            if stall {
                std::future::pending().await
            } else {
                self.state
                    .lock()
                    .unwrap()
                    .responses
                    .pop_front()
                    .expect("unexpected exchange")
            }
        }
    }
    fn response(status: u16, headers: &[(&str, &str)]) -> FetchResponse {
        let mut h = FetchHeaders::default();
        for (k, v) in headers {
            h.set(k, v).unwrap();
        }
        FetchResponse::new(status, "", h, Some(b"{\"ok\":true}".to_vec())).unwrap()
    }
    fn mock(rows: Vec<Result<FetchResponse, FetchError>>) -> (Mock, Arc<Mutex<State>>) {
        let state = Arc::new(Mutex::new(State {
            responses: rows.into(),
            ..State::default()
        }));
        (
            Mock {
                clock: clock(),
                state: state.clone(),
            },
            state,
        )
    }
    #[derive(Default)]
    struct Hooks {
        counts: Arc<Mutex<[u32; 3]>>,
    }
    impl FetchHooks for Hooks {
        async fn preflight(&self, r: &FetchRequest) -> Result<FetchRequest, FetchError> {
            self.counts.lock().unwrap()[0] += 1;
            let mut r = r.clone();
            r.headers_mut().set("x-public", "fixture")?;
            Ok(r)
        }
        async fn process(
            &self,
            _: &FetchRequest,
            r: FetchResponse,
        ) -> Result<FetchAction, FetchError> {
            self.counts.lock().unwrap()[1] += 1;
            Ok(FetchAction::Return(r))
        }
        async fn retry(
            &self,
            _: &FetchRequest,
            _: &FetchResponse,
            _: u32,
        ) -> Result<bool, FetchError> {
            self.counts.lock().unwrap()[2] += 1;
            Ok(true)
        }
    }
    #[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    async fn pipeline_bounds_retries_redirects_hooks_and_method_replay() {
        let (backend, state) = mock(vec![
            Ok(response(429, &[("retry-after", "0")])),
            Ok(response(302, &[("location", "/next")])),
            Ok(response(200, &[])),
        ]);
        let hooks = Hooks::default();
        let counts = hooks.counts.clone();
        let client = FetchClient::with_hooks(
            backend,
            FetchConfig {
                max_attempts: 4,
                max_redirects: 1,
                ..FetchConfig::default()
            },
            hooks,
        )
        .unwrap();
        let r = FetchRequest::new("https://example.invalid/start").unwrap();
        let got = client
            .send(&r, &FetchCancellation::default())
            .await
            .unwrap();
        assert_eq!(got.attempts, 3);
        assert_eq!(got.request.url(), "https://example.invalid/next");
        assert_eq!(got.original.url(), r.url());
        assert_eq!(got.response.json().unwrap(), json!({"ok":true}));
        assert_eq!(*counts.lock().unwrap(), [3, 2, 1]);
        assert_eq!(state.lock().unwrap().requests.len(), 3);
        // POST never replays by default, even if max_attempts permits retries.
        let (backend, state) = mock(vec![Ok(response(429, &[]))]);
        let client = FetchClient::new(
            backend,
            FetchConfig {
                max_attempts: 3,
                ..FetchConfig::default()
            },
        )
        .unwrap();
        let mut r = r.clone();
        r.set_text("PUBLIC").unwrap();
        assert_eq!(
            client
                .send(&r, &FetchCancellation::default())
                .await
                .unwrap()
                .response
                .status(),
            429
        );
        assert_eq!(state.lock().unwrap().requests.len(), 1);
        // A transport error is returned without retry.
        let (backend, state) = mock(vec![Err(FetchError::Transport)]);
        let client = FetchClient::new(
            backend,
            FetchConfig {
                max_attempts: 3,
                retry_non_idempotent: true,
                ..FetchConfig::default()
            },
        )
        .unwrap();
        assert!(matches!(
            client.send(&r, &FetchCancellation::default()).await,
            Err(FetchError::Transport)
        ));
        assert_eq!(state.lock().unwrap().requests.len(), 1);
        // Delta-seconds do not become milliseconds; delay beyond deadline fails.
        let (backend, state) = mock(vec![Ok(response(429, &[("retry-after", "1")]))]);
        let client = FetchClient::new(
            backend,
            FetchConfig {
                timeout_ms: 50,
                retry_delay_ms: 0,
                max_attempts: 2,
                ..FetchConfig::default()
            },
        )
        .unwrap();
        r.clear_body();
        assert!(matches!(
            client.send(&r, &FetchCancellation::default()).await,
            Err(FetchError::Timeout)
        ));
        assert_eq!(state.lock().unwrap().requests.len(), 1);
    }
    #[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    async fn deadlines_cancellation_and_drop_release_stalled_exchange_and_token() {
        let (backend, state) = mock(vec![]);
        state.lock().unwrap().stall = true;
        let client = FetchClient::new(
            backend,
            FetchConfig {
                timeout_ms: 30,
                retry_delay_ms: 0,
                ..FetchConfig::default()
            },
        )
        .unwrap();
        let req = FetchRequest::new("https://example.invalid/").unwrap();
        let token = FetchCancellation::default();
        assert!(matches!(
            client.send(&req, &token).await,
            Err(FetchError::Timeout)
        ));
        assert!(!state.lock().unwrap().active);
        {
            let future = client.send(&req, &token);
            let mut future = std::pin::pin!(future);
            std::future::poll_fn(|cx| {
                assert!(future.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            assert!(state.lock().unwrap().active);
            assert!(matches!(
                client.send(&req, &token).await,
                Err(FetchError::Active)
            ));
            token.cancel();
            assert!(matches!(future.await, Err(FetchError::Cancelled)));
        }
        assert!(!state.lock().unwrap().active);
        assert!(matches!(
            client.send(&req, &token).await,
            Err(FetchError::Cancelled)
        ));
        let token = FetchCancellation::default();
        {
            let future = client.send(&req, &token);
            let mut future = std::pin::pin!(future);
            std::future::poll_fn(|cx| {
                assert!(future.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
        }
        assert!(!state.lock().unwrap().active);
        {
            let mut s = state.lock().unwrap();
            s.stall = false;
            s.responses.push_back(Ok(response(200, &[])));
        }
        assert_eq!(
            client.send(&req, &token).await.unwrap().response.status(),
            200
        );
    }
    struct GatewayHook;
    impl FetchHooks for GatewayHook {
        async fn gateway(&self, r: &FetchRequest) -> Result<Option<FetchGateway>, FetchError> {
            Ok(match r.scheme() {
                "ipfs" => Some(FetchGateway::Request(ipfs_resource(
                    r.url(),
                    "https://gateway.invalid/ipfs/",
                )?)),
                "toy" => Some(FetchGateway::Response(response(200, &[]))),
                _ => None,
            })
        }
        async fn process(
            &self,
            _: &FetchRequest,
            r: FetchResponse,
        ) -> Result<FetchAction, FetchError> {
            Ok(FetchAction::Retry {
                response: r,
                delay_ms: 0,
            })
        }
    }
    #[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    async fn local_and_custom_gateways_obey_output_limits_and_processing() {
        let (backend, state) = mock(vec![Ok(response(200, &[])), Ok(response(200, &[]))]);
        let client = FetchClient::with_hooks(
            backend,
            FetchConfig {
                max_attempts: 2,
                ..FetchConfig::default()
            },
            GatewayHook,
        )
        .unwrap();
        let got = client
            .send(
                &FetchRequest::new("data:,hello").unwrap(),
                &FetchCancellation::default(),
            )
            .await
            .unwrap();
        assert_eq!(got.attempts, 0);
        assert_eq!(got.response.text().unwrap(), "hello");
        assert_eq!(
            client
                .send(
                    &FetchRequest::new("toy:public").unwrap(),
                    &FetchCancellation::default()
                )
                .await
                .unwrap()
                .attempts,
            0
        );
        let got = client
            .send(
                &FetchRequest::new("ipfs://QmPublic/a.json").unwrap(),
                &FetchCancellation::default(),
            )
            .await
            .unwrap();
        assert_eq!(got.attempts, 2);
        assert_eq!(
            got.request.url(),
            "https://gateway.invalid/ipfs/QmPublic/a.json"
        );
        assert_eq!(state.lock().unwrap().requests.len(), 2);
        let (backend, _) = mock(vec![]);
        let tiny = FetchClient::new(
            backend,
            FetchConfig {
                max_response_bytes: 1,
                ..FetchConfig::default()
            },
        )
        .unwrap();
        assert!(matches!(
            tiny.send(
                &FetchRequest::new("data:,abc").unwrap(),
                &FetchCancellation::default()
            )
            .await,
            Err(FetchError::Limit)
        ));
    }
    struct StalledHook;
    impl FetchHooks for StalledHook {
        async fn preflight(&self, _: &FetchRequest) -> Result<FetchRequest, FetchError> {
            std::future::pending().await
        }
    }
    #[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    async fn overall_deadline_and_cancellation_cover_preflight_before_network_io() {
        let (backend, state) = mock(vec![]);
        let client = FetchClient::with_hooks(
            backend,
            FetchConfig {
                timeout_ms: 30,
                retry_delay_ms: 0,
                ..FetchConfig::default()
            },
            StalledHook,
        )
        .unwrap();
        let r = FetchRequest::new("https://example.invalid/").unwrap();
        let token = FetchCancellation::default();
        assert!(matches!(
            client.send(&r, &token).await,
            Err(FetchError::Timeout)
        ));
        assert!(state.lock().unwrap().requests.is_empty());
        let mut future = std::pin::pin!(client.send(&r, &token));
        std::future::poll_fn(|cx| {
            assert!(future.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        token.cancel();
        assert!(matches!(future.await, Err(FetchError::Cancelled)));
        assert!(state.lock().unwrap().requests.is_empty());
    }
}
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen_test::wasm_bindgen_test]
#[allow(
    clippy::option_env_unwrap,
    reason = "Archive target compilation does not provide a running fixture server"
)]
async fn browser_streamed_http_echo_errors_gzip_bounds_and_opaque_redirects() {
    let base = option_env!("QUAI_BROWSER_FIXTURE_URL")
        .expect("run through the owned browser fixture harness");
    let client = FetchClient::new(
        quai_sdk::browser::BrowserResourceFetch,
        FetchConfig {
            timeout_ms: 1000,
            max_response_bytes: 1024,
            ..FetchConfig::default()
        },
    )
    .unwrap();
    let mut req = FetchRequest::new(&format!("{base}/resource/echo")).unwrap();
    req.set_text("PUBLIC π").unwrap();
    req.headers_mut().set("x-public", "yes").unwrap();
    assert_eq!(
        client
            .send(&req, &FetchCancellation::default())
            .await
            .unwrap()
            .response
            .text()
            .unwrap(),
        "PUBLIC π"
    );
    req.clear_body();
    req.set_url(&format!("{base}/resource/error")).unwrap();
    let got = client
        .send(&req, &FetchCancellation::default())
        .await
        .unwrap();
    assert_eq!(got.response.status(), 418);
    assert_eq!(got.response.text().unwrap(), "PUBLIC error");
    req.set_url(&format!("{base}/resource/gzip")).unwrap();
    assert_eq!(
        client
            .send(&req, &FetchCancellation::default())
            .await
            .unwrap()
            .response
            .json()
            .unwrap(),
        json!({"ok":true})
    );
    for path in ["bomb", "oversize"] {
        req.set_url(&format!("{base}/resource/{path}")).unwrap();
        assert!(matches!(
            client.send(&req, &FetchCancellation::default()).await,
            Err(FetchError::Limit)
        ));
    }
    req.set_url(&format!("{base}/resource/redirect")).unwrap();
    assert!(matches!(
        client.send(&req, &FetchCancellation::default()).await,
        Err(FetchError::Unsupported)
    ));
    req.set_url(&format!("{base}/resource/slow")).unwrap();
    let client = FetchClient::new(
        quai_sdk::browser::BrowserResourceFetch,
        FetchConfig {
            timeout_ms: 30,
            retry_delay_ms: 0,
            ..FetchConfig::default()
        },
    )
    .unwrap();
    assert!(matches!(
        client.send(&req, &FetchCancellation::default()).await,
        Err(FetchError::Timeout)
    ));
}
