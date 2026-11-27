use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;
use tower_sessions::{MemoryStore, SessionManagerLayer};

use modelrelay_cloud::routes;
use modelrelay_cloud::state::CloudState;

/// Build the cloud router with no database and no Stripe — the minimum viable
/// state for smoke-testing routes that don't require either.
///
/// NOTE: These tests run without a session layer, so the CSRF middleware
/// passes through (no session in request extensions = skip validation).
fn test_state() -> Arc<CloudState> {
    Arc::new(CloudState {
        db: None,
        stripe_key: None,
        webhook_secret: Some("whsec_test_secret".into()),
        admin_url: None,
        admin_token: None,
        public_relay_url: modelrelay_cloud::state::DEFAULT_PUBLIC_RELAY_URL.into(),
        http_client: modelrelay_cloud::state::default_http_client(),
        admin_emails: vec![],
        rate_limiter: std::sync::Arc::new(modelrelay_cloud::state::RateLimiter::new(
            5,
            std::time::Duration::from_secs(15 * 60),
        )),
    })
}

fn app() -> axum::Router {
    routes::router(test_state())
}

/// Build a router with an in-memory session layer attached, so session-dependent
/// routes don't 503 under the session guard middleware. Used by tests that
/// want to inspect rendered page HTML (canonical/og meta tags, etc.).
fn app_with_session() -> axum::Router {
    let session_layer = SessionManagerLayer::new(MemoryStore::default());
    routes::router(test_state()).layer(session_layer)
}

async fn get_with_session(path: &str) -> (StatusCode, String) {
    let resp = app_with_session()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
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
async fn health_returns_503_when_db_unavailable() {
    let (status, body) = get("/health").await;
    // db: None → should return 503
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    let json: serde_json::Value = serde_json::from_str(&body).expect("health response is JSON");
    assert_eq!(json["status"], "degraded");
    assert_eq!(json["db_connected"], false);
    assert_eq!(json["stripe_configured"], false);
}

// ─── Pricing page ──────────────────────────────────────────────────────────

#[tokio::test]
async fn pricing_returns_503_without_session_layer() {
    let (status, body) = get("/pricing").await;
    // Without a session layer the session guard middleware returns a styled 503.
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        body.contains("Service Temporarily Unavailable"),
        "expected styled 503 page"
    );
}

// ─── Auth pages ────────────────────────────────────────────────────────────

#[tokio::test]
async fn signup_page_is_closed_without_session_layer() {
    let (status, body) = get("/signup").await;
    assert_eq!(status, StatusCode::GONE);
    assert!(
        body.contains("Signups Closed"),
        "expected closed-signup page"
    );
}

#[tokio::test]
async fn login_page_returns_503_without_session_layer() {
    let (status, body) = get("/login").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        body.contains("Service Temporarily Unavailable"),
        "expected styled 503 page"
    );
}

// ─── POST /signup without DB returns error ─────────────────────────────────

#[tokio::test]
async fn signup_submit_without_session_is_closed() {
    let (status, body) = post(
        "/signup",
        "application/x-www-form-urlencoded",
        "email=test%40example.com&password=longpassword123",
    )
    .await;
    assert_eq!(status, StatusCode::GONE);
    assert!(
        body.contains("Signups Closed"),
        "expected closed-signup page"
    );
}

// ─── POST /login with no DB returns error ──────────────────────────────────

#[tokio::test]
async fn login_submit_without_session_returns_503() {
    let (status, body) = post(
        "/login",
        "application/x-www-form-urlencoded",
        "email=test%40example.com&password=wrongpassword",
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        body.contains("Service Temporarily Unavailable"),
        "expected styled 503 page"
    );
}

// ─── GET /dashboard without session redirects to /login ────────────────────

#[tokio::test]
async fn dashboard_without_session_returns_503() {
    let resp = app()
        .oneshot(
            Request::builder()
                .uri("/dashboard")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Without a session layer the session guard middleware returns 503.
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

// ─── Checkout routes ──────────────────────────────────────────────────────

#[tokio::test]
async fn checkout_without_session_returns_503() {
    // Without a session layer the session guard middleware returns 503.
    let (status, body) = post("/checkout", "application/x-www-form-urlencoded", "").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        body.contains("Service Temporarily Unavailable"),
        "expected styled 503 page"
    );
}

#[tokio::test]
async fn checkout_cancel_returns_200() {
    let (status, body) = get("/checkout/cancel").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains('<') && body.len() > 50,
        "expected HTML cancel page"
    );
}

#[tokio::test]
async fn checkout_success_without_session_returns_503() {
    // GET /checkout/success uses Session, so without session layer → 503.
    let (status, body) = get("/checkout/success").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        body.contains("Service Temporarily Unavailable"),
        "expected styled 503 page"
    );
}

// ─── SEO routes ───────────────────────────────────────────────────────────

#[tokio::test]
async fn robots_txt_returns_200_with_correct_content() {
    let (status, body) = get("/robots.txt").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("User-agent: *"),
        "missing User-agent directive"
    );
    assert!(body.contains("Allow: /"), "missing Allow directive");
    assert!(
        body.contains("Sitemap: https://modelrelay.io/sitemap.xml"),
        "missing Sitemap directive"
    );
}

#[tokio::test]
async fn sitemap_xml_returns_200_with_correct_content() {
    let (status, body) = get("/sitemap.xml").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("http://www.sitemaps.org/schemas/sitemap/0.9"),
        "missing sitemap namespace"
    );
    for path in &["/", "/login", "/setup", "/integrate", "/terms", "/privacy"] {
        assert!(
            body.contains(&format!("https://modelrelay.io{path}")),
            "missing URL for {path}"
        );
    }
}

// ─── Legal pages ──────────────────────────────────────────────────────────

#[tokio::test]
async fn terms_page_returns_200_with_heading() {
    // /terms is session-exempt, so it renders even without a session layer.
    let (status, body) = get("/terms").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<h1>Terms of Service</h1>"),
        "expected <h1>Terms of Service</h1> in response, got body starting: {}",
        &body[..body.len().min(500)]
    );
    assert!(
        body.contains(r#"<meta property="og:url" content="https://modelrelay.io/terms""#),
        "/terms should have per-page og:url"
    );
    assert!(
        body.contains(r#"<link rel="canonical" href="https://modelrelay.io/terms""#),
        "/terms should have per-page canonical link"
    );
}

#[tokio::test]
async fn privacy_page_returns_200_with_heading() {
    // /privacy is session-exempt, so it renders even without a session layer.
    let (status, body) = get("/privacy").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<h1>Privacy Policy</h1>"),
        "expected <h1>Privacy Policy</h1> in response, got body starting: {}",
        &body[..body.len().min(500)]
    );
    assert!(
        body.contains(r#"<meta property="og:url" content="https://modelrelay.io/privacy""#),
        "/privacy should have per-page og:url"
    );
    assert!(
        body.contains(r#"<link rel="canonical" href="https://modelrelay.io/privacy""#),
        "/privacy should have per-page canonical link"
    );
}

#[tokio::test]
async fn sitemap_xml_includes_legal_pages() {
    let (status, body) = get("/sitemap.xml").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<loc>https://modelrelay.io/terms</loc>"),
        "sitemap missing /terms"
    );
    assert!(
        body.contains("<loc>https://modelrelay.io/privacy</loc>"),
        "sitemap missing /privacy"
    );
}

#[tokio::test]
async fn signup_is_closed_even_with_session() {
    let (status, body) = get_with_session("/signup").await;
    assert_eq!(status, StatusCode::GONE, "/signup should remain closed");
    assert!(body.contains("Signups Closed"));
    assert!(
        !body.contains("<form"),
        "closed signup must not render a form"
    );
}

#[tokio::test]
async fn download_redirects_to_hash_anchor() {
    let resp = app()
        .oneshot(
            Request::builder()
                .uri("/download")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::TEMPORARY_REDIRECT,
        "/download should return 307 Temporary Redirect"
    );
    let location = resp
        .headers()
        .get(axum::http::header::LOCATION)
        .expect("Location header should be set")
        .to_str()
        .expect("Location header should be valid UTF-8");
    assert_eq!(
        location, "/#download",
        "/download should redirect to the #download anchor on the landing page"
    );
}

#[tokio::test]
async fn desktop_download_unknown_platform_falls_back_to_releases_page() {
    let resp = app()
        .oneshot(
            Request::builder()
                .uri("/download/desktop/freebsd")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
    let location = resp
        .headers()
        .get(axum::http::header::LOCATION)
        .expect("Location header should be set")
        .to_str()
        .expect("Location header should be valid UTF-8");
    assert_eq!(
        location, "https://github.com/ericflo/modelrelay/releases/latest",
        "unknown platform should redirect to the releases landing page"
    );
}

#[tokio::test]
async fn favicon_returns_success() {
    let resp = app()
        .oneshot(
            Request::builder()
                .uri("/favicon.ico")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let content_type = resp
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .expect("favicon should set Content-Type")
        .to_str()
        .expect("Content-Type should be valid UTF-8");
    assert_eq!(content_type, "image/x-icon");

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    // Windows .ico files start with 00 00 01 00 (reserved + type=icon).
    assert!(
        bytes.len() >= 4 && &bytes[..4] == b"\x00\x00\x01\x00",
        "expected a real Windows .ico resource, got {} bytes starting with {:?}",
        bytes.len(),
        &bytes[..bytes.len().min(8)]
    );
}

#[tokio::test]
async fn apple_touch_icon_returns_png() {
    let resp = app()
        .oneshot(
            Request::builder()
                .uri("/apple-touch-icon.png")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let content_type = resp
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .expect("apple-touch-icon should set Content-Type")
        .to_str()
        .expect("Content-Type should be valid UTF-8");
    assert_eq!(content_type, "image/png");

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    // PNG files start with 89 50 4E 47 0D 0A 1A 0A.
    assert!(
        bytes.len() >= 8 && &bytes[..8] == b"\x89PNG\r\n\x1a\n",
        "expected a real PNG, got {} bytes",
        bytes.len()
    );
}

#[tokio::test]
async fn og_image_returns_png() {
    let resp = app()
        .oneshot(
            Request::builder()
                .uri("/og-image.png")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let content_type = resp
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .expect("og-image should set Content-Type")
        .to_str()
        .expect("Content-Type should be valid UTF-8");
    assert!(
        content_type.starts_with("image/png"),
        "expected image/png, got {content_type}"
    );

    let cache_control = resp
        .headers()
        .get(axum::http::header::CACHE_CONTROL)
        .expect("og-image should set Cache-Control")
        .to_str()
        .expect("Cache-Control should be valid UTF-8");
    assert!(
        cache_control.contains("max-age"),
        "expected Cache-Control with max-age, got {cache_control}"
    );

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert!(
        body.len() > 1000,
        "og-image should be a real PNG, got {} bytes",
        body.len()
    );
    // PNG magic bytes: 89 50 4E 47 0D 0A 1A 0A
    assert_eq!(
        &body[..8],
        &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a],
        "body should start with PNG magic bytes"
    );
}

#[tokio::test]
async fn landing_page_includes_og_image_meta() {
    let (status, body) = get("/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(r#"property="og:image" content="https://modelrelay.io/og-image.png""#),
        "landing page should include og:image meta tag"
    );
    assert!(
        body.contains(r#"name="twitter:card" content="summary_large_image""#),
        "landing page should use summary_large_image twitter card"
    );
    assert!(
        body.contains(r#"name="twitter:image" content="https://modelrelay.io/og-image.png""#),
        "landing page should include twitter:image meta tag"
    );
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

// ─── Security headers ─────────────────────────────────────────────────────

#[tokio::test]
async fn responses_include_security_headers() {
    let resp = app()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    let headers = resp.headers();
    assert_eq!(
        headers
            .get("x-content-type-options")
            .map(|v| v.to_str().unwrap()),
        Some("nosniff"),
    );
    assert_eq!(
        headers.get("x-frame-options").map(|v| v.to_str().unwrap()),
        Some("DENY"),
    );
    assert_eq!(
        headers.get("referrer-policy").map(|v| v.to_str().unwrap()),
        Some("strict-origin-when-cross-origin"),
    );
    assert_eq!(
        headers
            .get("permissions-policy")
            .map(|v| v.to_str().unwrap()),
        Some("camera=(), microphone=(), geolocation=()"),
    );
    let csp = headers
        .get("content-security-policy")
        .map(|v| v.to_str().unwrap());
    assert!(csp.is_some(), "missing content-security-policy header");
    assert!(
        csp.unwrap().contains("https://js.stripe.com"),
        "CSP must allow Stripe JS"
    );
}

// ─── Per-page og:url + canonical meta tags ────────────────────────────────

#[tokio::test]
async fn landing_page_has_root_og_url_and_canonical() {
    let (status, body) = get_with_session("/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(r#"<meta property="og:url" content="https://modelrelay.io/""#),
        "landing page missing og:url for root path"
    );
    assert!(
        body.contains(r#"<link rel="canonical" href="https://modelrelay.io/""#),
        "landing page missing canonical for root path"
    );
}

#[tokio::test]
async fn login_page_has_per_page_og_url_and_canonical() {
    let (status, body) = get_with_session("/login").await;
    assert_eq!(status, StatusCode::OK, "/login should render with session");
    assert!(
        body.contains(r#"<meta property="og:url" content="https://modelrelay.io/login""#),
        "/login should have og:url pointing at /login, got body starting: {}",
        &body[..body.len().min(500)]
    );
    assert!(
        body.contains(r#"<link rel="canonical" href="https://modelrelay.io/login""#),
        "/login should have canonical link pointing at /login"
    );
}

#[tokio::test]
async fn signup_page_has_per_page_og_url_and_canonical() {
    let (status, body) = get_with_session("/signup").await;
    assert_eq!(status, StatusCode::GONE, "/signup should remain closed");
    assert!(
        body.contains(r#"<meta property="og:url" content="https://modelrelay.io/signup""#),
        "/signup should have og:url pointing at /signup"
    );
    assert!(
        body.contains(r#"<link rel="canonical" href="https://modelrelay.io/signup""#),
        "/signup should have canonical link pointing at /signup"
    );
}

#[tokio::test]
async fn pricing_page_has_og_url_and_canonical() {
    let (status, body) = get_with_session("/pricing").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "/pricing should render with session"
    );
    assert!(
        body.contains(r#"<meta property="og:url" content="https://modelrelay.io/pricing""#),
        "/pricing should have og:url pointing at /pricing"
    );
    assert!(
        body.contains(r#"<link rel="canonical" href="https://modelrelay.io/pricing""#),
        "/pricing should have canonical link pointing at /pricing"
    );
    assert!(
        body.contains(r#"<meta property="og:title" content="Pricing — ModelRelay""#),
        "/pricing should have og:title"
    );
    assert!(
        body.contains(r#"<meta property="og:description""#),
        "/pricing should have og:description"
    );
    assert!(
        body.contains(r#"<meta property="og:type" content="website""#),
        "/pricing should have og:type"
    );
    assert!(
        body.contains(r#"<meta name="twitter:card" content="summary_large_image""#),
        "/pricing should have twitter:card set to summary_large_image"
    );
    assert!(
        body.contains(r#"<meta name="twitter:title" content="Pricing — ModelRelay""#),
        "/pricing should have twitter:title"
    );
    assert!(
        body.contains(r#"<meta name="twitter:description""#),
        "/pricing should have twitter:description"
    );
}

#[tokio::test]
async fn checkout_cancel_has_per_page_og_url_and_canonical() {
    // /checkout/cancel is session-exempt, so it renders even without the session layer.
    let (status, body) = get("/checkout/cancel").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(r#"<meta property="og:url" content="https://modelrelay.io/checkout/cancel""#),
        "/checkout/cancel should have og:url pointing at /checkout/cancel"
    );
    assert!(
        body.contains(r#"<link rel="canonical" href="https://modelrelay.io/checkout/cancel""#),
        "/checkout/cancel should have canonical link pointing at /checkout/cancel"
    );
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
        public_relay_url: modelrelay_cloud::state::DEFAULT_PUBLIC_RELAY_URL.into(),
        http_client: modelrelay_cloud::state::default_http_client(),
        admin_emails: vec![],
        rate_limiter: std::sync::Arc::new(modelrelay_cloud::state::RateLimiter::new(
            5,
            std::time::Duration::from_secs(15 * 60),
        )),
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

// ─── CSRF Content-Type bypass regression (F-28) ───────────────────────────

#[tokio::test]
async fn csrf_rejects_text_plain_and_multipart_posts_without_token() {
    let app = app_with_session();

    for content_type in [
        "text/plain",
        "multipart/form-data; boundary=----abc",
        "application/json",
    ] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/dashboard/keys/generate")
                    .header("content-type", content_type)
                    .body(Body::from("x=1"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "POST with content-type `{content_type}` and no CSRF token must be rejected"
        );
    }
}

// ─── Public relay URL & CSP regression (F-21) ─────────────────────────────

#[tokio::test]
async fn csp_connect_src_includes_public_relay_origin() {
    let state = Arc::new(CloudState {
        db: None,
        stripe_key: None,
        webhook_secret: None,
        admin_url: Some("http://proxy.internal:8080".into()),
        admin_token: Some("secret".into()),
        public_relay_url: "https://relay.example.com".into(),
        http_client: modelrelay_cloud::state::default_http_client(),
        admin_emails: vec![],
        rate_limiter: std::sync::Arc::new(modelrelay_cloud::state::RateLimiter::new(
            5,
            std::time::Duration::from_secs(15 * 60),
        )),
    });

    let resp = routes::router(state)
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    let csp = resp
        .headers()
        .get("content-security-policy")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        csp.contains("https://relay.example.com"),
        "CSP connect-src must include public relay origin: {csp}"
    );
    assert!(
        !csp.contains("proxy.internal"),
        "CSP must never expose internal admin_url: {csp}"
    );
}

// ─── Overhauled website routes: /downloads, /docs, /integrate, /assets ────

#[tokio::test]
async fn downloads_page_returns_200_with_platforms_and_cli() {
    let (status, body) = get("/downloads").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("ModelRelay Desktop"));
    assert!(body.contains("/download/desktop/macos"));
    assert!(body.contains("/download/desktop/linux-appimage"));
    assert!(body.contains("/download/desktop/windows-msi"));
    assert!(body.contains("/assets/desktop-dashboard.png"));
    assert!(body.contains(r#"<link rel="canonical" href="https://modelrelay.io/downloads""#));
}

#[tokio::test]
async fn docs_hub_and_all_sections_render_cleanly() {
    for (path, expected) in [
        ("/docs", "ModelRelay Overview"),
        ("/docs/api", "api.modelrelay.io"),
        ("/docs/pi", "pi.registerProvider"),
        ("/docs/workers", "Desktop App"),
        ("/docs/self-hosting", "Self-Hosting ModelRelay"),
        ("/docs/architecture", "Architecture"),
    ] {
        let (status, body) = get(path).await;
        assert_eq!(status, StatusCode::OK, "{path} should return 200");
        assert!(
            body.contains(expected),
            "{path} should contain `{expected}`"
        );
        assert!(
            body.contains(&format!(
                r#"<link rel="canonical" href="https://modelrelay.io{path}""#
            )),
            "{path} missing canonical URL"
        );
    }

    let (status, _) = get("/docs/does-not-exist").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn integrate_page_focuses_on_api_curl_and_pi_provider() {
    let (status, body) = get("/integrate").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("api.modelrelay.io"));
    assert!(body.contains("pi.registerProvider"));
    assert!(body.contains("~/.pi/agent/extensions/modelrelay.ts"));
    assert!(body.contains("~/.pi/agent/auth.json"));
    assert!(body.contains("@earendil-works/pi-coding-agent"));
    assert!(body.contains("thinkingLevelMap"));
    assert!(body.contains("thinkingFormat"));
    assert!(body.contains("supportsReasoningEffort"));
    assert!(body.contains("contextWindow"));
    assert!(body.contains("maxTokens"));
    assert!(body.contains("MODELRELAY_CONTEXT_WINDOW"));
    assert!(
        !body.contains("Codex CLI") && !body.contains("Continue.dev") && !body.contains("Aider"),
        "/integrate should drop non-Pi agents and stay focused on api.modelrelay.io curl + Pi"
    );
    assert!(
        !body.contains("@mariozechner/pi-coding-agent"),
        "/integrate should not use the outdated Pi npm package name"
    );

    let (pi_doc_status, pi_doc) = get("/docs/pi").await;
    assert_eq!(pi_doc_status, StatusCode::OK);
    for needle in [
        "thinkingLevelMap",
        "thinkingFormat",
        "thinkingTokenBudgetField",
        "contextWindow",
        "maxTokens",
        "supportsUsageInStreaming",
    ] {
        assert!(
            pi_doc.contains(needle),
            "/docs/pi should document {needle} for Pi thinking levels & context limits"
        );
    }
}

#[tokio::test]
async fn desktop_screenshot_assets_return_png() {
    for path in [
        "/assets/desktop-dashboard.png",
        "/assets/desktop-onboarding.png",
        "/assets/desktop-settings.png",
    ] {
        let resp = app()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{path} should return 200");
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert!(
            bytes.len() > 1000 && &bytes[..8] == b"\x89PNG\r\n\x1a\n",
            "{path} should be a valid PNG"
        );
    }
}

#[tokio::test]
async fn sitemap_includes_downloads_and_docs_routes() {
    let (status, body) = get("/sitemap.xml").await;
    assert_eq!(status, StatusCode::OK);
    for path in &[
        "/downloads",
        "/docs",
        "/docs/api",
        "/docs/pi",
        "/docs/workers",
        "/docs/self-hosting",
        "/docs/architecture",
    ] {
        assert!(
            body.contains(&format!("<loc>https://modelrelay.io{path}</loc>")),
            "sitemap missing {path}"
        );
    }
}

// ─── OAuth 2.0 PKCE provider ──────────────────────────────────────────────

#[tokio::test]
async fn oauth_authorize_rejects_invalid_params_with_400() {
    let challenge = modelrelay_cloud::routes::oauth::compute_s256_challenge(
        "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
    );

    // Non-HTTPS non-localhost callback rejected
    let (status, _) = get_with_session(&format!(
        "/oauth/authorize?callback_url=http://evil.example/&code_challenge={challenge}&code_challenge_method=S256"
    ))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Non-S256 PKCE rejected
    let (status, _) = get_with_session(&format!(
        "/oauth/authorize?callback_url=https://chat.neuralcolumn.com/&code_challenge={challenge}&code_challenge_method=plain"
    ))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn oauth_authorize_logged_out_redirects_to_login_with_next() {
    let challenge = modelrelay_cloud::routes::oauth::compute_s256_challenge(
        "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
    );
    let uri = format!(
        "/oauth/authorize?callback_url=https%3A%2F%2Fchat.neuralcolumn.com%2F&code_challenge={challenge}&code_challenge_method=S256&state=mr.teststate&key_label=NeuralColumn+Chat"
    );
    let resp = app_with_session()
        .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let loc = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        loc.starts_with("/login?next=%2Foauth%2Fauthorize"),
        "expected redirect to /login?next=..., got: {loc}"
    );
}

#[tokio::test]
async fn login_preserves_safe_next_and_strips_open_redirect() {
    let (status, body) = get_with_session("/login?next=%2Foauth%2Fauthorize%3Fstate%3Dmr.1").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(r#"name="next" value="/oauth/authorize?state=mr.1""#));

    let (status, body) = get_with_session("/login?next=https%3A%2F%2Fevil.example%2F").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.contains("evil.example"));
}

#[tokio::test]
async fn oauth_token_endpoints_support_cors_preflight_and_bypass_csrf() {
    for path in ["/api/v1/auth/keys", "/oauth/token"] {
        let preflight = app()
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri(path)
                    .header("origin", "https://chat.neuralcolumn.com")
                    .header("access-control-request-method", "POST")
                    .header("access-control-request-headers", "content-type")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(preflight.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            preflight
                .headers()
                .get("access-control-allow-origin")
                .and_then(|v| v.to_str().ok()),
            Some("*"),
            "{path} OPTIONS must include Access-Control-Allow-Origin: *"
        );

        // POST JSON with missing verifier returns 400 JSON (NOT 403 CSRF) with CORS header
        let resp = app_with_session()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"code":"mrc_bad","code_verifier":""}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "{path} should bypass CSRF and return 400 on missing verifier"
        );
        assert_eq!(
            resp.headers()
                .get("access-control-allow-origin")
                .and_then(|v| v.to_str().ok()),
            Some("*")
        );
    }
}

#[tokio::test]
async fn oauth_authorize_deny_redirects_with_access_denied_and_state() {
    let challenge = modelrelay_cloud::routes::oauth::compute_s256_challenge(
        "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
    );
    let router = app_with_session();

    // First GET /login to acquire session cookie and CSRF token
    let login_resp = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let cookie = login_resp
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(';').next())
        .unwrap()
        .to_owned();
    let login_html = String::from_utf8(
        login_resp
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    let csrf_marker = r#"name="_csrf" value=""#;
    let start = login_html.find(csrf_marker).unwrap() + csrf_marker.len();
    let end = login_html[start..].find('"').unwrap() + start;
    let csrf_token = &login_html[start..end];

    let form_body = format!(
        "_csrf={csrf_token}&decision=deny&callback_url=https%3A%2F%2Fchat.neuralcolumn.com%2F&code_challenge={challenge}&code_challenge_method=S256&state=mr.cancel99&key_label=NeuralColumn+Chat"
    );
    let deny_resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/authorize")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form_body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(deny_resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        deny_resp
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok()),
        Some("https://chat.neuralcolumn.com/?error=access_denied&state=mr.cancel99")
    );
}

// ─── AGENTS.md & homepage agent-bootstrap section ───────────────────────────

#[tokio::test]
async fn agents_md_is_served_with_markdown_and_cors_on_both_cases() {
    for path in ["/AGENTS.md", "/agents.md"] {
        let resp = app()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "expected 200 for {path}");
        assert_eq!(
            resp.headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok()),
            Some("text/markdown; charset=utf-8")
        );
        assert_eq!(
            resp.headers()
                .get("access-control-allow-origin")
                .and_then(|v| v.to_str().ok()),
            Some("*")
        );
        let body = String::from_utf8(
            resp.into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        assert!(body.contains("modelrelay-server"));
        assert!(body.contains("modelrelay-worker"));
        assert!(body.contains("max_concurrency"));
        assert!(body.contains("~/.pi/agent/extensions/modelrelay.ts"));
        assert!(body.contains("~/.pi/agent/auth.json"));
        assert!(body.contains("Claude Code"));
        assert!(body.contains("Codex"));
        assert!(body.contains("/oauth/authorize"));
    }
}

#[tokio::test]
async fn landing_page_includes_agent_bootstrap_prompts_and_agents_md_link() {
    let (status, body) = get("/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("/AGENTS.md"));
    assert!(body.contains("https://modelrelay.io/AGENTS.md"));
    assert!(body.contains("Claude Code"));
    assert!(body.contains("Codex"));
    assert!(body.contains("prompt-full"));
    assert!(body.contains("prompt-worker"));
    assert!(body.contains("prompt-client"));
    assert!(body.contains("prompt-cli"));
}
