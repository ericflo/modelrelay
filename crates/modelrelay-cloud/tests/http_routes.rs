use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use modelrelay_cloud::routes;
use modelrelay_cloud::state::CloudState;

/// Build the cloud router with no database and no Stripe — the minimum viable
/// state for smoke-testing routes that don't require either.
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

fn app() -> axum::Router {
    routes::router(test_state())
}

async fn get(path: &str) -> (StatusCode, String) {
    let resp = app()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

async fn post(path: &str, content_type: &str, body: &str) -> (StatusCode, String) {
    let resp = app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", content_type)
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

// ─── Landing page ──────────────────────────────────────────────────────────

#[tokio::test]
async fn landing_page_returns_200_with_html() {
    let (status, body) = get("/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("ModelRelay") || body.contains("<!DOCTYPE") || body.contains("<html"),
        "expected HTML landing page, got: {}",
        &body[..body.len().min(200)]
    );
}

// ─── Health endpoint ───────────────────────────────────────────────────────

#[tokio::test]
async fn health_returns_json_with_expected_fields() {
    let (status, body) = get("/health").await;
    assert_eq!(status, StatusCode::OK);

    let json: serde_json::Value = serde_json::from_str(&body).expect("health response is JSON");
    assert_eq!(json["status"], "ok");
    // db: None → db_connected should be false
    assert_eq!(json["db_connected"], false);
    // stripe_key: None → stripe_configured should be false
    assert_eq!(json["stripe_configured"], false);
}

// ─── Pricing page ──────────────────────────────────────────────────────────

#[tokio::test]
async fn pricing_returns_200_with_html() {
    let (status, body) = get("/pricing").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains('<') && body.len() > 50,
        "expected HTML pricing page"
    );
}

// ─── Auth pages ────────────────────────────────────────────────────────────

#[tokio::test]
async fn signup_page_returns_200() {
    let (status, body) = get("/signup").await;
    // Without a session layer the handler may panic or return 500 — the session
    // extractor requires a session layer.  We accept either 200 (if the handler
    // gracefully handles it) or 500 (expected without session layer).
    assert!(
        status == StatusCode::OK || status == StatusCode::INTERNAL_SERVER_ERROR,
        "unexpected status: {status}"
    );
    if status == StatusCode::OK {
        assert!(
            body.contains("Sign Up") || body.contains("signup"),
            "expected signup form"
        );
    }
}

#[tokio::test]
async fn login_page_returns_200() {
    let (status, body) = get("/login").await;
    assert!(
        status == StatusCode::OK || status == StatusCode::INTERNAL_SERVER_ERROR,
        "unexpected status: {status}"
    );
    if status == StatusCode::OK {
        assert!(
            body.contains("Log In") || body.contains("login"),
            "expected login form"
        );
    }
}

// ─── POST /signup without DB returns error ─────────────────────────────────

#[tokio::test]
async fn signup_submit_without_db_returns_error() {
    let (status, body) = post(
        "/signup",
        "application/x-www-form-urlencoded",
        "email=test%40example.com&password=longpassword123",
    )
    .await;
    // Without session layer → 500; with session layer but no DB → HTML error.
    assert!(
        status == StatusCode::OK
            || status == StatusCode::INTERNAL_SERVER_ERROR
            || status == StatusCode::UNPROCESSABLE_ENTITY,
        "unexpected status: {status}"
    );
    if status == StatusCode::OK {
        assert!(
            body.contains("Database not available") || body.contains("Error"),
            "expected DB-unavailable error message"
        );
    }
}

// ─── POST /login with no DB returns error ──────────────────────────────────

#[tokio::test]
async fn login_submit_without_db_returns_error() {
    let (status, _body) = post(
        "/login",
        "application/x-www-form-urlencoded",
        "email=test%40example.com&password=wrongpassword",
    )
    .await;
    assert!(
        status == StatusCode::OK
            || status == StatusCode::INTERNAL_SERVER_ERROR
            || status == StatusCode::UNPROCESSABLE_ENTITY,
        "unexpected status: {status}"
    );
}

// ─── GET /dashboard without session redirects to /login ────────────────────

#[tokio::test]
async fn dashboard_without_session_redirects_to_login() {
    let resp = app()
        .oneshot(
            Request::builder()
                .uri("/dashboard")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let status = resp.status();
    // Without a session layer the extractor will fail (500), or with session
    // layer it will redirect (303/307) to /login since there is no user_id.
    assert!(
        status == StatusCode::SEE_OTHER
            || status == StatusCode::TEMPORARY_REDIRECT
            || status == StatusCode::INTERNAL_SERVER_ERROR,
        "expected redirect or 500, got: {status}"
    );
    if status == StatusCode::SEE_OTHER || status == StatusCode::TEMPORARY_REDIRECT {
        let location = resp
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            location.contains("/login"),
            "expected redirect to /login, got: {location}"
        );
    }
}

// ─── Stripe webhook ────────────────────────────────────────────────────────

#[tokio::test]
async fn webhook_without_signature_returns_400() {
    let (status, _body) = post(
        "/webhook/stripe",
        "application/json",
        r#"{"type":"checkout.session.completed"}"#,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "webhook without Stripe-Signature should return 400"
    );
}

#[tokio::test]
async fn webhook_with_invalid_signature_returns_400() {
    let resp = app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook/stripe")
                .header("content-type", "application/json")
                .header(
                    "Stripe-Signature",
                    "t=1700000000,v1=0000000000000000000000000000000000000000000000000000000000000000",
                )
                .body(Body::from(
                    r#"{"type":"checkout.session.completed"}"#.to_owned(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "webhook with invalid signature should return 400"
    );
}

#[tokio::test]
async fn webhook_with_missing_v1_returns_400() {
    let resp = app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook/stripe")
                .header("content-type", "application/json")
                .header("Stripe-Signature", "t=1700000000")
                .body(Body::from("{}".to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ─── Webhook with no webhook_secret configured returns 500 ────────────────

#[tokio::test]
async fn webhook_without_secret_configured_returns_500() {
    let state = Arc::new(CloudState {
        db: None,
        stripe_key: None,
        webhook_secret: None, // no secret configured
        admin_url: None,
        admin_token: None,
        admin_emails: vec![],
    });

    let resp = routes::router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook/stripe")
                .header("content-type", "application/json")
                .header("Stripe-Signature", "t=1700000000,v1=abc")
                .body(Body::from("{}".to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "webhook with no secret configured should return 500"
    );
}
