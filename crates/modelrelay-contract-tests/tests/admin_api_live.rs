//! End-to-end integration tests for the admin HTTP API.
//!
//! Spins up a real `modelrelay-server` with an in-memory key store and exercises
//! the admin endpoints through HTTP, deserializing responses into the shared
//! `modelrelay-protocol::admin_api` types to verify the wire contract.

use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;

use modelrelay_protocol::admin_api::{AdminKeysResponse, CreateKeyResponse};
use modelrelay_server::{InMemoryApiKeyStore, ProxyHttpApp, ProxyServerCore};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

const ADMIN_TOKEN: &str = "test-admin-secret";

async fn spawn_server() -> SocketAddr {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let store: Arc<dyn modelrelay_server::ApiKeyStore> = Arc::new(InMemoryApiKeyStore::new());
    let app = ProxyHttpApp::new(core)
        .with_admin_token(Some(ADMIN_TOKEN.to_string()))
        .with_api_key_store(store)
        .router();

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("listener addr");

    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    addr
}

async fn http_request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
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
        .expect("write request");

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("read response");

    let response = String::from_utf8(response).expect("utf-8");
    let (head, body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .expect("parse status");

    (status, body.trim().to_string())
}

/// Create a key via the admin API and deserialize the response into the shared
/// `CreateKeyResponse` type. This is the core contract test — if the server's wire
/// format ever diverges from the shared types, this test fails.
#[tokio::test]
async fn create_key_deserializes_into_shared_type() {
    let addr = spawn_server().await;

    let (status, body) = http_request(
        addr,
        "POST",
        "/admin/keys",
        &[("Authorization", &format!("Bearer {ADMIN_TOKEN}"))],
        Some(r#"{"name": "contract-test-key"}"#),
    )
    .await;

    assert_eq!(status, 201, "expected 201, got {status}: {body}");

    let response: CreateKeyResponse = serde_json::from_str(&body)
        .expect("deserialize server response into shared CreateKeyResponse");

    assert_eq!(response.metadata.name, "contract-test-key");
    assert!(!response.metadata.id.is_empty(), "id must be non-empty");
    assert!(
        response.secret.starts_with("mr_live_"),
        "secret must start with mr_live_ prefix, got: {}",
        response.secret
    );
    assert!(!response.metadata.revoked);
    assert!(response.metadata.created_at > 0);
}

/// List keys via the admin API and deserialize into the shared `AdminKeysResponse` type.
#[tokio::test]
async fn list_keys_deserializes_into_shared_type() {
    let addr = spawn_server().await;

    // Create two keys first
    for name in &["list-test-1", "list-test-2"] {
        let (status, _) = http_request(
            addr,
            "POST",
            "/admin/keys",
            &[("Authorization", &format!("Bearer {ADMIN_TOKEN}"))],
            Some(&format!(r#"{{"name": "{name}"}}"#)),
        )
        .await;
        assert_eq!(status, 201);
    }

    let (status, body) = http_request(
        addr,
        "GET",
        "/admin/keys",
        &[("Authorization", &format!("Bearer {ADMIN_TOKEN}"))],
        None,
    )
    .await;

    assert_eq!(status, 200, "expected 200, got {status}: {body}");

    let response: AdminKeysResponse = serde_json::from_str(&body)
        .expect("deserialize server response into shared AdminKeysResponse");

    assert_eq!(response.keys.len(), 2, "expected 2 keys");
    assert!(response.keys.iter().any(|k| k.name == "list-test-1"));
    assert!(response.keys.iter().any(|k| k.name == "list-test-2"));

    // Keys in list response must NOT contain full-length secrets (40 chars).
    // The prefix field legitimately contains the mr_live_ prefix + first 8 random chars,
    // so we check that no full-length key appears (prefix is 16 chars, full key is 40).
    for key in &response.keys {
        assert!(
            key.prefix.len() < 20,
            "prefix should be short, not a full secret: {}",
            key.prefix
        );
    }
}

/// Full lifecycle: create → list → revoke → list, all deserialized via shared types.
#[tokio::test]
async fn full_key_lifecycle_via_shared_types() {
    let addr = spawn_server().await;
    let auth = format!("Bearer {ADMIN_TOKEN}");

    // Create
    let (status, body) = http_request(
        addr,
        "POST",
        "/admin/keys",
        &[("Authorization", &auth)],
        Some(r#"{"name": "lifecycle-key"}"#),
    )
    .await;
    assert_eq!(status, 201);
    let created: CreateKeyResponse = serde_json::from_str(&body).expect("deserialize create");
    let key_id = &created.metadata.id;

    // List — should contain the key, not revoked
    let (_, body) = http_request(
        addr,
        "GET",
        "/admin/keys",
        &[("Authorization", &auth)],
        None,
    )
    .await;
    let listed: AdminKeysResponse = serde_json::from_str(&body).expect("deserialize list");
    let found = listed
        .keys
        .iter()
        .find(|k| k.id == *key_id)
        .expect("key in list");
    assert!(!found.revoked, "key should not be revoked yet");

    // Revoke
    let (status, _) = http_request(
        addr,
        "DELETE",
        &format!("/admin/keys/{key_id}"),
        &[("Authorization", &auth)],
        None,
    )
    .await;
    assert_eq!(status, 204);

    // List again — key should now be revoked
    let (_, body) = http_request(
        addr,
        "GET",
        "/admin/keys",
        &[("Authorization", &auth)],
        None,
    )
    .await;
    let listed: AdminKeysResponse =
        serde_json::from_str(&body).expect("deserialize list after revoke");
    let found = listed
        .keys
        .iter()
        .find(|k| k.id == *key_id)
        .expect("key still in list");
    assert!(found.revoked, "key should be revoked");
}
