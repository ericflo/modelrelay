use std::{collections::BTreeMap, error::Error, fmt::Write as _, net::SocketAddr, sync::Arc};

use axum::{
    Router,
    body::{Body, Bytes},
    extract::State,
    http::{self, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::stream;
use proxy_server::{
    ProviderQueuePolicy, ProxyHttpApp, ProxyServerCore, RequestState, WorkerSocketApp,
    WorkerSocketProviderConfig,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Mutex, oneshot},
    time::{Duration, sleep, timeout},
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
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
    advertised_models: Arc<Mutex<Vec<String>>>,
}

#[derive(Clone)]
struct DelayedResponsesBackendState {
    observed_request_tx: Arc<Mutex<Option<oneshot::Sender<ObservedBackendRequest>>>>,
    release_response_rx: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
}

#[derive(Clone)]
struct NonSuccessResponsesBackendState {
    observed_request_tx: Arc<Mutex<Option<oneshot::Sender<ObservedBackendRequest>>>>,
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

async fn spawn_mock_backend() -> (
    SocketAddr,
    oneshot::Receiver<ObservedBackendRequest>,
    Arc<Mutex<Vec<String>>>,
) {
    let (observed_request_tx, observed_request_rx) = oneshot::channel();
    let advertised_models = Arc::new(Mutex::new(vec![
        "gpt-4.1-mini".to_string(),
        "claude-3-7-sonnet-20250219".to_string(),
    ]));
    let state = BackendState {
        observed_request_tx: Arc::new(Mutex::new(Some(observed_request_tx))),
        advertised_models: advertised_models.clone(),
    };
    let app = Router::new()
        .route("/v1/models", get(mock_models_handler))
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

    (addr, observed_request_rx, advertised_models)
}

async fn mock_models_handler(State(state): State<BackendState>) -> impl IntoResponse {
    let models = state.advertised_models.lock().await.clone();
    let data = models
        .into_iter()
        .map(|model| {
            json!({
                "id": model,
                "object": "model",
                "owned_by": "mock-backend"
            })
        })
        .collect::<Vec<_>>();

    (
        axum::http::StatusCode::OK,
        [("content-type", "application/json")],
        json!({
            "object": "list",
            "data": data,
        })
        .to_string(),
    )
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
            openai_beta: header_value(&headers, "openai-beta"),
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
                Ok::<Bytes, std::convert::Infallible>(Bytes::from_static(chunk.as_bytes())),
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
            "anthropic-version",
            "2023-06-01"
                .parse()
                .expect("parse anthropic-version header"),
        );
        response.headers_mut().insert(
            "anthropic-beta",
            "tools-2024-04-04"
                .parse()
                .expect("parse anthropic-beta header"),
        );
        response.headers_mut().insert(
            "request-id",
            "anthropic-stream-mock-123"
                .parse()
                .expect("parse request-id header"),
        );
        return response;
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
        .into_response()
}

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
            openai_beta: header_value(&headers, "openai-beta"),
            x_api_key: header_value(&headers, "x-api-key"),
            anthropic_version: header_value(&headers, "anthropic-version"),
            anthropic_beta: header_value(&headers, "anthropic-beta"),
            content_type: header_value(&headers, "content-type"),
            body,
        });
    }

    if is_streaming {
        let chunks = [
            "data: {\"id\":\"resp_123\",\"type\":\"response.output_text.delta\",\"delta\":\"Hel\"}\n\n",
            "data: {\"id\":\"resp_123\",\"type\":\"response.output_text.delta\",\"delta\":\"lo\"}\n\n",
            "data: [DONE]\n\n",
        ];
        let stream = stream::unfold(0_usize, move |index| async move {
            let chunk = chunks.get(index)?;

            tokio::time::sleep(Duration::from_millis(10)).await;
            Some((
                Ok::<Bytes, std::convert::Infallible>(Bytes::from_static(chunk.as_bytes())),
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
            "cache-control",
            "no-cache".parse().expect("parse cache-control"),
        );
        response.headers_mut().insert(
            "openai-beta",
            "responses=v1".parse().expect("parse openai-beta"),
        );
        return response;
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

async fn assert_model_not_registered_for(
    addr: SocketAddr,
    unexpected_model: &str,
    duration: Duration,
) {
    timeout(duration, async {
        loop {
            let (_, body) = get_models(addr).await;
            let models = body["data"]
                .as_array()
                .expect("models array")
                .iter()
                .filter_map(|entry| entry["id"].as_str())
                .collect::<Vec<_>>();
            assert!(
                !models.contains(&unexpected_model),
                "model {unexpected_model} unexpectedly registered during cooldown"
            );
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect_err("model absence probe should run until timeout");
}

async fn wait_for_models_catalog(addr: SocketAddr, expected_models: &[&str]) {
    timeout(Duration::from_secs(2), async {
        loop {
            let (_, body) = get_models(addr).await;
            let models = body["data"]
                .as_array()
                .expect("models array")
                .iter()
                .filter_map(|entry| entry["id"].as_str())
                .collect::<Vec<_>>();
            if models == expected_models {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("models catalog did not match {expected_models:?}"));
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

async fn spawn_delayed_responses_backend() -> (
    SocketAddr,
    oneshot::Receiver<ObservedBackendRequest>,
    oneshot::Sender<()>,
) {
    let (observed_request_tx, observed_request_rx) = oneshot::channel();
    let (release_response_tx, release_response_rx) = oneshot::channel();
    let state = DelayedResponsesBackendState {
        observed_request_tx: Arc::new(Mutex::new(Some(observed_request_tx))),
        release_response_rx: Arc::new(Mutex::new(Some(release_response_rx))),
    };
    let app = Router::new()
        .route("/v1/responses", post(delayed_responses_handler))
        .with_state(state);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind delayed backend listener");
    let addr = listener
        .local_addr()
        .expect("delayed backend listener local addr");

    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve delayed backend app");
    });

    (addr, observed_request_rx, release_response_tx)
}

async fn spawn_non_success_responses_backend()
-> (SocketAddr, oneshot::Receiver<ObservedBackendRequest>) {
    let (observed_request_tx, observed_request_rx) = oneshot::channel();
    let state = NonSuccessResponsesBackendState {
        observed_request_tx: Arc::new(Mutex::new(Some(observed_request_tx))),
    };
    let app = Router::new()
        .route("/v1/responses", post(non_success_responses_handler))
        .with_state(state);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind non-success backend listener");
    let addr = listener
        .local_addr()
        .expect("non-success backend listener local addr");

    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve non-success backend app");
    });

    (addr, observed_request_rx)
}

async fn spawn_non_success_messages_backend()
-> (SocketAddr, oneshot::Receiver<ObservedBackendRequest>) {
    let (observed_request_tx, observed_request_rx) = oneshot::channel();
    let state = NonSuccessResponsesBackendState {
        observed_request_tx: Arc::new(Mutex::new(Some(observed_request_tx))),
    };
    let app = Router::new()
        .route("/v1/messages", post(non_success_messages_handler))
        .with_state(state);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind non-success backend listener");
    let addr = listener
        .local_addr()
        .expect("non-success backend listener local addr");

    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve non-success backend app");
    });

    (addr, observed_request_rx)
}

async fn delayed_responses_handler(
    State(state): State<DelayedResponsesBackendState>,
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

    if let Some(release_response_rx) = state.release_response_rx.lock().await.take() {
        let _ = release_response_rx.await;
    }

    (
        axum::http::StatusCode::CREATED,
        [
            ("content-type", "application/json"),
            ("openai-beta", "responses=v1"),
            ("x-backend-trace", "delayed-openai-responses-backend"),
        ],
        r#"{"id":"resp_drain_1","object":"response","model":"gpt-4.1-mini","output":[{"type":"message","id":"msg_drain_1","status":"completed","role":"assistant","content":[{"type":"output_text","text":"drain completed"}]}]}"#,
    )
}

async fn non_success_responses_handler(
    State(state): State<NonSuccessResponsesBackendState>,
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
        axum::http::StatusCode::TOO_MANY_REQUESTS,
        [
            ("content-type", "application/json"),
            ("openai-beta", "responses=v1"),
            ("retry-after", "17"),
            ("x-backend-trace", "openai-responses-rate-limit"),
        ],
        r#"{"error":{"message":"backend overloaded","type":"rate_limit_error","code":"backend_overloaded"}}"#,
    )
}

async fn non_success_messages_handler(
    State(state): State<NonSuccessResponsesBackendState>,
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
        axum::http::StatusCode::TOO_MANY_REQUESTS,
        [
            ("content-type", "application/json"),
            ("anthropic-version", "2023-06-01"),
            ("retry-after", "17"),
            ("request-id", "anthropic-rate-limit-123"),
            ("x-backend-trace", "anthropic-messages-rate-limit"),
        ],
        r#"{"type":"error","error":{"type":"rate_limit_error","message":"backend overloaded","request_id":"anthropic-rate-limit-123"}}"#,
    )
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

async fn wait_for_request_state(
    core: &Arc<Mutex<ProxyServerCore>>,
    request_id: &str,
    expected: RequestState,
) {
    timeout(Duration::from_secs(2), async {
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

async fn wait_for_worker_id(core: &Arc<Mutex<ProxyServerCore>>, provider: &str) -> String {
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
    .expect("worker registered before timeout")
}

async fn wait_for_worker_disconnect(core: &Arc<Mutex<ProxyServerCore>>, worker_id: &str) {
    timeout(Duration::from_secs(2), async {
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
    .unwrap_or_else(|_| panic!("worker {worker_id} did not disconnect before timeout"));
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

async fn next_close_frame(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    context: &str,
) -> tokio_tungstenite::tungstenite::protocol::CloseFrame {
    loop {
        let message = timeout(
            Duration::from_secs(2),
            futures_util::StreamExt::next(socket),
        )
        .await
        .unwrap_or_else(|_| panic!("receive {context} before timeout"))
        .expect("socket message")
        .expect("websocket message");

        match message {
            Message::Close(Some(close_frame)) => return close_frame,
            Message::Text(_) | Message::Binary(_) | Message::Frame(_) | Message::Pong(_) => {
                panic!("expected close frame {context}");
            }
            Message::Ping(_) => {}
            Message::Close(None) => panic!("expected close frame payload {context}"),
        }
    }
}

fn spawn_openai_daemon(
    proxy_addr: SocketAddr,
    worker_name: &str,
    backend_addr: SocketAddr,
) -> tokio::task::JoinHandle<Result<(), Box<dyn Error + Send + Sync>>> {
    let daemon = WorkerDaemon::new(WorkerDaemonConfig {
        proxy_base_url: format!("http://{proxy_addr}"),
        provider: "openai".to_string(),
        worker_secret: "top-secret".to_string(),
        worker_name: worker_name.to_string(),
        models: vec!["gpt-4.1-mini".to_string()],
        max_concurrent: 1,
        backend_base_url: format!("http://{backend_addr}"),
    });

    tokio::spawn(async move { daemon.run().await })
}

fn spawn_anthropic_daemon(
    proxy_addr: SocketAddr,
    worker_name: &str,
    backend_addr: SocketAddr,
) -> tokio::task::JoinHandle<Result<(), Box<dyn Error + Send + Sync>>> {
    let daemon = WorkerDaemon::new(WorkerDaemonConfig {
        proxy_base_url: format!("http://{proxy_addr}"),
        provider: "anthropic".to_string(),
        worker_secret: "top-secret".to_string(),
        worker_name: worker_name.to_string(),
        models: vec!["claude-3-7-sonnet-20250219".to_string()],
        max_concurrent: 1,
        backend_base_url: format!("http://{backend_addr}"),
    });

    tokio::spawn(async move { daemon.run().await })
}

fn assert_openai_responses_request(
    observed_request: &ObservedBackendRequest,
    authorization: &str,
    body: &str,
) {
    assert_eq!(
        observed_request,
        &ObservedBackendRequest {
            path: "/v1/responses".to_string(),
            authorization: Some(authorization.to_string()),
            openai_organization: None,
            openai_beta: Some("responses=v1".to_string()),
            x_api_key: None,
            anthropic_version: None,
            anthropic_beta: None,
            content_type: Some("application/json".to_string()),
            body: body.to_string(),
        }
    );
}

fn assert_created_responses_response(response: &str, trace_header: &str, body: &str) {
    assert!(response.starts_with("HTTP/1.1 201 Created\r\n"));
    assert!(response.contains("\r\ncontent-type: application/json\r\n"));
    assert!(response.contains("\r\nopenai-beta: responses=v1\r\n"));
    assert!(response.contains(&format!("\r\nx-backend-trace: {trace_header}\r\n")));
    assert!(response.ends_with(body));
}

fn assert_service_unavailable(response: &str, message: &str) {
    assert!(response.starts_with("HTTP/1.1 503 Service Unavailable\r\n"));
    assert!(response.contains("\r\ncontent-type: text/plain; charset=utf-8\r\n"));
    assert!(response.contains("\r\nx-content-type-options: nosniff\r\n"));
    assert!(response.ends_with(&format!("{message}\n")));
}

async fn begin_graceful_shutdown(core: &Arc<Mutex<ProxyServerCore>>, worker_id: &str) {
    let mut core = core.lock().await;
    let signals =
        core.begin_graceful_shutdown(Some("proxy server shutting down"), Duration::from_secs(1));
    assert_eq!(signals.len(), 1);
    assert!(core.worker_is_draining(worker_id));
}

#[tokio::test]
async fn worker_daemon_forwards_anthropic_messages_request_through_live_proxy() {
    let (proxy_addr, _) = spawn_proxy_server("anthropic").await;
    let (backend_addr, observed_request_rx, _) = spawn_mock_backend().await;

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
async fn worker_daemon_forwards_streaming_anthropic_messages_request_through_live_proxy() {
    let (proxy_addr, _) = spawn_proxy_server("anthropic").await;
    let (backend_addr, observed_request_rx, _) = spawn_mock_backend().await;

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
        "stream": true,
        "max_tokens": 128,
        "messages": [{"role": "user", "content": [{"type": "text", "text": "hello from proxy"}]}]
    })
    .to_string();
    let mut response_stream = open_messages_request(
        proxy_addr,
        &request_body,
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
            openai_beta: None,
            x_api_key: Some("test-anthropic-key".to_string()),
            anthropic_version: Some("2023-06-01".to_string()),
            anthropic_beta: Some("tools-2024-04-04".to_string()),
            content_type: Some("application/json".to_string()),
            body: request_body,
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
async fn worker_daemon_forwards_openai_responses_request_through_live_proxy() {
    let (proxy_addr, _) = spawn_proxy_server("openai").await;
    let (backend_addr, observed_request_rx, _) = spawn_mock_backend().await;

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
async fn worker_daemon_preserves_non_2xx_openai_responses_backend_response_through_live_proxy() {
    let (proxy_addr, _) = spawn_proxy_server("openai").await;
    let (backend_addr, observed_request_rx) = spawn_non_success_responses_backend().await;

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
        json!({"model": "gpt-4.1-mini", "input": "trigger backend overload"}).to_string();
    let response = post_responses(
        proxy_addr,
        &request_body,
        &[
            ("Authorization", "Bearer test-openai-key"),
            ("OpenAI-Beta", "responses=v1"),
            ("OpenAI-Organization", "org-demo"),
            ("X-Trace-Id", "trace-429"),
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
    assert!(response.starts_with("HTTP/1.1 429 Too Many Requests\r\n"));
    assert!(response.contains("\r\ncontent-type: application/json\r\n"));
    assert!(response.contains("\r\nopenai-beta: responses=v1\r\n"));
    assert!(response.contains("\r\nretry-after: 17\r\n"));
    assert!(response.contains("\r\nx-backend-trace: openai-responses-rate-limit\r\n"));
    assert!(response.ends_with(
        r#"{"error":{"message":"backend overloaded","type":"rate_limit_error","code":"backend_overloaded"}}"#
    ));

    daemon_handle.abort();
}

#[tokio::test]
async fn worker_daemon_preserves_non_2xx_anthropic_messages_backend_response_through_live_proxy() {
    let (proxy_addr, _) = spawn_proxy_server("anthropic").await;
    let (backend_addr, observed_request_rx) = spawn_non_success_messages_backend().await;

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
        "max_tokens": 32,
        "messages": [{"role": "user", "content": [{"type": "text", "text": "trigger backend overload"}]}]
    })
    .to_string();
    let response = post_messages(
        proxy_addr,
        &request_body,
        &[
            ("x-api-key", "test-anthropic-key"),
            ("anthropic-version", "2023-06-01"),
            ("anthropic-beta", "tools-2024-04-04"),
            ("x-trace-id", "trace-429"),
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
    assert!(response.starts_with("HTTP/1.1 429 Too Many Requests\r\n"));
    assert!(response.contains("\r\ncontent-type: application/json\r\n"));
    assert!(response.contains("\r\nanthropic-version: 2023-06-01\r\n"));
    assert!(response.contains("\r\nretry-after: 17\r\n"));
    assert!(response.contains("\r\nrequest-id: anthropic-rate-limit-123\r\n"));
    assert!(response.contains("\r\nx-backend-trace: anthropic-messages-rate-limit\r\n"));
    assert!(response.ends_with(
        r#"{"type":"error","error":{"type":"rate_limit_error","message":"backend overloaded","request_id":"anthropic-rate-limit-123"}}"#
    ));

    daemon_handle.abort();
}

#[tokio::test]
async fn worker_daemon_forwards_streaming_openai_responses_request_through_live_proxy() {
    let (proxy_addr, _) = spawn_proxy_server("openai").await;
    let (backend_addr, observed_request_rx, _) = spawn_mock_backend().await;

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
        json!({"model": "gpt-4.1-mini", "stream": true, "input": "hello from responses"})
            .to_string();
    let mut response_stream = open_responses_request(
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

    let first_fragment = read_until_contains(
        &mut response_stream,
        "data: {\"id\":\"resp_123\",\"type\":\"response.output_text.delta\",\"delta\":\"Hel\"}\n\n",
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
    assert!(first_fragment.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(first_fragment.contains("\r\ncontent-type: text/event-stream\r\n"));
    assert!(first_fragment.contains(
        "data: {\"id\":\"resp_123\",\"type\":\"response.output_text.delta\",\"delta\":\"Hel\"}\n\n"
    ));
    assert!(!first_fragment.contains("data: [DONE]\n\n"));

    let mut rest = Vec::new();
    response_stream
        .read_to_end(&mut rest)
        .await
        .expect("read streaming responses response");
    let full_response = first_fragment + &String::from_utf8(rest).expect("proxy response is utf-8");

    let first_delta_index = full_response
        .find("data: {\"id\":\"resp_123\",\"type\":\"response.output_text.delta\",\"delta\":\"Hel\"}\n\n")
        .expect("find first streamed chunk");
    let second_delta_index = full_response
        .find("data: {\"id\":\"resp_123\",\"type\":\"response.output_text.delta\",\"delta\":\"lo\"}\n\n")
        .expect("find second streamed chunk");
    let done_index = full_response
        .find("data: [DONE]\n\n")
        .expect("find done marker");

    assert!(first_delta_index < second_delta_index);
    assert!(second_delta_index < done_index);
    assert!(full_response.ends_with("0\r\n\r\n"));

    daemon_handle.abort();
}

#[tokio::test]
async fn worker_daemon_returns_sanitized_queue_timeout_error_through_live_proxy() {
    let (proxy_addr, core) = spawn_proxy_server("openai").await;
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
    let (backend_addr, first_observed_request_rx, release_response_tx) =
        spawn_delayed_responses_backend().await;
    let daemon_handle = spawn_openai_daemon(proxy_addr, "gpu-box-a", backend_addr);

    wait_for_registered_model(proxy_addr, "gpt-4.1-mini").await;

    let first_body = json!({"model": "gpt-4.1-mini", "input": "hold worker busy"}).to_string();
    let second_body = json!({"model": "gpt-4.1-mini", "input": "time out in queue"}).to_string();

    let first_request_task = tokio::spawn({
        let first_body = first_body.clone();
        async move {
            post_responses(
                proxy_addr,
                &first_body,
                &[
                    ("Authorization", "Bearer in-flight-openai-key"),
                    ("OpenAI-Beta", "responses=v1"),
                ],
            )
            .await
        }
    });

    let first_observed_request = timeout(Duration::from_secs(2), first_observed_request_rx)
        .await
        .expect("first backend observed request before timeout")
        .expect("first backend observed request");
    assert_openai_responses_request(
        &first_observed_request,
        "Bearer in-flight-openai-key",
        &first_body,
    );

    let second_request_task = tokio::spawn({
        let second_body = second_body.clone();
        async move {
            post_responses(
                proxy_addr,
                &second_body,
                &[
                    ("Authorization", "Bearer queued-openai-key"),
                    ("OpenAI-Beta", "responses=v1"),
                ],
            )
            .await
        }
    });

    wait_for_request_state(&core, "request-2", RequestState::Queued).await;

    {
        let mut core = core.lock().await;
        let failures = core.expire_queue_timeouts(std::time::Instant::now());
        assert_eq!(failures.len(), 1);
        assert_eq!(core.queued_request_ids("openai"), Vec::<String>::new());
    }

    let second_response = second_request_task
        .await
        .expect("join timed-out queued request");
    assert_service_unavailable(&second_response, "Request timed out waiting for worker");
    assert!(
        !second_response.contains("QueueTimedOut"),
        "sanitized boundary must not leak the internal failure reason enum"
    );

    release_response_tx
        .send(())
        .expect("release delayed backend response");

    let first_response = timeout(Duration::from_secs(2), first_request_task)
        .await
        .expect("first request completed before timeout")
        .expect("join first request task");
    assert_created_responses_response(
        &first_response,
        "delayed-openai-responses-backend",
        r#"{"id":"resp_drain_1","object":"response","model":"gpt-4.1-mini","output":[{"type":"message","id":"msg_drain_1","status":"completed","role":"assistant","content":[{"type":"output_text","text":"drain completed"}]}]}"#,
    );

    daemon_handle.abort();
}

#[tokio::test]
async fn worker_daemon_returns_sanitized_queue_full_error_through_live_proxy() {
    let (proxy_addr, core) = spawn_proxy_server("openai").await;
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
    let (backend_addr, first_observed_request_rx, release_response_tx) =
        spawn_delayed_responses_backend().await;
    let daemon_handle = spawn_openai_daemon(proxy_addr, "gpu-box-a", backend_addr);

    wait_for_registered_model(proxy_addr, "gpt-4.1-mini").await;

    let first_body = json!({"model": "gpt-4.1-mini", "input": "hold worker busy"}).to_string();
    let second_body =
        json!({"model": "gpt-4.1-mini", "input": "reject when queue is full"}).to_string();

    let first_request_task = tokio::spawn({
        let first_body = first_body.clone();
        async move {
            post_responses(
                proxy_addr,
                &first_body,
                &[
                    ("Authorization", "Bearer in-flight-openai-key"),
                    ("OpenAI-Beta", "responses=v1"),
                ],
            )
            .await
        }
    });

    let first_observed_request = timeout(Duration::from_secs(2), first_observed_request_rx)
        .await
        .expect("first backend observed request before timeout")
        .expect("first backend observed request");
    assert_openai_responses_request(
        &first_observed_request,
        "Bearer in-flight-openai-key",
        &first_body,
    );

    let second_response = post_responses(
        proxy_addr,
        &second_body,
        &[
            ("Authorization", "Bearer queued-openai-key"),
            ("OpenAI-Beta", "responses=v1"),
        ],
    )
    .await;
    assert_service_unavailable(
        &second_response,
        "Service temporarily at capacity, please retry",
    );
    assert!(
        !second_response.contains("QueueFull"),
        "sanitized boundary must not leak the internal failure reason enum"
    );
    assert!(
        !second_response.contains("queue is full"),
        "sanitized boundary must not leak the raw queue-full reason"
    );

    {
        let core = core.lock().await;
        assert_eq!(core.request_state("request-2"), None);
        assert_eq!(core.queued_request_ids("openai"), Vec::<String>::new());
    }

    release_response_tx
        .send(())
        .expect("release delayed backend response");

    let first_response = timeout(Duration::from_secs(2), first_request_task)
        .await
        .expect("first request completed before timeout")
        .expect("join first request task");
    assert_created_responses_response(
        &first_response,
        "delayed-openai-responses-backend",
        r#"{"id":"resp_drain_1","object":"response","model":"gpt-4.1-mini","output":[{"type":"message","id":"msg_drain_1","status":"completed","role":"assistant","content":[{"type":"output_text","text":"drain completed"}]}]}"#,
    );

    daemon_handle.abort();
}

#[tokio::test]
async fn worker_daemon_cancels_in_flight_backend_request_when_proxy_client_disconnects() {
    let (proxy_addr, _) = spawn_proxy_server("openai").await;
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

#[tokio::test]
async fn worker_daemon_cancels_in_flight_anthropic_backend_request_when_proxy_client_disconnects() {
    let (proxy_addr, _) = spawn_proxy_server("anthropic").await;
    let (backend_addr, observed_request_rx, cancelled_rx) = spawn_slow_cancellation_backend().await;

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
        "stream": true,
        "max_tokens": 128,
        "messages": [{"role": "user", "content": [{"type": "text", "text": "cancel me"}]}]
    })
    .to_string();
    let mut response_stream = open_messages_request(
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

#[tokio::test]
async fn worker_daemon_refreshes_models_catalog_without_reconnect() {
    let (proxy_addr, core) = spawn_proxy_server("openai").await;
    let (backend_addr, _observed_request_rx, advertised_models) = spawn_mock_backend().await;

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

    wait_for_models_catalog(proxy_addr, &["gpt-4.1-mini"]).await;

    {
        let mut models = advertised_models.lock().await;
        *models = vec!["gpt-4.1-mini".to_string(), "gpt-oss-120b".to_string()];
    }

    let worker_id = timeout(Duration::from_secs(2), async {
        loop {
            let worker_id = {
                let core = core.lock().await;
                core.worker_ids_for_provider("openai").into_iter().next()
            };
            if let Some(worker_id) = worker_id {
                break worker_id;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("worker registered before timeout");

    {
        let mut core = core.lock().await;
        let refresh = core.request_worker_models_refresh(&worker_id, Some("test catalog sync"));
        assert!(
            refresh.is_some(),
            "refresh signal queued for connected worker"
        );
    }

    wait_for_models_catalog(proxy_addr, &["gpt-4.1-mini", "gpt-oss-120b"]).await;

    daemon_handle.abort();
}

#[tokio::test]
async fn worker_daemon_drains_in_flight_request_before_replacement_receives_queued_work() {
    let (proxy_addr, core) = spawn_proxy_server("openai").await;
    let (draining_backend_addr, first_observed_request_rx, release_response_tx) =
        spawn_delayed_responses_backend().await;
    let draining_daemon_handle =
        spawn_openai_daemon(proxy_addr, "gpu-box-a", draining_backend_addr);

    wait_for_registered_model(proxy_addr, "gpt-4.1-mini").await;
    let draining_worker_id = wait_for_worker_id(&core, "openai").await;

    let first_body = json!({"model": "gpt-4.1-mini", "input": "finish before drain"}).to_string();
    let second_body =
        json!({"model": "gpt-4.1-mini", "input": "stay queued during drain"}).to_string();

    let first_request_body = first_body.clone();
    let first_request_task = tokio::spawn(async move {
        post_responses(
            proxy_addr,
            &first_request_body,
            &[
                ("Authorization", "Bearer test-openai-key"),
                ("OpenAI-Beta", "responses=v1"),
            ],
        )
        .await
    });

    let first_observed_request = timeout(Duration::from_secs(2), first_observed_request_rx)
        .await
        .expect("first backend observed request before timeout")
        .expect("first backend observed request");
    assert_openai_responses_request(
        &first_observed_request,
        "Bearer test-openai-key",
        &first_body,
    );

    let mut queued_request_stream = open_responses_request(
        proxy_addr,
        &second_body,
        &[
            ("Authorization", "Bearer queued-openai-key"),
            ("OpenAI-Beta", "responses=v1"),
        ],
    )
    .await;

    wait_for_request_state(&core, "request-2", RequestState::Queued).await;
    begin_graceful_shutdown(&core, &draining_worker_id).await;

    assert!(
        timeout(
            Duration::from_millis(200),
            read_until_contains(&mut queued_request_stream, "HTTP/1.1"),
        )
        .await
        .is_err(),
        "queued request should stay pending while the draining worker finishes in-flight work"
    );

    release_response_tx
        .send(())
        .expect("release delayed backend response");

    let first_response = timeout(Duration::from_secs(2), first_request_task)
        .await
        .expect("first request completed before timeout")
        .expect("join first request task");
    assert_created_responses_response(
        &first_response,
        "delayed-openai-responses-backend",
        r#"{"id":"resp_drain_1","object":"response","model":"gpt-4.1-mini","output":[{"type":"message","id":"msg_drain_1","status":"completed","role":"assistant","content":[{"type":"output_text","text":"drain completed"}]}]}"#,
    );

    timeout(Duration::from_secs(2), draining_daemon_handle)
        .await
        .expect("draining daemon exited before timeout")
        .expect("join draining daemon task")
        .expect("draining daemon exited cleanly");
    wait_for_worker_disconnect(&core, &draining_worker_id).await;

    {
        let core = core.lock().await;
        assert_eq!(core.request_state("request-2"), Some(RequestState::Queued));
        assert_eq!(
            core.queued_request_ids("openai"),
            vec!["request-2".to_string()]
        );
    }

    let (replacement_backend_addr, second_observed_request_rx, _) = spawn_mock_backend().await;
    let replacement_daemon_handle =
        spawn_openai_daemon(proxy_addr, "gpu-box-b", replacement_backend_addr);

    let second_observed_request = timeout(Duration::from_secs(2), second_observed_request_rx)
        .await
        .expect("replacement backend observed request before timeout")
        .expect("replacement backend observed request");
    assert_openai_responses_request(
        &second_observed_request,
        "Bearer queued-openai-key",
        &second_body,
    );

    let second_response = read_until_contains(
        &mut queued_request_stream,
        r#"{"id":"resp_123","object":"response","model":"gpt-4.1-mini","output":[{"type":"message","id":"msg_123","status":"completed","role":"assistant","content":[{"type":"output_text","text":"responses proxy success"}]}]}"#,
    )
    .await;
    assert_created_responses_response(
        &second_response,
        "openai-responses-backend",
        r#"{"id":"resp_123","object":"response","model":"gpt-4.1-mini","output":[{"type":"message","id":"msg_123","status":"completed","role":"assistant","content":[{"type":"output_text","text":"responses proxy success"}]}]}"#,
    );

    replacement_daemon_handle.abort();
}

#[tokio::test]
async fn worker_daemon_disconnects_promptly_when_graceful_shutdown_arrives_while_idle() {
    let (proxy_addr, core) = spawn_proxy_server("openai").await;
    let (backend_addr, _observed_request_rx, _) = spawn_mock_backend().await;

    let daemon_handle = spawn_openai_daemon(proxy_addr, "gpu-box-a", backend_addr);

    wait_for_registered_model(proxy_addr, "gpt-4.1-mini").await;
    let worker_id = wait_for_worker_id(&core, "openai").await;

    {
        let core = core.lock().await;
        assert!(core.worker_in_flight_request_ids(&worker_id).is_empty());
        assert_eq!(core.queued_request_ids("openai"), Vec::<String>::new());
    }

    begin_graceful_shutdown(&core, &worker_id).await;

    timeout(Duration::from_secs(2), daemon_handle)
        .await
        .expect("idle daemon exited before timeout")
        .expect("join idle daemon task")
        .expect("idle daemon exited cleanly");
    wait_for_worker_disconnect(&core, &worker_id).await;
    wait_for_models_catalog(proxy_addr, &[]).await;
}

#[tokio::test]
async fn worker_daemon_disconnects_in_flight_backend_request_when_proxy_graceful_shutdown_times_out()
 {
    let (proxy_addr, core) = spawn_proxy_server("openai").await;
    let (backend_addr, observed_request_rx, cancelled_rx) = spawn_slow_cancellation_backend().await;

    let daemon_handle = spawn_openai_daemon(proxy_addr, "gpu-box-a", backend_addr);

    wait_for_registered_model(proxy_addr, "gpt-4.1-mini").await;
    let worker_id = wait_for_worker_id(&core, "openai").await;

    let request_body =
        json!({"model": "gpt-4.1-mini", "input": "hang until drain timeout"}).to_string();
    let response_task = tokio::spawn({
        let request_body = request_body.clone();
        async move {
            post_responses(
                proxy_addr,
                &request_body,
                &[
                    ("Authorization", "Bearer test-openai-key"),
                    ("OpenAI-Beta", "responses=v1"),
                ],
            )
            .await
        }
    });

    let observed_request = timeout(Duration::from_secs(2), observed_request_rx)
        .await
        .expect("backend observed request before timeout")
        .expect("backend observed request");
    assert_openai_responses_request(&observed_request, "Bearer test-openai-key", &request_body);

    {
        let mut core = core.lock().await;
        let signals =
            core.begin_graceful_shutdown(Some("proxy server draining"), Duration::from_millis(50));
        assert_eq!(signals.len(), 1);
        assert!(core.worker_is_draining(&worker_id));
    }

    timeout(Duration::from_secs(2), daemon_handle)
        .await
        .expect("drain-timeout daemon exited before timeout")
        .expect("join drain-timeout daemon task")
        .expect("drain-timeout daemon exited cleanly");
    wait_for_worker_disconnect(&core, &worker_id).await;
    wait_for_models_catalog(proxy_addr, &[]).await;

    timeout(Duration::from_secs(2), cancelled_rx)
        .await
        .expect("backend cancellation before timeout")
        .expect("backend cancellation result")
        .expect("backend request was canceled");

    let response = timeout(Duration::from_secs(2), response_task)
        .await
        .expect("proxy client request completed before timeout")
        .expect("join proxy client request");
    assert!(
        !response.is_empty(),
        "client path should fail with a terminal HTTP response instead of hanging"
    );

    {
        let core = core.lock().await;
        assert_eq!(core.request_state("request-1"), None);
        assert!(core.queued_request_ids("openai").is_empty());
    }
}

#[tokio::test]
async fn worker_daemon_disconnects_in_flight_anthropic_backend_request_when_proxy_graceful_shutdown_times_out()
 {
    let (proxy_addr, core) = spawn_proxy_server("anthropic").await;
    let (backend_addr, observed_request_rx, cancelled_rx) = spawn_slow_cancellation_backend().await;

    let daemon_handle = spawn_anthropic_daemon(proxy_addr, "gpu-box-a", backend_addr);

    wait_for_registered_model(proxy_addr, "claude-3-7-sonnet-20250219").await;
    let worker_id = wait_for_worker_id(&core, "anthropic").await;

    let request_body = json!({
        "model": "claude-3-7-sonnet-20250219",
        "max_tokens": 128,
        "messages": [{"role": "user", "content": [{"type": "text", "text": "hang until drain timeout"}]}]
    })
    .to_string();
    let response_task = tokio::spawn({
        let request_body = request_body.clone();
        async move {
            post_messages(
                proxy_addr,
                &request_body,
                &[
                    ("x-api-key", "test-anthropic-key"),
                    ("anthropic-version", "2023-06-01"),
                    ("anthropic-beta", "tools-2024-04-04"),
                ],
            )
            .await
        }
    });

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

    {
        let mut core = core.lock().await;
        let signals =
            core.begin_graceful_shutdown(Some("proxy server draining"), Duration::from_millis(50));
        assert_eq!(signals.len(), 1);
        assert!(core.worker_is_draining(&worker_id));
    }

    timeout(Duration::from_secs(2), daemon_handle)
        .await
        .expect("drain-timeout daemon exited before timeout")
        .expect("join drain-timeout daemon task")
        .expect("drain-timeout daemon exited cleanly");
    wait_for_worker_disconnect(&core, &worker_id).await;
    wait_for_models_catalog(proxy_addr, &[]).await;

    timeout(Duration::from_secs(2), cancelled_rx)
        .await
        .expect("backend cancellation before timeout")
        .expect("backend cancellation result")
        .expect("backend request was canceled");

    let response = timeout(Duration::from_secs(2), response_task)
        .await
        .expect("proxy client request completed before timeout")
        .expect("join proxy client request");
    assert!(
        !response.is_empty(),
        "client path should fail with a terminal HTTP response instead of hanging"
    );

    {
        let core = core.lock().await;
        assert_eq!(core.request_state("request-1"), None);
        assert!(core.queued_request_ids("anthropic").is_empty());
    }
}

#[tokio::test]
async fn worker_daemon_recovers_after_worker_auth_rate_limit_window_expires() {
    let (proxy_addr, _) = spawn_proxy_server("openai").await;
    let (backend_addr, observed_request_rx, _) = spawn_mock_backend().await;

    for _ in 0..3 {
        let (mut socket, _) = connect_async(worker_connect_request(proxy_addr, "wrong-secret"))
            .await
            .expect("connect websocket");
        let close_frame = next_close_frame(&mut socket, "bad secret rejection").await;
        assert_eq!(u16::from(close_frame.code), 1008);
        assert_eq!(close_frame.reason, "worker authentication failed");
    }

    let throttled_daemon = WorkerDaemon::new(WorkerDaemonConfig {
        proxy_base_url: format!("http://{proxy_addr}"),
        provider: "openai".to_string(),
        worker_secret: "top-secret".to_string(),
        worker_name: "gpu-box-a".to_string(),
        models: vec!["gpt-4.1-mini".to_string()],
        max_concurrent: 1,
        backend_base_url: format!("http://{backend_addr}"),
    });
    let throttled_daemon_handle = tokio::spawn(async move { throttled_daemon.run().await });

    timeout(Duration::from_secs(2), async {
        loop {
            if throttled_daemon_handle.is_finished() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cooldown-rejected daemon exits before timeout");
    let throttled_result = throttled_daemon_handle
        .await
        .expect("join cooldown-rejected daemon");
    assert!(
        throttled_result.is_ok(),
        "daemon should observe proxy close cleanly during auth cooldown"
    );
    assert_model_not_registered_for(proxy_addr, "gpt-4.1-mini", Duration::from_millis(100)).await;

    sleep(Duration::from_millis(300)).await;

    let recovered_daemon_handle = spawn_openai_daemon(proxy_addr, "gpu-box-a", backend_addr);
    wait_for_registered_model(proxy_addr, "gpt-4.1-mini").await;

    let request_body =
        json!({"model": "gpt-4.1-mini", "input": "auth cooldown expired"}).to_string();
    let response = post_responses(
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
    assert!(response.starts_with("HTTP/1.1 201 Created\r\n"));
    assert!(response.contains("\r\ncontent-type: application/json\r\n"));
    assert!(response.contains("\r\nopenai-beta: responses=v1\r\n"));
    assert!(response.contains("\r\nx-backend-trace: openai-responses-backend\r\n"));
    assert!(response.ends_with(
        r#"{"id":"resp_123","object":"response","model":"gpt-4.1-mini","output":[{"type":"message","id":"msg_123","status":"completed","role":"assistant","content":[{"type":"output_text","text":"responses proxy success"}]}]}"#
    ));

    recovered_daemon_handle.abort();
}

#[tokio::test]
async fn worker_daemon_run_with_reconnect_reconnects_after_proxy_restart() {
    // Bind a listener to claim an ephemeral port, then use it as a "flaky proxy"
    // that drops connections immediately.  After the first failed attempt the daemon
    // should back off and retry; at that point the real proxy is already listening on
    // the same address.
    let flaky_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind flaky listener");
    let proxy_addr = flaky_listener.local_addr().expect("get proxy addr");

    let (backend_addr, observed_request_rx, _) = spawn_mock_backend().await;

    // Start the daemon before the flaky listener accepts anything.
    let daemon = WorkerDaemon::new(WorkerDaemonConfig {
        proxy_base_url: format!("http://{proxy_addr}"),
        provider: "openai".to_string(),
        worker_secret: "top-secret".to_string(),
        worker_name: "gpu-box-reconnect".to_string(),
        models: vec!["gpt-4.1-mini".to_string()],
        max_concurrent: 1,
        backend_base_url: format!("http://{backend_addr}"),
    });
    let daemon_handle = tokio::spawn(async move { daemon.run_with_reconnect().await });

    // Accept one connection and immediately drop it; the daemon will receive a
    // connection-reset or EOF and trigger its reconnect path.
    let (dropped_stream, _) = timeout(Duration::from_secs(2), flaky_listener.accept())
        .await
        .expect("flaky listener accepted before timeout")
        .expect("accept connection");
    drop(dropped_stream);
    drop(flaky_listener); // release the port so the real proxy can bind it

    // Give the OS a moment to release the port.
    sleep(Duration::from_millis(50)).await;

    // Bring up the real proxy on the same address.
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let worker_socket_app = WorkerSocketApp::new(core.clone())
        .with_provider("openai", WorkerSocketProviderConfig::enabled("top-secret"));
    let real_app = ProxyHttpApp::new(core.clone())
        .with_models_provider("openai")
        .with_worker_socket_app(worker_socket_app)
        .router();

    let real_listener = TcpListener::bind(proxy_addr)
        .await
        .expect("rebind proxy port for real proxy");
    tokio::spawn(async move {
        axum::serve(real_listener, real_app)
            .await
            .expect("serve real proxy");
    });

    // After the daemon's backoff (~1 s) it should reconnect and register.
    timeout(Duration::from_secs(5), async {
        loop {
            if get_models(proxy_addr).await.1["data"]
                .as_array()
                .is_some_and(|arr| arr.iter().any(|e| e["id"] == "gpt-4.1-mini"))
            {
                return;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("daemon reconnected and registered model within 5 s");

    // Confirm the reconnected daemon can serve a real request end-to-end.
    let request_body =
        json!({"model": "gpt-4.1-mini", "input": "hello after reconnect"}).to_string();
    let response = post_responses(
        proxy_addr,
        &request_body,
        &[
            ("Authorization", "Bearer test-openai-key"),
            ("OpenAI-Beta", "responses=v1"),
        ],
    )
    .await;

    let observed_request = timeout(Duration::from_secs(2), observed_request_rx)
        .await
        .expect("backend received request before timeout")
        .expect("backend observed request");
    assert_eq!(observed_request.path, "/v1/responses");
    assert_eq!(
        observed_request.authorization,
        Some("Bearer test-openai-key".to_string())
    );
    assert_eq!(observed_request.body, request_body);
    assert!(response.starts_with("HTTP/1.1 201 Created\r\n"));
    assert!(response.contains("\r\ncontent-type: application/json\r\n"));

    daemon_handle.abort();
}
