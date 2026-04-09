use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::middleware;
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::state::CloudState;

mod auth;
mod checkout;
pub mod csrf;
mod dashboard;
mod pricing;
mod webhook;

static LANDING_HTML: &str = include_str!("../../templates/index.html");

/// Build the full cloud router: commercial routes + OSS admin routes.
#[must_use = "returns the configured router"]
pub fn router(state: Arc<CloudState>) -> Router {
    Router::new()
        // Commercial routes: landing page, auth, billing, pricing
        .route("/", get(landing))
        .route("/health", get(health))
        .route("/pricing", get(pricing::page))
        .route("/signup", get(auth::signup_page).post(auth::signup_submit))
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
        .fallback(not_found)
        .layer(middleware::from_fn(csrf::csrf_middleware))
        .with_state(state)
}

async fn landing() -> Html<&'static str> {
    Html(LANDING_HTML)
}

async fn not_found() -> impl IntoResponse {
    let html = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>404 — ModelRelay</title>
<link rel="icon" href="data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'><rect width='100' height='100' rx='20' fill='%237c3aed'/><text x='50' y='72' font-size='60' font-weight='bold' text-anchor='middle' fill='white'>M</text></svg>">
<style>
*{margin:0;padding:0;box-sizing:border-box}
body{background:#0d1117;color:#e6edf3;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;display:flex;flex-direction:column;align-items:center;justify-content:center;min-height:100vh;text-align:center;padding:2rem}
h1{font-size:6rem;font-weight:800;color:#7c3aed;line-height:1}
h2{font-size:1.5rem;font-weight:600;margin:1rem 0 .5rem}
p{color:#8b949e;max-width:28rem;margin-bottom:2rem}
a{display:inline-block;padding:.75rem 2rem;background:#7c3aed;color:#fff;text-decoration:none;border-radius:.5rem;font-weight:600;transition:background .2s}
a:hover{background:#6d28d9}
nav{position:fixed;top:0;left:0;right:0;padding:1rem 2rem;display:flex;align-items:center}
nav span{font-size:1.25rem;font-weight:700;color:#e6edf3}
nav span em{color:#7c3aed;font-style:normal}
</style>
</head>
<body>
<nav><span>Model<em>Relay</em></span></nav>
<h1>404</h1>
<h2>Page Not Found</h2>
<p>The page you're looking for doesn't exist or has been moved.</p>
<a href="/">Back to Home</a>
</body>
</html>"#;
    (StatusCode::NOT_FOUND, Html(html))
}

async fn health(State(state): State<Arc<CloudState>>) -> Json<Value> {
    let db_ok = if let Some(ref pool) = state.db {
        sqlx::query_scalar::<_, i32>("SELECT 1")
            .fetch_one(pool)
            .await
            .is_ok()
    } else {
        false
    };

    Json(json!({
        "status": "ok",
        "db_connected": db_ok,
        "stripe_configured": state.stripe_key.is_some(),
    }))
}
