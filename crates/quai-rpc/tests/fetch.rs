//! Owned HTTP fixtures for exact resources, gzip limits and redirect isolation.
#![cfg(feature = "http")]
use quai_rpc::fetch::*;
use std::{
    io::Write,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
struct Server {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    delay: u64,
    chunked: bool,
}
fn reply(status: u16, body: &[u8]) -> Reply {
    Reply {
        status,
        headers: vec![],
        body: body.into(),
        delay: 0,
        chunked: false,
    }
}
async fn server(replies: Vec<Reply>) -> Server {
    let socket = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", socket.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(vec![]));
    let saved = requests.clone();
    let task = tokio::spawn(async move {
        for response in replies {
            let (mut stream, _) = socket.accept().await.unwrap();
            let mut bytes = vec![];
            while !bytes.ends_with(b"\r\n\r\n") {
                let b = stream.read_u8().await.unwrap();
                bytes.push(b);
                assert!(bytes.len() < 32_768);
            }
            let header = String::from_utf8(bytes.clone()).unwrap();
            let length = header
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|n| n.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            assert!(length < MAX_FETCH_BYTES);
            let mut body = vec![0; length];
            stream.read_exact(&mut body).await.unwrap();
            bytes.extend(body);
            saved
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(&bytes).into_owned());
            tokio::time::sleep(Duration::from_millis(response.delay)).await;
            let mut head = format!(
                "HTTP/1.1 {} Fixture\r\nConnection: close\r\n",
                response.status
            );
            for (k, v) in response.headers {
                head += &format!("{k}: {v}\r\n");
            }
            if response.chunked {
                head += "Transfer-Encoding: chunked\r\n\r\n";
            } else {
                head += &format!("Content-Length: {}\r\n\r\n", response.body.len());
            }
            if stream.write_all(head.as_bytes()).await.is_ok() {
                let data = if response.chunked {
                    let mut v = format!("{:x}\r\n", response.body.len()).into_bytes();
                    v.extend(response.body);
                    v.extend(b"\r\n0\r\n\r\n");
                    v
                } else {
                    response.body
                };
                let _ = stream.write_all(&data).await;
            }
        }
    });
    Server {
        url,
        requests,
        task,
    }
}
fn client(config: FetchConfig) -> FetchClient<NativeFetch> {
    FetchClient::new(NativeFetch::new(1000).unwrap(), config).unwrap()
}
#[tokio::test]
async fn actual_http_preserves_error_bodies_and_post_bytes_without_rpc_envelope() {
    let server = server(vec![
        reply(418, b"PUBLIC error"),
        reply(200, b"{\"ok\":true}"),
    ])
    .await;
    let client = client(FetchConfig::default());
    let mut r = FetchRequest::new(&format!("{}/resource?toy=1", server.url)).unwrap();
    r.set_text("PUBLIC π").unwrap();
    r.headers_mut().set("x-public", "yes").unwrap();
    let got = client
        .send(&r, &FetchCancellation::default())
        .await
        .unwrap();
    assert_eq!(got.response.status(), 418);
    assert_eq!(got.response.text().unwrap(), "PUBLIC error");
    assert_eq!(got.response.assert_ok(), Err(FetchError::Status(418)));
    let h = server.requests.lock().unwrap()[0].clone();
    assert!(h.starts_with("POST /resource?toy=1 HTTP/1.1"));
    assert!(h.contains("content-type: text/plain"));
    assert!(h.contains("x-public: yes"));
    assert!(h.ends_with("PUBLIC π"));
    r.clear_body();
    let got = client
        .send(&r, &FetchCancellation::default())
        .await
        .unwrap();
    assert_eq!(got.response.json().unwrap()["ok"], true);
}
fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(vec![], flate2::Compression::default());
    e.write_all(bytes).unwrap();
    e.finish().unwrap()
}
#[tokio::test]
async fn gzip_chunked_and_declared_lengths_enforce_decoded_and_wire_limits() {
    let mut compressed = reply(200, &gzip(b"{\"ok\":true}"));
    compressed
        .headers
        .push(("Content-Encoding".into(), "gzip".into()));
    let mut bomb = reply(200, &gzip(&vec![b'x'; 8192]));
    bomb.headers
        .push(("Content-Encoding".into(), "gzip".into()));
    let mut chunked = reply(200, &vec![b'x'; 2048]);
    chunked.chunked = true;
    let server = server(vec![
        compressed,
        bomb,
        chunked,
        reply(200, &vec![b'x'; 2048]),
    ])
    .await;
    let client = client(FetchConfig::default().with_max_response_bytes(1024));
    let req = FetchRequest::new(&server.url).unwrap();
    assert_eq!(
        client
            .send(&req, &FetchCancellation::default())
            .await
            .unwrap()
            .response
            .json()
            .unwrap()["ok"],
        true
    );
    for _ in 0..3 {
        assert!(matches!(
            client.send(&req, &FetchCancellation::default()).await,
            Err(FetchError::Limit)
        ));
    }
}
#[tokio::test]
async fn actual_redirect_clears_private_headers_and_slow_reads_obey_deadline() {
    let destination = server(vec![reply(200, b"public")]).await;
    let mut redirect = reply(302, b"");
    redirect
        .headers
        .push(("Location".into(), destination.url.clone()));
    let mut slow = reply(200, b"late");
    slow.delay = 200;
    let source = server(vec![redirect, slow]).await;
    let client = client(
        FetchConfig::default()
            .with_timeout_ms(1000)
            .with_retry_delay_ms(0)
            .with_max_attempts(2)
            .with_max_redirects(1),
    );
    let mut req = FetchRequest::new(&source.url).unwrap();
    req.headers_mut()
        .set("x-api-key", "PUBLIC-TOY-TOKEN")
        .unwrap();
    let result = client
        .send(&req, &FetchCancellation::default())
        .await
        .unwrap();
    assert_eq!(result.attempts, 2);
    assert_eq!(result.response.text().unwrap(), "public");
    assert!(!destination.requests.lock().unwrap()[0].contains("x-api-key"));
    let short = FetchClient::new(
        NativeFetch::new(1000).unwrap(),
        FetchConfig::default()
            .with_timeout_ms(30)
            .with_retry_delay_ms(0),
    )
    .unwrap();
    assert!(matches!(
        short.send(&req, &FetchCancellation::default()).await,
        Err(FetchError::Timeout)
    ));
}
