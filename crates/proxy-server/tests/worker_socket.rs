use std::{net::SocketAddr, sync::Arc};

use futures_util::{SinkExt, StreamExt};
use proxy_server::{
    ProxyServerCore, RequestState, SubmissionOutcome, WorkerSocketApp, WorkerSocketProviderConfig,
};
use tokio::{net::TcpListener, sync::Mutex, time::timeout};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use worker_protocol::{
    HeaderMap, RegisterMessage, ResponseCompleteMessage, ServerToWorkerMessage,
    WorkerToServerMessage,
};

type TestSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn spawn_server() -> (SocketAddr, Arc<Mutex<ProxyServerCore>>) {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let app = WorkerSocketApp::new(core.clone())
        .with_provider("openai", WorkerSocketProviderConfig::enabled("top-secret"))
        .router();

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("listener local addr");

    tokio::spawn(async move {
        axum::serve(listener, app)
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

async fn next_server_message(socket: &mut TestSocket, context: &str) -> ServerToWorkerMessage {
    serde_json::from_str(&next_text_message(socket, context).await)
        .unwrap_or_else(|_| panic!("deserialize {context}"))
}

async fn register_test_worker(socket: &mut TestSocket) -> worker_protocol::RegisterAck {
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

    let ack = next_server_message(socket, "register_ack").await;
    let ServerToWorkerMessage::RegisterAck(ack) = ack else {
        panic!("expected register_ack message");
    };

    ack
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

#[tokio::test]
async fn authenticated_worker_can_register_and_receive_register_ack() {
    let (addr, core) = spawn_server().await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");

    let register = WorkerToServerMessage::Register(RegisterMessage {
        worker_name: "gpu-box-a".to_string(),
        models: vec!["llama-3.1-70b".to_string(), " mistral-large ".to_string()],
        max_concurrent: 2,
        protocol_version: Some("2026-04-bridge-v1".to_string()),
        current_load: Some(0),
    });
    let register_payload = serde_json::to_string(&register).expect("serialize register");

    socket
        .send(Message::Text(register_payload.into()))
        .await
        .expect("send register");

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
