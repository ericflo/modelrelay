use std::{convert::Infallible, fmt::Write as _, net::SocketAddr, sync::Arc};

use axum::{
    Router,
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use futures_util::stream;
use proxy_server::{
    ProxyHttpApp, ProxyServerCore, RequestState, WorkerSocketApp, WorkerSocketProviderConfig,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Mutex, oneshot},
    time::{Duration, timeout},
};
use worker_daemon::{WorkerDaemon, WorkerDaemonConfig};

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservedBackendRequest {
    path: String,
    authorization: Option<String>,
    openai_organization: Option<String>,
    x_api_key: Option<String>,
    anthropic_version: Option<String>,
    anthropic_beta: Option<String>,
    content_type: Option<String>,
    body: String,
}

#[derive(Clone)]
struct BackendState {
    observed_request_tx: Arc<Mutex<Option<oneshot::Sender<ObservedBackendRequest>>>>,
}

#[derive(Clone)]
struct ControlledChatBackendState {
    observed_request_tx: Arc<Mutex<Option<oneshot::Sender<ObservedBackendRequest>>>>,
    response_gate_rx: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
    response_status: StatusCode,
    response_trace: &'static str,
    response_body: &'static str,
}

async fn spawn_proxy_server(models_provider: &str) -> (SocketAddr, Arc<Mutex<ProxyServerCore>>) {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let worker_socket_app = WorkerSocketApp::new(core.clone())
        .with_provider(
            "anthropic",
            WorkerSocketProviderConfig::enabled("top-secret"),
        )
        .with_provider("openai", WorkerSocketProviderConfig::enabled("top-secret"));
    let app = ProxyHttpApp::new(core.clone())
        .with_models_provider(models_provider)
        .with_worker_socket_app(worker_socket_app)
        .router();

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind proxy listener");
    let addr = listener.local_addr().expect("proxy listener local addr");

    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve proxy app");
    });

    (addr, core)
}

async fn spawn_mock_backend() -> (SocketAddr, oneshot::Receiver<ObservedBackendRequest>) {
    let (observed_request_tx, observed_request_rx) = oneshot::channel();
    let state = BackendState {
        observed_request_tx: Arc::new(Mutex::new(Some(observed_request_tx))),
    };
    let app = Router::new()
        .route("/v1/chat/completions", post(mock_chat_completions_handler))
        .route("/v1/messages", post(mock_messages_handler))
        .with_state(state);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind backend listener");
    let addr = listener.local_addr().expect("backend listener local addr");

    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve backend app");
    });

    (addr, observed_request_rx)
}

async fn spawn_controlled_chat_backend(
    response_status: StatusCode,
    response_trace: &'static str,
    response_body: &'static str,
    response_gate_rx: Option<oneshot::Receiver<()>>,
) -> (SocketAddr, oneshot::Receiver<ObservedBackendRequest>) {
    let (observed_request_tx, observed_request_rx) = oneshot::channel();
    let state = ControlledChatBackendState {
        observed_request_tx: Arc::new(Mutex::new(Some(observed_request_tx))),
        response_gate_rx: Arc::new(Mutex::new(response_gate_rx)),
        response_status,
        response_trace,
        response_body,
    };
    let app = Router::new()
        .route(
            "/v1/chat/completions",
            post(controlled_chat_completions_handler),
        )
        .with_state(state);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind controlled backend listener");
    let addr = listener
        .local_addr()
        .expect("controlled backend listener local addr");

    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve controlled backend app");
    });

    (addr, observed_request_rx)
}

async fn mock_chat_completions_handler(
    State(state): State<BackendState>,
    headers: HeaderMap,
    body: String,
) -> impl IntoResponse {
    let is_streaming = body.contains(r#""stream":true"#);

    if let Some(observed_request_tx) = state.observed_request_tx.lock().await.take() {
        let _ = observed_request_tx.send(ObservedBackendRequest {
            path: "/v1/chat/completions".to_string(),
            authorization: header_value(&headers, "authorization"),
            openai_organization: header_value(&headers, "openai-organization"),
            x_api_key: header_value(&headers, "x-api-key"),
            anthropic_version: header_value(&headers, "anthropic-version"),
            anthropic_beta: header_value(&headers, "anthropic-beta"),
            content_type: header_value(&headers, "content-type"),
            body,
        });
    }

    if is_streaming {
        let chunks = [
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
            "data: [DONE]\n\n",
        ];
        let stream = stream::unfold(0_usize, move |index| async move {
            let chunk = chunks.get(index)?;

            tokio::time::sleep(Duration::from_millis(10)).await;
            Some((
                Ok::<Bytes, Infallible>(Bytes::from_static(chunk.as_bytes())),
                index + 1,
            ))
        });

        let mut response = Response::new(Body::from_stream(stream));
        *response.status_mut() = StatusCode::OK;
        response.headers_mut().insert(
            "content-type",
            "text/event-stream".parse().expect("parse content-type"),
        );
        return response;
    }

    (
        StatusCode::ACCEPTED,
        [
            ("content-type", "application/json"),
            ("x-backend-trace", "mock-backend"),
        ],
        r#"{"id":"resp_123","object":"chat.completion","model":"gpt-4.1-mini","choices":[{"index":0,"message":{"role":"assistant","content":"proxy success"},"finish_reason":"stop"}]}"#,
    )
        .into_response()
}

async fn mock_messages_handler(
    State(state): State<BackendState>,
    headers: HeaderMap,
    body: String,
) -> impl IntoResponse {
    let is_streaming = body.contains(r#""stream":true"#);

    if let Some(observed_request_tx) = state.observed_request_tx.lock().await.take() {
        let _ = observed_request_tx.send(ObservedBackendRequest {
            path: "/v1/messages".to_string(),
            authorization: header_value(&headers, "authorization"),
            openai_organization: header_value(&headers, "openai-organization"),
            x_api_key: header_value(&headers, "x-api-key"),
            anthropic_version: header_value(&headers, "anthropic-version"),
            anthropic_beta: header_value(&headers, "anthropic-beta"),
            content_type: header_value(&headers, "content-type"),
            body,
        });
    }

    if is_streaming {
        let chunks = [
            "event: message_start\ndata: {\"type\":\"message_start\"}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ];
        let stream = stream::unfold(0_usize, move |index| async move {
            let chunk = chunks.get(index)?;

            tokio::time::sleep(Duration::from_millis(10)).await;
            Some((
                Ok::<Bytes, Infallible>(Bytes::from_static(chunk.as_bytes())),
                index + 1,
            ))
        });

        let mut response = Response::new(Body::from_stream(stream));
        *response.status_mut() = StatusCode::OK;
        response.headers_mut().insert(
            "content-type",
            "text/event-stream".parse().expect("parse content-type"),
        );
        response.headers_mut().insert(
            "anthropic-beta",
            "tools-2024-04-04"
                .parse()
                .expect("parse anthropic-beta header"),
        );
        response.headers_mut().insert(
            "x-backend-trace",
            "mock-anthropic-backend"
                .parse()
                .expect("parse x-backend-trace header"),
        );
        return response;
    }

    (
        StatusCode::OK,
        [
            ("content-type", "application/json"),
            ("anthropic-beta", "tools-2024-04-04"),
            ("x-backend-trace", "mock-anthropic-backend"),
        ],
        r#"{"id":"msg_123","type":"message","role":"assistant","content":[{"type":"text","text":"anthropic success"}]}"#,
    )
        .into_response()
}

async fn controlled_chat_completions_handler(
    State(state): State<ControlledChatBackendState>,
    headers: HeaderMap,
    body: String,
) -> impl IntoResponse {
    if let Some(observed_request_tx) = state.observed_request_tx.lock().await.take() {
        let _ = observed_request_tx.send(ObservedBackendRequest {
            path: "/v1/chat/completions".to_string(),
            authorization: header_value(&headers, "authorization"),
            openai_organization: header_value(&headers, "openai-organization"),
            x_api_key: header_value(&headers, "x-api-key"),
            anthropic_version: header_value(&headers, "anthropic-version"),
            anthropic_beta: header_value(&headers, "anthropic-beta"),
            content_type: header_value(&headers, "content-type"),
            body,
        });
    }

    if let Some(response_gate_rx) = state.response_gate_rx.lock().await.take() {
        let _ = response_gate_rx.await;
    }

    (
        state.response_status,
        [
            ("content-type", "application/json"),
            ("x-backend-trace", state.response_trace),
        ],
        state.response_body,
    )
        .into_response()
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
}

async fn get_models(addr: SocketAddr) -> (u16, Value) {
    let mut stream = TcpStream::connect(addr)
        .await
        .expect("connect to proxy server");
    let request = format!("GET /v1/models HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");

    stream
        .write_all(request.as_bytes())
        .await
        .expect("write models request");

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("read models response");

    parse_json_response(&response)
}

async fn post_chat_completions(addr: SocketAddr, body: &str, headers: &[(&str, &str)]) -> String {
    let mut stream = open_chat_completions_request(addr, body, headers).await;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("read chat completions response");

    String::from_utf8(response).expect("proxy response is utf-8")
}

async fn post_messages(addr: SocketAddr, body: &str, headers: &[(&str, &str)]) -> String {
    let mut stream = open_messages_request(addr, body, headers).await;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("read messages response");

    String::from_utf8(response).expect("proxy response is utf-8")
}

async fn open_chat_completions_request(
    addr: SocketAddr,
    body: &str,
    headers: &[(&str, &str)],
) -> TcpStream {
    let mut stream = TcpStream::connect(addr)
        .await
        .expect("connect to proxy server");
    let mut request = format!(
        "POST /v1/chat/completions HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        write!(request, "{name}: {value}\r\n").expect("append request header");
    }
    request.push_str("\r\n");
    request.push_str(body);

    stream
        .write_all(request.as_bytes())
        .await
        .expect("write chat completions request");

    stream
}

async fn open_messages_request(
    addr: SocketAddr,
    body: &str,
    headers: &[(&str, &str)],
) -> TcpStream {
    let mut stream = TcpStream::connect(addr)
        .await
        .expect("connect to proxy server");
    let mut request = format!(
        "POST /v1/messages HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        write!(request, "{name}: {value}\r\n").expect("append request header");
    }
    request.push_str("\r\n");
    request.push_str(body);

    stream
        .write_all(request.as_bytes())
        .await
        .expect("write messages request");

    stream
}

fn parse_json_response(response: &[u8]) -> (u16, Value) {
    let response = String::from_utf8(response.to_vec()).expect("response is utf-8");
    let (head, body) = response
        .split_once("\r\n\r\n")
        .expect("split response head and body");
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .expect("parse response status");

    (
        status,
        serde_json::from_str(body.trim()).expect("parse json response body"),
    )
}

async fn read_until_contains(stream: &mut TcpStream, needle: &str) -> String {
    let mut response = Vec::new();

    loop {
        if String::from_utf8_lossy(&response).contains(needle) {
            return String::from_utf8(response).expect("http response is utf-8");
        }

        let mut chunk = [0_u8; 1024];
        let read = timeout(Duration::from_secs(2), stream.read(&mut chunk))
            .await
            .expect("read response chunk before timeout")
            .expect("read response chunk");
        assert!(read > 0, "response closed before expected bytes arrived");
        response.extend_from_slice(&chunk[..read]);
    }
}

async fn wait_for_registered_model(addr: SocketAddr, expected_model: &str) {
    timeout(Duration::from_secs(2), async {
        loop {
            let (_, body) = get_models(addr).await;
            let models = body["data"]
                .as_array()
                .expect("models array")
                .iter()
                .filter_map(|entry| entry["id"].as_str())
                .collect::<Vec<_>>();
            if models.contains(&expected_model) {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("model {expected_model} was not registered"));
}

async fn wait_for_request_state(
    core: &Arc<Mutex<ProxyServerCore>>,
    request_id: &str,
    expected_state: RequestState,
) {
    let expected_state_for_loop = expected_state.clone();
    timeout(Duration::from_secs(2), async {
        loop {
            let state = {
                let core = core.lock().await;
                core.request_state(request_id)
            };
            if state == Some(expected_state_for_loop.clone()) {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("request {request_id} did not reach {expected_state:?}"));
}

async fn wait_for_queued_request_id(core: &Arc<Mutex<ProxyServerCore>>, provider: &str) -> String {
    timeout(Duration::from_secs(2), async {
        loop {
            let queued_request_ids = {
                let core = core.lock().await;
                core.queued_request_ids(provider)
            };
            if let Some(request_id) = queued_request_ids.into_iter().next() {
                return request_id;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("provider {provider} never queued a request"))
}

#[tokio::test]
async fn worker_daemon_forwards_non_streaming_openai_request_through_live_proxy() {
    let (proxy_addr, _core) = spawn_proxy_server("openai").await;
    let (backend_addr, observed_request_rx) = spawn_mock_backend().await;

    let daemon = WorkerDaemon::new(WorkerDaemonConfig {
        proxy_base_url: format!("http://{proxy_addr}"),
        provider: "openai".to_string(),
        worker_secret: "top-secret".to_string(),
        worker_name: "gpu-box-a".to_string(),
        models: vec!["gpt-4.1-mini".to_string()],
        max_concurrent: 1,
        backend_base_url: format!("http://{backend_addr}"),
    });

    let daemon_handle = tokio::spawn(async move { daemon.run().await });

    wait_for_registered_model(proxy_addr, "gpt-4.1-mini").await;

    let request_body =
        r#"{"model":"gpt-4.1-mini","messages":[{"role":"user","content":"hello from proxy"}]}"#;
    let response = post_chat_completions(
        proxy_addr,
        request_body,
        &[
            ("Authorization", "Bearer client-token"),
            ("OpenAI-Organization", "org-demo"),
        ],
    )
    .await;

    let observed_request = timeout(Duration::from_secs(2), observed_request_rx)
        .await
        .expect("backend observed request before timeout")
        .expect("backend observed request");

    assert_eq!(
        observed_request,
        ObservedBackendRequest {
            path: "/v1/chat/completions".to_string(),
            authorization: Some("Bearer client-token".to_string()),
            openai_organization: Some("org-demo".to_string()),
            x_api_key: None,
            anthropic_version: None,
            anthropic_beta: None,
            content_type: Some("application/json".to_string()),
            body: request_body.to_string(),
        }
    );
    assert!(response.starts_with("HTTP/1.1 202 Accepted\r\n"));
    assert!(response.contains("\r\ncontent-type: application/json\r\n"));
    assert!(response.contains("\r\nx-backend-trace: mock-backend\r\n"));
    assert!(response.ends_with(
        "{\"id\":\"resp_123\",\"object\":\"chat.completion\",\"model\":\"gpt-4.1-mini\",\"choices\":[{\"index\":0,\"message\":{\"role\":\"assistant\",\"content\":\"proxy success\"},\"finish_reason\":\"stop\"}]}"
    ));

    let (status, models_body) = get_models(proxy_addr).await;
    assert_eq!(status, 200);
    assert_eq!(
        models_body,
        json!({
            "object": "list",
            "data": [
                {
                    "id": "gpt-4.1-mini",
                    "object": "model",
                    "owned_by": "worker-proxy"
                }
            ]
        })
    );

    daemon_handle.abort();
}

#[tokio::test]
async fn worker_daemon_forwards_streaming_openai_request_through_live_proxy() {
    let (proxy_addr, _core) = spawn_proxy_server("openai").await;
    let (backend_addr, observed_request_rx) = spawn_mock_backend().await;

    let daemon = WorkerDaemon::new(WorkerDaemonConfig {
        proxy_base_url: format!("http://{proxy_addr}"),
        provider: "openai".to_string(),
        worker_secret: "top-secret".to_string(),
        worker_name: "gpu-box-a".to_string(),
        models: vec!["gpt-4.1-mini".to_string()],
        max_concurrent: 1,
        backend_base_url: format!("http://{backend_addr}"),
    });

    let daemon_handle = tokio::spawn(async move { daemon.run().await });

    wait_for_registered_model(proxy_addr, "gpt-4.1-mini").await;

    let request_body = r#"{"model":"gpt-4.1-mini","stream":true,"messages":[{"role":"user","content":"hello from proxy"}]}"#;
    let mut response_stream = open_chat_completions_request(
        proxy_addr,
        request_body,
        &[
            ("Authorization", "Bearer client-token"),
            ("OpenAI-Organization", "org-demo"),
        ],
    )
    .await;

    let first_fragment = read_until_contains(
        &mut response_stream,
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
    )
    .await;

    let observed_request = timeout(Duration::from_secs(2), observed_request_rx)
        .await
        .expect("backend observed request before timeout")
        .expect("backend observed request");

    assert_eq!(
        observed_request,
        ObservedBackendRequest {
            path: "/v1/chat/completions".to_string(),
            authorization: Some("Bearer client-token".to_string()),
            openai_organization: Some("org-demo".to_string()),
            x_api_key: None,
            anthropic_version: None,
            anthropic_beta: None,
            content_type: Some("application/json".to_string()),
            body: request_body.to_string(),
        }
    );
    assert!(first_fragment.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(first_fragment.contains("\r\ncontent-type: text/event-stream\r\n"));
    assert!(first_fragment.contains("data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n"));
    assert!(!first_fragment.contains("data: [DONE]\n\n"));

    let mut rest = Vec::new();
    response_stream
        .read_to_end(&mut rest)
        .await
        .expect("read streaming chat completions response");
    let full_response = first_fragment + &String::from_utf8(rest).expect("proxy response is utf-8");

    let hello_index = full_response
        .find("data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n")
        .expect("find first streamed chunk");
    let lo_index = full_response
        .find("data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n")
        .expect("find second streamed chunk");
    let done_index = full_response
        .find("data: [DONE]\n\n")
        .expect("find done marker");

    assert!(hello_index < lo_index);
    assert!(lo_index < done_index);
    assert!(full_response.ends_with("0\r\n\r\n"));

    daemon_handle.abort();
}

#[tokio::test]
async fn worker_daemon_forwards_anthropic_messages_request_through_live_proxy() {
    let (proxy_addr, _core) = spawn_proxy_server("anthropic").await;
    let (backend_addr, observed_request_rx) = spawn_mock_backend().await;

    let daemon = WorkerDaemon::new(WorkerDaemonConfig {
        proxy_base_url: format!("http://{proxy_addr}"),
        provider: "anthropic".to_string(),
        worker_secret: "top-secret".to_string(),
        worker_name: "gpu-box-a".to_string(),
        models: vec!["claude-3-5-sonnet-20241022".to_string()],
        max_concurrent: 1,
        backend_base_url: format!("http://{backend_addr}"),
    });

    let daemon_handle = tokio::spawn(async move { daemon.run().await });

    wait_for_registered_model(proxy_addr, "claude-3-5-sonnet-20241022").await;

    let request_body = r#"{"model":"claude-3-5-sonnet-20241022","max_tokens":64,"messages":[{"role":"user","content":"hello from proxy"}]}"#;
    let response = post_messages(
        proxy_addr,
        request_body,
        &[
            ("x-api-key", "test-anthropic-key"),
            ("anthropic-version", "2023-06-01"),
            ("anthropic-beta", "tools-2024-04-04"),
        ],
    )
    .await;

    let observed_request = timeout(Duration::from_secs(2), observed_request_rx)
        .await
        .expect("backend observed request before timeout")
        .expect("backend observed request");

    assert_eq!(
        observed_request,
        ObservedBackendRequest {
            path: "/v1/messages".to_string(),
            authorization: None,
            openai_organization: None,
            x_api_key: Some("test-anthropic-key".to_string()),
            anthropic_version: Some("2023-06-01".to_string()),
            anthropic_beta: Some("tools-2024-04-04".to_string()),
            content_type: Some("application/json".to_string()),
            body: request_body.to_string(),
        }
    );
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.contains("\r\ncontent-type: application/json\r\n"));
    assert!(response.contains("\r\nanthropic-beta: tools-2024-04-04\r\n"));
    assert!(response.contains("\r\nx-backend-trace: mock-anthropic-backend\r\n"));
    assert!(response.ends_with(
        r#"{"id":"msg_123","type":"message","role":"assistant","content":[{"type":"text","text":"anthropic success"}]}"#
    ));

    daemon_handle.abort();
}

#[tokio::test]
async fn worker_daemon_forwards_anthropic_streaming_request_through_live_proxy() {
    let (proxy_addr, _core) = spawn_proxy_server("anthropic").await;
    let (backend_addr, observed_request_rx) = spawn_mock_backend().await;

    let daemon = WorkerDaemon::new(WorkerDaemonConfig {
        proxy_base_url: format!("http://{proxy_addr}"),
        provider: "anthropic".to_string(),
        worker_secret: "top-secret".to_string(),
        worker_name: "gpu-box-a".to_string(),
        models: vec!["claude-3-5-sonnet-20241022".to_string()],
        max_concurrent: 1,
        backend_base_url: format!("http://{backend_addr}"),
    });

    let daemon_handle = tokio::spawn(async move { daemon.run().await });

    wait_for_registered_model(proxy_addr, "claude-3-5-sonnet-20241022").await;

    let request_body = r#"{"model":"claude-3-5-sonnet-20241022","stream":true,"max_tokens":64,"messages":[{"role":"user","content":"hello from proxy"}]}"#;
    let mut response_stream = open_messages_request(
        proxy_addr,
        request_body,
        &[
            ("x-api-key", "test-anthropic-key"),
            ("anthropic-version", "2023-06-01"),
            ("anthropic-beta", "tools-2024-04-04"),
        ],
    )
    .await;

    let first_fragment = read_until_contains(
        &mut response_stream,
        "event: message_start\ndata: {\"type\":\"message_start\"}\n\n",
    )
    .await;

    let observed_request = timeout(Duration::from_secs(2), observed_request_rx)
        .await
        .expect("backend observed request before timeout")
        .expect("backend observed request");

    assert_eq!(
        observed_request,
        ObservedBackendRequest {
            path: "/v1/messages".to_string(),
            authorization: None,
            openai_organization: None,
            x_api_key: Some("test-anthropic-key".to_string()),
            anthropic_version: Some("2023-06-01".to_string()),
            anthropic_beta: Some("tools-2024-04-04".to_string()),
            content_type: Some("application/json".to_string()),
            body: request_body.to_string(),
        }
    );
    assert!(first_fragment.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(first_fragment.contains("\r\ncontent-type: text/event-stream\r\n"));
    assert!(
        first_fragment.contains("event: message_start\ndata: {\"type\":\"message_start\"}\n\n")
    );
    assert!(!first_fragment.contains("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"));

    let mut rest = Vec::new();
    response_stream
        .read_to_end(&mut rest)
        .await
        .expect("read streaming anthropic messages response");
    let full_response = first_fragment + &String::from_utf8(rest).expect("proxy response is utf-8");

    let message_start_index = full_response
        .find("event: message_start\ndata: {\"type\":\"message_start\"}\n\n")
        .expect("find message_start event");
    let content_delta_index = full_response
        .find(
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
        )
        .expect("find content_block_delta event");
    let message_stop_index = full_response
        .find("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n")
        .expect("find message_stop event");

    assert!(message_start_index < content_delta_index);
    assert!(content_delta_index < message_stop_index);
    assert!(full_response.ends_with("0\r\n\r\n"));

    daemon_handle.abort();
}

#[tokio::test]
async fn worker_daemon_requeues_live_request_after_first_daemon_disconnect() {
    let (proxy_addr, core) = spawn_proxy_server("openai").await;

    let (first_backend_release_tx, first_backend_release_rx) = oneshot::channel();
    let (first_backend_addr, first_observed_request_rx) = spawn_controlled_chat_backend(
        StatusCode::OK,
        "first-backend",
        r#"{"id":"chatcmpl-first","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"should not arrive"},"finish_reason":"stop"}]}"#,
        Some(first_backend_release_rx),
    )
    .await;
    let first_daemon = WorkerDaemon::new(WorkerDaemonConfig {
        proxy_base_url: format!("http://{proxy_addr}"),
        provider: "openai".to_string(),
        worker_secret: "top-secret".to_string(),
        worker_name: "gpu-box-a".to_string(),
        models: vec!["gpt-4.1-mini".to_string()],
        max_concurrent: 1,
        backend_base_url: format!("http://{first_backend_addr}"),
    });
    let first_daemon_handle = tokio::spawn(async move { first_daemon.run().await });

    wait_for_registered_model(proxy_addr, "gpt-4.1-mini").await;

    let request_body = r#"{"model":"gpt-4.1-mini","messages":[{"role":"user","content":"finish after daemon reconnect"}]}"#;
    let http_request = tokio::spawn(post_chat_completions(
        proxy_addr,
        request_body,
        &[("Authorization", "Bearer client-token")],
    ));

    let first_observed_request = timeout(Duration::from_secs(2), first_observed_request_rx)
        .await
        .expect("first backend observed request before timeout")
        .expect("first backend observed request");
    assert_eq!(
        first_observed_request,
        ObservedBackendRequest {
            path: "/v1/chat/completions".to_string(),
            authorization: Some("Bearer client-token".to_string()),
            openai_organization: None,
            x_api_key: None,
            anthropic_version: None,
            anthropic_beta: None,
            content_type: Some("application/json".to_string()),
            body: request_body.to_string(),
        }
    );

    first_daemon_handle.abort();

    let request_id = wait_for_queued_request_id(&core, "openai").await;
    wait_for_request_state(&core, &request_id, RequestState::Queued).await;

    let (replacement_backend_addr, replacement_observed_request_rx) = spawn_controlled_chat_backend(
        StatusCode::OK,
        "replacement-backend",
        r#"{"id":"chatcmpl-requeued","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"replacement worker success"},"finish_reason":"stop"}]}"#,
        None,
    )
    .await;
    let replacement_daemon = WorkerDaemon::new(WorkerDaemonConfig {
        proxy_base_url: format!("http://{proxy_addr}"),
        provider: "openai".to_string(),
        worker_secret: "top-secret".to_string(),
        worker_name: "gpu-box-b".to_string(),
        models: vec!["gpt-4.1-mini".to_string()],
        max_concurrent: 1,
        backend_base_url: format!("http://{replacement_backend_addr}"),
    });
    let replacement_daemon_handle = tokio::spawn(async move { replacement_daemon.run().await });

    let replacement_observed_request =
        timeout(Duration::from_secs(5), replacement_observed_request_rx)
            .await
            .expect("replacement backend observed request before timeout")
            .expect("replacement backend observed request");
    assert_eq!(
        replacement_observed_request,
        ObservedBackendRequest {
            path: "/v1/chat/completions".to_string(),
            authorization: Some("Bearer client-token".to_string()),
            openai_organization: None,
            x_api_key: None,
            anthropic_version: None,
            anthropic_beta: None,
            content_type: Some("application/json".to_string()),
            body: request_body.to_string(),
        }
    );

    let response = timeout(Duration::from_secs(5), http_request)
        .await
        .expect("http request completed before timeout")
        .expect("join http request task");
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.contains("\r\ncontent-type: application/json\r\n"));
    assert!(response.contains("\r\nx-backend-trace: replacement-backend\r\n"));
    assert!(response.ends_with(
        r#"{"id":"chatcmpl-requeued","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"replacement worker success"},"finish_reason":"stop"}]}"#
    ));
    assert!(
        !response.to_ascii_lowercase().contains("disconnect"),
        "the client response should not leak an internal disconnect error: {response}"
    );

    drop(first_backend_release_tx);
    replacement_daemon_handle.abort();
}
