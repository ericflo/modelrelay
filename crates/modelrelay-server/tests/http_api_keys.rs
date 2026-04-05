use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;

use modelrelay_server::{
    ApiKeyStore, InMemoryApiKeyStore, ProviderQueuePolicy, ProxyHttpApp, ProxyServerCore,
    WorkerSocketApp, WorkerSocketProviderConfig,
};
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Mutex,
};

struct TestServer {
    addr: SocketAddr,
}

fn new_store() -> Arc<dyn ApiKeyStore> {
    Arc::new(InMemoryApiKeyStore::new())
}

async fn spawn_server(require_api_keys: bool, api_key_store: Arc<dyn ApiKeyStore>) -> TestServer {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    {
        let mut c = core.lock().await;
        // Configure queue with max_queue_len=0 so requests without workers
        // get an immediate 503 instead of hanging in the queue.
        c.configure_provider_queue(
            "openai",
            ProviderQueuePolicy {
                max_queue_len: 0,
                queue_timeout_ticks: None,
            },
        );
    }
    let worker_socket_app = WorkerSocketApp::new(core.clone())
        .with_provider("openai", WorkerSocketProviderConfig::enabled("top-secret"));
    let app = ProxyHttpApp::new(core)
        .with_worker_socket_app(worker_socket_app)
        .with_admin_token(Some("admin-secret".to_string()))
        .with_require_api_keys(require_api_keys)
        .with_api_key_store(api_key_store)
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

    TestServer { addr }
}

async fn http_request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> (u16, String) {
    let mut stream = TcpStream::connect(addr)
        .await
        .expect("connect to test server");

    let body_bytes = body.unwrap_or("");
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    for (name, value) in headers {
        let _ = write!(request, "{name}: {value}\r\n");
    }
    if body.is_some() {
        let _ = write!(request, "Content-Length: {}\r\n", body_bytes.len());
        request.push_str("Content-Type: application/json\r\n");
    }
    request.push_str("\r\n");
    request.push_str(body_bytes);

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
    let (head, body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .expect("parse response status");

    (status, body.trim().to_string())
}

// --- Admin key management tests ---

#[tokio::test]
async fn admin_create_key_returns_secret_once() {
    let store = new_store();
    let server = spawn_server(false, store).await;

    let (status, body) = http_request(
        server.addr,
        "POST",
        "/admin/keys",
        &[("Authorization", "Bearer admin-secret")],
        Some(r#"{"name": "test-key"}"#),
    )
    .await;

    assert_eq!(status, 201);
    let json: Value = serde_json::from_str(&body).expect("parse create key response");
    assert_eq!(json["name"], "test-key");
    assert!(json["id"].is_string());
    assert!(json["prefix"].is_string());
    assert!(json["created_at"].is_number());
    assert!(json["secret"].is_string());
    let secret = json["secret"].as_str().unwrap();
    assert!(
        secret.starts_with("mr_live_"),
        "secret should start with mr_live_ prefix"
    );
    assert!(!json["revoked"].as_bool().unwrap());
}

#[tokio::test]
async fn admin_create_key_requires_admin_auth() {
    let store = new_store();
    let server = spawn_server(false, store).await;

    let (status, _) = http_request(
        server.addr,
        "POST",
        "/admin/keys",
        &[],
        Some(r#"{"name": "test-key"}"#),
    )
    .await;

    assert_eq!(status, 403);
}

#[tokio::test]
async fn admin_list_keys_returns_metadata_without_secrets() {
    let store = new_store();
    store.create_key("key-one".to_string()).await.unwrap();
    store.create_key("key-two".to_string()).await.unwrap();

    let server = spawn_server(false, store).await;

    let (status, body) = http_request(
        server.addr,
        "GET",
        "/admin/keys",
        &[("Authorization", "Bearer admin-secret")],
        None,
    )
    .await;

    assert_eq!(status, 200);
    let json: Value = serde_json::from_str(&body).expect("parse list keys response");
    let keys = json["keys"].as_array().expect("keys is array");
    assert_eq!(keys.len(), 2);

    for key in keys {
        assert!(key["id"].is_string());
        assert!(key["name"].is_string());
        assert!(key["prefix"].is_string());
        assert!(key["created_at"].is_number());
        // Secret must never appear in list response
        assert!(key.get("secret").is_none() || key["secret"].is_null());
        assert!(key.get("hash").is_none() || key["hash"].is_null());
    }
}

#[tokio::test]
async fn admin_revoke_key_marks_key_as_revoked() {
    let store = new_store();
    let (metadata, _) = store.create_key("to-revoke".to_string()).await.unwrap();

    let server = spawn_server(false, store).await;

    let (status, _) = http_request(
        server.addr,
        "DELETE",
        &format!("/admin/keys/{}", metadata.id),
        &[("Authorization", "Bearer admin-secret")],
        None,
    )
    .await;

    assert_eq!(status, 204);
}

#[tokio::test]
async fn admin_revoke_nonexistent_key_returns_404() {
    let store = new_store();
    let server = spawn_server(false, store).await;

    let (status, _) = http_request(
        server.addr,
        "DELETE",
        "/admin/keys/nonexistent-id",
        &[("Authorization", "Bearer admin-secret")],
        None,
    )
    .await;

    assert_eq!(status, 404);
}

// --- Client API key auth tests ---

#[tokio::test]
async fn v1_route_works_without_auth_when_api_keys_not_required() {
    let store = new_store();
    let server = spawn_server(false, store).await;

    // No workers, so we expect 503 (no workers available), not 401
    let (status, _) = http_request(
        server.addr,
        "POST",
        "/v1/chat/completions",
        &[],
        Some(r#"{"model": "test-model", "stream": false}"#),
    )
    .await;

    assert_eq!(status, 503, "should get 503 (no workers), not 401");
}

#[tokio::test]
async fn v1_route_returns_401_without_bearer_when_api_keys_required() {
    let store = new_store();
    let server = spawn_server(true, store).await;

    let (status, body) = http_request(
        server.addr,
        "POST",
        "/v1/chat/completions",
        &[],
        Some(r#"{"model": "test-model", "stream": false}"#),
    )
    .await;

    assert_eq!(status, 401);
    let json: Value = serde_json::from_str(&body).expect("parse error response");
    assert_eq!(json["error"]["type"], "auth_error");
}

#[tokio::test]
async fn v1_route_returns_401_with_invalid_bearer_when_api_keys_required() {
    let store = new_store();
    let server = spawn_server(true, store).await;

    let (status, body) = http_request(
        server.addr,
        "POST",
        "/v1/chat/completions",
        &[("Authorization", "Bearer mr_live_invalid_key_here")],
        Some(r#"{"model": "test-model", "stream": false}"#),
    )
    .await;

    assert_eq!(status, 401);
    let json: Value = serde_json::from_str(&body).expect("parse error response");
    assert_eq!(json["error"]["type"], "auth_error");
}

#[tokio::test]
async fn v1_route_accepts_valid_api_key_when_required() {
    let store = new_store();
    let (_, secret) = store.create_key("valid-key".to_string()).await.unwrap();

    let server = spawn_server(true, store).await;

    // With a valid key, we should pass auth and get 503 (no workers), not 401
    let (status, _) = http_request(
        server.addr,
        "POST",
        "/v1/chat/completions",
        &[("Authorization", &format!("Bearer {secret}"))],
        Some(r#"{"model": "test-model", "stream": false}"#),
    )
    .await;

    assert_eq!(
        status, 503,
        "should get 503 (no workers) after passing auth, not 401"
    );
}

#[tokio::test]
async fn revoked_key_returns_401_on_v1_routes() {
    let store = new_store();
    let (metadata, secret) = store.create_key("will-revoke".to_string()).await.unwrap();
    store.revoke_key(&metadata.id).await.unwrap();

    let server = spawn_server(true, store).await;

    let (status, _) = http_request(
        server.addr,
        "POST",
        "/v1/chat/completions",
        &[("Authorization", &format!("Bearer {secret}"))],
        Some(r#"{"model": "test-model", "stream": false}"#),
    )
    .await;

    assert_eq!(status, 401, "revoked key should be rejected");
}

#[tokio::test]
async fn messages_route_enforces_api_key_auth() {
    let store = new_store();
    let server = spawn_server(true, store).await;

    let (status, _) = http_request(
        server.addr,
        "POST",
        "/v1/messages",
        &[],
        Some(r#"{"model": "claude-3", "stream": false}"#),
    )
    .await;

    assert_eq!(status, 401);
}

#[tokio::test]
async fn responses_route_enforces_api_key_auth() {
    let store = new_store();
    let server = spawn_server(true, store).await;

    let (status, _) = http_request(
        server.addr,
        "POST",
        "/v1/responses",
        &[],
        Some(r#"{"model": "gpt-4o", "stream": false}"#),
    )
    .await;

    assert_eq!(status, 401);
}
