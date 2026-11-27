use std::sync::Arc;

use axum::extract::{Path, Request, State};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::state::CloudState;

mod auth;
mod checkout;
pub mod csrf;
mod dashboard;
mod legal;
mod pricing;
mod updater;
mod webhook;

static LANDING_HTML: &str = include_str!("../../templates/index.html");

/// Build the full cloud router: commercial routes + OSS admin routes.
#[must_use = "returns the configured router"]
pub fn router(state: Arc<CloudState>) -> Router {
    Router::new()
        // Commercial routes: landing page, auth, billing, pricing
        .route("/", get(landing))
        .route("/download", get(download_redirect))
        .route(
            "/download/desktop/{platform}",
            get(desktop_download_redirect),
        )
        .route(
            "/updater/desktop/{target}/{arch}/{current_version}",
            get(updater::desktop_update_check),
        )
        .route("/health", get(health))
        .route("/pricing", get(pricing::page))
        .route("/privacy", get(legal::privacy_page))
        .route("/signup", get(signup_closed).post(signup_closed))
        .route("/terms", get(legal::terms_page))
        .route("/login", get(auth::login_page).post(auth::login_submit))
        .route("/logout", post(auth::logout))
        .route("/checkout", post(checkout::create))
        .route("/checkout/success", get(checkout::success))
        .route("/checkout/cancel", get(checkout::cancel))
        .route("/dashboard", get(dashboard::page))
        .route("/dashboard/billing-portal", post(dashboard::billing_portal))
        .route("/dashboard/keys/generate", post(dashboard::keys_generate))
        .route("/dashboard/keys/{id}/revoke", post(dashboard::keys_revoke))
        .route("/dashboard/workers", get(dashboard::workers))
        .route("/dashboard/stats", get(dashboard::stats))
        .route("/setup", get(dashboard::setup))
        .route("/integrate", get(dashboard::integrate))
        .route("/webhook/stripe", post(webhook::handle))
        .route("/robots.txt", get(robots_txt))
        .route("/sitemap.xml", get(sitemap_xml))
        .route("/favicon.ico", get(favicon_ico))
        .route("/favicon-32.png", get(favicon_32_png))
        .route("/apple-touch-icon.png", get(apple_touch_icon))
        .route("/icon-512.png", get(icon_512_png))
        .route("/og-image.png", get(og_image))
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            security_headers,
        ))
        .layer(middleware::from_fn(csrf::csrf_middleware))
        .layer(middleware::from_fn(session_guard))
        .with_state(state)
}

async fn landing() -> Html<&'static str> {
    Html(LANDING_HTML)
}

/// Public account creation is intentionally closed while the hosted service is
/// being recovered. This is deliberately a fixed route, not a runtime flag, so
/// a missing or stale environment variable cannot reopen signup.
async fn signup_closed() -> impl IntoResponse {
    let body = r#"<div style="display:flex;flex-direction:column;align-items:center;justify-content:center;min-height:60vh;text-align:center;padding:2rem">
<h1 style="font-size:3rem;font-weight:800;color:#a78bfa;line-height:1;margin:0">Signups Closed</h1>
<p style="color:#8b949e;max-width:32rem;margin:1.5rem 0 2rem">New account registration is currently closed. Existing users can still log in.</p>
<a href="/login" style="display:inline-block;padding:.75rem 2rem;background:#7c3aed;color:#fff;text-decoration:none;border-radius:.5rem;font-weight:600">Log In</a>
</div>"#;

    (
        StatusCode::GONE,
        Html(modelrelay_web::templates::page_shell(
            "Signups Closed",
            "/signup",
            body,
            false,
        )),
    )
}

/// Redirect `/download` to the `#download` section on the landing page.
///
/// This exists so shareable links like `https://modelrelay.io/download`
/// resolve instead of 404ing. Uses a temporary (302) redirect so the
/// destination can be changed later without cache headaches.
async fn download_redirect() -> Redirect {
    Redirect::temporary("/#download")
}

pub(super) async fn not_found(_request: Request) -> impl IntoResponse {
    let body = r#"<div style="display:flex;flex-direction:column;align-items:center;justify-content:center;min-height:60vh;text-align:center;padding:2rem">
<h1 style="font-size:6rem;font-weight:800;color:#7c3aed;line-height:1;margin:0">404</h1>
<h2 style="font-size:1.5rem;font-weight:600;margin:1rem 0 .5rem">Page Not Found</h2>
<p style="color:#8b949e;max-width:28rem;margin-bottom:2rem">The page you're looking for doesn't exist or has been moved.</p>
<a href="/" style="display:inline-block;padding:.75rem 2rem;background:#7c3aed;color:#fff;text-decoration:none;border-radius:.5rem;font-weight:600;transition:background .2s">Back to Home</a>
</div>"#;
    (
        StatusCode::NOT_FOUND,
        Html(modelrelay_web::templates::page_shell(
            "404 — Not Found",
            "/",
            body,
            false,
        )),
    )
}

async fn robots_txt() -> impl IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        "User-agent: *\nAllow: /\nSitemap: https://modelrelay.io/sitemap.xml\n",
    )
}

async fn sitemap_xml() -> impl IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "application/xml; charset=utf-8",
        )],
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>https://modelrelay.io/</loc></url>
  <url><loc>https://modelrelay.io/login</loc></url>
  <url><loc>https://modelrelay.io/setup</loc></url>
  <url><loc>https://modelrelay.io/integrate</loc></url>
  <url><loc>https://modelrelay.io/terms</loc></url>
  <url><loc>https://modelrelay.io/privacy</loc></url>
</urlset>
"#,
    )
}

/// Real multi-size Windows .ico (16x16 + 32x32 PNG sub-images) shipped by the
/// desktop crate. Baked in so the site and desktop app share branding.
static FAVICON_ICO_BYTES: &[u8] = include_bytes!("../../../modelrelay-desktop/icons/icon.ico");
static FAVICON_32_PNG_BYTES: &[u8] = include_bytes!("../../../modelrelay-desktop/icons/32x32.png");
static APPLE_TOUCH_ICON_BYTES: &[u8] =
    include_bytes!("../../../modelrelay-desktop/icons/128x128@2x.png");
static ICON_512_PNG_BYTES: &[u8] = include_bytes!("../../../modelrelay-desktop/icons/icon.png");

async fn favicon_ico() -> impl IntoResponse {
    (
        [
            (axum::http::header::CONTENT_TYPE, "image/x-icon"),
            (
                axum::http::header::CACHE_CONTROL,
                "public, max-age=86400, immutable",
            ),
        ],
        FAVICON_ICO_BYTES,
    )
}

async fn favicon_32_png() -> impl IntoResponse {
    (
        [
            (axum::http::header::CONTENT_TYPE, "image/png"),
            (
                axum::http::header::CACHE_CONTROL,
                "public, max-age=86400, immutable",
            ),
        ],
        FAVICON_32_PNG_BYTES,
    )
}

async fn apple_touch_icon() -> impl IntoResponse {
    (
        [
            (axum::http::header::CONTENT_TYPE, "image/png"),
            (
                axum::http::header::CACHE_CONTROL,
                "public, max-age=86400, immutable",
            ),
        ],
        APPLE_TOUCH_ICON_BYTES,
    )
}

async fn icon_512_png() -> impl IntoResponse {
    (
        [
            (axum::http::header::CONTENT_TYPE, "image/png"),
            (
                axum::http::header::CACHE_CONTROL,
                "public, max-age=86400, immutable",
            ),
        ],
        ICON_512_PNG_BYTES,
    )
}

/// 1200x630 branded social-share card used by `og:image` / `twitter:image`.
///
/// Baked into the binary so it ships with the image everywhere the cloud
/// service runs and so the site's CSP (`img-src 'self' data:`) can serve it
/// same-origin without special-casing an external host.
static OG_IMAGE_BYTES: &[u8] = include_bytes!("../../assets/og-image.png");

async fn og_image() -> impl IntoResponse {
    (
        [
            (axum::http::header::CONTENT_TYPE, "image/png"),
            (
                axum::http::header::CACHE_CONTROL,
                "public, max-age=86400, immutable",
            ),
        ],
        OG_IMAGE_BYTES,
    )
}

/// Routes that do not require a session.
const SESSION_EXEMPT_ROUTES: &[&str] = &[
    "/",
    "/download",
    "/health",
    "/robots.txt",
    "/sitemap.xml",
    "/favicon.ico",
    "/favicon-32.png",
    "/apple-touch-icon.png",
    "/icon-512.png",
    "/og-image.png",
    "/checkout/cancel",
    "/terms",
    "/privacy",
    "/signup",
];

/// URL prefixes that do not require a session. Use for routes with path
/// parameters (e.g. `/download/desktop/{platform}`).
const SESSION_EXEMPT_PREFIXES: &[&str] = &["/download/desktop/", "/updater/", "/webhook/"];

/// Middleware that returns a styled 503 page when a session-dependent route is
/// hit but the `SessionManagerLayer` has not injected a `Session` into the
/// request extensions (e.g. because the database is unreachable at startup).
async fn session_guard(request: Request, next: Next) -> Response {
    let path = request.uri().path().to_owned();

    // Let exempt routes through without checking for a session.
    let exempt = SESSION_EXEMPT_ROUTES.contains(&path.as_str())
        || SESSION_EXEMPT_PREFIXES
            .iter()
            .any(|prefix| path.starts_with(prefix));

    if !exempt {
        let has_session = request
            .extensions()
            .get::<tower_sessions::Session>()
            .is_some();

        if !has_session {
            let body = r#"<div style="display:flex;flex-direction:column;align-items:center;justify-content:center;min-height:60vh;text-align:center;padding:2rem">
<h1 style="font-size:3rem;font-weight:800;color:#fbbf24;line-height:1;margin:0">503</h1>
<h2 style="font-size:1.5rem;font-weight:600;margin:1rem 0 .5rem">Service Temporarily Unavailable</h2>
<p style="color:#8b949e;max-width:28rem;margin-bottom:2rem">We're experiencing a temporary issue connecting to our database. Please try again in a few moments.</p>
<a href="/" style="display:inline-block;padding:.75rem 2rem;background:#7c3aed;color:#fff;text-decoration:none;border-radius:.5rem;font-weight:600;transition:background .2s">Back to Home</a>
</div>"#;
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Html(modelrelay_web::templates::page_shell(
                    "Service Unavailable",
                    &path,
                    body,
                    false,
                )),
            )
                .into_response();
        }
    }

    next.run(request).await
}

/// Middleware that sets standard HTTP security headers on every response,
/// including the configured public relay origin in `connect-src` so browser
/// inference tests on `/setup` and `/integrate` are allowed.
async fn security_headers(
    State(state): State<Arc<CloudState>>,
    request: Request,
    next: Next,
) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert(
        "referrer-policy",
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    let relay_origin = state.public_relay_url.trim().trim_end_matches('/');
    let csp = if relay_origin.starts_with("http://") || relay_origin.starts_with("https://") {
        format!(
            "default-src 'self'; script-src 'self' 'unsafe-inline' https://js.stripe.com; \
             frame-src https://js.stripe.com; style-src 'self' 'unsafe-inline'; \
             img-src 'self' data:; connect-src 'self' https://api.stripe.com {relay_origin}; \
             font-src 'self'"
        )
    } else {
        "default-src 'self'; script-src 'self' 'unsafe-inline' https://js.stripe.com; \
         frame-src https://js.stripe.com; style-src 'self' 'unsafe-inline'; \
         img-src 'self' data:; connect-src 'self' https://api.stripe.com; font-src 'self'"
            .to_string()
    };
    if let Ok(value) = HeaderValue::from_str(&csp) {
        headers.insert("content-security-policy", value);
    }
    response
}

/// Fallback redirect when the GitHub API is unreachable or no asset matches.
const DESKTOP_FALLBACK_URL: &str = "https://github.com/ericflo/modelrelay/releases/latest";

/// Map a `/download/desktop/{platform}` slug to the asset filename suffix
/// produced by the Tauri release workflow.
fn desktop_asset_suffix(platform: &str) -> Option<&'static str> {
    match platform {
        "macos" => Some("_aarch64.dmg"),
        "macos-intel" => Some("_x64.dmg"),
        "linux" | "linux-deb" => Some("_amd64.deb"),
        "windows" => Some("_x64-setup.exe"),
        _ => None,
    }
}

/// Redirect `/download/desktop/{platform}` to the matching asset on the latest
/// `desktop-v*` GitHub release (using the shared updater cache), with a
/// graceful fallback to the releases landing page.
async fn desktop_download_redirect(
    State(state): State<Arc<CloudState>>,
    Path(platform): Path<String>,
) -> Redirect {
    let Some(suffix) = desktop_asset_suffix(&platform) else {
        return Redirect::temporary(DESKTOP_FALLBACK_URL);
    };

    let url = updater::latest_release(&state.http_client)
        .await
        .and_then(|release| {
            release
                .assets
                .into_iter()
                .find(|a| a.name.ends_with(suffix))
                .map(|a| a.url)
        });

    match url {
        Some(url) => Redirect::temporary(&url),
        None => Redirect::temporary(DESKTOP_FALLBACK_URL),
    }
}

async fn health(State(state): State<Arc<CloudState>>) -> (StatusCode, Json<Value>) {
    let db_ok = if let Some(ref pool) = state.db {
        sqlx::query_scalar::<_, i32>("SELECT 1")
            .fetch_one(pool)
            .await
            .is_ok()
    } else {
        false
    };

    let (status_code, status_text) = if db_ok {
        (StatusCode::OK, "ok")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "degraded")
    };

    (
        status_code,
        Json(json!({
            "status": status_text,
            "db_connected": db_ok,
            "stripe_configured": state.stripe_key.is_some(),
        })),
    )
}

#[cfg(test)]
mod tests {
    use super::{desktop_asset_suffix, not_found};

    #[test]
    fn maps_known_platforms_to_asset_suffixes() {
        assert_eq!(desktop_asset_suffix("macos"), Some("_aarch64.dmg"));
        assert_eq!(desktop_asset_suffix("macos-intel"), Some("_x64.dmg"));
        assert_eq!(desktop_asset_suffix("linux"), Some("_amd64.deb"));
        assert_eq!(desktop_asset_suffix("linux-deb"), Some("_amd64.deb"));
        assert_eq!(desktop_asset_suffix("windows"), Some("_x64-setup.exe"));
    }

    #[test]
    fn unknown_platforms_have_no_mapping() {
        assert_eq!(desktop_asset_suffix(""), None);
        assert_eq!(desktop_asset_suffix("freebsd"), None);
        assert_eq!(desktop_asset_suffix("MACOS"), None);
    }

    #[tokio::test]
    async fn not_found_does_not_self_canonicalize() {
        use axum::body::Body;
        use axum::http::Request as HttpRequest;
        use axum::response::IntoResponse;
        let req = HttpRequest::builder()
            .uri("/nonexistent-xyz")
            .body(Body::empty())
            .unwrap();
        let resp = not_found(req).await.into_response();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(
            !html.contains("/nonexistent-xyz"),
            "404 page must not emit the missing URL (found canonical/og:url pointing to it)"
        );
        assert!(
            html.contains("canonical\" href=\"https://modelrelay.io/\""),
            "404 page should canonicalize to home"
        );
    }
}
