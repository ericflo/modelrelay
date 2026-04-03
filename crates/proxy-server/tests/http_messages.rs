use std::{fmt::Write as _, net::SocketAddr, sync::Arc};

use futures_util::{SinkExt, StreamExt};
use proxy_server::{
    ProviderQueuePolicy, ProxyHttpApp, ProxyServerCore, RequestState, WorkerSocketApp,
    WorkerSocketProviderConfig,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Mutex,
    time::timeout,
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use worker_protocol::{
    CancelMessage, CancelReason, HeaderMap, ModelsUpdateMessage, RegisterMessage,
    ResponseChunkMessage, ResponseCompleteMessage, ServerToWorkerMessage, WorkerToServerMessage,
};

type TestSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn spawn_server() -> SocketAddr {
    spawn_server_with_core(Arc::new(Mutex::new(ProxyServerCore::new())), true).await
}

async fn spawn_server_with_core(
    core: Arc<Mutex<ProxyServerCore>>,
    provider_enabled: bool,
) -> SocketAddr {
    let worker_socket_app = WorkerSocketApp::new(core.clone()).with_provider(
        "anthropic",
        WorkerSocketProviderConfig::enabled("top-secret"),
    );
    let app = ProxyHttpApp::new(core)
        .with_models_provider("anthropic")
        .with_provider_enabled(provider_enabled)
        .with_worker_socket_app(worker_socket_app)
        .router();

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("listener local addr");

    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve proxy http app");
    });

    addr
}

async fn wait_for_request_state(
    core: &Arc<Mutex<ProxyServerCore>>,
    request_id: &str,
    expected: RequestState,
) {
    timeout(std::time::Duration::from_secs(2), async {
        loop {
            {
                let core = core.lock().await;
                if core.request_state(request_id) == Some(expected.clone()) {
                    return;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("request {request_id} did not reach expected state"));
}

fn assert_service_unavailable(response: &str, message: &str) {
    assert!(response.starts_with("HTTP/1.1 503 Service Unavailable\r\n"));
    assert!(response.contains("\r\ncontent-type: text/plain; charset=utf-8\r\n"));
    assert!(response.contains("\r\nx-content-type-options: nosniff\r\n"));
    assert!(response.ends_with(&format!("{message}\n")));
}

fn worker_connect_request(addr: SocketAddr, secret: &str) -> http::Request<()> {
    let mut request = format!("ws://{addr}/v1/worker/connect?provider=anthropic")
        .into_client_request()
        .expect("build websocket request");
    request.headers_mut().insert(
        "x-worker-secret",
        secret.parse().expect("parse worker secret header"),
    );
    request
}

async fn next_server_message(socket: &mut TestSocket, context: &str) -> ServerToWorkerMessage {
    let message = timeout(std::time::Duration::from_secs(2), socket.next())
        .await
        .unwrap_or_else(|_| panic!("receive {context} before timeout"))
        .expect("socket message")
        .expect("websocket message");
    let Message::Text(payload) = message else {
        panic!("expected text {context}");
    };

    serde_json::from_str(&payload).unwrap_or_else(|_| panic!("deserialize {context}"))
}

async fn register_test_worker(socket: &mut TestSocket) {
    let register = WorkerToServerMessage::Register(RegisterMessage {
        worker_name: "gpu-box-a".to_string(),
        models: vec!["claude-3-5-sonnet-20241022".to_string()],
        max_concurrent: 1,
        protocol_version: Some("2026-04-bridge-v1".to_string()),
        current_load: Some(0),
    });
    let register_payload = serde_json::to_string(&register).expect("serialize register");

    socket
        .send(Message::Text(register_payload.into()))
        .await
        .expect("send register");

    let ServerToWorkerMessage::RegisterAck(_) = next_server_message(socket, "register_ack").await
    else {
        panic!("expected register_ack message");
    };
}

async fn post_messages(addr: SocketAddr, body: &str, headers: &[(&str, &str)]) -> String {
    let mut stream = TcpStream::connect(addr)
        .await
        .expect("connect to test server");

    let mut request = format!(
        "POST /v1/messages HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        write!(request, "{name}: {value}\r\n").expect("append http request header");
    }
    request.push_str("\r\n");
    request.push_str(body);

    stream
        .write_all(request.as_bytes())
        .await
        .expect("write http request");

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("read http response");

    String::from_utf8(response).expect("http response is utf8")
}

async fn open_messages_request(
    addr: SocketAddr,
    body: &str,
    headers: &[(&str, &str)],
) -> TcpStream {
    let mut stream = TcpStream::connect(addr)
        .await
        .expect("connect to test server");

    let mut request = format!(
        "POST /v1/messages HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        write!(request, "{name}: {value}\r\n").expect("append http request header");
    }
    request.push_str("\r\n");
    request.push_str(body);

    stream
        .write_all(request.as_bytes())
        .await
        .expect("write http request");

    stream
}

async fn read_until_contains(stream: &mut TcpStream, needle: &str) -> String {
    let mut response = Vec::new();

    loop {
        if String::from_utf8_lossy(&response).contains(needle) {
            return String::from_utf8(response).expect("http response is utf8");
        }

        let mut chunk = [0_u8; 1024];
        let read = timeout(std::time::Duration::from_secs(2), stream.read(&mut chunk))
            .await
            .expect("read response chunk before timeout")
            .expect("read response chunk");
        assert!(read > 0, "response closed before expected bytes arrived");
        response.extend_from_slice(&chunk[..read]);
    }
}

async fn send_response_chunk(socket: &mut TestSocket, request_id: &str, chunk: &str) {
    let message = WorkerToServerMessage::ResponseChunk(ResponseChunkMessage {
        request_id: request_id.to_string(),
        chunk: chunk.to_string(),
    });
    let payload = serde_json::to_string(&message).expect("serialize response_chunk");

    socket
        .send(Message::Text(payload.into()))
        .await
        .expect("send response_chunk");
}

async fn send_models_update(socket: &mut TestSocket, models: Vec<String>, current_load: u32) {
    let message = WorkerToServerMessage::ModelsUpdate(ModelsUpdateMessage {
        models,
        current_load,
    });
    let payload = serde_json::to_string(&message).expect("serialize models_update");

    socket
        .send(Message::Text(payload.into()))
        .await
        .expect("send models_update");
}

#[tokio::test]
async fn worker_backed_messages_route_forwards_anthropic_request_and_preserves_response() {
    let addr = spawn_server().await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");
    register_test_worker(&mut socket).await;

    let body = r#"{"model":"claude-3-5-sonnet-20241022","max_tokens":64,"messages":[{"role":"user","content":"hello from messages"}]}"#;
    let http_request = tokio::spawn(post_messages(
        addr,
        body,
        &[
            ("x-api-key", "test-anthropic-key"),
            ("anthropic-version", "2023-06-01"),
            ("anthropic-beta", "tools-2024-04-04"),
        ],
    ));

    let ServerToWorkerMessage::Request(request) =
        next_server_message(&mut socket, "worker request").await
    else {
        panic!("expected worker request message");
    };

    assert_eq!(request.model, "claude-3-5-sonnet-20241022");
    assert_eq!(request.endpoint_path, "/v1/messages");
    assert!(!request.is_streaming);
    assert_eq!(request.body, body);
    assert_eq!(
        request.headers,
        HeaderMap::from([
            ("anthropic-beta".to_string(), "tools-2024-04-04".to_string()),
            ("anthropic-version".to_string(), "2023-06-01".to_string()),
            ("content-type".to_string(), "application/json".to_string()),
            ("x-api-key".to_string(), "test-anthropic-key".to_string()),
        ])
    );

    let complete = WorkerToServerMessage::ResponseComplete(ResponseCompleteMessage {
        request_id: request.request_id,
        status_code: 200,
        headers: HeaderMap::from([
            ("content-type".to_string(), "application/json".to_string()),
            ("anthropic-beta".to_string(), "tools-2024-04-04".to_string()),
            ("x-worker-backend".to_string(), "gpu-box-a".to_string()),
        ]),
        body: Some(
            r#"{"id":"msg_1","type":"message","role":"assistant","content":[{"type":"text","text":"hello"}]}"#
                .to_string(),
        ),
        token_counts: None,
    });
    let complete_payload = serde_json::to_string(&complete).expect("serialize response_complete");

    socket
        .send(Message::Text(complete_payload.into()))
        .await
        .expect("send response_complete");

    let response = http_request.await.expect("join http request task");
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.contains("\r\ncontent-type: application/json\r\n"));
    assert!(response.contains("\r\nanthropic-beta: tools-2024-04-04\r\n"));
    assert!(response.contains("\r\nx-worker-backend: gpu-box-a\r\n"));
    assert!(response.ends_with(
        r#"{"id":"msg_1","type":"message","role":"assistant","content":[{"type":"text","text":"hello"}]}"#
    ));
}

#[tokio::test]
async fn worker_backed_messages_route_preserves_upstream_http_error() {
    let addr = spawn_server().await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");
    register_test_worker(&mut socket).await;

    let body = r#"{"model":"claude-3-5-sonnet-20241022","max_tokens":64,"messages":[{"role":"user","content":"bad request"}]}"#;
    let http_request = tokio::spawn(post_messages(
        addr,
        body,
        &[
            ("x-api-key", "test-anthropic-key"),
            ("anthropic-version", "2023-06-01"),
        ],
    ));

    let ServerToWorkerMessage::Request(request) =
        next_server_message(&mut socket, "worker request").await
    else {
        panic!("expected worker request message");
    };

    let error_body = r#"{"type":"error","error":{"type":"invalid_request_error","message":"upstream rejected the payload"}}"#;
    let complete = WorkerToServerMessage::ResponseComplete(ResponseCompleteMessage {
        request_id: request.request_id,
        status_code: 429,
        headers: HeaderMap::from([
            ("content-type".to_string(), "application/json".to_string()),
            ("retry-after".to_string(), "7".to_string()),
            (
                "x-upstream-request-id".to_string(),
                "req-upstream-123".to_string(),
            ),
        ]),
        body: Some(error_body.to_string()),
        token_counts: None,
    });
    let complete_payload = serde_json::to_string(&complete).expect("serialize response_complete");

    socket
        .send(Message::Text(complete_payload.into()))
        .await
        .expect("send response_complete");

    let response = http_request.await.expect("join http request task");
    assert!(response.starts_with("HTTP/1.1 429 Too Many Requests\r\n"));
    assert!(response.contains("\r\ncontent-type: application/json\r\n"));
    assert!(response.contains("\r\nretry-after: 7\r\n"));
    assert!(response.contains("\r\nx-upstream-request-id: req-upstream-123\r\n"));
    assert!(response.ends_with(error_body));
}

#[tokio::test]
async fn worker_backed_messages_route_streams_live_anthropic_sse_events() {
    let addr = spawn_server().await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");
    register_test_worker(&mut socket).await;

    let body = r#"{"model":"claude-3-5-sonnet-20241022","stream":true,"max_tokens":64,"messages":[{"role":"user","content":"hello from messages"}]}"#;
    let mut http_stream = open_messages_request(
        addr,
        body,
        &[
            ("x-api-key", "test-anthropic-key"),
            ("anthropic-version", "2023-06-01"),
        ],
    )
    .await;

    let ServerToWorkerMessage::Request(request) =
        next_server_message(&mut socket, "streaming worker request").await
    else {
        panic!("expected worker request message");
    };

    assert_eq!(request.endpoint_path, "/v1/messages");
    assert!(request.is_streaming);
    assert_eq!(request.body, body);

    send_response_chunk(
        &mut socket,
        &request.request_id,
        "event: message_start\ndata: {\"type\":\"message_start\"}\n\n",
    )
    .await;

    let first_fragment = read_until_contains(
        &mut http_stream,
        "event: message_start\ndata: {\"type\":\"message_start\"}\n\n",
    )
    .await;
    assert!(first_fragment.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(first_fragment.contains("\r\ncontent-type: text/event-stream\r\n"));
    assert!(
        first_fragment.contains("event: message_start\ndata: {\"type\":\"message_start\"}\n\n")
    );
    assert!(!first_fragment.contains("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"));

    send_response_chunk(
        &mut socket,
        &request.request_id,
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
    )
    .await;
    send_response_chunk(
        &mut socket,
        &request.request_id,
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    )
    .await;

    let complete = WorkerToServerMessage::ResponseComplete(ResponseCompleteMessage {
        request_id: request.request_id,
        status_code: 200,
        headers: HeaderMap::from([("content-type".to_string(), "text/event-stream".to_string())]),
        body: Some(String::new()),
        token_counts: None,
    });
    let complete_payload = serde_json::to_string(&complete).expect("serialize response_complete");

    socket
        .send(Message::Text(complete_payload.into()))
        .await
        .expect("send response_complete");

    let mut rest = Vec::new();
    http_stream
        .read_to_end(&mut rest)
        .await
        .expect("read streaming http response");
    let full_response = first_fragment + &String::from_utf8(rest).expect("http response is utf8");

    assert!(full_response.contains(
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n"
    ));
    assert!(full_response.contains("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"));
    assert!(full_response.ends_with("0\r\n\r\n"));
}

#[tokio::test]
async fn worker_backed_messages_route_cancels_in_flight_request_when_http_client_disconnects() {
    let addr = spawn_server().await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");
    register_test_worker(&mut socket).await;

    let body = r#"{"model":"claude-3-5-sonnet-20241022","max_tokens":64,"messages":[{"role":"user","content":"cancel me"}]}"#;
    let mut http_stream = open_messages_request(
        addr,
        body,
        &[
            ("x-api-key", "test-anthropic-key"),
            ("anthropic-version", "2023-06-01"),
        ],
    )
    .await;

    let ServerToWorkerMessage::Request(request) =
        next_server_message(&mut socket, "worker request").await
    else {
        panic!("expected worker request message");
    };

    http_stream
        .shutdown()
        .await
        .expect("shutdown disconnected http client");
    drop(http_stream);

    assert_eq!(
        next_server_message(&mut socket, "worker cancel").await,
        ServerToWorkerMessage::Cancel(CancelMessage {
            request_id: request.request_id,
            reason: CancelReason::ClientDisconnect,
        })
    );
}

#[tokio::test]
async fn worker_backed_messages_route_returns_sanitized_no_workers_error() {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    {
        let mut core = core.lock().await;
        core.configure_provider_queue(
            "anthropic",
            ProviderQueuePolicy {
                max_queue_len: 0,
                queue_timeout_ticks: None,
            },
        );
    }
    let addr = spawn_server_with_core(core, true).await;

    let body = r#"{"model":"claude-3-5-sonnet-20241022","max_tokens":64,"messages":[{"role":"user","content":"hello from messages"}]}"#;
    let response = post_messages(
        addr,
        body,
        &[
            ("x-api-key", "test-anthropic-key"),
            ("anthropic-version", "2023-06-01"),
        ],
    )
    .await;

    assert_service_unavailable(&response, "No workers available to handle request");
    assert!(
        !response.contains("queue is full"),
        "the client boundary should not expose the raw queue rejection"
    );
}

#[tokio::test]
async fn worker_backed_messages_route_returns_sanitized_queue_timeout_error() {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    {
        let mut core = core.lock().await;
        core.configure_provider_queue(
            "anthropic",
            ProviderQueuePolicy {
                max_queue_len: 1,
                queue_timeout_ticks: Some(0),
            },
        );
    }
    let addr = spawn_server_with_core(core.clone(), true).await;

    let body = r#"{"model":"claude-3-5-sonnet-20241022","max_tokens":64,"messages":[{"role":"user","content":"timeout me"}]}"#;
    let http_request = tokio::spawn(post_messages(
        addr,
        body,
        &[
            ("x-api-key", "test-anthropic-key"),
            ("anthropic-version", "2023-06-01"),
        ],
    ));
    wait_for_request_state(&core, "request-1", RequestState::Queued).await;

    {
        let mut core = core.lock().await;
        let failures = core.expire_queue_timeouts(std::time::Instant::now());
        assert_eq!(failures.len(), 1);
    }

    let response = http_request.await.expect("join timed-out http request");
    assert_service_unavailable(&response, "Request timed out waiting for worker");
}

#[tokio::test]
async fn worker_backed_messages_route_returns_sanitized_queue_full_error() {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    {
        let mut core = core.lock().await;
        core.configure_provider_queue(
            "anthropic",
            ProviderQueuePolicy {
                max_queue_len: 1,
                queue_timeout_ticks: None,
            },
        );
    }
    let addr = spawn_server_with_core(core.clone(), true).await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");
    register_test_worker(&mut socket).await;

    let body = r#"{"model":"claude-3-5-sonnet-20241022","max_tokens":64,"messages":[{"role":"user","content":"hello from messages"}]}"#;
    let first_request = tokio::spawn(open_messages_request(
        addr,
        body,
        &[
            ("x-api-key", "test-anthropic-key"),
            ("anthropic-version", "2023-06-01"),
        ],
    ));
    let ServerToWorkerMessage::Request(_) =
        next_server_message(&mut socket, "first worker request").await
    else {
        panic!("expected first worker request message");
    };

    let second_request = tokio::spawn(open_messages_request(
        addr,
        body,
        &[
            ("x-api-key", "test-anthropic-key"),
            ("anthropic-version", "2023-06-01"),
        ],
    ));
    wait_for_request_state(&core, "request-2", RequestState::Queued).await;

    let response = post_messages(
        addr,
        body,
        &[
            ("x-api-key", "test-anthropic-key"),
            ("anthropic-version", "2023-06-01"),
        ],
    )
    .await;
    assert_service_unavailable(&response, "Service temporarily at capacity, please retry");
    assert!(
        !response.contains("queue is full"),
        "the client boundary should not expose the raw queue-full reason"
    );

    first_request.abort();
    second_request.abort();
}

#[tokio::test]
async fn worker_backed_messages_route_returns_sanitized_provider_disabled_error() {
    let addr = spawn_server_with_core(Arc::new(Mutex::new(ProxyServerCore::new())), false).await;

    let body = r#"{"model":"claude-3-5-sonnet-20241022","max_tokens":64,"messages":[{"role":"user","content":"hello from messages"}]}"#;
    let response = post_messages(
        addr,
        body,
        &[
            ("x-api-key", "test-anthropic-key"),
            ("anthropic-version", "2023-06-01"),
        ],
    )
    .await;

    assert_service_unavailable(&response, "Provider is currently disabled");
    assert!(
        !response.contains("virtual provider is disabled"),
        "the compatibility boundary should use the stable disabled message"
    );
}

#[tokio::test]
async fn worker_backed_messages_route_returns_sanitized_requeue_exhaustion_error() {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let addr = spawn_server_with_core(core.clone(), true).await;

    let (mut socket_one, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect first websocket");
    register_test_worker(&mut socket_one).await;

    let body = r#"{"model":"claude-3-5-sonnet-20241022","max_tokens":64,"messages":[{"role":"user","content":"keep retrying"}]}"#;
    let http_request = tokio::spawn(post_messages(
        addr,
        body,
        &[
            ("x-api-key", "test-anthropic-key"),
            ("anthropic-version", "2023-06-01"),
        ],
    ));

    let ServerToWorkerMessage::Request(first_request) =
        next_server_message(&mut socket_one, "first worker request").await
    else {
        panic!("expected first worker request message");
    };

    socket_one
        .close(None)
        .await
        .expect("close first worker socket");
    wait_for_request_state(&core, &first_request.request_id, RequestState::Queued).await;

    for label in ["second", "third", "fourth"] {
        let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
            .await
            .unwrap_or_else(|_| panic!("connect {label} websocket"));
        register_test_worker(&mut socket).await;
        send_models_update(
            &mut socket,
            vec!["claude-3-5-sonnet-20241022".to_string()],
            0,
        )
        .await;

        let ServerToWorkerMessage::Request(requeued_request) =
            next_server_message(&mut socket, &format!("{label} worker request")).await
        else {
            panic!("expected {label} worker request message");
        };
        assert_eq!(requeued_request.request_id, first_request.request_id);

        socket
            .close(None)
            .await
            .unwrap_or_else(|_| panic!("close {label} worker socket"));

        if label != "fourth" {
            wait_for_request_state(&core, &first_request.request_id, RequestState::Queued).await;
        }
    }

    let response = http_request
        .await
        .expect("join requeue exhaustion http request");
    assert_service_unavailable(
        &response,
        "Request could not be processed after multiple attempts",
    );
    assert!(
        !response.contains("exceeded maximum requeue"),
        "the client boundary should not expose the raw requeue exhaustion reason"
    );
}
