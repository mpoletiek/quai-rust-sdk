//! Real loopback WebSocket protocol, concurrency, lifecycle and resource-limit tests.
#![cfg(all(feature = "ws", not(target_arch = "wasm32")))]
use futures_util::{SinkExt, StreamExt};
use quai_primitives::{Address, Hash32};
use quai_rpc::{Endpoint, RpcError, Transport, WsConfig, WsSubscriptionKind, WsTransport};
use serde_json::{Value, json};
use std::{future::Future, time::Duration};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::JoinHandle,
};
use tokio_tungstenite::{WebSocketStream, accept_async, accept_hdr_async, tungstenite::Message};

type Socket = WebSocketStream<TcpStream>;
async fn server<F, Fut>(handler: F) -> (Endpoint, JoinHandle<()>)
where
    F: FnOnce(Socket) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = Endpoint::parse(&format!(
        "ws://{}/custom/cyprus1?token=PUBLIC_FIXTURE",
        listener.local_addr().unwrap()
    ))
    .unwrap();
    let task = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(5), async {
            let (tcp, _) = listener.accept().await.unwrap();
            let socket = accept_async(tcp).await.unwrap();
            handler(socket).await;
        })
        .await
        .expect("fixture timed out");
    });
    (endpoint, task)
}
async fn recv(socket: &mut Socket) -> Value {
    loop {
        match socket.next().await.unwrap().unwrap() {
            Message::Text(text) => return serde_json::from_str(text.as_str()).unwrap(),
            Message::Ping(_) | Message::Pong(_) => {}
            other => panic!("unexpected fixture message {other:?}"),
        }
    }
}
async fn respond(socket: &mut Socket, id: &Value, result: Value) {
    socket
        .send(Message::Text(
            json!({"jsonrpc":"2.0","id":id,"result":result})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
}
async fn notify(socket: &mut Socket, id: &str, result: Value) {
    socket.send(Message::Text(json!({"jsonrpc":"2.0","method":"quai_subscription","params":{"subscription":id,"result":result}}).to_string().into())).await.unwrap();
}
async fn close_seen(socket: &mut Socket) {
    let message = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap();
    assert!(matches!(
        message,
        None | Some(Ok(Message::Close(_))) | Some(Err(_))
    ));
}

#[tokio::test]
#[allow(clippy::result_large_err)] // Tungstenite's required server-handshake callback error type.
async fn handshake_preserves_path_and_query_and_checks_endpoint_identity() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = Endpoint::parse(&format!(
        "ws://{}/custom/cyprus1?token=PUBLIC_FIXTURE",
        listener.local_addr().unwrap()
    ))
    .unwrap();
    let task = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut socket = accept_hdr_async(
            tcp,
            |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
                assert_eq!(
                    request.uri().path_and_query().unwrap().as_str(),
                    "/custom/cyprus1?token=PUBLIC_FIXTURE"
                );
                Ok(response)
            },
        )
        .await
        .unwrap();
        let request = recv(&mut socket).await;
        assert_eq!(request["method"], "quai_chainId");
        assert_eq!(request["params"], json!([]));
        respond(&mut socket, &request["id"], json!("0x9")).await;
        close_seen(&mut socket).await;
    });
    let client = WsTransport::connect(endpoint.clone(), WsConfig::default())
        .await
        .unwrap();
    assert!(client.is_open());
    assert!(!format!("{client:?}").contains("PUBLIC_FIXTURE"));
    let other = Endpoint::parse("ws://127.0.0.1:1/other").unwrap();
    assert!(matches!(
        client.request(&other, "quai_chainId", json!([])).await,
        Err(RpcError::InvalidConfig)
    ));
    assert!(matches!(
        client
            .request(&endpoint, "quai_subscribe", json!(["newHeads"]))
            .await,
        Err(RpcError::InvalidConfig)
    ));
    assert_eq!(
        client
            .request(&endpoint, "quai_chainId", json!([]))
            .await
            .unwrap(),
        "0x9"
    );
    client.shutdown().await.unwrap();
    assert!(!client.is_open());
    task.await.unwrap();
}
#[tokio::test]
async fn concurrent_requests_are_matched_by_id_even_when_replies_reverse() {
    let (endpoint, task) = server(|mut socket| async move {
        let first = recv(&mut socket).await;
        let second = recv(&mut socket).await;
        assert_ne!(first["id"], second["id"]);
        respond(&mut socket, &second["id"], second["method"].clone()).await;
        respond(&mut socket, &first["id"], first["method"].clone()).await;
        close_seen(&mut socket).await;
    })
    .await;
    let client = WsTransport::connect(endpoint.clone(), WsConfig::default())
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        client.request(&endpoint, "quai_chainId", json!([])),
        client.request(&endpoint, "quai_blockNumber", json!([]))
    );
    assert_eq!(a.unwrap(), "quai_chainId");
    assert_eq!(b.unwrap(), "quai_blockNumber");
    client.shutdown().await.unwrap();
    task.await.unwrap();
}
#[tokio::test]
async fn registration_handles_immediate_notifications_and_unsubscribe_keeps_session_usable() {
    let (endpoint, task) = server(|mut socket| async move {
        let subscribe = recv(&mut socket).await;
        assert_eq!(subscribe["method"], "quai_subscribe");
        assert_eq!(subscribe["params"], json!(["newHeads"]));
        respond(&mut socket, &subscribe["id"], json!("0xsubscription")).await;
        notify(
            &mut socket,
            "0xsubscription",
            json!({"woHeader":{"number":"0x10"}}),
        )
        .await;
        let unsubscribe = recv(&mut socket).await;
        assert_eq!(unsubscribe["method"], "quai_unsubscribe");
        assert_eq!(unsubscribe["params"], json!(["0xsubscription"]));
        respond(&mut socket, &unsubscribe["id"], json!(true)).await;
        let read = recv(&mut socket).await;
        respond(&mut socket, &read["id"], json!("0x9")).await;
        close_seen(&mut socket).await;
    })
    .await;
    let client = WsTransport::connect(endpoint.clone(), WsConfig::default())
        .await
        .unwrap();
    let mut subscription = client
        .subscribe(WsSubscriptionKind::NewHeads)
        .await
        .unwrap();
    assert_eq!(
        subscription.recv().await.unwrap().unwrap()["woHeader"]["number"],
        "0x10"
    );
    subscription.unsubscribe().await.unwrap();
    assert_eq!(
        client
            .request(&endpoint, "quai_chainId", json!([]))
            .await
            .unwrap(),
        "0x9"
    );
    client.shutdown().await.unwrap();
    task.await.unwrap();
}
#[tokio::test]
async fn log_and_pending_filters_use_exact_quai_subscription_arguments() {
    let address: Address = "0x0000000000000000000000000000000000000000"
        .parse()
        .unwrap();
    let topic = Hash32::ZERO;
    // The reference subscribes to accesses with the mixed-case checksum form of
    // the address, so this one is deliberately not all zeros: it pins the
    // encoding rather than a string that looks the same in any case.
    let accessed: Address = "0x002b2596EcF05C93a31ff916E8b456DF6C77c750"
        .parse()
        .unwrap();
    assert_ne!(
        accessed.to_string(),
        accessed.to_string().to_ascii_lowercase(),
        "fixture no longer exercises the checksum form"
    );
    let (endpoint, task) = server(move |mut socket| async move {
        for expected in [
            json!(["newPendingTransactions"]),
            json!(["logs",{"address":[address.to_string()],"topics":[null,[topic.to_string()]]}]),
            json!(["accesses", accessed.to_string()]),
        ] {
            let request = recv(&mut socket).await;
            assert_eq!(request["params"], expected);
            respond(&mut socket, &request["id"], json!("sub")).await;
            let unsubscribe = recv(&mut socket).await;
            respond(&mut socket, &unsubscribe["id"], json!(true)).await;
        }
        close_seen(&mut socket).await;
    })
    .await;
    let client = WsTransport::connect(endpoint, WsConfig::default())
        .await
        .unwrap();
    client
        .subscribe(WsSubscriptionKind::NewPendingTransactions)
        .await
        .unwrap()
        .unsubscribe()
        .await
        .unwrap();
    client
        .subscribe(WsSubscriptionKind::Logs {
            addresses: vec![address],
            topics: vec![None, Some(vec![topic])],
        })
        .await
        .unwrap()
        .unsubscribe()
        .await
        .unwrap();
    client
        .subscribe(WsSubscriptionKind::Accesses { address: accessed })
        .await
        .unwrap()
        .unsubscribe()
        .await
        .unwrap();
    client.shutdown().await.unwrap();
    task.await.unwrap();
}
#[tokio::test]
async fn notification_overflow_ends_only_the_lagging_subscription() {
    // Two ways to fall behind: the subscriber's own queue fills, or the
    // session's shared byte budget cannot hold the notification. Either ends
    // that subscription with an explicit error and unsubscribes it on the node,
    // while every other subscription and request on the session carries on.
    for byte_budget in [false, true] {
        let (endpoint, task) = server(move |mut socket| async move {
            for id in ["lagger", "keeper"] {
                let request = recv(&mut socket).await;
                assert_eq!(request["method"], "quai_subscribe");
                respond(&mut socket, &request["id"], json!(id)).await;
            }
            if byte_budget {
                notify(&mut socket, "lagger", json!("x".repeat(2048))).await;
            } else {
                notify(&mut socket, "lagger", json!("first")).await;
                notify(&mut socket, "lagger", json!("second")).await;
            }
            let unsubscribe = recv(&mut socket).await;
            assert_eq!(unsubscribe["method"], "quai_unsubscribe");
            assert_eq!(unsubscribe["params"], json!(["lagger"]));
            respond(&mut socket, &unsubscribe["id"], json!(true)).await;
            // A notification already in flight when the node saw the
            // unsubscribe is tolerated, as for a caller's own unsubscribe.
            notify(&mut socket, "lagger", json!("late")).await;
            notify(&mut socket, "keeper", json!("alive")).await;
            let read = recv(&mut socket).await;
            respond(&mut socket, &read["id"], json!("0x9")).await;
            let unsubscribe = recv(&mut socket).await;
            assert_eq!(unsubscribe["params"], json!(["keeper"]));
            respond(&mut socket, &unsubscribe["id"], json!(true)).await;
            close_seen(&mut socket).await;
        })
        .await;
        let config = WsConfig::default()
            .with_subscription_capacity(1)
            .with_max_notification_bytes(1024);
        let client = WsTransport::connect(endpoint.clone(), config)
            .await
            .unwrap();
        let mut lagger = client
            .subscribe(WsSubscriptionKind::NewHeads)
            .await
            .unwrap();
        let mut keeper = client
            .subscribe(WsSubscriptionKind::NewHeads)
            .await
            .unwrap();
        assert!(matches!(
            lagger.recv().await,
            Err(RpcError::SubscriptionLagged)
        ));
        assert!(lagger.recv().await.unwrap().is_none());
        if byte_budget {
            // Already removed on the node, so this sends nothing and succeeds.
            lagger.unsubscribe().await.unwrap();
        } else {
            // Dropping an ended subscription leaves the session alone.
            drop(lagger);
        }
        assert_eq!(keeper.recv().await.unwrap().unwrap(), "alive");
        assert_eq!(
            client
                .request(&endpoint, "quai_chainId", json!([]))
                .await
                .unwrap(),
            "0x9"
        );
        assert!(client.is_open());
        keeper.unsubscribe().await.unwrap();
        client.shutdown().await.unwrap();
        task.await.unwrap();
    }
}
#[tokio::test]
async fn keepalive_closes_a_silent_peer_and_keeps_a_responsive_one() {
    // A peer that stops reading never answers a ping: two intervals of inbound
    // silence mark the session disconnected instead of leaving it silent.
    let (endpoint, task) = server(|socket| async move {
        tokio::time::sleep(Duration::from_millis(600)).await;
        drop(socket);
    })
    .await;
    let config = WsConfig::default().with_ping_interval(Some(Duration::from_millis(50)));
    let client = WsTransport::connect(endpoint.clone(), config.clone())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!client.is_open(), "a silent peer must be detected");
    assert!(matches!(
        client.request(&endpoint, "quai_chainId", json!([])).await,
        Err(RpcError::Disconnected)
    ));
    task.await.unwrap();

    // A peer that reads answers pings automatically, so an idle session stays
    // open well past two intervals.
    let (endpoint, task) = server(|mut socket| async move {
        let read = recv(&mut socket).await;
        respond(&mut socket, &read["id"], json!("0x9")).await;
        close_seen(&mut socket).await;
    })
    .await;
    let client = WsTransport::connect(endpoint.clone(), config)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(client.is_open(), "a responsive idle peer must stay open");
    assert_eq!(
        client
            .request(&endpoint, "quai_chainId", json!([]))
            .await
            .unwrap(),
        "0x9"
    );
    client.shutdown().await.unwrap();
    task.await.unwrap();
}
#[tokio::test]
async fn cancelled_rpc_releases_capacity_and_late_reply_cannot_match_next_request() {
    let (seen, seen_rx) = oneshot::channel();
    let (endpoint, task) = server(move |mut socket| async move {
        let abandoned = recv(&mut socket).await;
        seen.send(()).unwrap();
        let next = recv(&mut socket).await;
        assert_ne!(abandoned["id"], next["id"]);
        respond(&mut socket, &abandoned["id"], json!("OLD")).await;
        respond(&mut socket, &next["id"], json!("NEW")).await;
        close_seen(&mut socket).await;
    })
    .await;
    let client = WsTransport::connect(endpoint.clone(), WsConfig::default().with_max_in_flight(1))
        .await
        .unwrap();
    let worker = client.clone();
    let target = endpoint.clone();
    let abandoned =
        tokio::spawn(async move { worker.request(&target, "quai_call", json!([])).await });
    seen_rx.await.unwrap();
    abandoned.abort();
    assert!(abandoned.await.unwrap_err().is_cancelled());
    assert_eq!(
        client
            .request(&endpoint, "quai_chainId", json!([]))
            .await
            .unwrap(),
        "NEW"
    );
    client.shutdown().await.unwrap();
    task.await.unwrap();
}
#[tokio::test]
async fn overall_deadline_includes_permit_wait_and_does_not_replay() {
    let (endpoint, task) = server(|mut socket| async move {
        let _ = recv(&mut socket).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        close_seen(&mut socket).await;
    })
    .await;
    let client = WsTransport::connect(
        endpoint.clone(),
        WsConfig::default()
            .with_request_timeout(Duration::from_millis(40))
            .with_max_in_flight(1),
    )
    .await
    .unwrap();
    let started = tokio::time::Instant::now();
    let (a, b) = tokio::join!(
        client.request(&endpoint, "quai_chainId", json!([])),
        client.request(&endpoint, "quai_blockNumber", json!([]))
    );
    assert!(matches!(a, Err(RpcError::Timeout)));
    assert!(matches!(b, Err(RpcError::Timeout)));
    assert!(started.elapsed() < Duration::from_millis(300));
    client.shutdown().await.unwrap();
    task.await.unwrap();
}
#[tokio::test]
async fn oversize_inbound_and_outbound_messages_are_rejected() {
    let (endpoint, task) = server(|mut socket| async move {
        let _ = recv(&mut socket).await;
        let _ = socket.send(Message::Text("x".repeat(1024).into())).await;
        close_seen(&mut socket).await;
    })
    .await;
    let client = WsTransport::connect(
        endpoint.clone(),
        WsConfig::default()
            .with_max_message_bytes(256)
            .with_max_frame_bytes(256),
    )
    .await
    .unwrap();
    assert!(matches!(
        client
            .request(&endpoint, "quai_call", json!(["x".repeat(300)]))
            .await,
        Err(RpcError::RequestTooLarge)
    ));
    assert!(matches!(
        client.request(&endpoint, "quai_chainId", json!([])).await,
        Err(RpcError::ResponseTooLarge)
    ));
    task.await.unwrap();
}
#[tokio::test]
async fn unsolicited_or_malformed_messages_fail_outstanding_requests() {
    for body in [json!({"jsonrpc":"2.0","id":999,"result":"0x9"}).to_string(),
                 "{\"jsonrpc\":\"2.0\",\"id\":1,\"id\":1,\"result\":\"0x9\"}".into(),
                 json!({"jsonrpc":"2.0","method":"quai_subscription","params":{"subscription":"unknown","result":null}}).to_string()] {
        let (endpoint,task)=server(move|mut socket|async move {let _=recv(&mut socket).await;socket.send(Message::Text(body.into())).await.unwrap();close_seen(&mut socket).await;}).await;
        let client=WsTransport::connect(endpoint.clone(),WsConfig::default()).await.unwrap();
        assert!(matches!(client.request(&endpoint,"quai_chainId",json!([])).await,Err(RpcError::InvalidResponse(_))));task.await.unwrap();
    }
}
#[tokio::test]
async fn explicit_shutdown_and_active_subscription_drop_disconnect_other_calls() {
    let (endpoint, task) = server(|mut socket| async move {
        let request = recv(&mut socket).await;
        respond(&mut socket, &request["id"], json!("sub")).await;
        close_seen(&mut socket).await;
    })
    .await;
    let client = WsTransport::connect(endpoint.clone(), WsConfig::default())
        .await
        .unwrap();
    let sub = client
        .subscribe(WsSubscriptionKind::NewHeads)
        .await
        .unwrap();
    drop(sub);
    task.await.unwrap();
    assert!(matches!(
        client.request(&endpoint, "quai_chainId", json!([])).await,
        Err(RpcError::Disconnected)
    ));
    client.shutdown().await.unwrap();
}
#[tokio::test]
async fn failed_upgrade_redirect_is_not_followed_and_diagnostics_are_redacted() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint =
        Endpoint::parse(&format!("ws://{}/SECRET", listener.local_addr().unwrap())).unwrap();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0; 4096];
        let read = socket.read(&mut buf).await.unwrap();
        assert!(read > 0);
        socket.write_all(b"HTTP/1.1 302 Found\r\nLocation: ws://127.0.0.1:1/SECRET\r\nContent-Length: 0\r\n\r\n").await.unwrap();
    });
    let error = WsTransport::connect(endpoint, WsConfig::default())
        .await
        .unwrap_err();
    assert!(matches!(error, RpcError::HttpStatus(302)));
    assert!(!format!("{error:?}").contains("SECRET"));
    task.await.unwrap();
}

#[tokio::test]
async fn cancelled_registration_closes_session_without_leaking_server_subscription() {
    let (sent, received) = oneshot::channel();
    let (endpoint, server_task) = server(|mut socket| async move {
        let request = recv(&mut socket).await;
        assert_eq!(request["method"], "quai_subscribe");
        sent.send(()).unwrap();
        close_seen(&mut socket).await;
    })
    .await;
    let client = WsTransport::connect(endpoint.clone(), WsConfig::default())
        .await
        .unwrap();
    let owner = client.clone();
    let registration =
        tokio::spawn(async move { owner.subscribe(WsSubscriptionKind::NewHeads).await });
    received.await.unwrap();
    registration.abort();
    assert!(registration.await.unwrap_err().is_cancelled());
    server_task.await.unwrap();
    assert!(matches!(
        client.request(&endpoint, "quai_chainId", json!([])).await,
        Err(RpcError::Disconnected)
    ));
}

#[tokio::test]
async fn explicit_shutdown_terminates_pending_rpc_and_active_subscription() {
    let (sent, received) = oneshot::channel();
    let (endpoint, server_task) = server(|mut socket| async move {
        let request = recv(&mut socket).await;
        respond(&mut socket, &request["id"], json!("0xfeed")).await;
        let request = recv(&mut socket).await;
        assert_eq!(request["method"], "quai_chainId");
        sent.send(()).unwrap();
        close_seen(&mut socket).await;
    })
    .await;
    let client = WsTransport::connect(endpoint.clone(), WsConfig::default())
        .await
        .unwrap();
    let mut subscription = client
        .subscribe(WsSubscriptionKind::NewHeads)
        .await
        .unwrap();
    let owner = client.clone();
    let request =
        tokio::spawn(async move { owner.request(&endpoint, "quai_chainId", json!([])).await });
    received.await.unwrap();
    client.shutdown().await.unwrap();
    assert!(matches!(
        request.await.unwrap(),
        Err(RpcError::Disconnected)
    ));
    assert!(matches!(
        subscription.recv().await,
        Err(RpcError::Disconnected)
    ));
    assert!(subscription.recv().await.unwrap().is_none());
    server_task.await.unwrap();
}

#[tokio::test]
#[ignore = "requires explicit QUAI_WS_URL and QUAI_EXPECTED_CHAIN_ID; read-only subscriptions"]
async fn live_chain_and_subscription_handshake() {
    let endpoint =
        Endpoint::parse(&std::env::var("QUAI_WS_URL").expect("set exact ws/wss endpoint")).unwrap();
    let chain = std::env::var("QUAI_EXPECTED_CHAIN_ID")
        .expect("set decimal chain ID")
        .parse::<u64>()
        .unwrap();
    let client = WsTransport::connect(endpoint.clone(), WsConfig::default())
        .await
        .unwrap();
    assert_eq!(
        client
            .request(&endpoint, "quai_chainId", json!([]))
            .await
            .unwrap(),
        json!(format!("0x{chain:x}"))
    );
    let mut subscription = client
        .subscribe(WsSubscriptionKind::NewHeads)
        .await
        .unwrap();
    let notification = tokio::time::timeout(Duration::from_secs(30), subscription.recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(notification.get("woHeader").is_some());
    subscription.unsubscribe().await.unwrap();
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_notification_racing_its_own_unsubscribe_does_not_tear_down_the_session() {
    // go-quai is a geth derivative: notifications are written by the notifier
    // and RPC replies by the request handler, with no ordering guarantee
    // between them. A notification handed to the writer before the unsubscribe
    // was processed can therefore arrive after its acknowledgement. That is a
    // normal race, and it must not fail every other subscription and every
    // in-flight request sharing the connection.
    let (endpoint, task) = server(|mut socket| async move {
        let first = recv(&mut socket).await;
        assert_eq!(first["method"], "quai_subscribe");
        respond(&mut socket, &first["id"], json!("0xdoomed")).await;
        let second = recv(&mut socket).await;
        assert_eq!(second["method"], "quai_subscribe");
        respond(&mut socket, &second["id"], json!("0xkeeper")).await;

        let unsubscribe = recv(&mut socket).await;
        assert_eq!(unsubscribe["method"], "quai_unsubscribe");
        assert_eq!(unsubscribe["params"], json!(["0xdoomed"]));
        respond(&mut socket, &unsubscribe["id"], json!(true)).await;

        // The trailing notification, delivered after its own acknowledgement.
        notify(
            &mut socket,
            "0xdoomed",
            json!({"woHeader":{"number":"0x11"}}),
        )
        .await;

        // The surviving subscription must still receive, and ordinary requests
        // must still be answered, which is only possible if the session lived.
        notify(
            &mut socket,
            "0xkeeper",
            json!({"woHeader":{"number":"0x12"}}),
        )
        .await;
        let read = recv(&mut socket).await;
        respond(&mut socket, &read["id"], json!("0x9")).await;
        close_seen(&mut socket).await;
    })
    .await;

    let client = WsTransport::connect(endpoint.clone(), WsConfig::default())
        .await
        .unwrap();
    let doomed = client
        .subscribe(WsSubscriptionKind::NewHeads)
        .await
        .unwrap();
    let mut keeper = client
        .subscribe(WsSubscriptionKind::NewHeads)
        .await
        .unwrap();
    doomed.unsubscribe().await.unwrap();

    // The unrelated subscription still delivers.
    assert_eq!(
        keeper.recv().await.unwrap().unwrap()["woHeader"]["number"],
        "0x12"
    );
    // And the multiplexed connection still serves ordinary requests.
    assert_eq!(
        client
            .request(&endpoint, "quai_chainId", json!([]))
            .await
            .unwrap(),
        "0x9"
    );
    client.shutdown().await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn a_notification_for_a_never_known_subscription_still_fails_the_session() {
    // Distinct from the race above: an ID this session never registered is a
    // protocol violation by the node, and the strict posture is retained.
    let (endpoint, task) = server(|mut socket| async move {
        let subscribe = recv(&mut socket).await;
        respond(&mut socket, &subscribe["id"], json!("0xreal")).await;
        notify(
            &mut socket,
            "0xnever",
            json!({"woHeader":{"number":"0x13"}}),
        )
        .await;
        let _ = socket.next().await;
    })
    .await;
    let client = WsTransport::connect(endpoint.clone(), WsConfig::default())
        .await
        .unwrap();
    let mut subscription = client
        .subscribe(WsSubscriptionKind::NewHeads)
        .await
        .unwrap();
    assert!(
        subscription.recv().await.is_err(),
        "an unknown subscription ID must still terminate the session"
    );
    task.abort();
}

#[tokio::test]
async fn a_batch_multiplexes_over_one_session_and_keeps_request_order() {
    // WS batching is not JSON-RPC array framing: the session is already
    // pipelined, so issuing a group concurrently collapses the same round trips
    // without teaching the dispatch path to correlate many IDs from one frame.
    // The properties that matter are that results keep their request positions
    // even when the server replies out of order, and that a per-call remote
    // error stays attached to its own position.
    let (endpoint, task) = server(|mut socket| async move {
        // Collect the whole group before replying, which is only possible if
        // the calls were genuinely in flight together rather than sequential.
        let mut seen = Vec::new();
        for _ in 0..4 {
            seen.push(recv(&mut socket).await);
        }
        assert_eq!(seen.len(), 4);
        // Reply in reverse, and fail exactly one call.
        for request in seen.iter().rev() {
            let index = request["params"][0].as_u64().unwrap();
            if index == 2 {
                let id = &request["id"];
                socket
                    .send(Message::Text(
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":"public fixture"}})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
            } else {
                respond(&mut socket, &request["id"], json!(format!("0x{index}"))).await;
            }
        }
        close_seen(&mut socket).await;
    })
    .await;

    let client = WsTransport::connect(endpoint.clone(), WsConfig::default())
        .await
        .unwrap();
    let requests: Vec<(&str, Value)> = (0..4).map(|i| ("quai_chainId", json!([i]))).collect();
    let results = client
        .request_batch(&endpoint, requests)
        .await
        .expect("the WebSocket transport supports batching")
        .expect("the group itself succeeded");

    assert_eq!(results.len(), 4);
    // Order follows the request positions, not the reply order.
    assert_eq!(results[0].as_ref().unwrap(), &json!("0x0"));
    assert_eq!(results[1].as_ref().unwrap(), &json!("0x1"));
    assert_eq!(results[3].as_ref().unwrap(), &json!("0x3"));
    // The failing call keeps its own position and does not poison the rest.
    assert!(
        matches!(results[2], Err(RpcError::Remote(_))),
        "expected a per-call remote error, got {:?}",
        results[2]
    );

    client.shutdown().await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn a_batch_refuses_subscription_methods_and_an_oversize_group_without_sending() {
    let (endpoint, task) = server(|mut socket| async move {
        // Nothing must be sent for either rejection.
        let _ = socket.next().await;
    })
    .await;
    let client = WsTransport::connect(endpoint.clone(), WsConfig::default())
        .await
        .unwrap();

    // Subscriptions carry registration state and cannot ride in a plain group.
    assert!(matches!(
        client
            .request_batch(&endpoint, vec![("quai_subscribe", json!(["newHeads"]))])
            .await,
        Some(Err(RpcError::InvalidConfig))
    ));
    // Empty and oversize groups are refused rather than partially issued.
    assert!(matches!(
        client.request_batch(&endpoint, vec![]).await,
        Some(Err(RpcError::InvalidConfig))
    ));
    let oversize: Vec<(&str, Value)> = (0..129).map(|_| ("quai_chainId", json!([]))).collect();
    assert!(matches!(
        client.request_batch(&endpoint, oversize).await,
        Some(Err(RpcError::InvalidConfig))
    ));

    client.shutdown().await.unwrap();
    task.abort();
}
