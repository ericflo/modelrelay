use axum::response::Html;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};

static LANDING_HTML: &str = include_str!("../../templates/index.html");

pub fn router() -> Router {
    Router::new()
        .route("/", get(landing))
        .route("/health", get(health))
}

async fn landing() -> Html<&'static str> {
    Html(LANDING_HTML)
}

async fn health() -> Json<Value> {
    Json(json!({"status": "ok"}))
}
