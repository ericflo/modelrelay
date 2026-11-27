use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use axum::{
    Router,
    body::Body,
    extract::OriginalUri,
    http::{HeaderMap as AxumHeaderMap, StatusCode},
    response::Response,
    routing::any,
};
use futures_util::{SinkExt, StreamExt};
use modelrelay_protocol::{
    ProviderRouting, RegisterMessage, ResponseChunkMessage, ServerToWorkerMessage,
    WorkerToServerMessage,
};
use modelrelay_server::{
    ApiKeyStore, InMemoryApiKeyStore, ProviderQueuePolicy, ProxyHttpApp, ProxyServerCore,
    WorkerSocketApp, WorkerSocketProviderConfig,
};
use modelrelay_worker::{
    WorkerConnectionState, WorkerDaemon, WorkerDaemonConfig, WorkerStatusSnapshot,
};
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Mutex, Notify, watch},
    time::timeout,
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Bytes, Message, client::IntoClientRequest},
};

async fn spawn_proxy(
    core: Arc<Mutex<ProxyServerCore>>,
    worker_socket_app: WorkerSocketApp,
    http_builder: impl FnOnce(ProxyHttpApp) -> ProxyHttpApp,
) -> SocketAddr {
    let app =
        http_builder(ProxyHttpApp::new(core).with_worker_socket_app(worker_socket_app)).router();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

fn decode_chunked_body(raw: &str) -> String {
    let mut out = String::new();
    let mut rest = raw;
    while let Some((size_line, after_size)) = rest.split_once("\r\n") {
        let Ok(size) = usize::from_str_radix(size_line.trim(), 16) else {
            return raw.to_string();
        };
        if size == 0 {
            break;
        }
        if after_size.len() < size {
            break;
        }
        out.push_str(&after_size[..size]);
        rest = after_size[size..].strip_prefix("\r\n").unwrap_or("");
    }
    out
}

async fn send_raw_http(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> (u16, String, String) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    for (k, v) in headers {
        use std::fmt::Write as _;
        let _ = write!(req, "{k}: {v}\r\n");
    }
    if let Some(b) = body {
        use std::fmt::Write as _;
        let _ = write!(req, "Content-Length: {}\r\n", b.len());
    }
    req.push_str("\r\n");
    if let Some(b) = body {
        req.push_str(b);
    }
    stream.write_all(req.as_bytes()).await.unwrap();

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.unwrap();
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, rest) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .expect("valid http status");
    (status, head.to_string(), rest.to_string())
}

async fn wait_for_connected(rx: &mut watch::Receiver<WorkerStatusSnapshot>) {
    timeout(Duration::from_secs(5), async {
        loop {
            if rx.borrow().state == WorkerConnectionState::Connected {
                return;
            }
            rx.changed().await.unwrap();
        }
    })
    .await
    .expect("worker should connect within 5s");
}

/// F-01: Cancelling an in-flight request on a real `WorkerDaemon` with `max_concurrent=1`
/// must send `ResponseComplete(499)` and immediately free the worker slot for the next request.
#[tokio::test]
async fn cancel_frees_worker_slot_immediately_on_max_concurrent_one() {
    let started_first = Arc::new(Notify::new());
    let started_first_clone = Arc::clone(&started_first);
    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_clone = Arc::clone(&call_count);

    let backend = Router::new().fallback(any(move || {
        let started_first = Arc::clone(&started_first_clone);
        let call_count = Arc::clone(&call_count_clone);
        async move {
            let n = call_count.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                started_first.notify_one();
                // Stream one chunk then stall forever until cancelled.
                let s = futures_util::stream::unfold(0_u8, |step| async move {
                    if step == 0 {
                        Some((
                            Ok::<_, std::convert::Infallible>(axum::body::Bytes::from_static(
                                b"data: {\"chunk\":1}\n\n",
                            )),
                            1,
                        ))
                    } else {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                        None
                    }
                });
                Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "text/event-stream")
                    .body(Body::from_stream(s))
                    .unwrap()
            } else {
                Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"ok":true}"#))
                    .unwrap()
            }
        }
    }));

    let backend_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_addr = backend_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(backend_listener, backend).await.unwrap();
    });

    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let ws_app = WorkerSocketApp::new(Arc::clone(&core))
        .with_provider("local", WorkerSocketProviderConfig::enabled("secret"));
    let proxy_addr = spawn_proxy(Arc::clone(&core), ws_app, |app| {
        app.with_models_provider("local")
    })
    .await;

    let (status_tx, mut status_rx) = watch::channel(WorkerStatusSnapshot::default());
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let daemon = WorkerDaemon::new(WorkerDaemonConfig {
        proxy_base_url: format!("http://{proxy_addr}"),
        provider: "local".to_string(),
        worker_secret: "secret".to_string(),
        worker_name: "w1".to_string(),
        models: vec!["m1".to_string()],
        max_concurrent: 1,
        backend_base_url: format!("http://{backend_addr}"),
        endpoint_prefixes: vec![],
    })
    .with_status_sender(status_tx)
    .with_shutdown_rx(shutdown_rx);

    let worker_task = tokio::spawn(async move {
        let _ = daemon.run_with_reconnect().await;
    });
    wait_for_connected(&mut status_rx).await;

    // Issue streaming request, wait until first chunk arrives, then abruptly drop TCP!
    let mut client = TcpStream::connect(proxy_addr).await.unwrap();
    let body = r#"{"model":"m1","stream":true}"#;
    let req = format!(
        "POST /v1/chat/completions HTTP/1.1\r\nHost: {proxy_addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    client.write_all(req.as_bytes()).await.unwrap();
    started_first.notified().await;
    let mut buf = [0_u8; 512];
    let n = timeout(Duration::from_secs(3), client.read(&mut buf))
        .await
        .expect("read first chunk")
        .unwrap();
    assert!(n > 0);
    drop(client); // Client disconnects mid-stream!

    // Give cancel -> ResponseComplete(499) a brief moment, then issue a second request.
    let (status, _, resp_body) = timeout(
        Duration::from_secs(3),
        send_raw_http(
            proxy_addr,
            "POST",
            "/v1/chat/completions",
            &[("Content-Type", "application/json")],
            Some(r#"{"model":"m1","stream":false}"#),
        ),
    )
    .await
    .expect("second request must NOT wedge after cancel!");

    assert_eq!(status, 200);
    assert!(resp_body.contains(r#""ok":true"#), "got: {resp_body}");

    let _ = shutdown_tx.send(true);
    let _ = worker_task.await;
}

/// F-01 safety net: If a legacy worker ignores Cancel, `expire_cancelled_requests` reaps the slot.
#[test]
fn server_reaps_unacknowledged_cancelled_request_after_grace_period() {
    let mut core = ProxyServerCore::new();
    let wid = core
        .register_worker(
            "local",
            RegisterMessage {
                worker_name: "legacy".to_string(),
                models: vec!["m1".to_string()],
                max_concurrent: 1,
                protocol_version: Some("2026-04-bridge-v1".to_string()),
                current_load: Some(0),
                endpoint_prefixes: vec![],
                model_metadata: modelrelay_protocol::ModelMetadataMap::new(),
            },
        )
        .worker_id;

    let first = core.submit_request("local", "m1");
    assert!(matches!(
        first,
        modelrelay_server::SubmissionOutcome::Dispatched(_)
    ));
    let second = core.submit_request("local", "m1");
    assert!(matches!(
        second,
        modelrelay_server::SubmissionOutcome::Queued(_)
    ));

    let _ = core.cancel_request(
        "request-1",
        modelrelay_server::CancelReason::ClientDisconnected,
    );
    // Still occupied before grace expires
    assert_eq!(core.worker_in_flight_request_ids(&wid), vec!["request-1"]);

    // After grace expires, slot is reclaimed and request-2 is dispatched immediately!
    let reaped = core.expire_cancelled_requests(
        Instant::now() + Duration::from_secs(6),
        Duration::from_secs(5),
    );
    assert_eq!(reaped, vec!["request-1"]);
    assert_eq!(core.worker_in_flight_request_ids(&wid), vec!["request-2"]);
}

/// F-02: Multi-byte UTF-8 characters split byte-by-byte across backend SSE chunks must
/// be reassembled intact without crashing the worker session.
#[tokio::test]
async fn split_multibyte_utf8_stream_does_not_crash_worker() {
    let payload = "data: {\"text\":\"Hello — 世界 🚀!\"}\n\n".to_string();
    let payload_bytes = payload.into_bytes();

    let backend = Router::new().fallback(any(move || {
        let bytes = payload_bytes.clone();
        async move {
            // Emit 1 byte per chunk — splits every multi-byte UTF-8 codepoint!
            let s = futures_util::stream::iter(
                bytes
                    .into_iter()
                    .map(|b| Ok::<_, std::convert::Infallible>(axum::body::Bytes::from(vec![b]))),
            );
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(s))
                .unwrap()
        }
    }));

    let backend_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_addr = backend_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(backend_listener, backend).await.unwrap();
    });

    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let ws_app = WorkerSocketApp::new(Arc::clone(&core))
        .with_provider("local", WorkerSocketProviderConfig::enabled("secret"));
    let proxy_addr = spawn_proxy(Arc::clone(&core), ws_app, |app| {
        app.with_models_provider("local")
    })
    .await;

    let (status_tx, mut status_rx) = watch::channel(WorkerStatusSnapshot::default());
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let daemon = WorkerDaemon::new(WorkerDaemonConfig {
        proxy_base_url: format!("http://{proxy_addr}"),
        provider: "local".to_string(),
        worker_secret: "secret".to_string(),
        worker_name: "utf8-worker".to_string(),
        models: vec!["m1".to_string()],
        max_concurrent: 2,
        backend_base_url: format!("http://{backend_addr}"),
        endpoint_prefixes: vec![],
    })
    .with_status_sender(status_tx)
    .with_shutdown_rx(shutdown_rx);

    let worker_task = tokio::spawn(async move {
        let _ = daemon.run_with_reconnect().await;
    });
    wait_for_connected(&mut status_rx).await;

    let (status, _, body) = send_raw_http(
        proxy_addr,
        "POST",
        "/v1/chat/completions",
        &[("Content-Type", "application/json")],
        Some(r#"{"model":"m1","stream":true}"#),
    )
    .await;

    assert_eq!(status, 200);
    let decoded_body = decode_chunked_body(&body);
    assert!(
        decoded_body.contains("Hello — 世界 🚀!"),
        "expected intact UTF-8 text, got: {decoded_body}"
    );
    assert_eq!(status_rx.borrow().state, WorkerConnectionState::Connected);

    let _ = shutdown_tx.send(true);
    let _ = worker_task.await;
}

/// F-02 & F-15: A backend connection error on one request must return 502 JSON for that
/// request ONLY without aborting sibling in-flight requests or tearing down the worker WS.
#[tokio::test]
async fn single_backend_failure_does_not_abort_sibling_or_disconnect_worker() {
    let sibling_started = Arc::new(Notify::new());
    let sibling_started_clone = Arc::clone(&sibling_started);

    let backend_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_addr = backend_listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = backend_listener.accept().await else {
                break;
            };
            let sibling_started = Arc::clone(&sibling_started_clone);
            tokio::spawn(async move {
                let mut buf = [0_u8; 4096];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                if req.contains("/v1/crash") {
                    // Abruptly reset/drop connection without writing any HTTP response!
                    drop(sock);
                } else {
                    sibling_started.notify_one();
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    let body = r#"{"sibling":"survived"}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                }
            });
        }
    });

    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let ws_app = WorkerSocketApp::new(Arc::clone(&core))
        .with_provider("local", WorkerSocketProviderConfig::enabled("secret"));
    let proxy_addr = spawn_proxy(Arc::clone(&core), ws_app, |app| {
        app.with_models_provider("local")
    })
    .await;

    let (status_tx, mut status_rx) = watch::channel(WorkerStatusSnapshot::default());
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let daemon = WorkerDaemon::new(WorkerDaemonConfig {
        proxy_base_url: format!("http://{proxy_addr}"),
        provider: "local".to_string(),
        worker_secret: "secret".to_string(),
        worker_name: "isolated-worker".to_string(),
        models: vec!["m1".to_string()],
        max_concurrent: 2,
        backend_base_url: format!("http://{backend_addr}"),
        endpoint_prefixes: vec![],
    })
    .with_status_sender(status_tx)
    .with_shutdown_rx(shutdown_rx);

    let worker_task = tokio::spawn(async move {
        let _ = daemon.run_with_reconnect().await;
    });
    wait_for_connected(&mut status_rx).await;
    let initial_worker_id = status_rx.borrow().worker_id.clone();

    let sibling_fut = tokio::spawn(async move {
        send_raw_http(
            proxy_addr,
            "POST",
            "/v1/healthy",
            &[("Content-Type", "application/json")],
            Some(r#"{"model":"m1","stream":false}"#),
        )
        .await
    });

    sibling_started.notified().await;

    let (crash_status, _, crash_body) = send_raw_http(
        proxy_addr,
        "POST",
        "/v1/crash",
        &[("Content-Type", "application/json")],
        Some(r#"{"model":"m1","stream":false}"#),
    )
    .await;
    assert_eq!(crash_status, 502, "expected 502, got: {crash_body}");
    assert!(crash_body.contains("backend_error"));

    let (sib_status, _, sib_body) = sibling_fut.await.unwrap();
    assert_eq!(sib_status, 200);
    assert!(sib_body.contains("survived"));
    // Worker must STILL be the exact same connected session (never dropped/reconnected)
    assert_eq!(status_rx.borrow().worker_id, initial_worker_id);

    let _ = shutdown_tx.send(true);
    let _ = worker_task.await;
}

/// F-03: Transport-level WS Ping frames arriving faster than `heartbeat_interval` must NOT
/// prevent the server from sending JSON `Ping` and disconnecting a worker that never sends JSON `Pong`.
#[tokio::test]
async fn server_json_heartbeat_fires_despite_faster_client_ws_pings() {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let ws_app = WorkerSocketApp::new(Arc::clone(&core))
        .with_provider("local", WorkerSocketProviderConfig::enabled("secret"))
        .with_heartbeat(Duration::from_millis(120), Duration::from_millis(200));
    let proxy_addr = spawn_proxy(Arc::clone(&core), ws_app, |app| {
        app.with_models_provider("local")
    })
    .await;

    let mut req = format!("ws://{proxy_addr}/v1/worker/connect?provider=local")
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("x-worker-secret", "secret".parse().unwrap());
    let (mut ws, _) = connect_async(req).await.unwrap();

    let reg = serde_json::to_string(&WorkerToServerMessage::Register(RegisterMessage {
        worker_name: "ping-spammer".to_string(),
        models: vec!["m1".to_string()],
        max_concurrent: 1,
        protocol_version: Some("2026-04-bridge-v1".to_string()),
        current_load: Some(0),
        endpoint_prefixes: vec![],
        model_metadata: modelrelay_protocol::ModelMetadataMap::new(),
    }))
    .unwrap();
    ws.send(Message::Text(reg.into())).await.unwrap();

    // Receive RegisterAck
    let ack = ws.next().await.unwrap().unwrap();
    assert!(matches!(ack, Message::Text(_)));

    // Spam WS Ping every 30ms (< 120ms server heartbeat interval) and NEVER answer JSON Ping!
    let mut saw_json_ping = false;
    let mut saw_close = false;
    let mut ping_interval = tokio::time::interval(Duration::from_millis(30));

    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        tokio::select! {
            _ = ping_interval.tick() => {
                let _ = ws.send(Message::Ping(Bytes::new())).await;
            }
            msg = ws.next() => {
                match msg {
                    Some(Ok(Message::Text(t))) => {
                        if let Ok(ServerToWorkerMessage::Ping(_)) = serde_json::from_str(&t) {
                            saw_json_ping = true;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        saw_close = true;
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    assert!(
        saw_json_ping,
        "server must send JSON Ping despite WS Ping spam"
    );
    assert!(
        saw_close,
        "server must disconnect worker that ignores JSON Ping"
    );
    assert_eq!(core.lock().await.connected_worker_count(), 0);
}

/// F-06: Mid-stream worker disconnect must emit an SSE error event and NEVER requeue
/// the already-started stream onto another worker.
#[tokio::test]
async fn mid_stream_disconnect_emits_sse_error_without_requeueing() {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let ws_app = WorkerSocketApp::new(Arc::clone(&core))
        .with_provider("local", WorkerSocketProviderConfig::enabled("secret"));
    let proxy_addr = spawn_proxy(Arc::clone(&core), ws_app, |app| {
        app.with_models_provider("local")
    })
    .await;

    let connect_raw = |name: &'static str| async move {
        let mut req = format!("ws://{proxy_addr}/v1/worker/connect?provider=local")
            .into_client_request()
            .unwrap();
        req.headers_mut()
            .insert("x-worker-secret", "secret".parse().unwrap());
        let (mut ws, _) = connect_async(req).await.unwrap();
        let reg = serde_json::to_string(&WorkerToServerMessage::Register(RegisterMessage {
            worker_name: name.to_string(),
            models: vec!["m1".to_string()],
            max_concurrent: 1,
            protocol_version: Some("2026-04-bridge-v1".to_string()),
            current_load: Some(0),
            endpoint_prefixes: vec![],
            model_metadata: modelrelay_protocol::ModelMetadataMap::new(),
        }))
        .unwrap();
        ws.send(Message::Text(reg.into())).await.unwrap();
        let _ = ws.next().await.unwrap().unwrap(); // RegisterAck
        ws
    };

    let mut w1 = connect_raw("w1").await;
    let mut w2 = connect_raw("w2").await;

    let http_task = tokio::spawn(async move {
        send_raw_http(
            proxy_addr,
            "POST",
            "/v1/chat/completions",
            &[("Content-Type", "application/json")],
            Some(r#"{"model":"m1","stream":true}"#),
        )
        .await
    });

    // w1 receives the request, emits 1 chunk, then dies abruptly.
    let msg = w1.next().await.unwrap().unwrap();
    let Message::Text(text) = msg else {
        panic!("expected text Request");
    };
    let ServerToWorkerMessage::Request(req) = serde_json::from_str(&text).unwrap() else {
        panic!("expected Request");
    };

    let chunk = serde_json::to_string(&WorkerToServerMessage::ResponseChunk(
        ResponseChunkMessage {
            request_id: req.request_id,
            chunk: "data: {\"partial\":1}\n\n".to_string(),
        },
    ))
    .unwrap();
    w1.send(Message::Text(chunk.into())).await.unwrap();
    tokio::time::sleep(Duration::from_millis(40)).await;
    drop(w1);

    let (status, _, body) = timeout(Duration::from_secs(3), http_task)
        .await
        .expect("stream must terminate cleanly")
        .unwrap();
    assert_eq!(status, 200);
    assert!(body.contains("partial"));
    assert!(
        body.contains("event: error") && body.contains("disconnected mid-stream"),
        "expected SSE error frame on mid-stream disconnect, got: {body}"
    );

    // w2 must NOT receive a requeued Franken-stream copy!
    let w2_got = timeout(Duration::from_millis(150), w2.next()).await;
    assert!(
        w2_got.is_err(),
        "second worker must NOT receive a requeued mid-stream request: {w2_got:?}"
    );
}

/// F-10: A streaming request that receives HTTP 400 from the backend must relay HTTP 400
/// with the JSON error body instead of HTTP 200 `text/event-stream`.
#[tokio::test]
async fn streaming_non_2xx_backend_error_relays_real_http_status() {
    let backend = Router::new().fallback(any(|| async {
        Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"error":{"message":"context length exceeded"}}"#,
            ))
            .unwrap()
    }));

    let backend_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_addr = backend_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(backend_listener, backend).await.unwrap();
    });

    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let ws_app = WorkerSocketApp::new(Arc::clone(&core))
        .with_provider("local", WorkerSocketProviderConfig::enabled("secret"));
    let proxy_addr = spawn_proxy(Arc::clone(&core), ws_app, |app| {
        app.with_models_provider("local")
    })
    .await;

    let (status_tx, mut status_rx) = watch::channel(WorkerStatusSnapshot::default());
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let daemon = WorkerDaemon::new(WorkerDaemonConfig {
        proxy_base_url: format!("http://{proxy_addr}"),
        provider: "local".to_string(),
        worker_secret: "secret".to_string(),
        worker_name: "err-worker".to_string(),
        models: vec!["m1".to_string()],
        max_concurrent: 1,
        backend_base_url: format!("http://{backend_addr}"),
        endpoint_prefixes: vec![],
    })
    .with_status_sender(status_tx)
    .with_shutdown_rx(shutdown_rx);

    let worker_task = tokio::spawn(async move {
        let _ = daemon.run_with_reconnect().await;
    });
    wait_for_connected(&mut status_rx).await;

    let (status, head, body) = send_raw_http(
        proxy_addr,
        "POST",
        "/v1/chat/completions",
        &[("Content-Type", "application/json")],
        Some(r#"{"model":"m1","stream":true}"#),
    )
    .await;

    assert_eq!(status, 400, "must relay real 400 status, head={head}");
    assert!(body.contains("context length exceeded"));

    let _ = shutdown_tx.send(true);
    let _ = worker_task.await;
}

/// F-09, F-11, F-30: API key auth accepts `x-api-key`, enforces strict tenant isolation between
/// API-key workers, strips client credentials before forwarding, protects `/v1/models`, and
/// hides per-worker topology on `/health`.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn tenant_isolation_x_api_key_auth_header_stripping_and_health_privacy() {
    type SeenAuthHeaders = Arc<Mutex<Vec<(String, Option<String>, Option<String>)>>>;

    let seen_auth_headers: SeenAuthHeaders = Arc::new(Mutex::new(Vec::new()));

    let spawn_backend = |tag: &'static str, log: SeenAuthHeaders| async move {
        let app = Router::new().fallback(any(move |headers: AxumHeaderMap| {
            let log = Arc::clone(&log);
            async move {
                let auth = headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .map(ToOwned::to_owned);
                let xkey = headers
                    .get("x-api-key")
                    .and_then(|v| v.to_str().ok())
                    .map(ToOwned::to_owned);
                log.lock().await.push((tag.to_string(), auth, xkey));
                Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"tenant":"{tag}"}}"#)))
                    .unwrap()
            }
        }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        addr
    };

    let backend_a = spawn_backend("tenant-a", Arc::clone(&seen_auth_headers)).await;
    let backend_b = spawn_backend("tenant-b", Arc::clone(&seen_auth_headers)).await;

    let store: Arc<dyn ApiKeyStore> = Arc::new(InMemoryApiKeyStore::new());
    let (_, secret_a) = store.create_key("tenant-a".to_string()).await.unwrap();
    let (_, secret_b) = store.create_key("tenant-b".to_string()).await.unwrap();

    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    core.lock()
        .await
        .set_default_queue_policy(ProviderQueuePolicy {
            max_queue_len: 0,
            queue_timeout_ticks: None,
        });
    let ws_app = WorkerSocketApp::new(Arc::clone(&core)).with_api_key_store(Arc::clone(&store));
    let proxy_addr = spawn_proxy(Arc::clone(&core), ws_app, |app| {
        app.with_models_provider("local")
            .with_require_api_keys(true)
            .with_api_key_store(Arc::clone(&store))
    })
    .await;

    let (tx_a, mut rx_a) = watch::channel(WorkerStatusSnapshot::default());
    let (stop_a, stop_rx_a) = watch::channel(false);
    let daemon_a = WorkerDaemon::new(WorkerDaemonConfig {
        proxy_base_url: format!("http://{proxy_addr}"),
        provider: "local".to_string(),
        worker_secret: secret_a.clone(),
        worker_name: "worker-a".to_string(),
        models: vec!["shared-model".to_string(), "only-a".to_string()],
        max_concurrent: 1,
        backend_base_url: format!("http://{backend_a}"),
        endpoint_prefixes: vec![],
    })
    .with_status_sender(tx_a)
    .with_shutdown_rx(stop_rx_a);
    let task_a = tokio::spawn(async move {
        let _ = daemon_a.run_with_reconnect().await;
    });
    wait_for_connected(&mut rx_a).await;

    let (tx_b, mut rx_b) = watch::channel(WorkerStatusSnapshot::default());
    let (stop_b, stop_rx_b) = watch::channel(false);
    let daemon_b = WorkerDaemon::new(WorkerDaemonConfig {
        proxy_base_url: format!("http://{proxy_addr}"),
        provider: "local".to_string(),
        worker_secret: secret_b.clone(),
        worker_name: "worker-b".to_string(),
        models: vec!["shared-model".to_string(), "only-b".to_string()],
        max_concurrent: 1,
        backend_base_url: format!("http://{backend_b}"),
        endpoint_prefixes: vec![],
    })
    .with_status_sender(tx_b)
    .with_shutdown_rx(stop_rx_b);
    let task_b = tokio::spawn(async move {
        let _ = daemon_b.run_with_reconnect().await;
    });
    wait_for_connected(&mut rx_b).await;

    // 1. /health must NOT leak worker topology when require_api_keys = true
    let (h_status, _, h_body) = send_raw_http(proxy_addr, "GET", "/health", &[], None).await;
    assert_eq!(h_status, 200);
    let h_json: Value = serde_json::from_str(h_body.trim()).unwrap();
    assert!(
        h_json.get("workers").is_none(),
        "workers topology leaked on /health: {h_json}"
    );

    // 2. /v1/models requires API key and scopes results to caller tenant
    let (m_unauth, _, _) = send_raw_http(proxy_addr, "GET", "/v1/models", &[], None).await;
    assert_eq!(m_unauth, 401);

    let (m_status_a, _, m_body_a) = send_raw_http(
        proxy_addr,
        "GET",
        "/v1/models",
        &[("x-api-key", &secret_a)],
        None,
    )
    .await;
    assert_eq!(m_status_a, 200);
    assert!(m_body_a.contains("only-a"));
    assert!(
        !m_body_a.contains("only-b"),
        "tenant A must not see tenant B models: {m_body_a}"
    );

    // 3. Tenant A using Anthropic `x-api-key` routes ONLY to tenant A's worker
    let (status_a, _, body_a) = send_raw_http(
        proxy_addr,
        "POST",
        "/v1/messages",
        &[
            ("Content-Type", "application/json"),
            ("x-api-key", &secret_a),
        ],
        Some(r#"{"model":"shared-model","stream":false}"#),
    )
    .await;
    assert_eq!(status_a, 200);
    assert!(body_a.contains("tenant-a"));

    // 4. Tenant A requesting tenant B's exclusive model is rejected (503), NOT routed to tenant B!
    let (cross_status, _, _) = send_raw_http(
        proxy_addr,
        "POST",
        "/v1/messages",
        &[
            ("Content-Type", "application/json"),
            ("x-api-key", &secret_a),
        ],
        Some(r#"{"model":"only-b","stream":false}"#),
    )
    .await;
    assert_eq!(
        cross_status, 503,
        "tenant A must NEVER route to tenant B's worker"
    );

    // 5. Proxy must have stripped `x-api-key` and `authorization` before forwarding to worker
    let seen = seen_auth_headers.lock().await;
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "tenant-a");
    assert_eq!(seen[0].1, None, "authorization header leaked to worker");
    assert_eq!(seen[0].2, None, "x-api-key header leaked to worker");

    let _ = stop_a.send(true);
    let _ = stop_b.send(true);
    let _ = tokio::join!(task_a, task_b);
}

/// F-12 & F-18: `stream: null` is tolerated, query strings are preserved, `/v1` is not doubled,
/// and hop-by-hop headers are stripped.
#[tokio::test]
async fn stream_null_query_string_v1_dedup_and_hop_by_hop_stripping() {
    let observed_uri = Arc::new(Mutex::new(String::new()));
    let observed_uri_clone = Arc::clone(&observed_uri);

    let backend = Router::new().fallback(any(move |OriginalUri(uri): OriginalUri| {
        let observed_uri = Arc::clone(&observed_uri_clone);
        async move {
            *observed_uri.lock().await = uri.to_string();
            // Return a streamed body so hyper legitimately emits `transfer-encoding: chunked`
            // on the wire, plus a custom Connection-nominated hop-by-hop header.
            let body_stream = futures_util::stream::iter(vec![
                Ok::<_, std::convert::Infallible>(axum::body::Bytes::from_static(
                    br#"{"handled":"#,
                )),
                Ok::<_, std::convert::Infallible>(axum::body::Bytes::from_static(b"true}")),
            ]);
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "application/json")
                .header("connection", "x-hop-secret")
                .header("x-hop-secret", "must-be-stripped")
                .body(Body::from_stream(body_stream))
                .unwrap()
        }
    }));

    let backend_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_addr = backend_listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(backend_listener, backend).await.unwrap() });

    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let ws_app = WorkerSocketApp::new(Arc::clone(&core))
        .with_provider("local", WorkerSocketProviderConfig::enabled("secret"));
    let proxy_addr = spawn_proxy(Arc::clone(&core), ws_app, |app| {
        app.with_models_provider("local")
    })
    .await;

    let (tx, mut rx) = watch::channel(WorkerStatusSnapshot::default());
    let (stop_tx, stop_rx) = watch::channel(false);
    let daemon = WorkerDaemon::new(WorkerDaemonConfig {
        proxy_base_url: format!("http://{proxy_addr}"),
        provider: "local".to_string(),
        worker_secret: "secret".to_string(),
        worker_name: "v1-worker".to_string(),
        models: vec!["m1".to_string()],
        max_concurrent: 1,
        // Intentionally ends with `/v1` to verify deduplication!
        backend_base_url: format!("http://{backend_addr}/v1"),
        endpoint_prefixes: vec![],
    })
    .with_status_sender(tx)
    .with_shutdown_rx(stop_rx);
    let task = tokio::spawn(async move {
        let _ = daemon.run_with_reconnect().await;
    });
    wait_for_connected(&mut rx).await;

    let (status, head, body) = send_raw_http(
        proxy_addr,
        "POST",
        "/v1/chat/completions?api-version=2024-10-21",
        &[("Content-Type", "application/json")],
        Some(r#"{"model":"m1","stream":null,"provider":"local"}"#),
    )
    .await;

    assert_eq!(
        status, 200,
        "stream:null + string provider must succeed; body={body}"
    );
    assert!(body.contains(r#""handled":true"#));
    assert!(
        !head
            .to_ascii_lowercase()
            .contains("transfer-encoding: chunked"),
        "hop-by-hop transfer-encoding header must be stripped: {head}"
    );
    assert!(
        !head.to_ascii_lowercase().contains("x-hop-secret"),
        "Connection-nominated hop-by-hop header must be stripped: {head}"
    );
    assert_eq!(
        *observed_uri.lock().await,
        "/v1/chat/completions?api-version=2024-10-21"
    );

    let _ = stop_tx.send(true);
    let _ = task.await;
}

/// F-17: In-flight request timeout emits JSON 504 Gateway Timeout and sends Cancel(Timeout).
#[tokio::test]
async fn request_timeout_emits_504_json_and_cancels_worker() {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let ws_app = WorkerSocketApp::new(Arc::clone(&core))
        .with_provider("local", WorkerSocketProviderConfig::enabled("secret"));
    let proxy_addr = spawn_proxy(Arc::clone(&core), ws_app, |app| {
        app.with_models_provider("local")
            .with_request_timeout(Some(Duration::from_millis(150)))
    })
    .await;

    let mut req = format!("ws://{proxy_addr}/v1/worker/connect?provider=local")
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("x-worker-secret", "secret".parse().unwrap());
    let (mut ws, _) = connect_async(req).await.unwrap();
    let reg = serde_json::to_string(&WorkerToServerMessage::Register(RegisterMessage {
        worker_name: "slow-worker".to_string(),
        models: vec!["m1".to_string()],
        max_concurrent: 1,
        protocol_version: Some("2026-04-bridge-v1".to_string()),
        current_load: Some(0),
        endpoint_prefixes: vec![],
        model_metadata: modelrelay_protocol::ModelMetadataMap::new(),
    }))
    .unwrap();
    ws.send(Message::Text(reg.into())).await.unwrap();
    let _ = ws.next().await.unwrap().unwrap(); // RegisterAck

    let http_fut = tokio::spawn(async move {
        send_raw_http(
            proxy_addr,
            "POST",
            "/v1/chat/completions",
            &[("Content-Type", "application/json")],
            Some(r#"{"model":"m1","stream":false}"#),
        )
        .await
    });

    // Receive Request, do NOT reply
    let _ = ws.next().await.unwrap().unwrap();

    let (status, _, body) = http_fut.await.unwrap();
    assert_eq!(status, 504);
    assert!(body.contains("timeout"), "got: {body}");

    // Worker must receive Cancel { reason: Timeout }
    let cancel_msg = timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("worker should receive Cancel")
        .unwrap()
        .unwrap();
    let Message::Text(cancel_text) = cancel_msg else {
        panic!("expected text Cancel");
    };
    let parsed: ServerToWorkerMessage = serde_json::from_str(&cancel_text).unwrap();
    assert!(
        matches!(
            parsed,
            ServerToWorkerMessage::Cancel(ref c)
                if c.reason == modelrelay_protocol::CancelReason::Timeout
        ),
        "expected Cancel(Timeout), got: {parsed:?}"
    );
}

/// F-19: Queued dispatch enforces `ProviderRouting::only` and `ignore`, and `allow_fallbacks`
/// never dispatches to an explicitly `ignore`d provider.
#[test]
fn queued_dispatch_enforces_provider_routing_only_and_ignore() {
    let mut core = ProxyServerCore::new();
    let w_bad = core
        .register_worker(
            "bad-provider",
            RegisterMessage {
                worker_name: "w-bad".to_string(),
                models: vec!["m1".to_string()],
                max_concurrent: 1,
                protocol_version: Some("2026-04-bridge-v1".to_string()),
                current_load: Some(0),
                endpoint_prefixes: vec![],
                model_metadata: modelrelay_protocol::ModelMetadataMap::new(),
            },
        )
        .worker_id;

    // Request ignores bad-provider even with allow_fallbacks=true -> MUST NOT dispatch to w_bad!
    let outcome = core.submit_transport_request(
        "good-provider",
        "m1",
        "/v1/chat/completions",
        "POST",
        false,
        String::new(),
        modelrelay_protocol::HeaderMap::new(),
        Some(ProviderRouting {
            ignore: Some(vec!["bad-provider".to_string()]),
            allow_fallbacks: Some(true),
            ..Default::default()
        }),
    );
    assert!(
        matches!(outcome, modelrelay_server::SubmissionOutcome::Queued(_)),
        "must queue instead of dispatching to ignored provider: {outcome:?}"
    );

    // Even if bad-provider finishes unrelated work or updates models, it must NEVER pick up request-1!
    let dispatched = core.update_worker_models(
        &w_bad,
        modelrelay_protocol::ModelsUpdateMessage {
            models: vec!["m1".to_string()],
            current_load: 0,
            model_metadata: modelrelay_protocol::ModelMetadataMap::new(),
        },
    );
    assert!(
        dispatched.is_empty(),
        "ignored provider must never drain queued request"
    );

    // When a good-provider worker registers, it immediately drains request-1!
    let w_good = core
        .register_worker(
            "good-provider",
            RegisterMessage {
                worker_name: "w-good".to_string(),
                models: vec!["m1".to_string()],
                max_concurrent: 1,
                protocol_version: Some("2026-04-bridge-v1".to_string()),
                current_load: Some(0),
                endpoint_prefixes: vec![],
                model_metadata: modelrelay_protocol::ModelMetadataMap::new(),
            },
        )
        .worker_id;
    assert_eq!(
        core.worker_in_flight_request_ids(&w_good),
        vec!["request-1"]
    );
}

/// F-08 & F-16: Worker daemon reports `AuthFailed` over its status watch channel when the
/// proxy closes the connection with code 1008.
#[tokio::test]
async fn worker_status_watch_reports_auth_failed_on_close_1008() {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let ws_app = WorkerSocketApp::new(Arc::clone(&core))
        .with_provider("local", WorkerSocketProviderConfig::enabled("right-secret"));
    let proxy_addr = spawn_proxy(Arc::clone(&core), ws_app, |app| {
        app.with_models_provider("local")
    })
    .await;

    let (status_tx, mut status_rx) = watch::channel(WorkerStatusSnapshot::default());
    let (stop_tx, stop_rx) = watch::channel(false);
    let daemon = WorkerDaemon::new(WorkerDaemonConfig {
        proxy_base_url: format!("http://{proxy_addr}"),
        provider: "local".to_string(),
        worker_secret: "wrong-secret".to_string(),
        worker_name: "bad-auth".to_string(),
        models: vec!["m1".to_string()],
        max_concurrent: 1,
        backend_base_url: "http://127.0.0.1:1".to_string(),
        endpoint_prefixes: vec![],
    })
    .with_status_sender(status_tx)
    .with_shutdown_rx(stop_rx);

    let task = tokio::spawn(async move {
        let _ = daemon.run_with_reconnect().await;
    });

    timeout(Duration::from_secs(5), async {
        loop {
            if status_rx.borrow().state == WorkerConnectionState::AuthFailed {
                return;
            }
            status_rx.changed().await.unwrap();
        }
    })
    .await
    .expect("worker must surface AuthFailed on 1008 close");

    let snap = status_rx.borrow().clone();
    assert_eq!(snap.state, WorkerConnectionState::AuthFailed);
    assert!(
        snap.last_error
            .as_deref()
            .is_some_and(|e| e.contains("authentication failed")),
        "expected auth failure reason, got: {:?}",
        snap.last_error
    );

    let _ = stop_tx.send(true);
    let _ = task.await;
}
