use std::{net::SocketAddr, sync::Arc};

use futures_util::{SinkExt, StreamExt};
use modelrelay_protocol::{
    HeaderMap, ModelMetadataMap, ModelsUpdateMessage, RegisterMessage, ResponseCompleteMessage,
    ServerToWorkerMessage, WorkerToServerMessage,
};
use modelrelay_server::{
    ProxyHttpApp, ProxyServerCore, WorkerSocketApp, WorkerSocketProviderConfig,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Mutex,
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

async fn spawn_server() -> (SocketAddr, Arc<Mutex<ProxyServerCore>>) {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let worker_socket_app = WorkerSocketApp::new(core.clone())
        .with_provider("openai", WorkerSocketProviderConfig::enabled("top-secret"))
        .with_heartbeat(
            tokio::time::Duration::from_millis(100),
            tokio::time::Duration::from_millis(300),
        );
    let app = ProxyHttpApp::new(core.clone())
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

    (addr, core)
}

async fn get_json(addr: SocketAddr, path: &str) -> (u16, Value) {
    let mut stream = TcpStream::connect(addr)
        .await
        .expect("connect to test server");
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");

    stream
        .write_all(request.as_bytes())
        .await
        .expect("write http request");

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("read http response");

    let response = String::from_utf8(response).expect("response is utf-8");
    let (head, body) = response
        .split_once("\r\n\r\n")
        .expect("split http response head and body");
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .expect("parse response status");

    (
        status,
        serde_json::from_str(body.trim()).expect("parse response body"),
    )
}

fn register_message(models: &[&str]) -> RegisterMessage {
    RegisterMessage {
        worker_name: "gpu-box".to_string(),
        models: models.iter().map(|model| (*model).to_string()).collect(),
        max_concurrent: 1,
        protocol_version: Some("2026-04-bridge-v1".to_string()),
        current_load: Some(0),
        endpoint_prefixes: vec![],
        model_metadata: ModelMetadataMap::new(),
    }
}

#[tokio::test]
async fn models_endpoint_returns_openai_compatible_catalog_shape_for_live_workers() {
    let (addr, core) = spawn_server().await;

    {
        let mut core = core.lock().await;
        core.register_worker(
            "openai",
            register_message(&["llama-3.1-70b", "mistral-large", "llama-3.1-70b"]),
        );
    }

    let (status, body) = get_json(addr, "/v1/models").await;

    assert_eq!(status, 200);
    assert_eq!(
        body,
        json!({
            "object": "list",
            "data": [
                {
                    "id": "llama-3.1-70b",
                    "object": "model",
                    "owned_by": "worker-proxy"
                },
                {
                    "id": "mistral-large",
                    "object": "model",
                    "owned_by": "worker-proxy"
                }
            ]
        })
    );
}

#[tokio::test]
async fn models_endpoint_tracks_live_model_updates_and_disconnects_without_stale_entries() {
    let (addr, core) = spawn_server().await;

    let (first_worker_id, second_worker_id) = {
        let mut core = core.lock().await;
        let first = core.register_worker("openai", register_message(&["llama-3.1-70b"]));
        let second = core.register_worker("openai", register_message(&["mistral-large"]));
        (first.worker_id, second.worker_id)
    };

    let (initial_status, initial_body) = get_json(addr, "/v1/models").await;
    assert_eq!(initial_status, 200);
    assert_eq!(
        initial_body,
        json!({
            "object": "list",
            "data": [
                {
                    "id": "llama-3.1-70b",
                    "object": "model",
                    "owned_by": "worker-proxy"
                },
                {
                    "id": "mistral-large",
                    "object": "model",
                    "owned_by": "worker-proxy"
                }
            ]
        })
    );

    {
        let mut core = core.lock().await;
        assert!(
            core.update_worker_models(
                &first_worker_id,
                ModelsUpdateMessage {
                    models: vec!["llama-3.1-70b".to_string(), "gpt-oss-120b".to_string()],
                    current_load: 0,
                    model_metadata: ModelMetadataMap::new(),
                },
            )
            .is_empty()
        );
        assert!(
            core.update_worker_models(
                &second_worker_id,
                ModelsUpdateMessage {
                    models: vec!["gpt-oss-120b".to_string()],
                    current_load: 0,
                    model_metadata: ModelMetadataMap::new(),
                },
            )
            .is_empty()
        );
    }

    let (updated_status, updated_body) = get_json(addr, "/v1/models").await;
    assert_eq!(updated_status, 200);
    assert_eq!(
        updated_body,
        json!({
            "object": "list",
            "data": [
                {
                    "id": "llama-3.1-70b",
                    "object": "model",
                    "owned_by": "worker-proxy"
                },
                {
                    "id": "gpt-oss-120b",
                    "object": "model",
                    "owned_by": "worker-proxy"
                }
            ]
        })
    );

    {
        let mut core = core.lock().await;
        let disconnect = core
            .disconnect_worker(&first_worker_id)
            .expect("disconnect first worker");
        assert!(disconnect.requeued_request_ids.is_empty());
        assert!(disconnect.failed_requests.is_empty());
    }

    let (after_disconnect_status, after_disconnect_body) = get_json(addr, "/v1/models").await;
    assert_eq!(after_disconnect_status, 200);
    assert_eq!(
        after_disconnect_body,
        json!({
            "object": "list",
            "data": [
                {
                    "id": "gpt-oss-120b",
                    "object": "model",
                    "owned_by": "worker-proxy"
                }
            ]
        })
    );
}

#[tokio::test]
async fn models_endpoint_preserves_backend_metadata_from_register_and_updates() {
    let (addr, core) = spawn_server().await;

    let mut initial_metadata = ModelMetadataMap::new();
    initial_metadata.insert(
        "qwen3.5-27b".to_string(),
        json!({
            "id": "qwen3.5-27b",
            "object": "model",
            "owned_by": "loom",
            "context_length": 131_072,
            "max_output_tokens": 32_768,
            "reasoning": true,
            "reasoning_efforts": ["none", "low", "medium", "high"],
            "architecture": { "input_modalities": ["text", "image"] }
        })
        .as_object()
        .expect("metadata json object")
        .clone(),
    );

    let worker_id = {
        let mut core = core.lock().await;
        core.register_worker(
            "openai",
            RegisterMessage {
                worker_name: "gpu-box".to_string(),
                models: vec!["qwen3.5-27b".to_string()],
                max_concurrent: 2,
                protocol_version: Some("2026-04-bridge-v1".to_string()),
                current_load: Some(0),
                endpoint_prefixes: vec![],
                model_metadata: initial_metadata,
            },
        )
        .worker_id
    };

    let (status, body) = get_json(addr, "/v1/models").await;
    assert_eq!(status, 200);
    assert_eq!(body["data"][0]["id"], "qwen3.5-27b");
    assert_eq!(body["data"][0]["owned_by"], "loom");
    assert_eq!(body["data"][0]["context_length"], 131_072);
    assert_eq!(body["data"][0]["max_output_tokens"], 32_768);
    assert_eq!(body["data"][0]["reasoning"], true);
    assert_eq!(
        body["data"][0]["reasoning_efforts"],
        json!(["none", "low", "medium", "high"])
    );
    assert_eq!(
        body["data"][0]["architecture"]["input_modalities"],
        json!(["text", "image"])
    );

    // Updating metadata on ModelsUpdate (same model ID, bigger context & xhigh effort) updates /v1/models live.
    let mut updated_metadata = ModelMetadataMap::new();
    updated_metadata.insert(
        "qwen3.5-27b".to_string(),
        json!({
            "id": "qwen3.5-27b",
            "object": "model",
            "owned_by": "loom",
            "context_length": 262_144,
            "reasoning_efforts": ["none", "low", "medium", "high", "xhigh"]
        })
        .as_object()
        .expect("updated metadata json object")
        .clone(),
    );
    {
        let mut core = core.lock().await;
        core.update_worker_models(
            &worker_id,
            ModelsUpdateMessage {
                models: vec!["qwen3.5-27b".to_string()],
                current_load: 0,
                model_metadata: updated_metadata,
            },
        );
    }

    let (updated_status, updated_body) = get_json(addr, "/v1/models").await;
    assert_eq!(updated_status, 200);
    assert_eq!(updated_body["data"][0]["context_length"], 262_144);
    assert_eq!(
        updated_body["data"][0]["reasoning_efforts"],
        json!(["none", "low", "medium", "high", "xhigh"])
    );
}

#[tokio::test]
async fn models_endpoint_probes_legacy_workers_and_caches_metadata() {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let worker_socket_app = WorkerSocketApp::new(core.clone())
        .with_provider("openai", WorkerSocketProviderConfig::enabled("top-secret"));
    let app = ProxyHttpApp::new(core.clone())
        .with_worker_socket_app(worker_socket_app)
        .with_legacy_models_probe(true)
        .router();

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("listener local addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve proxy app");
    });

    let mut req = format!("ws://{addr}/v1/worker/connect?provider=openai")
        .into_client_request()
        .expect("ws request");
    req.headers_mut().insert(
        "x-worker-secret",
        "top-secret".parse().expect("secret header"),
    );
    let (mut ws, _) = connect_async(req).await.expect("connect legacy worker");

    // Register WITHOUT model_metadata (legacy worker payload)
    let legacy_reg = r#"{"type":"register","worker_name":"legacy-gpu","models":["inkling-2-mini"],"max_concurrent":2,"protocol_version":"2026-04-bridge-v1","current_load":0}"#;
    ws.send(Message::Text(legacy_reg.into()))
        .await
        .expect("send legacy register");
    let _ = ws.next().await.expect("register ack").expect("ws text");

    // Simulate the legacy worker answering a GET /v1/models probe once.
    let probe_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let probe_count_worker = Arc::clone(&probe_count);
    let worker_task = tokio::spawn(async move {
        while let Some(Ok(Message::Text(text))) = ws.next().await {
            if let Ok(ServerToWorkerMessage::Request(req)) = serde_json::from_str(&text) {
                assert_eq!(req.method, "GET");
                assert_eq!(req.endpoint_path, "/v1/models");
                probe_count_worker.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let resp = WorkerToServerMessage::ResponseComplete(ResponseCompleteMessage {
                    request_id: req.request_id,
                    status_code: 200,
                    headers: HeaderMap::from([(
                        "content-type".to_string(),
                        "application/json".to_string(),
                    )]),
                    body: Some(
                        json!({
                            "object": "list",
                            "data": [{
                                "id": "inkling-2-mini",
                                "object": "model",
                                "owned_by": "loom",
                                "context_length": 131_072,
                                "max_output_tokens": 16_384,
                                "reasoning": true,
                                "reasoning_efforts": ["none", "low", "medium", "high"]
                            }]
                        })
                        .to_string(),
                    ),
                    token_counts: None,
                });
                let payload = serde_json::to_string(&resp).expect("serialize probe response");
                ws.send(Message::Text(payload.into()))
                    .await
                    .expect("send probe response");
            }
        }
    });

    // First call probes the legacy worker and returns rich metadata.
    let (status1, body1) = get_json(addr, "/v1/models").await;
    assert_eq!(status1, 200);
    assert_eq!(body1["data"][0]["id"], "inkling-2-mini");
    assert_eq!(body1["data"][0]["context_length"], 131_072);
    assert_eq!(body1["data"][0]["max_output_tokens"], 16_384);
    assert_eq!(
        body1["data"][0]["reasoning_efforts"],
        json!(["none", "low", "medium", "high"])
    );

    // Second call uses the cached metadata without re-probing the worker.
    let (status2, body2) = get_json(addr, "/v1/models").await;
    assert_eq!(status2, 200);
    assert_eq!(body2, body1);
    assert_eq!(probe_count.load(std::sync::atomic::Ordering::SeqCst), 1);

    worker_task.abort();
}

async fn send_raw_http_status(addr: SocketAddr, request: &str) -> u16 {
    let mut stream = TcpStream::connect(addr)
        .await
        .expect("connect to test server");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write http request");

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("read http response");
    let response = String::from_utf8_lossy(&response);
    response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .expect("parse response status")
}

#[tokio::test]
async fn unsupported_probe_endpoints_fast_fail_with_404_instead_of_queuing() {
    let (addr, core) = spawn_server().await;

    {
        let mut core = core.lock().await;
        core.register_worker(
            "openai",
            RegisterMessage {
                worker_name: "chat-only-worker".to_string(),
                models: vec!["qwen3.5-27b".to_string()],
                max_concurrent: 2,
                protocol_version: Some("2026-04-bridge-v1".to_string()),
                current_load: Some(0),
                endpoint_prefixes: vec!["/v1/chat".to_string()],
                model_metadata: ModelMetadataMap::new(),
            },
        );
    }

    let requests = [
        format!(
            "GET /props?model=qwen3.5-27b&autoload=false HTTP/1.1\r\nHost: {addr}\r\nX-Model: qwen3.5-27b\r\nConnection: close\r\n\r\n"
        ),
        format!(
            "GET /api/v1/models HTTP/1.1\r\nHost: {addr}\r\nX-Model: qwen3.5-27b\r\nConnection: close\r\n\r\n"
        ),
        format!(
            "GET /api/v0/models HTTP/1.1\r\nHost: {addr}\r\nX-Model: qwen3.5-27b\r\nConnection: close\r\n\r\n"
        ),
        format!(
            "POST /api/show HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: 23\r\nX-Model: qwen3.5-27b\r\nConnection: close\r\n\r\n{{\"model\":\"qwen3.5-27b\"}}"
        ),
    ];

    for raw_req in requests {
        let status = tokio::time::timeout(
            tokio::time::Duration::from_secs(1),
            send_raw_http_status(addr, &raw_req),
        )
        .await
        .expect("unsupported probe path must fail fast instead of hanging in queue");
        assert_eq!(status, 404);
    }
}

#[tokio::test]
async fn hanging_metadata_probe_times_out_promptly_instead_of_request_timeout() {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let worker_socket_app = WorkerSocketApp::new(core.clone())
        .with_provider("openai", WorkerSocketProviderConfig::enabled("top-secret"));
    let app = ProxyHttpApp::new(core.clone())
        .with_worker_socket_app(worker_socket_app)
        .with_request_timeout(Some(tokio::time::Duration::from_secs(300)))
        .router();

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("listener local addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve proxy app");
    });

    let mut req = format!("ws://{addr}/v1/worker/connect?provider=openai")
        .into_client_request()
        .expect("ws request");
    req.headers_mut().insert(
        "x-worker-secret",
        "top-secret".parse().expect("secret header"),
    );
    let (mut ws, _) = connect_async(req).await.expect("connect silent worker");
    let reg = r#"{"type":"register","worker_name":"silent-worker","models":["qwen3.5-27b"],"max_concurrent":2,"protocol_version":"2026-04-bridge-v1","current_load":0}"#;
    ws.send(Message::Text(reg.into()))
        .await
        .expect("send register");
    let _ = ws.next().await.expect("register ack").expect("ws text");

    // Keep WS open but never answer the dispatched /props probe.
    let worker_task = tokio::spawn(async move { while let Some(Ok(_)) = ws.next().await {} });

    let started = std::time::Instant::now();
    let raw_req = format!(
        "GET /props?model=qwen3.5-27b&autoload=false HTTP/1.1\r\nHost: {addr}\r\nX-Model: qwen3.5-27b\r\nConnection: close\r\n\r\n"
    );
    let status = tokio::time::timeout(
        tokio::time::Duration::from_secs(7),
        send_raw_http_status(addr, &raw_req),
    )
    .await
    .expect("hanging /props probe must hit short metadata timeout, not 300s request timeout");
    assert_eq!(status, 504);
    assert!(
        started.elapsed() < tokio::time::Duration::from_secs(6),
        "expected prompt metadata probe timeout, took {:?}",
        started.elapsed()
    );

    worker_task.abort();
}

#[tokio::test]
async fn models_endpoint_legacy_probe_falls_back_from_v1_models_to_models() {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let worker_socket_app = WorkerSocketApp::new(core.clone())
        .with_provider("openai", WorkerSocketProviderConfig::enabled("top-secret"));
    let app = ProxyHttpApp::new(core.clone())
        .with_worker_socket_app(worker_socket_app)
        .with_legacy_models_probe(true)
        .router();

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("listener local addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve proxy app");
    });

    let mut req = format!("ws://{addr}/v1/worker/connect?provider=openai")
        .into_client_request()
        .expect("ws request");
    req.headers_mut().insert(
        "x-worker-secret",
        "top-secret".parse().expect("secret header"),
    );
    let (mut ws, _) = connect_async(req).await.expect("connect legacy worker");

    let legacy_reg = r#"{"type":"register","worker_name":"ryzen-box","models":["inkling-2-mini"],"max_concurrent":2,"protocol_version":"2026-04-bridge-v1","current_load":0}"#;
    ws.send(Message::Text(legacy_reg.into()))
        .await
        .expect("send legacy register");
    let _ = ws.next().await.expect("register ack").expect("ws text");

    let worker_task = tokio::spawn(async move {
        while let Some(Ok(Message::Text(text))) = ws.next().await {
            if let Ok(ServerToWorkerMessage::Request(req)) = serde_json::from_str(&text) {
                let resp = if req.endpoint_path == "/v1/models" {
                    ResponseCompleteMessage {
                        request_id: req.request_id,
                        status_code: 404,
                        headers: HeaderMap::new(),
                        body: Some("Not Found".to_string()),
                        token_counts: None,
                    }
                } else {
                    assert_eq!(req.endpoint_path, "/models");
                    ResponseCompleteMessage {
                        request_id: req.request_id,
                        status_code: 200,
                        headers: HeaderMap::from([(
                            "content-type".to_string(),
                            "application/json".to_string(),
                        )]),
                        body: Some(
                            json!({
                                "object": "list",
                                "data": [{
                                    "id": "inkling-2-mini",
                                    "object": "model",
                                    "status": { "value": "loaded" },
                                    "meta": { "n_ctx": 262_144 },
                                    "context_window": 262_144,
                                    "max_output_tokens": 262_136,
                                    "supported_reasoning_efforts": ["none", "minimal", "low", "medium", "high", "xhigh", "max"]
                                }]
                            })
                            .to_string(),
                        ),
                        token_counts: None,
                    }
                };
                let payload = serde_json::to_string(&WorkerToServerMessage::ResponseComplete(resp))
                    .expect("serialize probe response");
                ws.send(Message::Text(payload.into()))
                    .await
                    .expect("send probe response");
            }
        }
    });

    let (status, body) = get_json(addr, "/v1/models").await;
    assert_eq!(status, 200);
    assert_eq!(body["data"][0]["id"], "inkling-2-mini");
    assert_eq!(body["data"][0]["context_window"], 262_144);
    assert_eq!(body["data"][0]["max_output_tokens"], 262_136);
    assert_eq!(
        body["data"][0]["supported_reasoning_efforts"],
        json!(["none", "minimal", "low", "medium", "high", "xhigh", "max"])
    );

    worker_task.abort();
}
