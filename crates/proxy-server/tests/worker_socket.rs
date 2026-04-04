use std::{net::SocketAddr, sync::Arc};

use futures_util::{SinkExt, StreamExt};
use proxy_server::{
    CancelReason as ProxyCancelReason, ProxyServerCore, RequestState, SubmissionOutcome,
    WorkerSocketApp, WorkerSocketProviderConfig,
};
use tokio::{
    net::TcpListener,
    sync::Mutex,
    time::{sleep, timeout},
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use worker_protocol::{
    CancelMessage, CancelReason as ProtocolCancelReason, GracefulShutdownMessage, HeaderMap,
    ModelsUpdateMessage, PingMessage, PongMessage, RegisterMessage, ResponseChunkMessage,
    ResponseCompleteMessage, ServerToWorkerMessage, WorkerToServerMessage,
};

type TestSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn spawn_server() -> (SocketAddr, Arc<Mutex<ProxyServerCore>>) {
    spawn_server_with_provider_config(WorkerSocketProviderConfig::enabled("top-secret")).await
}

async fn spawn_server_with_provider_config(
    provider_config: WorkerSocketProviderConfig,
) -> (SocketAddr, Arc<Mutex<ProxyServerCore>>) {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let app = WorkerSocketApp::new(core.clone())
        .with_provider("openai", provider_config)
        .router();

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("listener local addr");

    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .expect("serve worker socket app");
    });

    (addr, core)
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

fn worker_connect_request_with_query_secret(addr: SocketAddr, secret: &str) -> http::Request<()> {
    format!("ws://{addr}/v1/worker/connect?provider=openai&worker_secret={secret}")
        .into_client_request()
        .expect("build websocket request")
}

async fn next_text_message(socket: &mut TestSocket, context: &str) -> String {
    let message = timeout(std::time::Duration::from_secs(2), socket.next())
        .await
        .unwrap_or_else(|_| panic!("receive {context} before timeout"))
        .expect("socket message")
        .expect("websocket message");
    let Message::Text(payload) = message else {
        panic!("expected text {context}");
    };

    payload.to_string()
}

async fn next_non_heartbeat_text_message(socket: &mut TestSocket, context: &str) -> String {
    loop {
        let payload = next_text_message(socket, context).await;
        let Ok(ServerToWorkerMessage::Ping(_)) = serde_json::from_str(&payload) else {
            return payload;
        };
    }
}

async fn next_server_message(socket: &mut TestSocket, context: &str) -> ServerToWorkerMessage {
    serde_json::from_str(&next_non_heartbeat_text_message(socket, context).await)
        .unwrap_or_else(|_| panic!("deserialize {context}"))
}

async fn next_close_message(
    socket: &mut TestSocket,
    context: &str,
) -> tokio_tungstenite::tungstenite::protocol::CloseFrame {
    loop {
        let message = timeout(std::time::Duration::from_secs(2), socket.next())
            .await
            .unwrap_or_else(|_| panic!("receive {context} before timeout"))
            .expect("socket message")
            .expect("websocket message");

        match message {
            Message::Text(payload) => {
                if matches!(
                    serde_json::from_str::<ServerToWorkerMessage>(&payload),
                    Ok(ServerToWorkerMessage::Ping(_))
                ) {
                    continue;
                }
                panic!("expected close frame {context}");
            }
            Message::Close(Some(close_frame)) => return close_frame,
            _ => panic!("expected close frame {context}"),
        }
    }
}

async fn assert_no_non_heartbeat_message(socket: &mut TestSocket, wait_for: std::time::Duration) {
    let deadline = tokio::time::Instant::now() + wait_for;

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return;
        }

        let next = timeout(remaining, socket.next()).await;
        let Ok(Some(Ok(Message::Text(payload)))) = next else {
            return;
        };

        let Ok(ServerToWorkerMessage::Ping(_)) =
            serde_json::from_str::<ServerToWorkerMessage>(&payload)
        else {
            panic!("unexpected non-heartbeat message while waiting for quiet socket");
        };
    }
}

async fn wait_for_worker_reported_load(
    core: &Arc<Mutex<ProxyServerCore>>,
    worker_id: &str,
    expected_load: usize,
) {
    timeout(std::time::Duration::from_secs(2), async {
        loop {
            {
                let core = core.lock().await;
                if core.worker_reported_load(worker_id) == Some(expected_load) {
                    return;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("worker {worker_id} reported load did not reach {expected_load}"));
}

async fn wait_for_worker_disconnect(core: &Arc<Mutex<ProxyServerCore>>, worker_id: &str) {
    timeout(std::time::Duration::from_secs(2), async {
        loop {
            {
                let core = core.lock().await;
                if !core.has_worker(worker_id) {
                    return;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("worker {worker_id} was not disconnected"));
}

async fn submit_heartbeat_initial_request(core: &Arc<Mutex<ProxyServerCore>>, worker_id: &str) {
    let mut core = core.lock().await;
    assert_eq!(
        core.submit_transport_request(
            "openai",
            "llama-3.1-70b",
            "/v1/chat/completions",
            false,
            r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"heartbeat me"}]}"#,
            HeaderMap::new(),
        ),
        SubmissionOutcome::Dispatched(proxy_server::DispatchAssignment {
            request_id: "request-1".to_string(),
            worker_id: worker_id.to_string(),
        })
    );
}

async fn submit_heartbeat_follow_up_request(core: &Arc<Mutex<ProxyServerCore>>) {
    let mut core = core.lock().await;
    assert_eq!(
        core.submit_transport_request(
            "openai",
            "llama-3.1-70b",
            "/v1/chat/completions",
            false,
            r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"stay queued until complete"}]}"#,
            HeaderMap::new(),
        ),
        SubmissionOutcome::Queued(proxy_server::QueuedAssignment {
            request_id: "request-2".to_string(),
            queue_len: 1,
        })
    );
}

async fn assert_heartbeat_ping(socket: &mut TestSocket) {
    assert_eq!(
        serde_json::from_str::<ServerToWorkerMessage>(
            &next_text_message(socket, "heartbeat ping").await
        )
        .expect("deserialize heartbeat ping"),
        ServerToWorkerMessage::Ping(PingMessage {
            timestamp_unix_ms: None,
        })
    );
}

async fn register_test_worker(socket: &mut TestSocket) -> worker_protocol::RegisterAck {
    register_test_worker_with(socket, vec!["llama-3.1-70b".to_string()], 1, Some(0)).await
}

async fn register_test_worker_with(
    socket: &mut TestSocket,
    models: Vec<String>,
    max_concurrent: u32,
    current_load: Option<u32>,
) -> worker_protocol::RegisterAck {
    let register = WorkerToServerMessage::Register(RegisterMessage {
        worker_name: "gpu-box-a".to_string(),
        models,
        max_concurrent,
        protocol_version: Some("2026-04-bridge-v1".to_string()),
        current_load,
    });
    let register_payload = serde_json::to_string(&register).expect("serialize register");

    socket
        .send(Message::Text(register_payload.into()))
        .await
        .expect("send register");

    let ack = next_server_message(socket, "register_ack").await;
    let ServerToWorkerMessage::RegisterAck(ack) = ack else {
        panic!("expected register_ack message");
    };

    ack
}

async fn send_register_message(socket: &mut TestSocket, register: RegisterMessage) {
    let register_payload = serde_json::to_string(&WorkerToServerMessage::Register(register))
        .expect("serialize register");

    socket
        .send(Message::Text(register_payload.into()))
        .await
        .expect("send register");
}

fn expected_request_message(
    request_id: &str,
    body: &str,
    headers: HeaderMap,
) -> ServerToWorkerMessage {
    ServerToWorkerMessage::Request(worker_protocol::RequestMessage {
        request_id: request_id.to_string(),
        model: "llama-3.1-70b".to_string(),
        endpoint_path: "/v1/chat/completions".to_string(),
        is_streaming: false,
        body: body.to_string(),
        headers,
    })
}

async fn send_response_complete(socket: &mut TestSocket, request_id: &str) {
    let complete = WorkerToServerMessage::ResponseComplete(ResponseCompleteMessage {
        request_id: request_id.to_string(),
        status_code: 200,
        headers: HeaderMap::from([("content-type".to_string(), "application/json".to_string())]),
        body: Some(r#"{"id":"resp-1"}"#.to_string()),
        token_counts: None,
    });
    let complete_payload = serde_json::to_string(&complete).expect("serialize response_complete");

    socket
        .send(Message::Text(complete_payload.into()))
        .await
        .expect("send response_complete");
}

async fn send_response_chunk(socket: &mut TestSocket, request_id: &str, chunk: &str) {
    let chunk = WorkerToServerMessage::ResponseChunk(ResponseChunkMessage {
        request_id: request_id.to_string(),
        chunk: chunk.to_string(),
    });
    let chunk_payload = serde_json::to_string(&chunk).expect("serialize response_chunk");

    socket
        .send(Message::Text(chunk_payload.into()))
        .await
        .expect("send response_chunk");
}

async fn queue_cancel_test_requests(
    core: &Arc<Mutex<ProxyServerCore>>,
    worker_id: &str,
    second_headers: &HeaderMap,
) {
    let mut core = core.lock().await;
    assert_eq!(
        core.submit_transport_request(
            "openai",
            "llama-3.1-70b",
            "/v1/chat/completions",
            false,
            r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"cancel me"}]}"#,
            HeaderMap::new(),
        ),
        SubmissionOutcome::Dispatched(proxy_server::DispatchAssignment {
            request_id: "request-1".to_string(),
            worker_id: worker_id.to_string(),
        })
    );
    assert_eq!(
        core.submit_transport_request(
            "openai",
            "llama-3.1-70b",
            "/v1/chat/completions",
            false,
            r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"after cancel"}]}"#,
            second_headers.clone(),
        ),
        SubmissionOutcome::Queued(proxy_server::QueuedAssignment {
            request_id: "request-2".to_string(),
            queue_len: 1,
        })
    );
}

async fn cancel_in_flight_request(
    core: &Arc<Mutex<ProxyServerCore>>,
    worker_id: &str,
    reason: ProxyCancelReason,
) {
    let mut core = core.lock().await;
    assert_eq!(
        core.cancel_request("request-1", reason),
        Some(proxy_server::CancellationOutcome::WorkerCancelSent(
            proxy_server::WorkerCancelSignal {
                worker_id: worker_id.to_string(),
                request_id: "request-1".to_string(),
                reason,
            }
        ))
    );
}

async fn assert_canceled_request_state(
    core: &Arc<Mutex<ProxyServerCore>>,
    worker_id: &str,
    reason: ProxyCancelReason,
) {
    let core = core.lock().await;
    assert_eq!(
        core.request_state("request-1"),
        Some(RequestState::InFlight {
            worker_id: worker_id.to_string(),
            cancellation: Some(reason),
        })
    );
    assert_eq!(
        core.queued_request_ids("openai"),
        vec!["request-2".to_string()]
    );
}

async fn submit_graceful_shutdown_test_requests(
    core: &Arc<Mutex<ProxyServerCore>>,
    worker_id: &str,
) {
    let mut core = core.lock().await;
    assert_eq!(
        core.submit_transport_request(
            "openai",
            "llama-3.1-70b",
            "/v1/chat/completions",
            false,
            r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"finish before drain"}]}"#,
            HeaderMap::new(),
        ),
        SubmissionOutcome::Dispatched(proxy_server::DispatchAssignment {
            request_id: "request-1".to_string(),
            worker_id: worker_id.to_string(),
        })
    );
    assert_eq!(
        core.submit_transport_request(
            "openai",
            "llama-3.1-70b",
            "/v1/chat/completions",
            false,
            r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"stay queued during drain"}]}"#,
            HeaderMap::new(),
        ),
        SubmissionOutcome::Queued(proxy_server::QueuedAssignment {
            request_id: "request-2".to_string(),
            queue_len: 1,
        })
    );
}

#[tokio::test]
async fn authenticated_worker_can_register_and_receive_register_ack() {
    let (addr, core) = spawn_server().await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");

    send_register_message(
        &mut socket,
        RegisterMessage {
            worker_name: "gpu-box-a".to_string(),
            models: vec!["llama-3.1-70b".to_string(), " mistral-large ".to_string()],
            max_concurrent: 2,
            protocol_version: Some("2026-04-bridge-v1".to_string()),
            current_load: Some(0),
        },
    )
    .await;

    let ack_message = timeout(std::time::Duration::from_secs(2), socket.next())
        .await
        .expect("receive register_ack before timeout")
        .expect("socket message")
        .expect("websocket message");
    let Message::Text(ack_payload) = ack_message else {
        panic!("expected text register_ack");
    };

    let ack = serde_json::from_str::<ServerToWorkerMessage>(&ack_payload)
        .expect("deserialize register ack");
    assert_eq!(
        ack,
        ServerToWorkerMessage::RegisterAck(worker_protocol::RegisterAck {
            worker_id: "worker-1".to_string(),
            models: vec!["llama-3.1-70b".to_string(), "mistral-large".to_string()],
            warnings: Vec::new(),
            protocol_version: Some("2026-04-bridge-v1".to_string()),
        })
    );

    let core = core.lock().await;
    assert_eq!(
        core.provider_models("openai"),
        vec!["llama-3.1-70b".to_string(), "mistral-large".to_string()]
    );
}

#[tokio::test]
async fn mismatched_protocol_version_is_closed_with_protocol_error() {
    let (addr, core) = spawn_server().await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");

    send_register_message(
        &mut socket,
        RegisterMessage {
            worker_name: "gpu-box-a".to_string(),
            models: vec!["llama-3.1-70b".to_string()],
            max_concurrent: 1,
            protocol_version: Some("katamari-pre-release".to_string()),
            current_load: Some(0),
        },
    )
    .await;

    let close_frame = next_close_message(&mut socket, "protocol version mismatch rejection").await;

    assert_eq!(u16::from(close_frame.code), 1002);
    assert_eq!(close_frame.reason, "worker registration protocol error");

    let core = core.lock().await;
    assert!(core.provider_models("openai").is_empty());
}

#[tokio::test]
async fn rejected_auth_connection_is_closed_with_policy_violation() {
    let (addr, core) = spawn_server().await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "wrong-secret"))
        .await
        .expect("connect websocket");

    let close_message = timeout(std::time::Duration::from_secs(2), socket.next())
        .await
        .expect("receive auth close before timeout")
        .expect("socket message")
        .expect("websocket message");
    let Message::Close(Some(close_frame)) = close_message else {
        panic!("expected auth rejection close frame");
    };

    assert_eq!(u16::from(close_frame.code), 1008);
    assert_eq!(close_frame.reason, "worker authentication failed");

    let core = core.lock().await;
    assert!(core.provider_models("openai").is_empty());
}

#[tokio::test]
async fn repeated_failed_auth_attempts_are_rate_limited_until_cooldown_expires() {
    let (addr, core) = spawn_server().await;

    for _ in 0..3 {
        let (mut socket, _) = connect_async(worker_connect_request(addr, "wrong-secret"))
            .await
            .expect("connect websocket");

        let close_frame = next_close_message(&mut socket, "bad secret rejection").await;
        assert_eq!(u16::from(close_frame.code), 1008);
        assert_eq!(close_frame.reason, "worker authentication failed");
    }

    let (mut throttled_socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");
    let close_frame = next_close_message(&mut throttled_socket, "auth rate limit rejection").await;
    assert_eq!(u16::from(close_frame.code), 1008);
    assert_eq!(
        close_frame.reason,
        "worker authentication temporarily rate limited"
    );

    sleep(std::time::Duration::from_millis(300)).await;

    let (mut recovered_socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");
    let ack = register_test_worker(&mut recovered_socket).await;

    let core = core.lock().await;
    assert_eq!(ack.worker_id, "worker-1");
    assert_eq!(
        core.provider_models("openai"),
        vec!["llama-3.1-70b".to_string()]
    );
}

#[tokio::test]
async fn worker_secret_query_string_fallback_authenticates_connection() {
    let (addr, core) = spawn_server().await;
    let (mut socket, _) =
        connect_async(worker_connect_request_with_query_secret(addr, "top-secret"))
            .await
            .expect("connect websocket");

    let ack = register_test_worker(&mut socket).await;

    let core = core.lock().await;
    assert_eq!(ack.worker_id, "worker-1");
    assert_eq!(
        core.provider_models("openai"),
        vec!["llama-3.1-70b".to_string()]
    );
}

#[tokio::test]
async fn disabled_provider_connection_is_closed_with_policy_violation() {
    let (addr, core) =
        spawn_server_with_provider_config(WorkerSocketProviderConfig::disabled("top-secret")).await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");

    let close_frame = next_close_message(&mut socket, "disabled provider rejection").await;

    assert_eq!(u16::from(close_frame.code), 1008);
    assert_eq!(close_frame.reason, "worker authentication failed");

    let core = core.lock().await;
    assert!(core.provider_models("openai").is_empty());
}

#[tokio::test]
async fn registered_worker_receives_request_and_response_complete_dispatches_next_queued_request() {
    let (addr, core) = spawn_server().await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");
    let ack = register_test_worker(&mut socket).await;

    let first_headers = HeaderMap::from([
        ("authorization".to_string(), "Bearer token-1".to_string()),
        ("x-trace-id".to_string(), "trace-1".to_string()),
    ]);
    let second_headers = HeaderMap::from([("x-trace-id".to_string(), "trace-2".to_string())]);

    {
        let mut core = core.lock().await;
        assert_eq!(
            core.submit_transport_request(
                "openai",
                "llama-3.1-70b",
                "/v1/chat/completions",
                false,
                r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"hi"}]}"#,
                first_headers.clone(),
            ),
            SubmissionOutcome::Dispatched(proxy_server::DispatchAssignment {
                request_id: "request-1".to_string(),
                worker_id: ack.worker_id.clone(),
            })
        );
        assert_eq!(
            core.submit_transport_request(
                "openai",
                "llama-3.1-70b",
                "/v1/chat/completions",
                false,
                r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"next"}]}"#,
                second_headers.clone(),
            ),
            SubmissionOutcome::Queued(proxy_server::QueuedAssignment {
                request_id: "request-2".to_string(),
                queue_len: 1,
            })
        );
    }

    assert_eq!(
        next_server_message(&mut socket, "dispatched request").await,
        expected_request_message(
            "request-1",
            r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"hi"}]}"#,
            first_headers,
        )
    );

    {
        let core = core.lock().await;
        assert_eq!(
            core.request_state("request-1"),
            Some(RequestState::InFlight {
                worker_id: ack.worker_id.clone(),
                cancellation: None,
            })
        );
        assert_eq!(
            core.queued_request_ids("openai"),
            vec!["request-2".to_string()]
        );
    }

    send_response_complete(&mut socket, "request-1").await;
    assert_eq!(
        next_server_message(&mut socket, "next dispatched request").await,
        expected_request_message(
            "request-2",
            r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"next"}]}"#,
            second_headers,
        )
    );

    let core = core.lock().await;
    assert_eq!(core.request_state("request-1"), None);
    assert_eq!(
        core.request_state("request-2"),
        Some(RequestState::InFlight {
            worker_id: ack.worker_id,
            cancellation: None,
        })
    );
    assert!(core.queued_request_ids("openai").is_empty());
    assert_eq!(
        core.worker_in_flight_request_ids("worker-1"),
        vec!["request-2".to_string()]
    );
}

#[tokio::test]
async fn worker_models_update_dispatches_newly_compatible_queued_request_without_reconnect() {
    let (addr, core) = spawn_server().await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");
    let ack =
        register_test_worker_with(&mut socket, vec!["llama-3.1-70b".to_string()], 1, Some(1)).await;

    {
        let mut core = core.lock().await;
        assert_eq!(
            core.submit_transport_request(
                "openai",
                "mistral-large",
                "/v1/chat/completions",
                false,
                r#"{"model":"mistral-large","messages":[{"role":"user","content":"route after models_update"}]}"#,
                HeaderMap::new(),
            ),
            SubmissionOutcome::Queued(proxy_server::QueuedAssignment {
                request_id: "request-1".to_string(),
                queue_len: 1,
            })
        );
        assert_eq!(
            core.queued_request_ids("openai"),
            vec!["request-1".to_string()]
        );
        assert_eq!(
            core.worker_in_flight_request_ids(&ack.worker_id),
            Vec::<String>::new()
        );
    }

    let models_update = WorkerToServerMessage::ModelsUpdate(ModelsUpdateMessage {
        models: vec!["llama-3.1-70b".to_string(), "mistral-large".to_string()],
        current_load: 0,
    });
    let update_payload = serde_json::to_string(&models_update).expect("serialize models_update");
    socket
        .send(Message::Text(update_payload.into()))
        .await
        .expect("send models_update");

    assert_eq!(
        next_server_message(&mut socket, "queued request after models_update").await,
        ServerToWorkerMessage::Request(worker_protocol::RequestMessage {
            request_id: "request-1".to_string(),
            model: "mistral-large".to_string(),
            endpoint_path: "/v1/chat/completions".to_string(),
            is_streaming: false,
            body: r#"{"model":"mistral-large","messages":[{"role":"user","content":"route after models_update"}]}"#.to_string(),
            headers: HeaderMap::new(),
        })
    );

    let core = core.lock().await;
    assert_eq!(
        core.provider_models("openai"),
        vec!["llama-3.1-70b".to_string(), "mistral-large".to_string()]
    );
    assert!(core.queued_request_ids("openai").is_empty());
    assert_eq!(
        core.request_state("request-1"),
        Some(RequestState::InFlight {
            worker_id: ack.worker_id.clone(),
            cancellation: None,
        })
    );
    assert_eq!(
        core.worker_in_flight_request_ids(&ack.worker_id),
        vec!["request-1".to_string()]
    );
}

#[tokio::test]
async fn streaming_response_chunks_preserve_in_flight_request_until_response_complete() {
    let (addr, core) = spawn_server().await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");
    let ack = register_test_worker(&mut socket).await;

    let second_headers =
        HeaderMap::from([("x-trace-id".to_string(), "trace-stream-next".to_string())]);

    {
        let mut core = core.lock().await;
        assert_eq!(
            core.submit_transport_request(
                "openai",
                "llama-3.1-70b",
                "/v1/chat/completions",
                true,
                r#"{"model":"llama-3.1-70b","stream":true,"messages":[{"role":"user","content":"stream"}]}"#,
                HeaderMap::new(),
            ),
            SubmissionOutcome::Dispatched(proxy_server::DispatchAssignment {
                request_id: "request-1".to_string(),
                worker_id: ack.worker_id.clone(),
            })
        );
        assert_eq!(
            core.submit_transport_request(
                "openai",
                "llama-3.1-70b",
                "/v1/chat/completions",
                false,
                r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"after-stream"}]}"#,
                second_headers.clone(),
            ),
            SubmissionOutcome::Queued(proxy_server::QueuedAssignment {
                request_id: "request-2".to_string(),
                queue_len: 1,
            })
        );
    }

    assert_eq!(
        next_server_message(&mut socket, "streaming dispatched request").await,
        ServerToWorkerMessage::Request(worker_protocol::RequestMessage {
            request_id: "request-1".to_string(),
            model: "llama-3.1-70b".to_string(),
            endpoint_path: "/v1/chat/completions".to_string(),
            is_streaming: true,
            body: r#"{"model":"llama-3.1-70b","stream":true,"messages":[{"role":"user","content":"stream"}]}"#.to_string(),
            headers: HeaderMap::new(),
        })
    );

    send_response_chunk(&mut socket, "request-1", "data: {\"delta\":\"hel\"}\n\n").await;
    send_response_chunk(&mut socket, "request-1", "data: {\"delta\":\"lo\"}\n\n").await;

    assert_no_non_heartbeat_message(&mut socket, std::time::Duration::from_millis(150)).await;

    {
        let core = core.lock().await;
        assert_eq!(
            core.request_state("request-1"),
            Some(RequestState::InFlight {
                worker_id: ack.worker_id.clone(),
                cancellation: None,
            })
        );
        assert_eq!(
            core.queued_request_ids("openai"),
            vec!["request-2".to_string()]
        );
        assert_eq!(
            core.worker_in_flight_request_ids("worker-1"),
            vec!["request-1".to_string()]
        );
    }

    send_response_complete(&mut socket, "request-1").await;
    assert_eq!(
        next_server_message(&mut socket, "post-stream queued dispatch").await,
        expected_request_message(
            "request-2",
            r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"after-stream"}]}"#,
            second_headers,
        )
    );

    let core = core.lock().await;
    assert_eq!(core.request_state("request-1"), None);
    assert_eq!(
        core.request_state("request-2"),
        Some(RequestState::InFlight {
            worker_id: ack.worker_id,
            cancellation: None,
        })
    );
}

#[tokio::test]
async fn canceling_in_flight_request_emits_single_worker_cancel_until_response_complete() {
    let (addr, core) = spawn_server().await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");
    let ack = register_test_worker(&mut socket).await;

    let second_headers =
        HeaderMap::from([("x-trace-id".to_string(), "trace-after-cancel".to_string())]);

    queue_cancel_test_requests(&core, &ack.worker_id, &second_headers).await;

    assert_eq!(
        next_server_message(&mut socket, "initial dispatched request").await,
        expected_request_message(
            "request-1",
            r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"cancel me"}]}"#,
            HeaderMap::new(),
        )
    );

    cancel_in_flight_request(&core, &ack.worker_id, ProxyCancelReason::ClientDisconnected).await;
    assert_canceled_request_state(&core, &ack.worker_id, ProxyCancelReason::ClientDisconnected)
        .await;

    assert_eq!(
        next_server_message(&mut socket, "worker cancel").await,
        ServerToWorkerMessage::Cancel(CancelMessage {
            request_id: "request-1".to_string(),
            reason: ProtocolCancelReason::ClientDisconnect,
        })
    );

    assert_no_non_heartbeat_message(&mut socket, std::time::Duration::from_millis(150)).await;

    assert_canceled_request_state(&core, &ack.worker_id, ProxyCancelReason::ClientDisconnected)
        .await;

    send_response_complete(&mut socket, "request-1").await;
    assert_eq!(
        next_server_message(&mut socket, "post-cancel queued dispatch").await,
        expected_request_message(
            "request-2",
            r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"after cancel"}]}"#,
            second_headers,
        )
    );

    let core = core.lock().await;
    assert_eq!(core.request_state("request-1"), None);
    assert_eq!(
        core.request_state("request-2"),
        Some(RequestState::InFlight {
            worker_id: ack.worker_id,
            cancellation: None,
        })
    );
}

#[tokio::test]
async fn timing_out_in_flight_request_emits_single_worker_cancel_until_response_complete() {
    let (addr, core) = spawn_server().await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");
    let ack = register_test_worker(&mut socket).await;

    let second_headers =
        HeaderMap::from([("x-trace-id".to_string(), "trace-after-timeout".to_string())]);

    queue_cancel_test_requests(&core, &ack.worker_id, &second_headers).await;

    assert_eq!(
        next_server_message(&mut socket, "initial dispatched request").await,
        expected_request_message(
            "request-1",
            r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"cancel me"}]}"#,
            HeaderMap::new(),
        )
    );

    cancel_in_flight_request(&core, &ack.worker_id, ProxyCancelReason::RequestTimedOut).await;
    assert_canceled_request_state(&core, &ack.worker_id, ProxyCancelReason::RequestTimedOut).await;

    assert_eq!(
        next_server_message(&mut socket, "worker timeout cancel").await,
        ServerToWorkerMessage::Cancel(CancelMessage {
            request_id: "request-1".to_string(),
            reason: ProtocolCancelReason::Timeout,
        })
    );

    assert_no_non_heartbeat_message(&mut socket, std::time::Duration::from_millis(150)).await;

    assert_canceled_request_state(&core, &ack.worker_id, ProxyCancelReason::RequestTimedOut).await;

    send_response_complete(&mut socket, "request-1").await;
    assert_eq!(
        next_server_message(&mut socket, "post-timeout queued dispatch").await,
        expected_request_message(
            "request-2",
            r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"after cancel"}]}"#,
            second_headers,
        )
    );

    let core = core.lock().await;
    assert_eq!(core.request_state("request-1"), None);
    assert_eq!(
        core.request_state("request-2"),
        Some(RequestState::InFlight {
            worker_id: ack.worker_id,
            cancellation: None,
        })
    );
}

#[tokio::test]
async fn heartbeat_ping_pong_updates_live_worker_load_without_reconnect_or_early_dispatch() {
    let (addr, core) = spawn_server().await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");
    let ack =
        register_test_worker_with(&mut socket, vec!["llama-3.1-70b".to_string()], 2, Some(0)).await;

    submit_heartbeat_initial_request(&core, &ack.worker_id).await;

    assert_eq!(
        next_server_message(&mut socket, "initial request before heartbeat").await,
        expected_request_message(
            "request-1",
            r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"heartbeat me"}]}"#,
            HeaderMap::new(),
        )
    );

    assert_heartbeat_ping(&mut socket).await;

    let pong = WorkerToServerMessage::Pong(PongMessage {
        current_load: 2,
        timestamp_unix_ms: None,
    });
    socket
        .send(Message::Text(
            serde_json::to_string(&pong).expect("serialize pong").into(),
        ))
        .await
        .expect("send pong");

    wait_for_worker_reported_load(&core, &ack.worker_id, 2).await;

    {
        let core = core.lock().await;
        assert!(core.has_worker(&ack.worker_id));
        assert_eq!(core.worker_reported_load(&ack.worker_id), Some(2));
        assert_eq!(
            core.request_state("request-1"),
            Some(RequestState::InFlight {
                worker_id: ack.worker_id.clone(),
                cancellation: None,
            })
        );
    }

    submit_heartbeat_follow_up_request(&core).await;

    assert_no_non_heartbeat_message(&mut socket, std::time::Duration::from_millis(150)).await;

    {
        let core = core.lock().await;
        assert_eq!(
            core.queued_request_ids("openai"),
            vec!["request-2".to_string()]
        );
        assert_eq!(
            core.worker_in_flight_request_ids(&ack.worker_id),
            vec!["request-1".to_string()]
        );
    }

    send_response_complete(&mut socket, "request-1").await;
    assert_eq!(
        next_server_message(&mut socket, "queued request after heartbeat").await,
        expected_request_message(
            "request-2",
            r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"stay queued until complete"}]}"#,
            HeaderMap::new(),
        )
    );
}

#[tokio::test]
async fn stale_heartbeat_worker_is_disconnected_and_removed_from_routing() {
    let (addr, core) = spawn_server().await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");
    let ack = register_test_worker(&mut socket).await;

    assert_heartbeat_ping(&mut socket).await;

    let close_frame = timeout(
        std::time::Duration::from_millis(350),
        next_close_message(&mut socket, "stale heartbeat disconnect"),
    )
    .await
    .expect("close stale heartbeat worker within pong window");
    assert!(u16::from(close_frame.code) >= 1000);

    wait_for_worker_disconnect(&core, &ack.worker_id).await;

    let mut core = core.lock().await;
    assert_eq!(
        core.submit_transport_request(
            "openai",
            "llama-3.1-70b",
            "/v1/chat/completions",
            false,
            r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"still there?"}]}"#,
            HeaderMap::new(),
        ),
        SubmissionOutcome::Queued(proxy_server::QueuedAssignment {
            request_id: "request-1".to_string(),
            queue_len: 1,
        })
    );
}

#[tokio::test]
async fn graceful_shutdown_drains_in_flight_request_and_disconnects_without_dispatching_queued_work()
 {
    let (addr, core) = spawn_server().await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");
    let ack = register_test_worker(&mut socket).await;

    submit_graceful_shutdown_test_requests(&core, &ack.worker_id).await;

    assert_eq!(
        next_server_message(&mut socket, "initial request before drain").await,
        expected_request_message(
            "request-1",
            r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"finish before drain"}]}"#,
            HeaderMap::new(),
        )
    );

    {
        let mut core = core.lock().await;
        assert_eq!(
            core.begin_graceful_shutdown(
                Some("proxy server shutting down"),
                std::time::Duration::from_millis(250),
            ),
            vec![proxy_server::GracefulShutdownSignal {
                worker_id: ack.worker_id.clone(),
                reason: Some("proxy server shutting down".to_string()),
                drain_timeout: std::time::Duration::from_millis(250),
            }]
        );
        assert!(core.worker_is_draining(&ack.worker_id));
    }

    assert_eq!(
        next_server_message(&mut socket, "graceful shutdown").await,
        ServerToWorkerMessage::GracefulShutdown(GracefulShutdownMessage {
            reason: Some("proxy server shutting down".to_string()),
            drain_timeout_secs: Some(1),
        })
    );

    send_response_complete(&mut socket, "request-1").await;

    let close_frame = next_close_message(&mut socket, "post-drain disconnect").await;
    assert_eq!(u16::from(close_frame.code), 1000);
    assert_eq!(close_frame.reason, "graceful shutdown complete");

    let core = core.lock().await;
    assert!(!core.has_worker(&ack.worker_id));
    assert_eq!(core.request_state("request-1"), None);
    assert_eq!(core.request_state("request-2"), Some(RequestState::Queued));
    assert_eq!(
        core.queued_request_ids("openai"),
        vec!["request-2".to_string()]
    );
}

#[tokio::test]
async fn graceful_shutdown_timeout_disconnects_worker_and_removes_in_flight_request() {
    let (addr, core) = spawn_server().await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");
    let ack = register_test_worker(&mut socket).await;

    {
        let mut core = core.lock().await;
        assert_eq!(
            core.submit_transport_request(
                "openai",
                "llama-3.1-70b",
                "/v1/chat/completions",
                false,
                r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"timeout during drain"}]}"#,
                HeaderMap::new(),
            ),
            SubmissionOutcome::Dispatched(proxy_server::DispatchAssignment {
                request_id: "request-1".to_string(),
                worker_id: ack.worker_id.clone(),
            })
        );
        assert_eq!(
            core.begin_graceful_shutdown(
                Some("proxy server draining"),
                std::time::Duration::from_millis(50),
            ),
            vec![proxy_server::GracefulShutdownSignal {
                worker_id: ack.worker_id.clone(),
                reason: Some("proxy server draining".to_string()),
                drain_timeout: std::time::Duration::from_millis(50),
            }]
        );
    }

    assert_eq!(
        next_server_message(&mut socket, "request before timeout").await,
        expected_request_message(
            "request-1",
            r#"{"model":"llama-3.1-70b","messages":[{"role":"user","content":"timeout during drain"}]}"#,
            HeaderMap::new(),
        )
    );
    assert_eq!(
        next_server_message(&mut socket, "graceful shutdown before timeout").await,
        ServerToWorkerMessage::GracefulShutdown(GracefulShutdownMessage {
            reason: Some("proxy server draining".to_string()),
            drain_timeout_secs: Some(1),
        })
    );

    let close_frame = next_close_message(&mut socket, "drain timeout disconnect").await;
    assert_eq!(u16::from(close_frame.code), 1000);
    assert_eq!(close_frame.reason, "graceful shutdown timed out");

    let core = core.lock().await;
    assert!(!core.has_worker(&ack.worker_id));
    assert_eq!(core.request_state("request-1"), None);
}

#[tokio::test]
async fn server_models_refresh_signal_is_forwarded_to_connected_worker() {
    let (addr, core) = spawn_server().await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");
    let ack = register_test_worker(&mut socket).await;

    {
        let mut core = core.lock().await;
        let signal = core.request_worker_models_refresh(&ack.worker_id, Some("test-refresh"));
        assert!(
            signal.is_some(),
            "expected signal to be queued for connected worker"
        );
    }

    assert_eq!(
        next_server_message(&mut socket, "models refresh signal").await,
        ServerToWorkerMessage::ModelsRefresh(worker_protocol::ModelsRefreshMessage {
            reason: Some("test-refresh".to_string()),
        })
    );
}
