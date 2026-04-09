use axum::response::Html;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};

/// Build the OSS admin dashboard router.
///
/// This provides health checks and the admin monitoring dashboard for
/// self-hosted deployments. The commercial `modelrelay-cloud` crate adds
/// Stripe billing, user accounts, and its own routes on top.
pub fn router() -> Router {
    Router::new()
        .route("/", get(landing))
        .route("/health", get(health))
        .route("/dashboard", get(dashboard))
        .route("/setup", get(setup))
        .route("/integrate", get(integrate))
}

async fn landing() -> Html<String> {
    Html(crate::templates::page_shell(
        "ModelRelay Admin",
        "<div class=\"card\">\
           <h2>Welcome to ModelRelay</h2>\
           <p>This is the open-source admin dashboard for your ModelRelay deployment.</p>\
           <p style=\"margin-top:12px;\"><a href=\"/dashboard\" class=\"btn\">Go to Dashboard &rarr;</a></p>\
         </div>",
        false,
    ))
}

async fn health() -> Json<Value> {
    Json(json!({
        "status": "ok",
    }))
}

async fn dashboard() -> Html<String> {
    Html(crate::templates::dashboard_page())
}

async fn setup() -> Html<String> {
    Html(crate::templates::setup_wizard_page())
}

async fn integrate() -> Html<String> {
    Html(crate::templates::integrate_page())
}
