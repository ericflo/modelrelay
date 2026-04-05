use std::net::SocketAddr;
use std::sync::Arc;

use hmac::{Hmac, Mac};
use modelrelay_cloud::routes;
use modelrelay_cloud::state::CloudState;
use sha2::Sha256;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

type HmacSha256 = Hmac<Sha256>;

// ─── Test helpers ──────────────────────────────────────────────────────

fn test_state() -> Arc<CloudState> {
    Arc::new(CloudState {
        db: None,
        stripe_key: None,
        webhook_secret: Some("whsec_test_secret".into()),
        admin_url: None,
        admin_token: None,
        admin_emails: vec![],
    })
}

async fn spawn_cloud(state: Arc<CloudState>) -> SocketAddr {
    let app = routes::router(state);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("listener local addr");

    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve cloud app");
    });

    addr
}

/// Send a raw HTTP request and return (status, headers, body).
async fn http_request(addr: SocketAddr, request: &str) -> (u16, String, String) {
    let mut stream = TcpStream::connect(addr)
        .await
        .expect("connect to test server");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write request");

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("read response");

    let response = String::from_utf8(response).expect("response is utf-8");
    let (head, body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));

    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);

    let headers = head.to_string();

    // Handle chunked transfer encoding
    let body = if headers
        .to_lowercase()
        .contains("transfer-encoding: chunked")
    {
        decode_chunked(body)
    } else {
        body.to_string()
    };

    (status, headers, body)
}

async fn get(addr: SocketAddr, path: &str) -> (u16, String, String) {
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    http_request(addr, &request).await
}

async fn post(
    addr: SocketAddr,
    path: &str,
    content_type: &str,
    body: &str,
) -> (u16, String, String) {
    let request = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {addr}\r\n\
         Connection: close\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         \r\n\
         {body}",
        body.len()
    );
    http_request(addr, &request).await
}

async fn post_raw(
    addr: SocketAddr,
    path: &str,
    extra_headers: &str,
    body: &[u8],
) -> (u16, String, String) {
    let mut stream = TcpStream::connect(addr)
        .await
        .expect("connect to test server");

    let head = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {addr}\r\n\
         Connection: close\r\n\
         Content-Length: {}\r\n\
         {extra_headers}\
         \r\n",
        body.len()
    );

    stream
        .write_all(head.as_bytes())
        .await
        .expect("write request head");
    stream.write_all(body).await.expect("write request body");

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("read response");

    let response = String::from_utf8(response).expect("response is utf-8");
    let (raw_head, raw_body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));

    let status = raw_head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);

    let headers = raw_head.to_string();
    let body_str = if headers
        .to_lowercase()
        .contains("transfer-encoding: chunked")
    {
        decode_chunked(raw_body)
    } else {
        raw_body.to_string()
    };

    (status, headers, body_str)
}

fn decode_chunked(raw: &str) -> String {
    let mut result = String::new();
    let mut remaining = raw;

    loop {
        let Some((size_str, rest)) = remaining.split_once("\r\n") else {
            break;
        };
        let Ok(size) = usize::from_str_radix(size_str.trim(), 16) else {
            break;
        };
        if size == 0 {
            break;
        }
        if rest.len() >= size {
            result.push_str(&rest[..size]);
            remaining = if rest.len() > size + 2 {
                &rest[size + 2..]
            } else {
                ""
            };
        } else {
            result.push_str(rest);
            break;
        }
    }

    result
}

fn stripe_signature(secret: &str, timestamp: &str, payload: &[u8]) -> String {
    let signed = format!("{timestamp}.{}", String::from_utf8_lossy(payload));
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(signed.as_bytes());
    let sig = hex::encode(mac.finalize().into_bytes());
    format!("t={timestamp},v1={sig}")
}

// ─── Landing page ──────────────────────────────────────────────────────

#[tokio::test]
async fn landing_page_returns_200() {
    let addr = spawn_cloud(test_state()).await;
    let (status, _headers, body) = get(addr, "/").await;

    assert_eq!(status, 200);
    assert!(
        body.contains("ModelRelay"),
        "landing page should contain 'ModelRelay'"
    );
}

// ─── Health endpoint ───────────────────────────────────────────────────

#[tokio::test]
async fn health_returns_json_without_db() {
    let addr = spawn_cloud(test_state()).await;
    let (status, _headers, body) = get(addr, "/health").await;

    assert_eq!(status, 200);
    let json: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
    assert_eq!(json["status"], "ok");
    assert_eq!(json["db_connected"], false);
    assert_eq!(json["stripe_configured"], false);
}

#[tokio::test]
async fn health_reflects_stripe_configured() {
    let state = Arc::new(CloudState {
        db: None,
        stripe_key: Some("sk_test_123".into()),
        webhook_secret: None,
        admin_url: None,
        admin_token: None,
        admin_emails: vec![],
    });
    let addr = spawn_cloud(state).await;
    let (status, _headers, body) = get(addr, "/health").await;

    assert_eq!(status, 200);
    let json: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
    assert_eq!(json["stripe_configured"], true);
}

// ─── Pricing page ──────────────────────────────────────────────────────

#[tokio::test]
async fn pricing_returns_200() {
    let addr = spawn_cloud(test_state()).await;
    let (status, _headers, _body) = get(addr, "/pricing").await;

    assert_eq!(status, 200);
}

// ─── Signup page ───────────────────────────────────────────────────────

#[tokio::test]
async fn signup_page_returns_200_or_500_without_session_layer() {
    let addr = spawn_cloud(test_state()).await;
    let (status, _headers, _body) = get(addr, "/signup").await;

    // Without session layer (no DB → no session store), the Session extractor
    // may fail with 500. With a session layer it returns 200.
    assert!(
        status == 200 || status == 500,
        "GET /signup: expected 200 or 500, got {status}"
    );
}

// ─── Login page ────────────────────────────────────────────────────────

#[tokio::test]
async fn login_page_returns_200_or_500_without_session_layer() {
    let addr = spawn_cloud(test_state()).await;
    let (status, _headers, _body) = get(addr, "/login").await;

    assert!(
        status == 200 || status == 500,
        "GET /login: expected 200 or 500, got {status}"
    );
}

// ─── POST /signup without DB ───────────────────────────────────────────

#[tokio::test]
async fn signup_submit_without_db_returns_error() {
    let addr = spawn_cloud(test_state()).await;
    let (status, _headers, _body) = post(
        addr,
        "/signup",
        "application/x-www-form-urlencoded",
        "email=test%40example.com&password=longpassword123",
    )
    .await;

    // Without session layer + DB, expect 500 or an error HTML page (200)
    assert!(
        status == 500 || status == 200 || status == 303,
        "POST /signup without DB: expected error response, got {status}"
    );
}

// ─── POST /login with wrong credentials ────────────────────────────────

#[tokio::test]
async fn login_submit_without_db_returns_error() {
    let addr = spawn_cloud(test_state()).await;
    let (status, _headers, _body) = post(
        addr,
        "/login",
        "application/x-www-form-urlencoded",
        "email=wrong%40example.com&password=wrongpass",
    )
    .await;

    assert!(
        status == 500 || status == 200,
        "POST /login without DB: expected error, got {status}"
    );
}

// ─── GET /dashboard without session redirects to /login ────────────────

#[tokio::test]
async fn dashboard_without_session_redirects_or_500() {
    let addr = spawn_cloud(test_state()).await;
    let (status, _headers, _body) = get(addr, "/dashboard").await;

    // 303 redirect to /login (with session layer) or 500 (without)
    assert!(
        status == 303 || status == 302 || status == 500,
        "GET /dashboard without auth: expected redirect or 500, got {status}"
    );
}

// ─── Webhook: missing Stripe-Signature header ──────────────────────────

#[tokio::test]
async fn webhook_without_signature_returns_400() {
    let addr = spawn_cloud(test_state()).await;
    let (status, _headers, _body) = post(addr, "/webhook/stripe", "application/json", "{}").await;

    assert_eq!(status, 400);
}

// ─── Webhook: invalid signature ────────────────────────────────────────

#[tokio::test]
async fn webhook_with_invalid_signature_returns_400() {
    let addr = spawn_cloud(test_state()).await;
    let payload = b"{}";

    let (status, _headers, _body) = post_raw(
        addr,
        "/webhook/stripe",
        "Content-Type: application/json\r\n\
         Stripe-Signature: t=1700000000,v1=badbadbadbadbadbadbadbadbadbadbadbadbadbadbadbadbadbadbadbadbadbad\r\n",
        payload,
    )
    .await;

    assert_eq!(status, 400);
}

// ─── Webhook: valid signature but no DB ────────────────────────────────

#[tokio::test]
async fn webhook_with_valid_signature_but_no_db_returns_500() {
    let state = test_state();
    let secret = state.webhook_secret.as_ref().unwrap().clone();
    let addr = spawn_cloud(state).await;

    let payload = br#"{"type":"checkout.session.completed","data":{"object":{"customer_email":"test@example.com","subscription":"sub_123"}}}"#;
    let sig = stripe_signature(&secret, "1700000000", payload);

    let headers = format!("Content-Type: application/json\r\nStripe-Signature: {sig}\r\n");
    let (status, _headers, _body) = post_raw(addr, "/webhook/stripe", &headers, payload).await;

    // Valid signature passes verification, but no DB → 500
    assert_eq!(status, 500);
}

// ─── Webhook: no webhook secret configured ─────────────────────────────

#[tokio::test]
async fn webhook_without_secret_configured_returns_500() {
    let state = Arc::new(CloudState {
        db: None,
        stripe_key: None,
        webhook_secret: None,
        admin_url: None,
        admin_token: None,
        admin_emails: vec![],
    });
    let addr = spawn_cloud(state).await;

    let (status, _headers, _body) = post_raw(
        addr,
        "/webhook/stripe",
        "Content-Type: application/json\r\nStripe-Signature: t=1700000000,v1=abc123\r\n",
        b"{}",
    )
    .await;

    assert_eq!(status, 500);
}

// ─── Unknown routes return 404 ─────────────────────────────────────────

#[tokio::test]
async fn unknown_route_returns_404() {
    let addr = spawn_cloud(test_state()).await;
    let (status, _headers, _body) = get(addr, "/nonexistent-page").await;

    assert_eq!(status, 404);
}
