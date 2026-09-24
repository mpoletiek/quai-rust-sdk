//! Real loopback HTTP tests for routing, protocol validation, limits and cancellation.
#![cfg(all(feature = "http", not(target_arch = "wasm32")))]
use quai_primitives::{Shard, Zone};
use quai_rpc::{Endpoint, HttpConfig, HttpProxy, HttpTransport, Routing, RpcError, Transport};
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
    raw_server(move |request| reply(request).into_bytes()).await
}

async fn raw_server<F>(reply: F) -> (Endpoint, oneshot::Receiver<(String, Value)>, JoinHandle<()>)
where
    F: FnOnce(&Value) -> Vec<u8> + Send + 'static,
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
        let _ = socket.write_all(&response).await;
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
async fn explicit_batch_uses_one_post_and_preserves_reordered_remote_errors() {
    let (endpoint, rx, task) =
        server(|req| {
            assert_eq!(req.as_array().unwrap().len(), 2);
            response(&json!([
            {"jsonrpc":"2.0","id":req[1]["id"],"error":{"code":-32000,"message":"unavailable"}},
            {"jsonrpc":"2.0","id":req[0]["id"],"result":"0x3a98"}
        ]).to_string())
        })
        .await;
    let rows = transport()
        .batch(
            &endpoint,
            vec![("quai_chainId", json!([])), ("quai_blockNumber", json!([]))],
        )
        .await
        .unwrap();
    assert_eq!(rows[0].as_ref().unwrap(), "0x3a98");
    assert!(matches!(&rows[1], Err(RpcError::Remote(_))));
    let (_, sent) = rx.await.unwrap();
    assert_ne!(sent[0]["id"], sent[1]["id"]);
    task.await.unwrap();
}

#[tokio::test]
async fn batch_bounds_and_http_failures_do_not_fall_back_or_retry() {
    let offline = Endpoint::parse("http://127.0.0.1:1").unwrap();
    let client = transport();
    assert!(matches!(
        client
            .batch(&offline, vec![("x", json!(["x".repeat(2 * 1024 * 1024)]))])
            .await,
        Err(RpcError::RequestTooLarge)
    ));
    for requests in [
        vec![],
        vec![("x", json!([])); 129],
        vec![("", json!([]))],
        vec![("x", json!(null))],
    ] {
        assert!(matches!(
            client.batch(&offline, requests).await,
            Err(RpcError::InvalidConfig)
        ));
    }
    let (endpoint, rx, task) = server(|_| {
        "HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into()
    })
    .await;
    assert!(matches!(
        client
            .batch(&endpoint, vec![("quai_chainId", json!([]))])
            .await,
        Err(RpcError::HttpStatus(429))
    ));
    rx.await.unwrap();
    task.await.unwrap();
    let (endpoint, rx, task) = server(|_| response(&" ".repeat(4096))).await;
    let client = HttpTransport::new(HttpConfig::default().with_max_response_bytes(1024)).unwrap();
    assert!(matches!(
        client
            .batch(&endpoint, vec![("quai_chainId", json!([]))])
            .await,
        Err(RpcError::ResponseTooLarge)
    ));
    rx.await.unwrap();
    task.await.unwrap();
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
        let client = HttpTransport::new(HttpConfig::default().with_max_response_bytes(64)).unwrap();
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
    let client = HttpTransport::new(
        HttpConfig::default()
            .with_max_in_flight(1)
            .with_timeout(Duration::from_millis(250)),
    )
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
        HttpTransport::new(HttpConfig::default().with_max_in_flight(0)),
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

#[tokio::test]
async fn an_explicit_proxy_carries_the_request_and_is_redacted() {
    // The fixture server acts as the proxy: a plain-http endpoint reaches it
    // as an absolute-form request line naming the real destination.
    let (proxy, rx, task) = server(|req| {
        response(&json!({"jsonrpc":"2.0","id":req["id"],"result":"0x3a98"}).to_string())
    })
    .await;
    let proxy_url = proxy
        .as_str()
        .split('/')
        .take(3)
        .collect::<Vec<_>>()
        .join("/");
    let with_credentials = proxy_url.replacen("http://", "http://user:SECRET@", 1);
    let config =
        HttpConfig::default().with_proxy(Some(HttpProxy::parse(&with_credentials).unwrap()));
    assert!(!format!("{config:?}").contains("SECRET"));
    let destination = Endpoint::parse("http://rpc.invalid:8545/cyprus1").unwrap();
    let result = HttpTransport::new(config)
        .unwrap()
        .request(&destination, "quai_chainId", json!([]))
        .await
        .unwrap();
    assert_eq!(result, "0x3a98");
    let (headers, _) = rx.await.unwrap();
    assert!(
        headers.starts_with("POST http://rpc.invalid:8545/cyprus1 HTTP/1.1\r\n"),
        "{headers}"
    );
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("proxy-authorization: basic")
    );
    task.await.unwrap();

    for invalid in [
        "socks4://127.0.0.1:1080",
        "socks4a://127.0.0.1:1080",
        "ftp://proxy",
        "not a url",
    ] {
        assert!(matches!(
            HttpProxy::parse(invalid),
            Err(RpcError::InvalidConfig)
        ));
    }
}

/// What a loopback SOCKS5 proxy was asked for.
struct SocksRequest {
    /// Username/password sub-negotiation, if the client offered it.
    credentials: Option<(String, String)>,
    /// Address type byte: 1 for IPv4, 3 for a domain name.
    address_type: u8,
    /// Requested host, as a name or a dotted IPv4 address.
    host: String,
    port: u16,
    /// The HTTP request line carried through the tunnel.
    request_line: String,
}

/// A one-connection SOCKS5 proxy that answers the tunnelled JSON-RPC request
/// itself, so no destination is contacted.
async fn socks5_proxy() -> (String, oneshot::Receiver<SocksRequest>, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let (tx, rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut head = [0; 2];
        socket.read_exact(&mut head).await.unwrap();
        assert_eq!(head[0], 5);
        let mut methods = vec![0; head[1] as usize];
        socket.read_exact(&mut methods).await.unwrap();
        let credentials = if methods.contains(&2) {
            socket.write_all(&[5, 2]).await.unwrap();
            let mut version = [0; 2];
            socket.read_exact(&mut version).await.unwrap();
            assert_eq!(version[0], 1);
            let mut user = vec![0; version[1] as usize];
            socket.read_exact(&mut user).await.unwrap();
            let mut length = [0; 1];
            socket.read_exact(&mut length).await.unwrap();
            let mut password = vec![0; length[0] as usize];
            socket.read_exact(&mut password).await.unwrap();
            socket.write_all(&[1, 0]).await.unwrap();
            Some((
                String::from_utf8(user).unwrap(),
                String::from_utf8(password).unwrap(),
            ))
        } else {
            assert!(methods.contains(&0));
            socket.write_all(&[5, 0]).await.unwrap();
            None
        };
        let mut connect = [0; 4];
        socket.read_exact(&mut connect).await.unwrap();
        assert_eq!(connect[..3], [5, 1, 0], "CONNECT command");
        let address_type = connect[3];
        let host = match address_type {
            1 => {
                let mut ip = [0; 4];
                socket.read_exact(&mut ip).await.unwrap();
                std::net::Ipv4Addr::from(ip).to_string()
            }
            3 => {
                let mut length = [0; 1];
                socket.read_exact(&mut length).await.unwrap();
                let mut name = vec![0; length[0] as usize];
                socket.read_exact(&mut name).await.unwrap();
                String::from_utf8(name).unwrap()
            }
            other => panic!("unexpected address type {other}"),
        };
        let mut port = [0; 2];
        socket.read_exact(&mut port).await.unwrap();
        socket
            .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0])
            .await
            .unwrap();
        let (header, request) = read_request(&mut socket).await;
        let body = json!({"jsonrpc":"2.0","id":request["id"],"result":"0x3a98"}).to_string();
        let _ = socket.write_all(response(&body).as_bytes()).await;
        let _ = tx.send(SocksRequest {
            credentials,
            address_type,
            host,
            port: u16::from_be_bytes(port),
            request_line: header.lines().next().unwrap().to_owned(),
        });
    });
    (address, rx, task)
}

#[tokio::test]
async fn a_socks5h_proxy_resolves_the_endpoint_name_and_is_redacted() {
    let (address, rx, task) = socks5_proxy().await;
    let config = HttpConfig::default().with_proxy(Some(
        HttpProxy::parse(&format!("socks5h://wallet:SECRET@{address}")).unwrap(),
    ));
    assert!(!format!("{config:?}").contains("SECRET"));
    // `.invalid` never resolves locally, so success shows the name went to the proxy.
    let destination = Endpoint::parse("http://rpc.invalid:8545/cyprus1").unwrap();
    let result = HttpTransport::new(config)
        .unwrap()
        .request(&destination, "quai_chainId", json!([]))
        .await
        .unwrap();
    assert_eq!(result, "0x3a98");
    let seen = rx.await.unwrap();
    assert_eq!(seen.address_type, 3, "domain name, not a resolved address");
    assert_eq!((seen.host.as_str(), seen.port), ("rpc.invalid", 8545));
    assert_eq!(
        seen.credentials,
        Some(("wallet".to_owned(), "SECRET".to_owned()))
    );
    assert_eq!(seen.request_line, "POST /cyprus1 HTTP/1.1");
    task.await.unwrap();
}

#[tokio::test]
async fn a_socks5_proxy_is_never_sent_the_endpoint_name() {
    let (address, rx, task) = socks5_proxy().await;
    let config = HttpConfig::default().with_proxy(Some(
        HttpProxy::parse(&format!("socks5://{address}")).unwrap(),
    ));
    let destination = Endpoint::parse("http://localhost:8545/cyprus1").unwrap();
    let result = HttpTransport::new(config)
        .unwrap()
        .request(&destination, "quai_chainId", json!([]))
        .await
        .unwrap();
    assert_eq!(result, "0x3a98");
    let seen = rx.await.unwrap();
    // The name never reaches the proxy. An IPv6 result is sent as the bracketed
    // text `[::1]` under the domain-name type (reqwest 0.12.28 with hyper-util
    // 0.1.20), which a real proxy cannot resolve; an IPv4 result is sent as an
    // address.
    assert_ne!(seen.host, "localhost");
    assert!(
        matches!(
            (seen.address_type, seen.host.as_str()),
            (1, "127.0.0.1") | (3, "[::1]")
        ),
        "{} {}",
        seen.address_type,
        seen.host
    );
    assert_eq!(seen.port, 8545);
    assert_eq!(seen.credentials, None);
    task.await.unwrap();
}

fn gzip_response(body: &[u8], encoding: &str) -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(body).unwrap();
    let compressed = encoder.finish().unwrap();
    let mut out = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Encoding: {encoding}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        compressed.len()
    )
    .into_bytes();
    out.extend(compressed);
    out
}

#[tokio::test]
async fn gzip_is_requested_and_decoded() {
    let (endpoint, rx, task) = raw_server(|req| {
        let body = json!({"jsonrpc":"2.0","id":req["id"],"result":"0x9"}).to_string();
        gzip_response(body.as_bytes(), "gzip")
    })
    .await;
    let result = transport()
        .request(&endpoint, "quai_chainId", json!([]))
        .await
        .unwrap();
    assert_eq!(result, "0x9");
    let (header, _) = rx.await.unwrap();
    assert!(
        header
            .lines()
            .any(|l| l.eq_ignore_ascii_case("accept-encoding: gzip")),
        "{header}"
    );
    task.await.unwrap();
}

#[tokio::test]
async fn the_response_limit_bounds_the_decoded_body() {
    // 8 MiB of spaces compresses to about 8 KB: under the limit on the wire,
    // far over it decoded.
    let (endpoint, rx, task) = raw_server(|_| gzip_response(&vec![b' '; 8 << 20], "gzip")).await;
    let client =
        HttpTransport::new(HttpConfig::default().with_max_response_bytes(64 * 1024)).unwrap();
    assert!(matches!(
        client.request(&endpoint, "quai_chainId", json!([])).await,
        Err(RpcError::ResponseTooLarge)
    ));
    rx.await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn an_unknown_encoding_or_a_corrupt_gzip_body_is_refused() {
    let (endpoint, rx, task) = raw_server(|_| gzip_response(b"{}", "br")).await;
    assert!(matches!(
        transport()
            .request(&endpoint, "quai_chainId", json!([]))
            .await,
        Err(RpcError::InvalidResponse(_))
    ));
    rx.await.unwrap();
    task.await.unwrap();
    let (endpoint, rx, task) = raw_server(|_| {
        let mut response = gzip_response(b"{}", "gzip");
        let last = response.len() - 1;
        response[last] ^= 0xff;
        response
    })
    .await;
    assert!(matches!(
        transport()
            .request(&endpoint, "quai_chainId", json!([]))
            .await,
        Err(RpcError::InvalidResponse(_))
    ));
    rx.await.unwrap();
    task.await.unwrap();
}
