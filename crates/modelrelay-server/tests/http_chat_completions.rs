mod support;
use support::*;

use std::{fmt::Write as _, net::SocketAddr, sync::Arc};

use futures_util::SinkExt;
use modelrelay_protocol::{
    CancelMessage, CancelReason, HeaderMap, ModelsUpdateMessage, RegisterMessage,
    ResponseChunkMessage, ResponseCompleteMessage, ServerToWorkerMessage, WorkerToServerMessage,
};
use modelrelay_server::{
    CancelReason as ProxyCancelReason, ProviderQueuePolicy, ProxyHttpApp, ProxyServerCore,
    RequestState, WorkerSocketApp, WorkerSocketProviderConfig,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Mutex,
    time::{sleep, timeout},
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

async fn spawn_server() -> SocketAddr {
    spawn_server_with_core(Arc::new(Mutex::new(ProxyServerCore::new())), true).await
}

async fn spawn_server_with_core(
    core: Arc<Mutex<ProxyServerCore>>,
    provider_enabled: bool,
) -> SocketAddr {
    let worker_socket_app = WorkerSocketApp::new(core.clone())
        .with_provider("openai", WorkerSocketProviderConfig::enabled("top-secret"))
        .with_heartbeat(
            tokio::time::Duration::from_millis(100),
            tokio::time::Duration::from_millis(300),
        );
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

async fn register_test_worker(socket: &mut TestSocket) {
    let register = WorkerToServerMessage::Register(RegisterMessage {
        worker_name: "gpu-box-a".to_string(),
        models: vec!["llama-3.1-70b".to_string()],
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

async fn post_chat_completions(addr: SocketAddr, body: &str, headers: &[(&str, &str)]) -> String {
    let mut stream = TcpStream::connect(addr)
        .await
        .expect("connect to test server");

    let mut request = format!(
        "POST /v1/chat/completions HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
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

async fn open_chat_completions_request(
    addr: SocketAddr,
    body: &str,
    headers: &[(&str, &str)],
) -> TcpStream {
    let mut stream = TcpStream::connect(addr)
        .await
        .expect("connect to test server");

    let mut request = format!(
        "POST /v1/chat/completions HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
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

fn assert_chat_completion_response(response: &str, worker_backend: &str, body: &str) {
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.contains("\r\ncontent-type: application/json\r\n"));
    assert!(response.contains(&format!("\r\nx-worker-backend: {worker_backend}\r\n")));
    assert!(response.ends_with(body));
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

async fn send_response_complete(
    socket: &mut TestSocket,
    request_id: &str,
    worker_backend: &str,
    body: &str,
) {
    let message = WorkerToServerMessage::ResponseComplete(ResponseCompleteMessage {
        request_id: request_id.to_string(),
        status_code: 200,
        headers: HeaderMap::from([
            ("content-type".to_string(), "application/json".to_string()),
            ("x-worker-backend".to_string(), worker_backend.to_string()),
        ]),
        body: Some(body.to_string()),
        token_counts: None,
    });
    let payload = serde_json::to_string(&message).expect("serialize response_complete");

    socket
        .send(Message::Text(payload.into()))
        .await
        .expect("send response_complete");
}

async fn assert_timed_out_request_state(core: &Arc<Mutex<ProxyServerCore>>, request_id: &str) {
    let core = core.lock().await;
    assert_eq!(
        core.request_state(request_id),
        Some(RequestState::InFlight {
            worker_id: "worker-1".to_string(),
            cancellation: Some(ProxyCancelReason::RequestTimedOut),
        })
    );
    assert_eq!(core.request_state("request-2"), Some(RequestState::Queued));
    assert_eq!(
        core.queued_request_ids("openai"),
        vec!["request-2".to_string()]
    );
}

async fn assert_timeout_keeps_http_clients_pending(
    first_http_request: &mut tokio::task::JoinHandle<String>,
    second_http_stream: &mut TcpStream,
) {
    assert!(
        timeout(std::time::Duration::from_millis(200), first_http_request)
            .await
            .is_err(),
        "timed-out in-flight HTTP request should remain pending until the worker completes it"
    );
    assert!(
        timeout(
            std::time::Duration::from_millis(200),
            read_until_contains(second_http_stream, "HTTP/1.1"),
        )
        .await
        .is_err(),
        "queued follow-up HTTP request should remain pending while the timed-out request is uncleared"
    );
}

async fn connect_and_register_replacement_worker(addr: SocketAddr) -> TestSocket {
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect second websocket");
    register_test_worker(&mut socket).await;
    send_models_update(&mut socket, vec!["llama-3.1-70b".to_string()], 0).await;
    socket
}

#[tokio::test]
async fn worker_backed_chat_completions_route_forwards_request_and_preserves_response() {
    let addr = spawn_server().await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");
    register_test_worker(&mut socket).await;

    let body = r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"hello"}]}"#;
    let http_request = tokio::spawn(post_chat_completions(
        addr,
        body,
        &[
            ("Authorization", "Bearer test-token"),
            ("X-Trace-Id", "trace-123"),
        ],
    ));

    let ServerToWorkerMessage::Request(request) =
        next_server_message(&mut socket, "worker request").await
    else {
        panic!("expected worker request message");
    };

    assert_eq!(request.model, "llama-3.1-70b");
    assert_eq!(request.endpoint_path, "/v1/chat/completions");
    assert!(!request.is_streaming);
    assert_eq!(request.body, body);
    assert_eq!(
        request.headers,
        HeaderMap::from([
            ("authorization".to_string(), "Bearer test-token".to_string()),
            ("content-type".to_string(), "application/json".to_string()),
            ("x-trace-id".to_string(), "trace-123".to_string()),
        ])
    );

    let complete = WorkerToServerMessage::ResponseComplete(ResponseCompleteMessage {
        request_id: request.request_id,
        status_code: 202,
        headers: HeaderMap::from([
            ("content-type".to_string(), "application/json".to_string()),
            ("x-worker-backend".to_string(), "gpu-box-a".to_string()),
        ]),
        body: Some(r#"{"id":"chatcmpl-1","object":"chat.completion"}"#.to_string()),
        token_counts: None,
    });
    let complete_payload = serde_json::to_string(&complete).expect("serialize response_complete");

    socket
        .send(Message::Text(complete_payload.into()))
        .await
        .expect("send response_complete");

    let response = http_request.await.expect("join http request task");
    assert!(response.starts_with("HTTP/1.1 202 Accepted\r\n"));
    assert!(response.contains("\r\ncontent-type: application/json\r\n"));
    assert!(response.contains("\r\nx-worker-backend: gpu-box-a\r\n"));
    assert!(response.ends_with(r#"{"id":"chatcmpl-1","object":"chat.completion"}"#));
}

#[tokio::test]
async fn worker_backed_chat_completions_route_preserves_upstream_http_error() {
    let addr = spawn_server().await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");
    register_test_worker(&mut socket).await;

    let body = r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"bad request"}]}"#;
    let http_request = tokio::spawn(post_chat_completions(addr, body, &[]));

    let ServerToWorkerMessage::Request(request) =
        next_server_message(&mut socket, "worker request").await
    else {
        panic!("expected worker request message");
    };

    let error_body =
        r#"{"error":{"message":"upstream rejected the payload","type":"invalid_request_error"}}"#;
    let complete = WorkerToServerMessage::ResponseComplete(ResponseCompleteMessage {
        request_id: request.request_id,
        status_code: 422,
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
    assert!(response.starts_with("HTTP/1.1 422 Unprocessable Entity\r\n"));
    assert!(response.contains("\r\ncontent-type: application/json\r\n"));
    assert!(response.contains("\r\nretry-after: 7\r\n"));
    assert!(response.contains("\r\nx-upstream-request-id: req-upstream-123\r\n"));
    assert!(response.ends_with(error_body));
}

#[tokio::test]
async fn worker_backed_chat_completions_streams_live_sse_chunks() {
    let addr = spawn_server().await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");
    register_test_worker(&mut socket).await;

    let body =
        r#"{"model":"llama-3.1-70b","stream":true,"messages":[{"role":"user","content":"hello"}]}"#;
    let mut http_stream =
        open_chat_completions_request(addr, body, &[("Authorization", "Bearer test-token")]).await;

    let ServerToWorkerMessage::Request(request) =
        next_server_message(&mut socket, "streaming worker request").await
    else {
        panic!("expected worker request message");
    };

    assert_eq!(request.endpoint_path, "/v1/chat/completions");
    assert!(request.is_streaming);
    assert_eq!(request.body, body);

    send_response_chunk(
        &mut socket,
        &request.request_id,
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
    )
    .await;

    let first_fragment = read_until_contains(
        &mut http_stream,
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
    )
    .await;
    assert!(first_fragment.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(first_fragment.contains("\r\ncontent-type: text/event-stream\r\n"));
    assert!(first_fragment.contains("data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n"));
    assert!(!first_fragment.contains("data: [DONE]\n\n"));

    send_response_chunk(
        &mut socket,
        &request.request_id,
        "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
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

    assert!(full_response.contains("data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n"));
    assert!(full_response.contains("data: [DONE]\n\n"));
    assert!(full_response.ends_with("0\r\n\r\n"));
}

#[tokio::test]
async fn worker_backed_chat_completions_route_forwards_large_sse_chunks() {
    let addr = spawn_server().await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");
    register_test_worker(&mut socket).await;

    let body = r#"{"model":"llama-3.1-70b","stream":true,"messages":[{"role":"user","content":"overflow"}]}"#;
    let mut http_stream = open_chat_completions_request(addr, body, &[]).await;

    let ServerToWorkerMessage::Request(request) =
        next_server_message(&mut socket, "large streaming worker request").await
    else {
        panic!("expected large streaming worker request message");
    };

    send_response_chunk(
        &mut socket,
        &request.request_id,
        "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n",
    )
    .await;

    let first_fragment = read_until_contains(
        &mut http_stream,
        "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n",
    )
    .await;
    assert!(first_fragment.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(first_fragment.contains("\r\ncontent-type: text/event-stream\r\n"));

    // Send a large chunk (~80 KB) — should be forwarded, not rejected
    let large_marker = "large-chunk-data-".repeat(4_200);
    let large_chunk =
        format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{large_marker}\"}}}}]}}\n\n");
    send_response_chunk(&mut socket, &request.request_id, &large_chunk).await;

    let rest = timeout(
        std::time::Duration::from_secs(2),
        read_until_contains(&mut http_stream, &large_marker),
    )
    .await
    .expect("large chunk should be forwarded within timeout");
    let full_response = first_fragment + &rest;

    assert!(
        full_response.contains(&large_marker),
        "large chunks should be forwarded to the HTTP client"
    );
    assert!(
        !full_response.contains("stream exceeded size limit"),
        "large chunks should not trigger the size limit error"
    );

    send_response_complete(
        &mut socket,
        &request.request_id,
        "gpu-box-a",
        r#"{"id":"chatcmpl-late","object":"chat.completion.chunk","choices":[]}"#,
    )
    .await;
    send_response_chunk(&mut socket, &request.request_id, "data: [DONE]\n\n").await;
}

#[tokio::test]
async fn worker_backed_chat_completions_route_cancels_in_flight_request_when_http_client_disconnects()
 {
    let addr = spawn_server().await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");
    register_test_worker(&mut socket).await;

    let body = r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"cancel me"}]}"#;
    let mut http_stream =
        open_chat_completions_request(addr, body, &[("Authorization", "Bearer test-token")]).await;

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
async fn worker_backed_chat_completions_route_times_out_in_flight_request_before_redispatching_queued_work()
 {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let addr = spawn_server_with_core(core.clone(), true).await;

    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");
    register_test_worker(&mut socket).await;

    let first_body = r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"time out before completion"}]}"#;
    let second_body = r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"stay queued until timeout clears"}]}"#;

    let mut first_http_request = tokio::spawn(post_chat_completions(addr, first_body, &[]));
    let ServerToWorkerMessage::Request(first_request) =
        next_server_message(&mut socket, "first worker request").await
    else {
        panic!("expected first worker request message");
    };
    assert_eq!(first_request.body, first_body);

    let mut second_http_stream = open_chat_completions_request(addr, second_body, &[]).await;
    wait_for_request_state(&core, "request-2", RequestState::Queued).await;

    {
        let mut core = core.lock().await;
        assert_eq!(
            core.cancel_request(
                &first_request.request_id,
                ProxyCancelReason::RequestTimedOut
            ),
            Some(modelrelay_server::CancellationOutcome::WorkerCancelSent(
                modelrelay_server::WorkerCancelSignal {
                    worker_id: "worker-1".to_string(),
                    request_id: first_request.request_id.clone(),
                    reason: ProxyCancelReason::RequestTimedOut,
                }
            ))
        );
    }
    assert_timed_out_request_state(&core, &first_request.request_id).await;
    assert_worker_emits_single_timeout_cancel_and_stays_idle(
        &mut socket,
        &first_request.request_id,
    )
    .await;
    assert_timeout_keeps_http_clients_pending(&mut first_http_request, &mut second_http_stream)
        .await;

    send_response_complete(
        &mut socket,
        &first_request.request_id,
        "gpu-box-a",
        r#"{"id":"chatcmpl-timeout-1","object":"chat.completion","choices":[]}"#,
    )
    .await;

    let _first_response = timeout(std::time::Duration::from_secs(2), &mut first_http_request)
        .await
        .expect("first timed-out http request completed before timeout")
        .expect("join first timed-out http request task");

    {
        let core = core.lock().await;
        assert_eq!(core.request_state("request-2"), Some(RequestState::Queued));
        assert_eq!(
            core.queued_request_ids("openai"),
            vec!["request-2".to_string()]
        );
    }

    assert_worker_socket_closes(&mut socket).await;
    let mut replacement_socket = connect_and_register_replacement_worker(addr).await;

    let ServerToWorkerMessage::Request(second_request) = next_server_message(
        &mut replacement_socket,
        "queued worker request after timeout clears",
    )
    .await
    else {
        panic!("expected queued worker request after timeout clears");
    };
    assert_eq!(second_request.request_id, "request-2");
    assert_eq!(second_request.endpoint_path, "/v1/chat/completions");
    assert_eq!(second_request.body, second_body);

    send_response_complete(
        &mut replacement_socket,
        &second_request.request_id,
        "gpu-box-b",
        r#"{"id":"chatcmpl-timeout-2","object":"chat.completion","choices":[]}"#,
    )
    .await;

    let second_response = read_http_response(&mut second_http_stream).await;
    assert_chat_completion_response(
        &second_response,
        "gpu-box-b",
        r#"{"id":"chatcmpl-timeout-2","object":"chat.completion","choices":[]}"#,
    );
    assert!(
        !second_response.contains("chatcmpl-timeout-1"),
        "late output from the timed-out request should not leak into the queued follow-up response"
    );
}

#[tokio::test]
async fn worker_backed_chat_completions_route_returns_sanitized_no_workers_error() {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    {
        let mut core = core.lock().await;
        core.configure_provider_queue(
            "openai",
            ProviderQueuePolicy {
                max_queue_len: 0,
                queue_timeout_ticks: None,
            },
        );
    }
    let addr = spawn_server_with_core(core, true).await;

    let body = r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"hello"}]}"#;
    let response = post_chat_completions(addr, body, &[]).await;

    assert_service_unavailable(&response, "No workers available to handle request");
    assert!(
        !response.contains("queue is full"),
        "the client boundary should not expose the internal queue rejection"
    );
}

#[tokio::test]
async fn worker_backed_chat_completions_route_returns_sanitized_queue_timeout_error() {
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

    let body = r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"timeout me"}]}"#;
    let http_request = tokio::spawn(post_chat_completions(addr, body, &[]));
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
async fn worker_backed_chat_completions_route_returns_sanitized_queue_full_error() {
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

    let body = r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"hello"}]}"#;
    let first_request = tokio::spawn(open_chat_completions_request(addr, body, &[]));
    let ServerToWorkerMessage::Request(_) =
        next_server_message(&mut socket, "first worker request").await
    else {
        panic!("expected first worker request message");
    };

    let second_request = tokio::spawn(open_chat_completions_request(addr, body, &[]));
    wait_for_request_state(&core, "request-2", RequestState::Queued).await;

    let response = post_chat_completions(addr, body, &[]).await;
    assert_service_unavailable(&response, "Service temporarily at capacity, please retry");
    assert!(
        !response.contains("queue is full"),
        "the client boundary should not expose the raw queue-full reason"
    );

    first_request.abort();
    second_request.abort();
}

#[tokio::test]
async fn worker_backed_chat_completions_route_returns_sanitized_provider_disabled_error() {
    let addr = spawn_server_with_core(Arc::new(Mutex::new(ProxyServerCore::new())), false).await;

    let body = r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"hello"}]}"#;
    let response = post_chat_completions(addr, body, &[]).await;

    assert_service_unavailable(&response, "Provider is currently disabled");
    assert!(
        !response.contains("virtual provider is disabled"),
        "the compatibility boundary should use the stable disabled message"
    );
}

#[tokio::test]
async fn worker_backed_chat_completions_route_returns_sanitized_provider_deleted_error() {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let addr = spawn_server_with_core(core.clone(), true).await;

    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");
    register_test_worker(&mut socket).await;

    let body = r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"delete provider while in flight"}]}"#;
    let http_request = tokio::spawn(post_chat_completions(addr, body, &[]));

    let ServerToWorkerMessage::Request(_) =
        next_server_message(&mut socket, "in-flight worker request").await
    else {
        panic!("expected in-flight worker request message");
    };

    {
        let mut core = core.lock().await;
        core.delete_provider("openai");
    }

    let response = http_request
        .await
        .expect("join provider-deleted http request");
    assert_service_unavailable(&response, "Internal server error processing request");
    assert!(
        !response.contains("provider was deleted"),
        "the compatibility boundary should not leak the internal provider-deletion reason"
    );

    assert_worker_socket_closes(&mut socket).await;
}

#[tokio::test]
async fn worker_backed_chat_completions_route_recovers_after_worker_auth_rate_limit_window_expires()
{
    let addr = spawn_server().await;

    for _ in 0..3 {
        let (mut socket, _) = connect_async(worker_connect_request(addr, "wrong-secret"))
            .await
            .expect("connect websocket");
        let close_frame = next_close_frame(&mut socket, "bad secret rejection").await;
        assert_eq!(u16::from(close_frame.code), 1008);
        assert_eq!(close_frame.reason, "worker authentication failed");
    }

    let (mut throttled_socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");
    let close_frame = next_close_frame(&mut throttled_socket, "auth rate limit rejection").await;
    assert_eq!(u16::from(close_frame.code), 1008);
    assert_eq!(
        close_frame.reason,
        "worker authentication temporarily rate limited"
    );

    sleep(std::time::Duration::from_millis(300)).await;

    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");
    register_test_worker(&mut socket).await;

    let body = r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"auth cooldown expired"}]}"#;
    let http_request = tokio::spawn(post_chat_completions(addr, body, &[]));

    let ServerToWorkerMessage::Request(request) =
        next_server_message(&mut socket, "chat completions request after auth cooldown").await
    else {
        panic!("expected request message");
    };
    assert_eq!(request.endpoint_path, "/v1/chat/completions");
    assert_eq!(request.body, body);
    assert_eq!(
        request.headers,
        HeaderMap::from([("content-type".to_string(), "application/json".to_string()),])
    );

    send_response_complete(
        &mut socket,
        &request.request_id,
        "gpu-box-a",
        r#"{"id":"chatcmpl-auth-expiry","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"worker re-authenticated"},"finish_reason":"stop"}]}"#,
    )
    .await;

    let response = http_request.await.expect("join http request");
    assert_chat_completion_response(
        &response,
        "gpu-box-a",
        r#"{"id":"chatcmpl-auth-expiry","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"worker re-authenticated"},"finish_reason":"stop"}]}"#,
    );
}

#[tokio::test]
async fn worker_backed_chat_completions_route_requeues_live_request_after_worker_disconnect() {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let addr = spawn_server_with_core(core.clone(), true).await;

    let (mut socket_one, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect first websocket");
    register_test_worker(&mut socket_one).await;

    let body = r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"finish after reconnect"}]}"#;
    let http_request = tokio::spawn(post_chat_completions(addr, body, &[]));

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

    let (mut socket_two, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect second websocket");
    register_test_worker(&mut socket_two).await;
    send_models_update(&mut socket_two, vec!["llama-3.1-70b".to_string()], 0).await;

    let ServerToWorkerMessage::Request(requeued_request) =
        next_server_message(&mut socket_two, "requeued worker request").await
    else {
        panic!("expected requeued worker request message");
    };

    assert_eq!(requeued_request.request_id, first_request.request_id);
    assert_eq!(requeued_request.endpoint_path, "/v1/chat/completions");
    assert_eq!(requeued_request.body, body);
    assert_eq!(
        requeued_request.headers,
        HeaderMap::from([("content-type".to_string(), "application/json".to_string()),])
    );

    let complete = WorkerToServerMessage::ResponseComplete(ResponseCompleteMessage {
        request_id: requeued_request.request_id,
        status_code: 200,
        headers: HeaderMap::from([
            ("content-type".to_string(), "application/json".to_string()),
            ("x-worker-backend".to_string(), "gpu-box-b".to_string()),
        ]),
        body: Some(
            r#"{"id":"chatcmpl-requeued","object":"chat.completion","choices":[]}"#.to_string(),
        ),
        token_counts: None,
    });
    let complete_payload = serde_json::to_string(&complete).expect("serialize response_complete");

    socket_two
        .send(Message::Text(complete_payload.into()))
        .await
        .expect("send response_complete");

    let response = timeout(std::time::Duration::from_secs(2), http_request)
        .await
        .expect("http request completed before timeout")
        .expect("join http request task");
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.contains("\r\ncontent-type: application/json\r\n"));
    assert!(response.contains("\r\nx-worker-backend: gpu-box-b\r\n"));
    assert!(
        response.ends_with(r#"{"id":"chatcmpl-requeued","object":"chat.completion","choices":[]}"#)
    );
}

#[tokio::test]
async fn worker_backed_chat_completions_route_drains_in_flight_request_before_redispatching_queued_work()
 {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let addr = spawn_server_with_core(core.clone(), true).await;

    let (mut socket_one, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect first websocket");
    register_test_worker(&mut socket_one).await;

    let first_body =
        r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"finish before drain"}]}"#;
    let second_body = r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"stay queued during drain"}]}"#;

    let first_http_request = tokio::spawn(post_chat_completions(addr, first_body, &[]));
    let ServerToWorkerMessage::Request(first_request) =
        next_server_message(&mut socket_one, "first worker request").await
    else {
        panic!("expected first worker request message");
    };
    assert_eq!(first_request.body, first_body);

    let mut second_http_stream = open_chat_completions_request(addr, second_body, &[]).await;
    wait_for_request_state(&core, "request-2", RequestState::Queued).await;
    begin_graceful_shutdown(&core).await;
    assert_graceful_shutdown_signal(&mut socket_one).await;
    assert_draining_worker_stays_idle(&mut socket_one).await;
    assert!(
        timeout(
            std::time::Duration::from_millis(200),
            read_until_contains(&mut second_http_stream, "HTTP/1.1"),
        )
        .await
        .is_err(),
        "queued HTTP client should remain pending until a replacement worker is available"
    );

    send_response_complete(
        &mut socket_one,
        &first_request.request_id,
        "gpu-box-a",
        r#"{"id":"chatcmpl-drain-1","object":"chat.completion","choices":[]}"#,
    )
    .await;

    let first_response = timeout(std::time::Duration::from_secs(2), first_http_request)
        .await
        .expect("first http request completed before timeout")
        .expect("join first http request task");
    assert_chat_completion_response(
        &first_response,
        "gpu-box-a",
        r#"{"id":"chatcmpl-drain-1","object":"chat.completion","choices":[]}"#,
    );
    assert_post_drain_close(&mut socket_one).await;

    {
        let core = core.lock().await;
        assert_eq!(core.request_state("request-2"), Some(RequestState::Queued));
        assert_eq!(
            core.queued_request_ids("openai"),
            vec!["request-2".to_string()]
        );
    }

    let mut socket_two = connect_and_register_replacement_worker(addr).await;

    let ServerToWorkerMessage::Request(second_request) =
        next_server_message(&mut socket_two, "queued worker request after drain").await
    else {
        panic!("expected queued worker request after drain");
    };
    assert_eq!(second_request.request_id, "request-2");
    assert_eq!(second_request.endpoint_path, "/v1/chat/completions");
    assert_eq!(second_request.body, second_body);

    send_response_complete(
        &mut socket_two,
        &second_request.request_id,
        "gpu-box-b",
        r#"{"id":"chatcmpl-drain-2","object":"chat.completion","choices":[]}"#,
    )
    .await;

    let second_response = read_http_response(&mut second_http_stream).await;
    assert_chat_completion_response(
        &second_response,
        "gpu-box-b",
        r#"{"id":"chatcmpl-drain-2","object":"chat.completion","choices":[]}"#,
    );
}

#[tokio::test]
async fn worker_backed_chat_completions_route_returns_sanitized_requeue_exhaustion_error() {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let addr = spawn_server_with_core(core.clone(), true).await;

    let (mut socket_one, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect first websocket");
    register_test_worker(&mut socket_one).await;

    let body =
        r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"keep retrying"}]}"#;
    let http_request = tokio::spawn(post_chat_completions(addr, body, &[]));

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
        send_models_update(&mut socket, vec!["llama-3.1-70b".to_string()], 0).await;

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

    let response = timeout(std::time::Duration::from_secs(2), http_request)
        .await
        .expect("http request completed before timeout")
        .expect("join http request task");
    assert_service_unavailable(
        &response,
        "Request could not be processed after multiple attempts",
    );
    assert!(
        !response.contains("MaxRequeuesExceeded"),
        "the client boundary should not expose the internal failure enum"
    );
    assert!(
        !response.to_ascii_lowercase().contains("requeue"),
        "the client boundary should not expose raw requeue wording"
    );
    assert!(
        !response.to_ascii_lowercase().contains("max retries"),
        "the client boundary should not expose raw retry-limit wording"
    );
}
