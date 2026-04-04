use std::{net::SocketAddr, sync::Arc};

use modelrelay_protocol::RegisterMessage;
use modelrelay_server::{
    ProxyHttpApp, ProxyServerCore, WorkerSocketApp, WorkerSocketProviderConfig,
};
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Mutex,
};

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
async fn health_endpoint_returns_ok_with_no_workers() {
    let (addr, _core) = spawn_server().await;

    let (status, body) = get_json(addr, "/health").await;

    assert_eq!(status, 200);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["workers_connected"], 0);
    assert_eq!(body["queue_depth"], 0);
    assert!(body["version"].is_string());
    assert!(!body["version"].as_str().unwrap().is_empty());
    assert!(body["uptime_secs"].as_f64().unwrap() >= 0.0);
}

#[tokio::test]
async fn health_endpoint_reflects_connected_workers() {
    let (addr, core) = spawn_server().await;

    {
        let mut core = core.lock().await;
        core.register_worker("openai", register_message(&["llama-3.1-70b"]));
        core.register_worker("openai", register_message(&["mistral-large"]));
    }

    let (status, body) = get_json(addr, "/health").await;

    assert_eq!(status, 200);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["workers_connected"], 2);
    assert_eq!(body["queue_depth"], 0);
}

#[tokio::test]
async fn health_endpoint_reflects_worker_disconnect() {
    let (addr, core) = spawn_server().await;

    let worker_id = {
        let mut core = core.lock().await;
        let registered = core.register_worker("openai", register_message(&["llama-3.1-70b"]));
        registered.worker_id
    };

    let (_, body_before) = get_json(addr, "/health").await;
    assert_eq!(body_before["workers_connected"], 1);

    {
        let mut core = core.lock().await;
        core.disconnect_worker(&worker_id);
    }

    let (_, body_after) = get_json(addr, "/health").await;
    assert_eq!(body_after["workers_connected"], 0);
}
