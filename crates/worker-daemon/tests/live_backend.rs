use std::{fmt::Write as _, net::SocketAddr, sync::Arc};

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

async fn spawn_proxy_server() -> SocketAddr {
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
    let proxy_addr = spawn_proxy_server().await;
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
