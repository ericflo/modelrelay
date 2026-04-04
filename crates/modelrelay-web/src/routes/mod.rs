use std::sync::Arc;

use axum::extract::State;
use axum::response::Html;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::state::AppState;

mod checkout;
mod dashboard;
mod pricing;

static LANDING_HTML: &str = include_str!("../../templates/index.html");

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(landing))
        .route("/health", get(health))
        .route("/pricing", get(pricing::page))
        .route("/checkout", post(checkout::create))
        .route("/checkout/success", get(checkout::success))
        .route("/checkout/cancel", get(checkout::cancel))
        .route("/dashboard", get(dashboard::page))
        .with_state(state)
}

async fn landing() -> Html<&'static str> {
    Html(LANDING_HTML)
}

async fn health(State(state): State<Arc<AppState>>) -> Json<Value> {
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
