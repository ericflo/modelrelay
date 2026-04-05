use std::sync::Arc;

use axum::extract::State;
use axum::response::Html;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};

use crate::state::CloudState;

mod auth;
mod checkout;
mod dashboard;
mod pricing;
mod webhook;

static LANDING_HTML: &str = include_str!("../../templates/index.html");

/// Build the full cloud router: commercial routes + OSS admin routes.
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
        .route("/webhook/stripe", post(webhook::handle))
        .with_state(state)
}

async fn landing() -> Html<&'static str> {
    Html(LANDING_HTML)
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
