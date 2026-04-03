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
    content_type: Option<String>,
    body: String,
}

#[derive(Clone)]
struct BackendState {
    observed_request_tx: Arc<Mutex<Option<oneshot::Sender<ObservedBackendRequest>>>>,
}

async fn spawn_proxy_server() -> (SocketAddr, Arc<Mutex<ProxyServerCore>>) {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let worker_socket_app = WorkerSocketApp::new(core.clone())
        .with_provider("openai", WorkerSocketProviderConfig::enabled("top-secret"));
    let app = ProxyHttpApp::new(core.clone())
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

#[tokio::test]
async fn worker_daemon_forwards_non_streaming_openai_request_through_live_proxy() {
    let (proxy_addr, _core) = spawn_proxy_server().await;
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
    let (proxy_addr, _core) = spawn_proxy_server().await;
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
