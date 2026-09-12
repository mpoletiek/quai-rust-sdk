//! Real loopback HTTP tests for routing, protocol validation, limits and cancellation.
#![cfg(all(feature = "http", not(target_arch = "wasm32")))]
use quai_primitives::{Shard, Zone};
use quai_rpc::{Endpoint, HttpConfig, HttpTransport, Routing, RpcError, Transport};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::JoinHandle,
};

async fn read_request(socket: &mut TcpStream) -> (String, Value) {
    let mut bytes = Vec::new();
    let end = loop {
        let mut buf = [0; 1024];
        let n = socket.read(&mut buf).await.unwrap();
        assert_ne!(n, 0);
        bytes.extend_from_slice(&buf[..n]);
        assert!(bytes.len() < 64 * 1024);
        if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let header = std::str::from_utf8(&bytes[..end]).unwrap().to_owned();
    let length: usize = header
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().unwrap())
        })
        .unwrap();
    assert!(length < 64 * 1024);
    while bytes.len() < end + length {
        let mut buf = [0; 1024];
        let n = socket.read(&mut buf).await.unwrap();
        assert_ne!(n, 0);
        bytes.extend_from_slice(&buf[..n]);
    }
    (
        header,
        serde_json::from_slice(&bytes[end..end + length]).unwrap(),
    )
}

async fn server<F>(reply: F) -> (Endpoint, oneshot::Receiver<(String, Value)>, JoinHandle<()>)
where
    F: FnOnce(&Value) -> String + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = Endpoint::parse(&format!(
        "http://{}/secret?token=HIDDEN",
        listener.local_addr().unwrap()
    ))
    .unwrap();
    let (tx, rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        let response = reply(&request.1);
        tx.send(request).unwrap();
        let _ = socket.write_all(response.as_bytes()).await;
    });
    (endpoint, rx, task)
}

fn response(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}
fn transport() -> HttpTransport {
    HttpTransport::new(HttpConfig::default()).unwrap()
}

#[tokio::test]
async fn sends_exact_direct_path_and_correlates_envelope() {
    let (endpoint, rx, task) = server(|req| {
        response(&json!({"jsonrpc":"2.0","id":req["id"],"result":"0x3a98"}).to_string())
    })
    .await;
    let routes =
        Routing::with_pathing(endpoint.as_str(), Shard::Zone(Zone::Cyprus1), false).unwrap();
    let result = transport()
        .request(
            routes.endpoint(Zone::Cyprus1.into()).unwrap(),
            "quai_chainId",
            json!([]),
        )
        .await
        .unwrap();
    assert_eq!(result, "0x3a98");
    let (headers, body) = rx.await.unwrap();
    assert!(headers.starts_with("POST /secret?token=HIDDEN HTTP/1.1\r\n"));
    assert_eq!(body["method"], "quai_chainId");
    assert_eq!(body["params"], json!([]));
    assert_eq!(body["jsonrpc"], "2.0");
    task.await.unwrap();
}

#[tokio::test]
async fn gateway_appends_shard_after_prefix_and_before_query() {
    let (endpoint, rx, task) =
        server(|req| response(&json!({"jsonrpc":"2.0","id":req["id"],"result":null}).to_string()))
            .await;
    let routes = Routing::with_pathing(endpoint.as_str(), Zone::Cyprus1.into(), true).unwrap();
    assert_eq!(
        transport()
            .request(
                routes.endpoint(Zone::Cyprus1.into()).unwrap(),
                "quai_getTransactionByHash",
                json!(["0x00"])
            )
            .await
            .unwrap(),
        Value::Null
    );
    assert!(
        rx.await
            .unwrap()
            .0
            .starts_with("POST /secret/cyprus1?token=HIDDEN HTTP/1.1\r\n")
    );
    task.await.unwrap();
}

#[tokio::test]
async fn rejects_malformed_or_uncorrelated_envelopes() {
    for body in [
        r#"{"jsonrpc":"2.0","id":999,"result":1}"#,
        r#"{"jsonrpc":"1.0","id":1,"result":1}"#,
        r#"{"jsonrpc":"2.0","id":"1","result":1}"#,
        r#"{"jsonrpc":"2.0","id":1,"result":null,"error":{"code":1,"message":"x"}}"#,
        r#"{"jsonrpc":"2.0","id":1}"#,
        r#"{"jsonrpc":"2.0","id":1,"error":null}"#,
        r#"{"jsonrpc":"2.0","id":1,"error":{"code":"1","message":"x"}}"#,
        r#"{"jsonrpc":"2.0","id":1,"id":1,"result":0}"#,
        r#"{"jsonrpc":"2.0","id":1,"result":1,"result":2}"#,
        r#"[{"jsonrpc":"2.0","id":1,"result":1}]"#,
        "not json",
    ] {
        let (endpoint, rx, task) = server(move |_| response(body)).await;
        let error = transport()
            .request(&endpoint, "quai_chainId", json!([]))
            .await
            .unwrap_err();
        assert!(
            matches!(error, RpcError::InvalidResponse(_)),
            "{body}: {error:?}"
        );
        rx.await.unwrap();
        task.await.unwrap();
    }
}

#[tokio::test]
async fn remote_details_are_explicit_but_never_in_error_display_or_debug() {
    let (endpoint, rx, task) = server(|req| response(&json!({"jsonrpc":"2.0","id":req["id"],"error":{"code":-32000,"message":"HIDDEN MESSAGE","data":{"key":"HIDDEN DATA"}}}).to_string())).await;
    let error = transport()
        .request(&endpoint, "quai_chainId", json!([]))
        .await
        .unwrap_err();
    assert!(!format!("{error} {error:?}").contains("HIDDEN"));
    match error {
        RpcError::Remote(remote) => {
            assert_eq!(remote.code, -32000);
            assert_eq!(remote.message, "HIDDEN MESSAGE");
            assert_eq!(remote.data.unwrap()["key"], "HIDDEN DATA");
        }
        _ => panic!("expected remote error"),
    }
    rx.await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn response_limit_applies_to_content_length_and_chunked_bodies() {
    for chunked in [false, true] {
        let (endpoint, rx, task) = server(move |_| {
            let body = "x".repeat(256);
            if chunked { format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{body}\r\n0\r\n\r\n", body.len()) } else { response(&body) }
        }).await;
        let client = HttpTransport::new(HttpConfig {
            max_response_bytes: 64,
            ..HttpConfig::default()
        })
        .unwrap();
        assert!(matches!(
            client.request(&endpoint, "quai_chainId", json!([])).await,
            Err(RpcError::ResponseTooLarge)
        ));
        rx.await.unwrap();
        task.await.unwrap();
    }
}

#[tokio::test]
async fn redirect_is_not_followed_and_response_body_is_not_leaked() {
    let (endpoint, rx, task) = server(|_| "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://127.0.0.1:1/HIDDEN\r\nContent-Length: 6\r\nConnection: close\r\n\r\nHIDDEN".into()).await;
    let error = transport()
        .request(&endpoint, "quai_chainId", json!([]))
        .await
        .unwrap_err();
    assert!(matches!(error, RpcError::HttpStatus(307)));
    assert!(!format!("{error:?} {error}").contains("HIDDEN"));
    rx.await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn timeout_and_cancellation_release_concurrency_permit() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = Endpoint::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let (started_tx, started_rx) = oneshot::channel();
    let blocked_server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let _ = read_request(&mut socket).await;
        started_tx.send(()).unwrap();
        std::future::pending::<()>().await;
    });
    let client = HttpTransport::new(HttpConfig {
        max_in_flight: 1,
        timeout: Duration::from_millis(250),
        ..HttpConfig::default()
    })
    .unwrap();
    let cloned = client.clone();
    let request =
        tokio::spawn(async move { cloned.request(&endpoint, "quai_chainId", json!([])).await });
    started_rx.await.unwrap();
    assert!(matches!(request.await.unwrap(), Err(RpcError::Timeout)));
    blocked_server.abort();
    let (endpoint, rx, task) =
        server(|req| response(&json!({"jsonrpc":"2.0","id":req["id"],"result":42}).to_string()))
            .await;
    assert_eq!(
        client
            .request(&endpoint, "quai_chainId", json!([]))
            .await
            .unwrap(),
        42
    );
    rx.await.unwrap();
    task.await.unwrap();

    // Explicit future cancellation also returns the single permit.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = Endpoint::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let cloned = client.clone();
    let request =
        tokio::spawn(async move { cloned.request(&endpoint, "quai_chainId", json!([])).await });
    let (mut socket, _) = listener.accept().await.unwrap();
    let _ = read_request(&mut socket).await;
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    let (endpoint, rx, task) =
        server(|req| response(&json!({"jsonrpc":"2.0","id":req["id"],"result":43}).to_string()))
            .await;
    assert_eq!(
        client
            .request(&endpoint, "quai_chainId", json!([]))
            .await
            .unwrap(),
        43
    );
    rx.await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn rejects_invalid_config_and_non_http_without_network() {
    assert!(matches!(
        HttpTransport::new(HttpConfig {
            max_in_flight: 0,
            ..HttpConfig::default()
        }),
        Err(RpcError::InvalidConfig)
    ));
    let client = transport();
    assert!(matches!(
        client
            .request(
                &Endpoint::parse("wss://example.invalid").unwrap(),
                "quai_chainId",
                json!([])
            )
            .await,
        Err(RpcError::InvalidConfig)
    ));
    assert!(matches!(
        client
            .request(
                &Endpoint::parse("https://example.invalid").unwrap(),
                "quai_chainId",
                Value::Null
            )
            .await,
        Err(RpcError::InvalidConfig)
    ));
}
