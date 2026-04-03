use std::{fmt::Write as _, net::SocketAddr, sync::Arc};

use futures_util::{SinkExt, StreamExt};
use proxy_server::{ProxyHttpApp, ProxyServerCore, WorkerSocketApp, WorkerSocketProviderConfig};
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
    HeaderMap, RegisterMessage, ResponseChunkMessage, ResponseCompleteMessage,
    ServerToWorkerMessage, WorkerToServerMessage,
};

type TestSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn spawn_server() -> SocketAddr {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let worker_socket_app = WorkerSocketApp::new(core.clone()).with_provider(
        "anthropic",
        WorkerSocketProviderConfig::enabled("top-secret"),
    );
    let app = ProxyHttpApp::new(core)
        .with_models_provider("anthropic")
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
