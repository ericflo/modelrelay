use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{FromRequestParts, Path, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use tower_sessions::Session;

use crate::state::CloudState;

mod auth;
mod checkout;
pub mod csrf;
mod dashboard;
mod docs;
mod downloads;
mod landing;
mod legal;
pub mod oauth;
mod pricing;
mod updater;
mod webhook;

/// Non-failing extractor for `tower_sessions::Session` on session-exempt public routes.
///
/// Allows public pages (`/`, `/downloads`, `/docs`, `/integrate`, `/setup`, `/terms`, `/privacy`)
/// to render the authenticated navigation bar when a user is signed in while still serving
/// cleanly when the session middleware is not attached (e.g. during DB outages or unit tests).
pub(super) struct OptionalSession(pub(super) Option<Session>);

impl<S: Send + Sync> FromRequestParts<S> for OptionalSession {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(parts.extensions.get::<Session>().cloned()))
    }
}

/// Build the full cloud router: commercial routes + OSS admin routes.
#[must_use = "returns the configured router"]
pub fn router(state: Arc<CloudState>) -> Router {
    Router::new()
        .route("/", get(landing::page))
        .route("/downloads", get(downloads::page))
        .route("/download", get(download_redirect))
        .route(
            "/download/desktop/{platform}",
            get(desktop_download_redirect),
        )
        .route(
            "/updater/desktop/{target}/{arch}/{current_version}",
            get(updater::desktop_update_check),
        )
        .route("/docs", get(docs::index))
        .route("/docs/{slug}", get(docs::page))
        .route("/health", get(health))
        .route("/pricing", get(pricing::page))
        .route("/privacy", get(legal::privacy_page))
        .route("/signup", get(signup_closed).post(signup_closed))
        .route("/terms", get(legal::terms_page))
        .route("/login", get(auth::login_page).post(auth::login_submit))
        .route("/logout", post(auth::logout))
        .route(
            "/oauth/authorize",
            get(oauth::authorize_get).post(oauth::authorize_post),
        )
        .route(
            "/api/v1/auth/keys",
            post(oauth::exchange_code).options(oauth::token_preflight),
        )
        .route(
            "/oauth/token",
            post(oauth::exchange_code).options(oauth::token_preflight),
        )
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
        .route("/AGENTS.md", get(agents_md))
        .route("/agents.md", get(agents_md))
        .route("/favicon.ico", get(favicon_ico))
        .route("/favicon-32.png", get(favicon_32_png))
        .route("/apple-touch-icon.png", get(apple_touch_icon))
        .route("/icon-512.png", get(icon_512_png))
        .route("/og-image.png", get(og_image))
        .route(
            "/assets/desktop-dashboard.png",
            get(desktop_dashboard_screenshot),
        )
        .route(
            "/assets/desktop-onboarding.png",
            get(desktop_onboarding_screenshot),
        )
        .route(
            "/assets/desktop-settings.png",
            get(desktop_settings_screenshot),
        )
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            security_headers,
        ))
        .layer(middleware::from_fn(csrf::csrf_middleware))
        .layer(middleware::from_fn(session_guard))
        .with_state(state)
}

/// Public account creation is intentionally closed while the hosted service is
/// restricted to existing users. This is deliberately a fixed route, not a
/// runtime flag, so a missing or stale environment variable cannot reopen signup.
async fn signup_closed(OptionalSession(session): OptionalSession) -> impl IntoResponse {
    let logged_in = match session {
        Some(s) => s.get::<String>("user_id").await.ok().flatten().is_some(),
        None => false,
    };

    let body = r#"<div style="display:flex;flex-direction:column;align-items:center;justify-content:center;min-height:55vh;text-align:center;padding:2rem">
<h1 style="font-size:2.6rem;font-weight:800;color:var(--accent-bright);line-height:1.1;margin:0">Signups Closed</h1>
<p style="color:var(--text-secondary);max-width:32rem;margin:1.25rem 0 2rem">New hosted account registration is currently closed. Existing users can log in below, or anyone can self-host ModelRelay for free.</p>
<div style="display:flex;gap:12px;flex-wrap:wrap;justify-content:center;">
  <a href="/login" class="btn">Log In</a>
  <a href="/docs/self-hosting" class="btn btn-secondary">Self-Hosting Guide</a>
</div>
</div>"#;

    (
        StatusCode::GONE,
        Html(modelrelay_web::templates::page_shell_custom(
            "Signups Closed",
            "/signup",
            body,
            logged_in,
            "",
            "",
        )),
    )
}

/// Redirect `/download` to the `#download` section on the landing page.
async fn download_redirect() -> Redirect {
    Redirect::temporary("/#download")
}

pub(super) async fn not_found(request: Request) -> impl IntoResponse {
    let logged_in = if let Some(s) = request.extensions().get::<Session>() {
        s.get::<String>("user_id").await.ok().flatten().is_some()
    } else {
        false
    };
    let body = r#"<div style="display:flex;flex-direction:column;align-items:center;justify-content:center;min-height:55vh;text-align:center;padding:2rem">
<h1 style="font-size:5rem;font-weight:800;color:var(--accent-bright);line-height:1;margin:0">404</h1>
<h2 style="font-size:1.4rem;font-weight:600;margin:1rem 0 .5rem">Page Not Found</h2>
<p style="color:var(--text-secondary);max-width:28rem;margin-bottom:2rem">The page you&rsquo;re looking for doesn&rsquo;t exist or has been moved.</p>
<div style="display:flex;gap:12px;flex-wrap:wrap;justify-content:center;">
  <a href="/" class="btn">Back to Overview</a>
  <a href="/docs" class="btn btn-secondary">Browse Docs</a>
</div>
</div>"#;
    (
        StatusCode::NOT_FOUND,
        Html(modelrelay_web::templates::page_shell_custom(
            "404 — Not Found",
            "/",
            body,
            logged_in,
            "",
            "",
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

static AGENTS_MD_TEXT: &str = include_str!("../../assets/AGENTS.md");

async fn agents_md() -> impl IntoResponse {
    (
        [
            (
                axum::http::header::CONTENT_TYPE,
                "text/markdown; charset=utf-8",
            ),
            (axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
            (axum::http::header::CACHE_CONTROL, "public, max-age=300"),
        ],
        AGENTS_MD_TEXT,
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
  <url><loc>https://modelrelay.io/AGENTS.md</loc></url>
  <url><loc>https://modelrelay.io/downloads</loc></url>
  <url><loc>https://modelrelay.io/integrate</loc></url>
  <url><loc>https://modelrelay.io/setup</loc></url>
  <url><loc>https://modelrelay.io/docs</loc></url>
  <url><loc>https://modelrelay.io/docs/api</loc></url>
  <url><loc>https://modelrelay.io/docs/pi</loc></url>
  <url><loc>https://modelrelay.io/docs/workers</loc></url>
  <url><loc>https://modelrelay.io/docs/self-hosting</loc></url>
  <url><loc>https://modelrelay.io/docs/architecture</loc></url>
  <url><loc>https://modelrelay.io/login</loc></url>
  <url><loc>https://modelrelay.io/terms</loc></url>
  <url><loc>https://modelrelay.io/privacy</loc></url>
</urlset>
"#,
    )
}

static FAVICON_ICO_BYTES: &[u8] = include_bytes!("../../../modelrelay-desktop/icons/icon.ico");
static FAVICON_32_PNG_BYTES: &[u8] = include_bytes!("../../../modelrelay-desktop/icons/32x32.png");
static APPLE_TOUCH_ICON_BYTES: &[u8] =
    include_bytes!("../../../modelrelay-desktop/icons/128x128@2x.png");
static ICON_512_PNG_BYTES: &[u8] = include_bytes!("../../../modelrelay-desktop/icons/icon.png");
static OG_IMAGE_BYTES: &[u8] = include_bytes!("../../assets/og-image.png");
static DESKTOP_DASHBOARD_PNG: &[u8] =
    include_bytes!("../../../../docs/screenshots/desktop/dashboard.png");
static DESKTOP_ONBOARDING_PNG: &[u8] =
    include_bytes!("../../../../docs/screenshots/desktop/onboarding.png");
static DESKTOP_SETTINGS_PNG: &[u8] =
    include_bytes!("../../../../docs/screenshots/desktop/settings.png");

fn immutable_asset(content_type: &'static str, bytes: &'static [u8]) -> impl IntoResponse {
    (
        [
            (axum::http::header::CONTENT_TYPE, content_type),
            (
                axum::http::header::CACHE_CONTROL,
                "public, max-age=86400, immutable",
            ),
        ],
        bytes,
    )
}

async fn favicon_ico() -> impl IntoResponse {
    immutable_asset("image/x-icon", FAVICON_ICO_BYTES)
}

async fn favicon_32_png() -> impl IntoResponse {
    immutable_asset("image/png", FAVICON_32_PNG_BYTES)
}

async fn apple_touch_icon() -> impl IntoResponse {
    immutable_asset("image/png", APPLE_TOUCH_ICON_BYTES)
}

async fn icon_512_png() -> impl IntoResponse {
    immutable_asset("image/png", ICON_512_PNG_BYTES)
}

async fn og_image() -> impl IntoResponse {
    immutable_asset("image/png", OG_IMAGE_BYTES)
}

async fn desktop_dashboard_screenshot() -> impl IntoResponse {
    immutable_asset("image/png", DESKTOP_DASHBOARD_PNG)
}

async fn desktop_onboarding_screenshot() -> impl IntoResponse {
    immutable_asset("image/png", DESKTOP_ONBOARDING_PNG)
}

async fn desktop_settings_screenshot() -> impl IntoResponse {
    immutable_asset("image/png", DESKTOP_SETTINGS_PNG)
}

/// Routes that do not require a session.
const SESSION_EXEMPT_ROUTES: &[&str] = &[
    "/",
    "/downloads",
    "/download",
    "/docs",
    "/setup",
    "/integrate",
    "/health",
    "/robots.txt",
    "/sitemap.xml",
    "/AGENTS.md",
    "/agents.md",
    "/favicon.ico",
    "/favicon-32.png",
    "/apple-touch-icon.png",
    "/icon-512.png",
    "/og-image.png",
    "/checkout/cancel",
    "/terms",
    "/privacy",
    "/signup",
    "/api/v1/auth/keys",
    "/oauth/token",
];

/// URL prefixes that do not require a session.
const SESSION_EXEMPT_PREFIXES: &[&str] = &[
    "/download/desktop/",
    "/docs/",
    "/assets/",
    "/updater/",
    "/webhook/",
];

/// Middleware that returns a styled 503 page when a session-dependent route is
/// hit but the `SessionManagerLayer` has not injected a `Session` into the
/// request extensions (e.g. because the database is unreachable at startup).
async fn session_guard(request: Request, next: Next) -> Response {
    let path = request.uri().path().to_owned();

    let exempt = SESSION_EXEMPT_ROUTES.contains(&path.as_str())
        || SESSION_EXEMPT_PREFIXES
            .iter()
            .any(|prefix| path.starts_with(prefix));

    if !exempt {
        let has_session = request.extensions().get::<Session>().is_some();

        if !has_session {
            let body = r#"<div style="display:flex;flex-direction:column;align-items:center;justify-content:center;min-height:55vh;text-align:center;padding:2rem">
<h1 style="font-size:3rem;font-weight:800;color:var(--warning);line-height:1;margin:0">503</h1>
<h2 style="font-size:1.45rem;font-weight:600;margin:1rem 0 .5rem">Service Temporarily Unavailable</h2>
<p style="color:var(--text-secondary);max-width:28rem;margin-bottom:2rem">We&rsquo;re experiencing a temporary issue connecting to our database. Please try again in a few moments.</p>
<a href="/" class="btn">Back to Home</a>
</div>"#;
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Html(modelrelay_web::templates::page_shell_custom(
                    "Service Unavailable",
                    &path,
                    body,
                    false,
                    "",
                    "",
                )),
            )
                .into_response();
        }
    }

    next.run(request).await
}

/// Middleware that sets standard HTTP security headers on every response,
/// including the configured public relay origin in `connect-src` so browser
/// inference tests on `/setup`, `/integrate`, and `/dashboard` are allowed.
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

const DESKTOP_FALLBACK_URL: &str = "https://github.com/ericflo/modelrelay/releases/latest";

/// Map a `/download/desktop/{platform}` slug to the asset filename suffix
/// produced by the Tauri release workflow.
fn desktop_asset_suffix(platform: &str) -> Option<&'static str> {
    match platform {
        "macos" => Some("_aarch64.dmg"),
        "macos-intel" => Some("_x64.dmg"),
        "linux" | "linux-deb" => Some("_amd64.deb"),
        "linux-appimage" => Some("_amd64.AppImage"),
        "windows" | "windows-exe" => Some("_x64-setup.exe"),
        "windows-msi" => Some("_x64_en-US.msi"),
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
        assert_eq!(
            desktop_asset_suffix("linux-appimage"),
            Some("_amd64.AppImage")
        );
        assert_eq!(desktop_asset_suffix("windows"), Some("_x64-setup.exe"));
        assert_eq!(desktop_asset_suffix("windows-exe"), Some("_x64-setup.exe"));
        assert_eq!(desktop_asset_suffix("windows-msi"), Some("_x64_en-US.msi"));
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
