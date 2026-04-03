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
    CancelMessage, CancelReason, HeaderMap, RegisterMessage, ResponseChunkMessage,
    ResponseCompleteMessage, ServerToWorkerMessage, WorkerToServerMessage,
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
    let worker_socket_app = WorkerSocketApp::new(core.clone())
        .with_provider("openai", WorkerSocketProviderConfig::enabled("top-secret"));
    let app = ProxyHttpApp::new(core)
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
    let mut request = format!("ws://{addr}/v1/worker/connect?provider=openai")
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
        models: vec!["gpt-4.1-mini".to_string()],
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

async fn post_responses(addr: SocketAddr, body: &str, headers: &[(&str, &str)]) -> String {
    let mut stream = TcpStream::connect(addr)
        .await
        .expect("connect to test server");

    let mut request = format!(
        "POST /v1/responses HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
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

async fn open_responses_request(
    addr: SocketAddr,
    body: &str,
    headers: &[(&str, &str)],
) -> TcpStream {
    let mut stream = TcpStream::connect(addr)
        .await
        .expect("connect to test server");

    let mut request = format!(
        "POST /v1/responses HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
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

#[tokio::test]
async fn worker_backed_responses_route_forwards_request_and_preserves_response() {
    let addr = spawn_server().await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");
    register_test_worker(&mut socket).await;

    let body = r#"{"model":"gpt-4.1-mini","input":"hello from responses"}"#;
    let http_request = tokio::spawn(post_responses(
        addr,
        body,
        &[
            ("Authorization", "Bearer test-token"),
            ("OpenAI-Beta", "responses=v1"),
            ("X-Trace-Id", "trace-456"),
        ],
    ));

    let ServerToWorkerMessage::Request(request) =
        next_server_message(&mut socket, "worker request").await
    else {
        panic!("expected worker request message");
    };

    assert_eq!(request.model, "gpt-4.1-mini");
    assert_eq!(request.endpoint_path, "/v1/responses");
    assert!(!request.is_streaming);
    assert_eq!(request.body, body);
    assert_eq!(
        request.headers,
        HeaderMap::from([
            ("authorization".to_string(), "Bearer test-token".to_string()),
            ("content-type".to_string(), "application/json".to_string()),
            ("openai-beta".to_string(), "responses=v1".to_string()),
            ("x-trace-id".to_string(), "trace-456".to_string()),
        ])
    );

    let complete = WorkerToServerMessage::ResponseComplete(ResponseCompleteMessage {
        request_id: request.request_id,
        status_code: 201,
        headers: HeaderMap::from([
            ("content-type".to_string(), "application/json".to_string()),
            ("openai-beta".to_string(), "responses=v1".to_string()),
            ("x-worker-backend".to_string(), "gpu-box-a".to_string()),
        ]),
        body: Some(r#"{"id":"resp_1","object":"response","output":[]}"#.to_string()),
        token_counts: None,
    });
    let complete_payload = serde_json::to_string(&complete).expect("serialize response_complete");

    socket
        .send(Message::Text(complete_payload.into()))
        .await
        .expect("send response_complete");

    let response = http_request.await.expect("join http request task");
    assert!(response.starts_with("HTTP/1.1 201 Created\r\n"));
    assert!(response.contains("\r\ncontent-type: application/json\r\n"));
    assert!(response.contains("\r\nopenai-beta: responses=v1\r\n"));
    assert!(response.contains("\r\nx-worker-backend: gpu-box-a\r\n"));
    assert!(response.ends_with(r#"{"id":"resp_1","object":"response","output":[]}"#));
}

#[tokio::test]
async fn worker_backed_responses_route_streams_live_sse_chunks() {
    let addr = spawn_server().await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");
    register_test_worker(&mut socket).await;

    let body = r#"{"model":"gpt-4.1-mini","stream":true,"input":"hello from responses"}"#;
    let mut http_stream =
        open_responses_request(addr, body, &[("OpenAI-Beta", "responses=v1")]).await;

    let ServerToWorkerMessage::Request(request) =
        next_server_message(&mut socket, "streaming worker request").await
    else {
        panic!("expected worker request message");
    };

    assert_eq!(request.endpoint_path, "/v1/responses");
    assert!(request.is_streaming);
    assert_eq!(request.body, body);

    send_response_chunk(
        &mut socket,
        &request.request_id,
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hel\"}\n\n",
    )
    .await;

    let first_fragment = read_until_contains(
        &mut http_stream,
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hel\"}\n\n",
    )
    .await;
    assert!(first_fragment.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(first_fragment.contains("\r\ncontent-type: text/event-stream\r\n"));
    assert!(
        first_fragment
            .contains("data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hel\"}\n\n")
    );
    assert!(!first_fragment.contains("data: [DONE]\n\n"));

    send_response_chunk(
        &mut socket,
        &request.request_id,
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"lo\"}\n\n",
    )
    .await;
    send_response_chunk(&mut socket, &request.request_id, "data: [DONE]\n\n").await;

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

    assert!(
        full_response
            .contains("data: {\"type\":\"response.output_text.delta\",\"delta\":\"lo\"}\n\n")
    );
    assert!(full_response.contains("data: [DONE]\n\n"));
    assert!(full_response.ends_with("0\r\n\r\n"));
}

#[tokio::test]
async fn worker_backed_responses_route_cancels_in_flight_request_when_http_client_disconnects() {
    let addr = spawn_server().await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");
    register_test_worker(&mut socket).await;

    let body = r#"{"model":"gpt-4.1-mini","stream":true,"input":"cancel me"}"#;
    let mut http_stream = open_responses_request(
        addr,
        body,
        &[
            ("Authorization", "Bearer test-token"),
            ("OpenAI-Beta", "responses=v1"),
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
async fn worker_backed_responses_route_returns_sanitized_queue_timeout_error() {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    {
        let mut core = core.lock().await;
        core.configure_provider_queue(
            "openai",
            ProviderQueuePolicy {
                max_queue_len: 1,
                queue_timeout_ticks: Some(0),
            },
        );
    }
    let addr = spawn_server_with_core(core.clone(), true).await;

    let body = r#"{"model":"gpt-4.1-mini","input":"timeout me"}"#;
    let http_request = tokio::spawn(post_responses(
        addr,
        body,
        &[("OpenAI-Beta", "responses=v1")],
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
async fn worker_backed_responses_route_returns_sanitized_queue_full_error() {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    {
        let mut core = core.lock().await;
        core.configure_provider_queue(
            "openai",
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

    let body = r#"{"model":"gpt-4.1-mini","input":"hello from responses"}"#;
    let first_request = tokio::spawn(open_responses_request(
        addr,
        body,
        &[("OpenAI-Beta", "responses=v1")],
    ));
    let ServerToWorkerMessage::Request(_) =
        next_server_message(&mut socket, "first worker request").await
    else {
        panic!("expected first worker request message");
    };

    let second_request = tokio::spawn(open_responses_request(
        addr,
        body,
        &[("OpenAI-Beta", "responses=v1")],
    ));
    wait_for_request_state(&core, "request-2", RequestState::Queued).await;

    let response = post_responses(addr, body, &[("OpenAI-Beta", "responses=v1")]).await;
    assert_service_unavailable(&response, "Service temporarily at capacity, please retry");
    assert!(
        !response.contains("queue is full"),
        "the client boundary should not expose the raw queue-full reason"
    );

    first_request.abort();
    second_request.abort();
}

#[tokio::test]
async fn worker_backed_responses_route_returns_sanitized_provider_disabled_error() {
    let addr = spawn_server_with_core(Arc::new(Mutex::new(ProxyServerCore::new())), false).await;

    let body = r#"{"model":"gpt-4.1-mini","input":"hello from responses"}"#;
    let response = post_responses(addr, body, &[("OpenAI-Beta", "responses=v1")]).await;

    assert_service_unavailable(&response, "Provider is currently disabled");
    assert!(
        !response.contains("virtual provider is disabled"),
        "the compatibility boundary should use the stable disabled message"
    );
}
