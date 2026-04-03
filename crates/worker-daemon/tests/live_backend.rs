use std::{collections::BTreeMap, fmt::Write as _, net::SocketAddr, sync::Arc};

use axum::{Router, extract::State, http::HeaderMap, response::IntoResponse, routing::post};
use proxy_server::{ProxyHttpApp, ProxyServerCore, WorkerSocketApp, WorkerSocketProviderConfig};
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
    openai_beta: Option<String>,
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

async fn spawn_proxy_server(models_provider: &str) -> SocketAddr {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let worker_socket_app = WorkerSocketApp::new(core.clone())
        .with_provider(
            "anthropic",
            WorkerSocketProviderConfig::enabled("top-secret"),
        )
        .with_provider("openai", WorkerSocketProviderConfig::enabled("top-secret"));
    let app = ProxyHttpApp::new(core)
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

    addr
}

async fn spawn_mock_backend() -> (SocketAddr, oneshot::Receiver<ObservedBackendRequest>) {
    let (observed_request_tx, observed_request_rx) = oneshot::channel();
    let state = BackendState {
        observed_request_tx: Arc::new(Mutex::new(Some(observed_request_tx))),
    };
    let app = Router::new()
        .route("/v1/responses", post(mock_responses_handler))
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

async fn mock_messages_handler(
    State(state): State<BackendState>,
    headers: HeaderMap,
    body: String,
) -> impl IntoResponse {
    if let Some(observed_request_tx) = state.observed_request_tx.lock().await.take() {
        let _ = observed_request_tx.send(ObservedBackendRequest {
            path: "/v1/messages".to_string(),
            authorization: header_value(&headers, "authorization"),
            openai_organization: header_value(&headers, "openai-organization"),
            openai_beta: header_value(&headers, "openai-beta"),
            x_api_key: header_value(&headers, "x-api-key"),
            anthropic_version: header_value(&headers, "anthropic-version"),
            anthropic_beta: header_value(&headers, "anthropic-beta"),
            content_type: header_value(&headers, "content-type"),
            body,
        });
    }

    (
        axum::http::StatusCode::OK,
        [
            ("content-type", "application/json"),
            ("anthropic-version", "2023-06-01"),
            ("anthropic-beta", "tools-2024-04-04"),
            ("request-id", "anthropic-mock-123"),
        ],
        r#"{"id":"msg_123","type":"message","role":"assistant","model":"claude-3-7-sonnet-20250219","content":[{"type":"text","text":"anthropic proxy success"}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":12,"output_tokens":7}}"#,
    )
}

async fn mock_responses_handler(
    State(state): State<BackendState>,
    headers: HeaderMap,
    body: String,
) -> impl IntoResponse {
    if let Some(observed_request_tx) = state.observed_request_tx.lock().await.take() {
        let _ = observed_request_tx.send(ObservedBackendRequest {
            path: "/v1/responses".to_string(),
            authorization: header_value(&headers, "authorization"),
            openai_organization: header_value(&headers, "openai-organization"),
            openai_beta: header_value(&headers, "openai-beta"),
            x_api_key: header_value(&headers, "x-api-key"),
            anthropic_version: header_value(&headers, "anthropic-version"),
            anthropic_beta: header_value(&headers, "anthropic-beta"),
            content_type: header_value(&headers, "content-type"),
            body,
        });
    }

    (
        axum::http::StatusCode::CREATED,
        [
            ("content-type", "application/json"),
            ("openai-beta", "responses=v1"),
            ("x-backend-trace", "openai-responses-backend"),
        ],
        r#"{"id":"resp_123","object":"response","model":"gpt-4.1-mini","output":[{"type":"message","id":"msg_123","status":"completed","role":"assistant","content":[{"type":"output_text","text":"responses proxy success"}]}]}"#,
    )
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

async fn post_messages(addr: SocketAddr, body: &str, headers: &[(&str, &str)]) -> String {
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

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("read messages response");

    String::from_utf8(response).expect("proxy response is utf-8")
}

async fn post_responses(addr: SocketAddr, body: &str, headers: &[(&str, &str)]) -> String {
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

async fn spawn_slow_cancellation_backend() -> (
    SocketAddr,
    oneshot::Receiver<ObservedBackendRequest>,
    oneshot::Receiver<Result<(), String>>,
) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind slow backend listener");
    let addr = listener
        .local_addr()
        .expect("slow backend listener local addr");
    let (observed_request_tx, observed_request_rx) = oneshot::channel();
    let (cancelled_tx, cancelled_rx) = oneshot::channel();

    tokio::spawn(async move {
        let result = async {
            let (mut stream, _) = listener.accept().await.map_err(|error| error.to_string())?;
            let request = read_raw_http_request(&mut stream).await?;

            observed_request_tx
                .send(ObservedBackendRequest {
                    path: request.path,
                    authorization: request.headers.get("authorization").cloned(),
                    openai_organization: request.headers.get("openai-organization").cloned(),
                    openai_beta: request.headers.get("openai-beta").cloned(),
                    x_api_key: request.headers.get("x-api-key").cloned(),
                    anthropic_version: request.headers.get("anthropic-version").cloned(),
                    anthropic_beta: request.headers.get("anthropic-beta").cloned(),
                    content_type: request.headers.get("content-type").cloned(),
                    body: request.body,
                })
                .map_err(|_| "failed to send observed backend request".to_string())?;

            wait_for_socket_close(&mut stream).await
        }
        .await;

        let _ = cancelled_tx.send(result);
    });

    (addr, observed_request_rx, cancelled_rx)
}

struct RawHttpRequest {
    path: String,
    headers: BTreeMap<String, String>,
    body: String,
}

async fn read_raw_http_request(stream: &mut TcpStream) -> Result<RawHttpRequest, String> {
    let mut request_bytes = Vec::new();
    let mut chunk = [0_u8; 1024];

    let header_end = loop {
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Err("backend client closed before sending headers".to_string());
        }
        request_bytes.extend_from_slice(&chunk[..read]);
        if let Some(position) = find_bytes(&request_bytes, b"\r\n\r\n") {
            break position + 4;
        }
    };

    let head = String::from_utf8(request_bytes[..header_end].to_vec())
        .map_err(|error| error.to_string())?;
    let mut lines = head.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| "missing request line".to_string())?;
    let path = request_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| "missing request path".to_string())?
        .to_string();

    let mut headers = BTreeMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| format!("invalid header line: {line}"))?;
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }

    let content_length = headers
        .get("content-length")
        .ok_or_else(|| "missing content-length header".to_string())?
        .parse::<usize>()
        .map_err(|error| error.to_string())?;
    let mut body = request_bytes[header_end..].to_vec();
    while body.len() < content_length {
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Err("backend client closed before sending full body".to_string());
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(content_length);

    Ok(RawHttpRequest {
        path,
        headers,
        body: String::from_utf8(body).map_err(|error| error.to_string())?,
    })
}

async fn wait_for_socket_close(stream: &mut TcpStream) -> Result<(), String> {
    timeout(Duration::from_secs(2), async {
        let mut buffer = [0_u8; 1];
        loop {
            let read = stream
                .read(&mut buffer)
                .await
                .map_err(|error| error.to_string())?;
            if read == 0 {
                return Ok(());
            }
        }
    })
    .await
    .map_err(|_| "backend connection was not canceled before timeout".to_string())?
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
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

#[tokio::test]
async fn worker_daemon_forwards_anthropic_messages_request_through_live_proxy() {
    let proxy_addr = spawn_proxy_server("anthropic").await;
    let (backend_addr, observed_request_rx) = spawn_mock_backend().await;

    let daemon = WorkerDaemon::new(WorkerDaemonConfig {
        proxy_base_url: format!("http://{proxy_addr}"),
        provider: "anthropic".to_string(),
        worker_secret: "top-secret".to_string(),
        worker_name: "gpu-box-a".to_string(),
        models: vec!["claude-3-7-sonnet-20250219".to_string()],
        max_concurrent: 1,
        backend_base_url: format!("http://{backend_addr}"),
    });

    let daemon_handle = tokio::spawn(async move { daemon.run().await });

    wait_for_registered_model(proxy_addr, "claude-3-7-sonnet-20250219").await;

    let request_body = json!({
        "model": "claude-3-7-sonnet-20250219",
        "max_tokens": 128,
        "messages": [{"role": "user", "content": [{"type": "text", "text": "hello from proxy"}]}]
    })
    .to_string();
    let response = post_messages(
        proxy_addr,
        &request_body,
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
            openai_beta: None,
            x_api_key: Some("test-anthropic-key".to_string()),
            anthropic_version: Some("2023-06-01".to_string()),
            anthropic_beta: Some("tools-2024-04-04".to_string()),
            content_type: Some("application/json".to_string()),
            body: request_body,
        }
    );
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.contains("\r\ncontent-type: application/json\r\n"));
    assert!(response.contains("\r\nanthropic-version: 2023-06-01\r\n"));
    assert!(response.contains("\r\nanthropic-beta: tools-2024-04-04\r\n"));
    assert!(response.contains("\r\nrequest-id: anthropic-mock-123\r\n"));
    assert!(response.ends_with(
        "{\"id\":\"msg_123\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-3-7-sonnet-20250219\",\"content\":[{\"type\":\"text\",\"text\":\"anthropic proxy success\"}],\"stop_reason\":\"end_turn\",\"stop_sequence\":null,\"usage\":{\"input_tokens\":12,\"output_tokens\":7}}"
    ));

    daemon_handle.abort();
}

#[tokio::test]
async fn worker_daemon_forwards_openai_responses_request_through_live_proxy() {
    let proxy_addr = spawn_proxy_server("openai").await;
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
        json!({"model": "gpt-4.1-mini", "input": "hello from responses"}).to_string();
    let response = post_responses(
        proxy_addr,
        &request_body,
        &[
            ("Authorization", "Bearer test-openai-key"),
            ("OpenAI-Beta", "responses=v1"),
            ("OpenAI-Organization", "org-demo"),
            ("X-Trace-Id", "trace-456"),
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
            authorization: Some("Bearer test-openai-key".to_string()),
            openai_organization: Some("org-demo".to_string()),
            openai_beta: Some("responses=v1".to_string()),
            x_api_key: None,
            anthropic_version: None,
            anthropic_beta: None,
            content_type: Some("application/json".to_string()),
            body: request_body,
        }
    );
    assert!(response.starts_with("HTTP/1.1 201 Created\r\n"));
    assert!(response.contains("\r\ncontent-type: application/json\r\n"));
    assert!(response.contains("\r\nopenai-beta: responses=v1\r\n"));
    assert!(response.contains("\r\nx-backend-trace: openai-responses-backend\r\n"));
    assert!(response.ends_with(
        r#"{"id":"resp_123","object":"response","model":"gpt-4.1-mini","output":[{"type":"message","id":"msg_123","status":"completed","role":"assistant","content":[{"type":"output_text","text":"responses proxy success"}]}]}"#
    ));

    daemon_handle.abort();
}

#[tokio::test]
async fn worker_daemon_cancels_in_flight_backend_request_when_proxy_client_disconnects() {
    let proxy_addr = spawn_proxy_server("openai").await;
    let (backend_addr, observed_request_rx, cancelled_rx) = spawn_slow_cancellation_backend().await;

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
        json!({"model": "gpt-4.1-mini", "stream": true, "input": "cancel me"}).to_string();
    let mut response_stream = open_responses_request(
        proxy_addr,
        &request_body,
        &[
            ("Authorization", "Bearer test-openai-key"),
            ("OpenAI-Beta", "responses=v1"),
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
            path: "/v1/responses".to_string(),
            authorization: Some("Bearer test-openai-key".to_string()),
            openai_organization: Some("org-demo".to_string()),
            openai_beta: Some("responses=v1".to_string()),
            x_api_key: None,
            anthropic_version: None,
            anthropic_beta: None,
            content_type: Some("application/json".to_string()),
            body: request_body,
        }
    );

    response_stream
        .shutdown()
        .await
        .expect("shutdown disconnected proxy client");
    drop(response_stream);

    timeout(Duration::from_secs(2), cancelled_rx)
        .await
        .expect("backend cancellation before timeout")
        .expect("backend cancellation result")
        .expect("backend request was canceled");

    daemon_handle.abort();
}
