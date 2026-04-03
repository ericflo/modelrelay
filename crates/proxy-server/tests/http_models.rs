use std::{net::SocketAddr, sync::Arc};

use proxy_server::{ProxyHttpApp, ProxyServerCore, WorkerSocketApp, WorkerSocketProviderConfig};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Mutex,
};
use worker_protocol::{ModelsUpdateMessage, RegisterMessage};

async fn spawn_server() -> (SocketAddr, Arc<Mutex<ProxyServerCore>>) {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let worker_socket_app = WorkerSocketApp::new(core.clone())
        .with_provider("openai", WorkerSocketProviderConfig::enabled("top-secret"));
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
