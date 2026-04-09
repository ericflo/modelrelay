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
use modelrelay_server::{
    ApiKeyStore, ProxyHttpApp, ProxyServerCore, RequestState, WorkerSocketApp,
    WorkerSocketProviderConfig,
};
use modelrelay_worker::{WorkerDaemon, WorkerDaemonConfig};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Mutex, oneshot},
    time::{Duration, timeout},
};

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
        .with_provider("openai", WorkerSocketProviderConfig::enabled("top-secret"))
        .with_heartbeat(Duration::from_millis(100), Duration::from_millis(300));
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

async fn spawn_proxy_server_with_api_key_store(
    models_provider: &str,
    api_key_store: Arc<dyn ApiKeyStore>,
) -> (SocketAddr, Arc<Mutex<ProxyServerCore>>) {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let worker_socket_app = WorkerSocketApp::new(core.clone())
        .with_api_key_store(api_key_store.clone())
        .with_heartbeat(Duration::from_millis(100), Duration::from_millis(300));
    let app = ProxyHttpApp::new(core.clone())
        .with_models_provider(models_provider)
        .with_worker_socket_app(worker_socket_app)
        .with_api_key_store(api_key_store)
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

fn assert_observed_openai_chat_request(
    observed_request: &ObservedBackendRequest,
    authorization: Option<&str>,
    body: &str,
) {
    assert_eq!(
        *observed_request,
        ObservedBackendRequest {
            path: "/v1/chat/completions".to_string(),
            authorization: authorization.map(ToOwned::to_owned),
            openai_organization: None,
            x_api_key: None,
            anthropic_version: None,
            anthropic_beta: None,
            content_type: Some("application/json".to_string()),
            body: body.to_string(),
        }
    );
}

fn assert_successful_chat_completion_response(response: &str, trace: &str, body: &str) {
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.contains("\r\ncontent-type: application/json\r\n"));
    assert!(response.contains(&format!("\r\nx-backend-trace: {trace}\r\n")));
    assert!(response.ends_with(body));
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

async fn wait_for_worker_reported_load(
    core: &Arc<Mutex<ProxyServerCore>>,
    worker_id: &str,
    expected_load: usize,
) {
    timeout(Duration::from_secs(2), async {
        loop {
            let reported_load = {
                let core = core.lock().await;
                core.worker_reported_load(worker_id)
            };
            if reported_load == Some(expected_load) {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("worker {worker_id} never reported load {expected_load}"));
}

async fn wait_for_provider_worker_id(core: &Arc<Mutex<ProxyServerCore>>, provider: &str) -> String {
    timeout(Duration::from_secs(2), async {
        loop {
            let worker_id = {
                let core = core.lock().await;
                core.worker_ids_for_provider(provider).into_iter().next()
            };
            if let Some(worker_id) = worker_id {
                return worker_id;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("provider {provider} never registered a worker"))
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
async fn worker_daemon_preserves_non_2xx_openai_backend_response_through_live_proxy() {
    let (proxy_addr, _core) = spawn_proxy_server("openai").await;
    let (backend_addr, observed_request_rx) = spawn_controlled_chat_backend(
        StatusCode::TOO_MANY_REQUESTS,
        "rate-limit-backend",
        r#"{"error":{"message":"backend overloaded","type":"rate_limit_error","code":"too_many_requests"}}"#,
        None,
    )
    .await;

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

    let request_body = r#"{"model":"gpt-4.1-mini","messages":[{"role":"user","content":"trigger backend error"}]}"#;
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
    assert!(response.starts_with("HTTP/1.1 429 Too Many Requests\r\n"));
    assert!(response.contains("\r\ncontent-type: application/json\r\n"));
    assert!(response.contains("\r\nx-backend-trace: rate-limit-backend\r\n"));
    assert!(response.ends_with(
        r#"{"error":{"message":"backend overloaded","type":"rate_limit_error","code":"too_many_requests"}}"#
    ));
    assert!(
        !response.to_ascii_lowercase().contains("worker unavailable"),
        "the proxy should preserve the backend error instead of flattening it: {response}"
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

async fn wait_for_worker_count(core: &Arc<Mutex<ProxyServerCore>>, provider: &str, count: usize) {
    timeout(Duration::from_secs(2), async {
        loop {
            let current = {
                let core = core.lock().await;
                core.worker_ids_for_provider(provider).len()
            };
            if current >= count {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("provider {provider} never reached {count} registered workers"));
}

#[tokio::test]
#[allow(clippy::similar_names, clippy::too_many_lines)]
async fn two_worker_daemons_receive_concurrent_requests_via_load_balanced_routing() {
    let (proxy_addr, core) = spawn_proxy_server("openai").await;

    // Backend A: holds the request until released
    let (backend_a_gate_tx, backend_a_gate_rx) = oneshot::channel::<()>();
    let (backend_a_addr, backend_a_observed_rx) = spawn_controlled_chat_backend(
        StatusCode::OK,
        "worker-a-backend",
        r#"{"id":"chatcmpl-a","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"response from worker a"},"finish_reason":"stop"}]}"#,
        Some(backend_a_gate_rx),
    )
    .await;

    // Backend B: holds the request until released
    let (backend_b_gate_tx, backend_b_gate_rx) = oneshot::channel::<()>();
    let (backend_b_addr, backend_b_observed_rx) = spawn_controlled_chat_backend(
        StatusCode::OK,
        "worker-b-backend",
        r#"{"id":"chatcmpl-b","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"response from worker b"},"finish_reason":"stop"}]}"#,
        Some(backend_b_gate_rx),
    )
    .await;

    // Start both worker daemons, each with capacity=1 pointing to its own backend
    let daemon_a = WorkerDaemon::new(WorkerDaemonConfig {
        proxy_base_url: format!("http://{proxy_addr}"),
        provider: "openai".to_string(),
        worker_secret: "top-secret".to_string(),
        worker_name: "gpu-box-a".to_string(),
        models: vec!["gpt-4.1-mini".to_string()],
        max_concurrent: 1,
        backend_base_url: format!("http://{backend_a_addr}"),
    });
    let daemon_a_handle = tokio::spawn(async move { daemon_a.run().await });

    let daemon_b = WorkerDaemon::new(WorkerDaemonConfig {
        proxy_base_url: format!("http://{proxy_addr}"),
        provider: "openai".to_string(),
        worker_secret: "top-secret".to_string(),
        worker_name: "gpu-box-b".to_string(),
        models: vec!["gpt-4.1-mini".to_string()],
        max_concurrent: 1,
        backend_base_url: format!("http://{backend_b_addr}"),
    });
    let daemon_b_handle = tokio::spawn(async move { daemon_b.run().await });

    // Wait until both workers have registered their model with the proxy
    wait_for_worker_count(&core, "openai", 2).await;
    wait_for_registered_model(proxy_addr, "gpt-4.1-mini").await;

    // Send two concurrent requests; both workers have capacity so neither should queue
    let request_body_1 =
        r#"{"model":"gpt-4.1-mini","messages":[{"role":"user","content":"request for worker a"}]}"#;
    let request_body_2 =
        r#"{"model":"gpt-4.1-mini","messages":[{"role":"user","content":"request for worker b"}]}"#;

    let http_request_1 = tokio::spawn(post_chat_completions(
        proxy_addr,
        request_body_1,
        &[("Authorization", "Bearer client-token")],
    ));
    let http_request_2 = tokio::spawn(post_chat_completions(
        proxy_addr,
        request_body_2,
        &[("Authorization", "Bearer client-token")],
    ));

    // Both backends must each receive exactly one request
    let observed_a = timeout(Duration::from_secs(5), backend_a_observed_rx)
        .await
        .expect("backend A observed its request before timeout")
        .expect("backend A observed request channel intact");
    let observed_b = timeout(Duration::from_secs(5), backend_b_observed_rx)
        .await
        .expect("backend B observed its request before timeout")
        .expect("backend B observed request channel intact");

    assert_eq!(observed_a.path, "/v1/chat/completions");
    assert_eq!(observed_b.path, "/v1/chat/completions");

    // Verify the proxy routed one request in-flight to each worker
    let worker_ids = {
        let core = core.lock().await;
        core.worker_ids_for_provider("openai")
    };
    assert_eq!(
        worker_ids.len(),
        2,
        "expected exactly two workers registered"
    );
    {
        let core = core.lock().await;
        let in_flight_a = core.worker_in_flight_request_ids(&worker_ids[0]);
        let in_flight_b = core.worker_in_flight_request_ids(&worker_ids[1]);
        assert_eq!(
            in_flight_a.len() + in_flight_b.len(),
            2,
            "both requests must be in-flight across the two workers"
        );
        assert!(
            in_flight_a.len() <= 1 && in_flight_b.len() <= 1,
            "each worker must hold at most one request (capacity=1): worker-a={in_flight_a:?}, worker-b={in_flight_b:?}"
        );
        assert_eq!(
            core.queued_request_ids("openai").len(),
            0,
            "no requests should be queued when both workers have capacity"
        );
    }

    // Release both backends and collect the HTTP responses
    let _ = backend_a_gate_tx.send(());
    let _ = backend_b_gate_tx.send(());

    let response_1 = timeout(Duration::from_secs(5), http_request_1)
        .await
        .expect("first HTTP request completed before timeout")
        .expect("join first HTTP request task");
    let response_2 = timeout(Duration::from_secs(5), http_request_2)
        .await
        .expect("second HTTP request completed before timeout")
        .expect("join second HTTP request task");

    assert!(
        response_1.starts_with("HTTP/1.1 200 OK\r\n"),
        "first response should be 200: {response_1}"
    );
    assert!(
        response_2.starts_with("HTTP/1.1 200 OK\r\n"),
        "second response should be 200: {response_2}"
    );

    // The two responses must come from different backends (load-balanced)
    let trace_a_in_1 = response_1.contains("\r\nx-backend-trace: worker-a-backend\r\n");
    let trace_b_in_1 = response_1.contains("\r\nx-backend-trace: worker-b-backend\r\n");
    let trace_a_in_2 = response_2.contains("\r\nx-backend-trace: worker-a-backend\r\n");
    let trace_b_in_2 = response_2.contains("\r\nx-backend-trace: worker-b-backend\r\n");
    assert!(
        (trace_a_in_1 && trace_b_in_2) || (trace_b_in_1 && trace_a_in_2),
        "the two responses must come from different backends (load balanced); response_1 traces: a={trace_a_in_1} b={trace_b_in_1}, response_2 traces: a={trace_a_in_2} b={trace_b_in_2}"
    );

    daemon_a_handle.abort();
    daemon_b_handle.abort();
}

#[tokio::test]
async fn worker_daemon_reports_live_in_flight_load_in_heartbeat_pongs() {
    let (proxy_addr, core) = spawn_proxy_server("openai").await;
    let response_body = r#"{"id":"chatcmpl-heartbeat","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"heartbeat success"},"finish_reason":"stop"}]}"#;

    let (backend_release_tx, backend_release_rx) = oneshot::channel();
    let (backend_addr, observed_request_rx) = spawn_controlled_chat_backend(
        StatusCode::OK,
        "heartbeat-backend",
        response_body,
        Some(backend_release_rx),
    )
    .await;

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
    let worker_id = wait_for_provider_worker_id(&core, "openai").await;

    let first_request_body =
        r#"{"model":"gpt-4.1-mini","messages":[{"role":"user","content":"hold the worker busy"}]}"#;
    let first_http_request = tokio::spawn(post_chat_completions(
        proxy_addr,
        first_request_body,
        &[("Authorization", "Bearer client-token")],
    ));

    let observed_request = timeout(Duration::from_secs(2), observed_request_rx)
        .await
        .expect("backend observed request before timeout")
        .expect("backend observed request");
    assert_observed_openai_chat_request(
        &observed_request,
        Some("Bearer client-token"),
        first_request_body,
    );

    wait_for_request_state(
        &core,
        "request-1",
        RequestState::InFlight {
            worker_id: worker_id.clone(),
            cancellation: None,
        },
    )
    .await;

    tokio::time::sleep(Duration::from_millis(250)).await;
    wait_for_worker_reported_load(&core, &worker_id, 1).await;

    {
        let core = core.lock().await;
        assert_eq!(core.worker_reported_load(&worker_id), Some(1));
        assert_eq!(
            core.worker_in_flight_request_ids(&worker_id),
            vec!["request-1".to_string()]
        );
    }

    let second_request_body = r#"{"model":"gpt-4.1-mini","messages":[{"role":"user","content":"stay queued until the heartbeat-reported load clears"}]}"#;
    let mut second_http_request = tokio::spawn(post_chat_completions(
        proxy_addr,
        second_request_body,
        &[("Authorization", "Bearer client-token")],
    ));

    wait_for_request_state(&core, "request-2", RequestState::Queued).await;
    wait_for_queued_request_id(&core, "openai").await;

    {
        let core = core.lock().await;
        assert_eq!(core.worker_reported_load(&worker_id), Some(1));
        assert_eq!(
            core.queued_request_ids("openai"),
            vec!["request-2".to_string()]
        );
        assert_eq!(
            core.worker_in_flight_request_ids(&worker_id),
            vec!["request-1".to_string()]
        );
    }

    assert!(
        timeout(Duration::from_millis(150), &mut second_http_request)
            .await
            .is_err(),
        "the queued follow-up request should stay blocked while the heartbeat load reports the worker as full"
    );

    drop(backend_release_tx);

    let first_response = timeout(Duration::from_secs(5), first_http_request)
        .await
        .expect("first http request completed before timeout")
        .expect("join first http request task");
    assert_successful_chat_completion_response(&first_response, "heartbeat-backend", response_body);

    let second_response = timeout(Duration::from_secs(5), second_http_request)
        .await
        .expect("second http request completed before timeout")
        .expect("join second http request task");
    assert_successful_chat_completion_response(
        &second_response,
        "heartbeat-backend",
        response_body,
    );

    daemon_handle.abort();
}

#[derive(Clone)]
struct SlowStreamingBackendState {
    observed_request_tx: Arc<Mutex<Option<oneshot::Sender<ObservedBackendRequest>>>>,
    backend_abort_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

async fn spawn_slow_streaming_backend() -> (
    SocketAddr,
    oneshot::Receiver<ObservedBackendRequest>,
    oneshot::Receiver<()>,
) {
    let (observed_request_tx, observed_request_rx) = oneshot::channel();
    let (backend_abort_tx, backend_abort_rx) = oneshot::channel();
    let state = SlowStreamingBackendState {
        observed_request_tx: Arc::new(Mutex::new(Some(observed_request_tx))),
        backend_abort_tx: Arc::new(Mutex::new(Some(backend_abort_tx))),
    };
    let app = Router::new()
        .route("/v1/chat/completions", post(slow_streaming_chat_handler))
        .with_state(state);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind slow streaming backend listener");
    let addr = listener
        .local_addr()
        .expect("slow streaming backend listener local addr");

    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve slow streaming backend app");
    });

    (addr, observed_request_rx, backend_abort_rx)
}

async fn slow_streaming_chat_handler(
    State(state): State<SlowStreamingBackendState>,
    headers: HeaderMap,
    body: String,
) -> impl IntoResponse {
    if let Some(tx) = state.observed_request_tx.lock().await.take() {
        let _ = tx.send(ObservedBackendRequest {
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

    // The sentinel is held in the stream state. When the stream is dropped (because the
    // reqwest client in the worker daemon aborted), the sentinel drops, which closes the
    // oneshot channel — the test-side receiver observes Err(RecvError).
    let sentinel = state.backend_abort_tx.lock().await.take();

    let stream = stream::unfold((0_usize, sentinel), |(idx, sentinel)| async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let chunk =
            format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"chunk-{idx}\"}}}}]}}\n\n");
        Some((
            Ok::<Bytes, Infallible>(Bytes::from(chunk)),
            (idx + 1, sentinel),
        ))
    });

    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        "content-type",
        "text/event-stream"
            .parse()
            .expect("parse content-type header"),
    );
    response
}

#[tokio::test]
async fn worker_daemon_cancels_backend_request_when_http_client_disconnects() {
    let (proxy_addr, _core) = spawn_proxy_server("openai").await;
    let (backend_addr, observed_request_rx, backend_abort_rx) =
        spawn_slow_streaming_backend().await;

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

    // Open a streaming request — stream:true causes the proxy to use the streaming path
    // with an HttpRequestCancellationGuard that fires when the response Body is dropped.
    let request_body = r#"{"model":"gpt-4.1-mini","stream":true,"messages":[{"role":"user","content":"cancel me"}]}"#;
    let mut client_stream = open_chat_completions_request(
        proxy_addr,
        request_body,
        &[("Authorization", "Bearer client-token")],
    )
    .await;

    // Wait for the backend to receive the request, confirming the worker daemon has
    // forwarded it and the in-flight backend request is active.
    timeout(Duration::from_secs(2), observed_request_rx)
        .await
        .expect("backend received request before timeout")
        .expect("backend received request");

    // Wait for at least the first SSE chunk to arrive at the proxy client. This confirms
    // the streaming pipeline is live end-to-end before we simulate a disconnect.
    let _ = read_until_contains(&mut client_stream, "data:").await;

    // Simulate HTTP client disconnect by dropping the TCP stream. The chain that follows:
    //   proxy Body stream dropped -> HttpRequestCancellationGuard fires
    //   -> core.cancel_request(ClientDisconnected)
    //   -> Cancel message sent to worker daemon over WebSocket
    //   -> worker daemon calls handle.abort() on the in-flight task
    //   -> forward_request future dropped -> reqwest connection to backend closed
    //   -> backend Body::from_stream stream dropped -> sentinel (oneshot::Sender) dropped
    //   -> backend_abort_rx resolves with Err(RecvError)
    drop(client_stream);

    let result = timeout(Duration::from_secs(5), backend_abort_rx).await;
    assert!(
        result.is_ok(),
        "backend stream should have been aborted within 5s after HTTP client disconnect"
    );

    daemon_handle.abort();
}

#[tokio::test]
async fn worker_daemon_run_with_reconnect_reconnects_after_proxy_graceful_shutdown() {
    let (proxy_addr, core) = spawn_proxy_server("openai").await;

    let (backend_addr, _backend_rx) = spawn_mock_backend().await;
    let daemon = WorkerDaemon::new(WorkerDaemonConfig {
        proxy_base_url: format!("http://{proxy_addr}"),
        provider: "openai".to_string(),
        worker_secret: "top-secret".to_string(),
        worker_name: "gpu-box-reconnect-test".to_string(),
        models: vec!["gpt-4.1-mini".to_string()],
        max_concurrent: 1,
        backend_base_url: format!("http://{backend_addr}"),
    });

    // run_with_reconnect loops over run_session. After graceful shutdown it should
    // reconnect (not exit), because the server is being replaced during a rolling deploy.
    let daemon_handle = tokio::spawn(async move { daemon.run_with_reconnect().await });

    // Wait until the daemon has registered its model with the proxy.
    wait_for_registered_model(proxy_addr, "gpt-4.1-mini").await;

    // Trigger proxy-side graceful shutdown — the socket handler sends a GracefulShutdown
    // message to the daemon on the next loop tick (≤25 ms) and then closes the socket
    // once the daemon is idle.
    {
        let mut core = core.lock().await;
        core.begin_graceful_shutdown(Some("test graceful shutdown"), Duration::from_secs(5));
    }

    // The daemon should NOT exit — it should attempt to reconnect. Wait briefly for
    // the graceful shutdown to complete and the reconnect cycle to start, then verify
    // the daemon is still running (the task hasn't finished).
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        !daemon_handle.is_finished(),
        "run_with_reconnect should keep running after graceful shutdown, not exit"
    );

    // Clean up — abort the reconnect loop.
    daemon_handle.abort();
}

// ---- Responses API (OpenAI /v1/responses) live daemon tests ----

async fn mock_responses_handler(
    State(state): State<BackendState>,
    headers: HeaderMap,
    body: String,
) -> impl IntoResponse {
    let is_streaming = body.contains(r#""stream":true"#);

    if let Some(observed_request_tx) = state.observed_request_tx.lock().await.take() {
        let _ = observed_request_tx.send(ObservedBackendRequest {
            path: "/v1/responses".to_string(),
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
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hel\"}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"lo\"}\n\n",
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
        response.headers_mut().insert(
            "openai-beta",
            "responses=v1".parse().expect("parse openai-beta header"),
        );
        return response;
    }

    (
        StatusCode::OK,
        [
            ("content-type", "application/json"),
            ("openai-beta", "responses=v1"),
            ("x-backend-trace", "mock-responses-backend"),
        ],
        r#"{"id":"resp_456","object":"response","model":"gpt-4.1-mini","output":[{"type":"message","content":[{"type":"output_text","text":"proxy success"}]}]}"#,
    )
        .into_response()
}

async fn spawn_mock_responses_backend() -> (SocketAddr, oneshot::Receiver<ObservedBackendRequest>) {
    let (observed_request_tx, observed_request_rx) = oneshot::channel();
    let state = BackendState {
        observed_request_tx: Arc::new(Mutex::new(Some(observed_request_tx))),
    };
    let app = Router::new()
        .route("/v1/responses", post(mock_responses_handler))
        .with_state(state);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind responses backend listener");
    let addr = listener
        .local_addr()
        .expect("responses backend listener local addr");

    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve responses backend app");
    });

    (addr, observed_request_rx)
}

async fn post_responses(addr: SocketAddr, body: &str, headers: &[(&str, &str)]) -> String {
    let mut stream = open_responses_request(addr, body, headers).await;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("read responses response");
    String::from_utf8(response).expect("proxy response is utf-8")
}

async fn open_responses_request(
    addr: SocketAddr,
    body: &str,
    headers: &[(&str, &str)],
) -> TcpStream {
    let mut stream = TcpStream::connect(addr)
        .await
        .expect("connect to proxy server");
    let mut request = format!(
        "POST /v1/responses HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
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
        .expect("write responses request");

    stream
}

#[tokio::test]
async fn worker_daemon_forwards_non_streaming_openai_responses_request_through_live_proxy() {
    let (proxy_addr, _core) = spawn_proxy_server("openai").await;
    let (backend_addr, observed_request_rx) = spawn_mock_responses_backend().await;

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

    let request_body = r#"{"model":"gpt-4.1-mini","input":"hello from responses proxy"}"#;
    let response = post_responses(
        proxy_addr,
        request_body,
        &[
            ("Authorization", "Bearer client-token"),
            ("OpenAI-Beta", "responses=v1"),
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
            path: "/v1/responses".to_string(),
            authorization: Some("Bearer client-token".to_string()),
            openai_organization: None,
            x_api_key: None,
            anthropic_version: None,
            anthropic_beta: None,
            content_type: Some("application/json".to_string()),
            body: request_body.to_string(),
        }
    );
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.contains("\r\ncontent-type: application/json\r\n"));
    assert!(response.contains("\r\nopenai-beta: responses=v1\r\n"));
    assert!(response.contains("\r\nx-backend-trace: mock-responses-backend\r\n"));
    assert!(response.ends_with(
        r#"{"id":"resp_456","object":"response","model":"gpt-4.1-mini","output":[{"type":"message","content":[{"type":"output_text","text":"proxy success"}]}]}"#
    ));

    daemon_handle.abort();
}

#[tokio::test]
async fn worker_daemon_forwards_streaming_openai_responses_request_through_live_proxy() {
    let (proxy_addr, _core) = spawn_proxy_server("openai").await;
    let (backend_addr, observed_request_rx) = spawn_mock_responses_backend().await;

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
        r#"{"model":"gpt-4.1-mini","stream":true,"input":"hello streaming from responses proxy"}"#;
    let mut response_stream = open_responses_request(
        proxy_addr,
        request_body,
        &[
            ("Authorization", "Bearer client-token"),
            ("OpenAI-Beta", "responses=v1"),
        ],
    )
    .await;

    let first_fragment = read_until_contains(
        &mut response_stream,
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hel\"}\n\n",
    )
    .await;

    let observed_request = timeout(Duration::from_secs(2), observed_request_rx)
        .await
        .expect("backend observed request before timeout")
        .expect("backend observed request");

    assert_eq!(
        observed_request,
        ObservedBackendRequest {
            path: "/v1/responses".to_string(),
            authorization: Some("Bearer client-token".to_string()),
            openai_organization: None,
            x_api_key: None,
            anthropic_version: None,
            anthropic_beta: None,
            content_type: Some("application/json".to_string()),
            body: request_body.to_string(),
        }
    );
    assert!(first_fragment.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(first_fragment.contains("\r\ncontent-type: text/event-stream\r\n"));
    assert!(
        first_fragment
            .contains("data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hel\"}\n\n")
    );
    assert!(!first_fragment.contains("data: [DONE]\n\n"));

    let mut rest = Vec::new();
    response_stream
        .read_to_end(&mut rest)
        .await
        .expect("read streaming responses response");
    let full_response = first_fragment + &String::from_utf8(rest).expect("proxy response is utf-8");

    let hel_index = full_response
        .find("data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hel\"}\n\n")
        .expect("find first streamed chunk");
    let lo_index = full_response
        .find("data: {\"type\":\"response.output_text.delta\",\"delta\":\"lo\"}\n\n")
        .expect("find second streamed chunk");
    let done_index = full_response
        .find("data: [DONE]\n\n")
        .expect("find done marker");

    assert!(hel_index < lo_index);
    assert!(lo_index < done_index);
    assert!(full_response.ends_with("0\r\n\r\n"));

    daemon_handle.abort();
}

#[tokio::test]
async fn worker_daemon_authenticates_with_api_key_instead_of_static_secret() {
    let store = Arc::new(modelrelay_server::InMemoryApiKeyStore::new());
    let (_meta, raw_key) = store.create_key("test-key".to_string()).await.unwrap();

    let (proxy_addr, _core) =
        spawn_proxy_server_with_api_key_store("openai", store as Arc<dyn ApiKeyStore>).await;
    let (backend_addr, observed_request_rx) = spawn_mock_backend().await;

    let daemon = WorkerDaemon::new(WorkerDaemonConfig {
        proxy_base_url: format!("http://{proxy_addr}"),
        provider: "openai".to_string(),
        worker_secret: raw_key,
        worker_name: "apikey-worker".to_string(),
        models: vec!["gpt-4.1-mini".to_string()],
        max_concurrent: 1,
        backend_base_url: format!("http://{backend_addr}"),
    });

    let daemon_handle = tokio::spawn(async move { daemon.run().await });

    wait_for_registered_model(proxy_addr, "gpt-4.1-mini").await;

    let request_body =
        r#"{"model":"gpt-4.1-mini","messages":[{"role":"user","content":"hello via api key"}]}"#;
    let response = post_chat_completions(
        proxy_addr,
        request_body,
        &[("Authorization", "Bearer client-token")],
    )
    .await;

    assert!(!response.is_empty(), "expected non-empty proxy response");

    let observed = observed_request_rx.await.expect("backend received request");
    assert!(observed.body.contains("hello via api key"));

    daemon_handle.abort();
}
