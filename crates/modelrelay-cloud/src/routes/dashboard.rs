use std::fmt::Write;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use tower_sessions::Session;

use crate::state::CloudState;

use super::{OptionalSession, csrf};

// ─── Helpers shared across handlers ─────────────────────────────────────────

/// Load `user_id` from session, returning a redirect to /login if absent.
async fn require_user(session: &Session) -> Result<uuid::Uuid, Response> {
    let user_id: Option<String> = session.get("user_id").await.unwrap_or(None);
    let Some(user_id) = user_id else {
        return Err(Redirect::to("/login").into_response());
    };
    user_id
        .parse()
        .map_err(|_| Redirect::to("/login").into_response())
}

#[derive(sqlx::FromRow)]
struct UserRow {
    #[allow(dead_code)]
    id: uuid::Uuid,
    email: String,
    stripe_customer_id: Option<String>,
    is_admin: bool,
}

#[derive(sqlx::FromRow)]
struct SubscriptionRow {
    #[allow(dead_code)]
    id: uuid::Uuid,
    #[allow(dead_code)]
    stripe_subscription_id: String,
    status: String,
    api_key_id: Option<String>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(sqlx::FromRow)]
struct ApiKeyRow {
    id: uuid::Uuid,
    #[allow(dead_code)]
    key_id: String,
    raw_key: String,
    name: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

// ─── GET /dashboard ─────────────────────────────────────────────────────────

/// GET /dashboard — show live relay telemetry, workers, API keys, and Pi/curl quick connect.
#[allow(clippy::too_many_lines)]
pub async fn page(session: Session, State(state): State<Arc<CloudState>>) -> Response {
    let Some(ref pool) = state.db else {
        return Html(modelrelay_web::templates::page_shell(
            "Dashboard",
            "/dashboard",
            &no_db_html(),
            true,
        ))
        .into_response();
    };

    let user_id = match require_user(&session).await {
        Ok(id) => id,
        Err(r) => return r,
    };

    let csrf_field = csrf::hidden_field(&session).await;

    let user = sqlx::query_as::<_, UserRow>(
        "SELECT id, email, stripe_customer_id, is_admin FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await;

    let user = match user {
        Ok(Some(u)) => u,
        Ok(None) => return Redirect::to("/login").into_response(),
        Err(e) => {
            tracing::error!("dashboard user query error: {e}");
            return Html(modelrelay_web::templates::page_shell(
                "Dashboard",
                "/dashboard",
                "<div class=\"card\"><h2>Error</h2><p>Could not load your account. Please try again later.</p></div>",
                true,
            ))
            .into_response();
        }
    };

    if user.is_admin {
        let keys = sqlx::query_as::<_, ApiKeyRow>(
            "SELECT id, key_id, raw_key, name, created_at FROM api_keys \
             WHERE user_id = $1 AND revoked_at IS NULL ORDER BY created_at DESC",
        )
        .bind(user_id)
        .fetch_all(pool)
        .await
        .unwrap_or_default();

        let html = admin_dashboard_html(&user.email, &state.public_relay_url, &keys, &csrf_field);
        Html(modelrelay_web::templates::page_shell_custom(
            "Dashboard",
            "/dashboard",
            &html,
            true,
            dashboard_css(),
            dashboard_shared_js(),
        ))
        .into_response()
    } else {
        let subscription = sqlx::query_as::<_, SubscriptionRow>(
            "SELECT id, stripe_subscription_id, status, api_key_id, updated_at \
             FROM subscriptions WHERE user_id = $1 ORDER BY updated_at DESC LIMIT 1",
        )
        .bind(user_id)
        .fetch_optional(pool)
        .await;

        let sub = match subscription {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("dashboard subscription query error: {e}");
                None
            }
        };

        let api_key_display: Option<String> = if sub.is_some() {
            sqlx::query_scalar(
                "SELECT raw_key FROM api_keys WHERE user_id = $1 AND revoked_at IS NULL \
                 ORDER BY created_at DESC LIMIT 1",
            )
            .bind(user_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
        } else {
            None
        };

        let has_stripe_customer = user.stripe_customer_id.is_some();
        let html = subscriber_dashboard_html(
            &user.email,
            &state.public_relay_url,
            sub.as_ref(),
            has_stripe_customer,
            api_key_display.as_deref(),
            &csrf_field,
        );
        Html(modelrelay_web::templates::page_shell_custom(
            "Dashboard",
            "/dashboard",
            &html,
            true,
            dashboard_css(),
            dashboard_shared_js(),
        ))
        .into_response()
    }
}

// ─── POST /dashboard/billing-portal ─────────────────────────────────────────

pub async fn billing_portal(session: Session, State(state): State<Arc<CloudState>>) -> Response {
    let Some(ref key) = state.stripe_key else {
        return error_page("/dashboard/billing-portal", "Billing not configured").into_response();
    };

    let Some(ref pool) = state.db else {
        return error_page("/dashboard/billing-portal", "Database not available").into_response();
    };

    let user_id = match require_user(&session).await {
        Ok(id) => id,
        Err(r) => return r,
    };

    let customer_id: Option<String> =
        sqlx::query_scalar("SELECT stripe_customer_id FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();

    let Some(customer_id) = customer_id else {
        return Html(modelrelay_web::templates::page_shell(
            "Error",
            "/dashboard/billing-portal",
            "<div class=\"card\"><h2>No billing account</h2>\
             <p>No Stripe customer found for your account.</p>\
             <p class=\"back-link\"><a href=\"/dashboard\">&larr; Back to dashboard</a></p></div>",
            true,
        ))
        .into_response();
    };

    let params = [
        ("customer", customer_id.as_str()),
        ("return_url", "https://modelrelay.io/dashboard"),
    ];

    let resp = state
        .http_client
        .post("https://api.stripe.com/v1/billing_portal/sessions")
        .bearer_auth(key)
        .form(&params)
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => match r.json::<serde_json::Value>().await {
            Ok(body) => {
                if let Some(url) = body["url"].as_str() {
                    Redirect::to(url).into_response()
                } else {
                    error_page(
                        "/dashboard/billing-portal",
                        "Stripe did not return a portal URL.",
                    )
                    .into_response()
                }
            }
            Err(e) => {
                tracing::error!("stripe portal response parse error: {e}");
                error_page(
                    "/dashboard/billing-portal",
                    "Could not process billing portal response.",
                )
                .into_response()
            }
        },
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            tracing::error!("stripe billing portal API error: {status} — {body}");
            error_page(
                "/dashboard/billing-portal",
                "Could not open billing portal. Please try again later.",
            )
            .into_response()
        }
        Err(e) => {
            tracing::error!("stripe billing portal request error: {e}");
            error_page(
                "/dashboard/billing-portal",
                "Could not reach payment provider. Please try again later.",
            )
            .into_response()
        }
    }
}

// ─── POST /dashboard/keys/generate ──────────────────────────────────────────

pub async fn keys_generate(session: Session, State(state): State<Arc<CloudState>>) -> Response {
    let Some(ref pool) = state.db else {
        return error_page("/dashboard/keys/generate", "Database not available").into_response();
    };

    let user_id = match require_user(&session).await {
        Ok(id) => id,
        Err(r) => return r,
    };

    let is_admin: Option<bool> = sqlx::query_scalar("SELECT is_admin FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();

    if is_admin != Some(true) {
        return (StatusCode::FORBIDDEN, "Forbidden").into_response();
    }

    let Some(ref admin_url) = state.admin_url else {
        return error_page(
            "/dashboard/keys/generate",
            "Admin API not configured. Cannot generate API keys at this time.",
        )
        .into_response();
    };
    let Some(ref admin_token) = state.admin_token else {
        return error_page(
            "/dashboard/keys/generate",
            "Admin API not configured. Cannot generate API keys at this time.",
        )
        .into_response();
    };

    let email: String = match sqlx::query_scalar("SELECT email FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(pool)
        .await
    {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("keys_generate user lookup error: {e}");
            return error_page(
                "/dashboard/keys/generate",
                "Could not look up your account.",
            )
            .into_response();
        }
    };

    let ts = chrono::Utc::now().format("%Y%m%d%H%M%S");
    let key_name = format!("admin-{email}-{ts}");

    match super::webhook::provision_api_key(&state.http_client, admin_url, admin_token, &key_name)
        .await
    {
        Ok((key_id, raw_key)) => {
            if let Err(e) = sqlx::query(
                "INSERT INTO api_keys (user_id, key_id, raw_key, name) VALUES ($1, $2, $3, $4)",
            )
            .bind(user_id)
            .bind(&key_id)
            .bind(&raw_key)
            .bind(&key_name)
            .execute(pool)
            .await
            {
                tracing::error!(
                    "keys_generate db insert error: {e}; attempting compensating revoke"
                );
                if let Err(revoke_err) = super::webhook::revoke_api_key(
                    &state.http_client,
                    admin_url,
                    admin_token,
                    &key_id,
                )
                .await
                {
                    tracing::error!(
                        key_id = %key_id,
                        error = %revoke_err,
                        "compensating revoke failed after DB insert error"
                    );
                }
                return error_page(
                    "/dashboard/keys/generate",
                    "Could not save the generated API key. Please try again.",
                )
                .into_response();
            }
            tracing::info!(key_id = %key_id, email = %email, "admin generated new API key");
            Redirect::to("/dashboard").into_response()
        }
        Err(e) => {
            tracing::error!("keys_generate provision error: {e}");
            error_page(
                "/dashboard/keys/generate",
                "Could not generate API key. The relay server may be unreachable. Please try again later.",
            )
            .into_response()
        }
    }
}

// ─── POST /dashboard/keys/:id/revoke ────────────────────────────────────────

pub async fn keys_revoke(
    session: Session,
    State(state): State<Arc<CloudState>>,
    Path(key_uuid): Path<uuid::Uuid>,
) -> Response {
    let path = format!("/dashboard/keys/{key_uuid}/revoke");
    let Some(ref pool) = state.db else {
        return error_page(&path, "Database not available").into_response();
    };

    let user_id = match require_user(&session).await {
        Ok(id) => id,
        Err(r) => return r,
    };

    let is_admin: Option<bool> = sqlx::query_scalar("SELECT is_admin FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();

    if is_admin != Some(true) {
        return (StatusCode::FORBIDDEN, "Forbidden").into_response();
    }

    let key_row: Option<(String,)> = sqlx::query_as(
        "SELECT key_id FROM api_keys WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(key_uuid)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    let Some((key_id,)) = key_row else {
        return error_page(&path, "API key not found or already revoked.").into_response();
    };

    if let (Some(admin_url), Some(admin_token)) = (&state.admin_url, &state.admin_token) {
        if let Err(e) =
            super::webhook::revoke_api_key(&state.http_client, admin_url, admin_token, &key_id)
                .await
        {
            tracing::error!(key_id = %key_id, error = %e, "keys_revoke server call failed");
            return error_page(
                &path,
                "Could not revoke the API key on the relay server. The key is still active — please retry.",
            )
            .into_response();
        }
    } else {
        tracing::warn!("admin API not configured — revoking key locally only");
    }

    if let Err(e) = sqlx::query("UPDATE api_keys SET revoked_at = now() WHERE id = $1")
        .bind(key_uuid)
        .execute(pool)
        .await
    {
        tracing::error!("keys_revoke db update error: {e}");
        return error_page(&path, "Could not revoke key. Please try again.").into_response();
    }

    tracing::info!(key_id = %key_id, "admin revoked API key");
    Redirect::to("/dashboard").into_response()
}

async fn user_is_admin(state: &CloudState, user_id: uuid::Uuid) -> bool {
    let Some(ref pool) = state.db else {
        return false;
    };
    sqlx::query_scalar::<_, bool>("SELECT is_admin FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        == Some(true)
}

// ─── GET /dashboard/workers ─────────────────────────────────────────────────

pub async fn workers(session: Session, State(state): State<Arc<CloudState>>) -> Response {
    let user_id = match require_user(&session).await {
        Ok(id) => id,
        Err(r) => return r,
    };

    if !user_is_admin(&state, user_id).await {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({"error": "admin access required", "workers": []})),
        )
            .into_response();
    }

    let Some(ref admin_url) = state.admin_url else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({"error": "admin API not configured", "workers": []})),
        )
            .into_response();
    };
    let Some(ref admin_token) = state.admin_token else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({"error": "admin API not configured", "workers": []})),
        )
            .into_response();
    };

    let url = format!("{}/admin/workers", admin_url.trim_end_matches('/'));
    match state
        .http_client
        .get(&url)
        .bearer_auth(admin_token)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
            Ok(body) => axum::Json(body).into_response(),
            Err(e) => {
                tracing::error!("workers proxy parse error: {e}");
                axum::Json(serde_json::json!({"workers": []})).into_response()
            }
        },
        Ok(resp) => {
            let status = resp.status();
            tracing::warn!("workers proxy upstream error: {status}");
            axum::Json(serde_json::json!({"workers": []})).into_response()
        }
        Err(e) => {
            tracing::error!("workers proxy request error: {e}");
            axum::Json(serde_json::json!({"workers": []})).into_response()
        }
    }
}

// ─── GET /dashboard/stats ───────────────────────────────────────────────────

pub async fn stats(session: Session, State(state): State<Arc<CloudState>>) -> Response {
    let user_id = match require_user(&session).await {
        Ok(id) => id,
        Err(r) => return r,
    };

    let is_admin = user_is_admin(&state, user_id).await;
    let empty_stats = || serde_json::json!({"queue_depth": {}, "active_workers": 0});

    let Some(ref admin_url) = state.admin_url else {
        return (StatusCode::SERVICE_UNAVAILABLE, axum::Json(empty_stats())).into_response();
    };
    let Some(ref admin_token) = state.admin_token else {
        return (StatusCode::SERVICE_UNAVAILABLE, axum::Json(empty_stats())).into_response();
    };

    let url = format!("{}/admin/stats", admin_url.trim_end_matches('/'));
    match state
        .http_client
        .get(&url)
        .bearer_auth(admin_token)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
            Ok(body) => {
                if is_admin {
                    axum::Json(body).into_response()
                } else {
                    let active_workers = body["active_workers"].as_u64().unwrap_or(0);
                    let total_queued: u64 = body["queue_depth"].as_object().map_or(0, |m| {
                        m.values().filter_map(serde_json::Value::as_u64).sum()
                    });
                    let queue_depth = if total_queued > 0 {
                        serde_json::json!({"total": total_queued})
                    } else {
                        serde_json::json!({})
                    };
                    axum::Json(serde_json::json!({
                        "queue_depth": queue_depth,
                        "active_workers": active_workers,
                    }))
                    .into_response()
                }
            }
            Err(e) => {
                tracing::error!("stats proxy parse error: {e}");
                axum::Json(empty_stats()).into_response()
            }
        },
        Ok(resp) => {
            let status = resp.status();
            tracing::warn!("stats proxy upstream error: {status}");
            axum::Json(empty_stats()).into_response()
        }
        Err(e) => {
            tracing::error!("stats proxy request error: {e}");
            axum::Json(empty_stats()).into_response()
        }
    }
}

async fn load_cloud_config(
    session: Option<Session>,
    state: &CloudState,
) -> Option<modelrelay_web::templates::CloudWizardConfig> {
    let session = session?;
    let user_id = require_user(&session).await.ok()?;
    let api_key: Option<String> = if let Some(ref pool) = state.db {
        sqlx::query_scalar(
            "SELECT raw_key FROM api_keys WHERE user_id = $1 AND revoked_at IS NULL \
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
    } else {
        None
    };

    Some(modelrelay_web::templates::CloudWizardConfig {
        server_url: state.public_relay_url.clone(),
        worker_secret: api_key.clone(),
        api_key,
        workers_poll_url: "/dashboard/workers".to_string(),
    })
}

// ─── GET /setup ─────────────────────────────────────────────────────────────

pub async fn setup(
    OptionalSession(session): OptionalSession,
    State(state): State<Arc<CloudState>>,
) -> Response {
    let cloud_config = load_cloud_config(session, &state).await;
    Html(modelrelay_web::templates::setup_wizard_page_with_urls(
        cloud_config.as_ref(),
        &state.public_relay_url,
        &state.public_site_url,
    ))
    .into_response()
}

// ─── GET /integrate ──────────────────────────────────────────────────────────

pub async fn integrate(
    OptionalSession(session): OptionalSession,
    State(state): State<Arc<CloudState>>,
) -> Response {
    let cloud_config = load_cloud_config(session, &state).await;
    Html(modelrelay_web::templates::integrate_page_with_config(
        cloud_config.as_ref(),
    ))
    .into_response()
}

// ─── HTML rendering ─────────────────────────────────────────────────────────

fn error_page(path: &str, message: &str) -> Html<String> {
    Html(modelrelay_web::templates::page_shell(
        "Error",
        path,
        &format!(
            "<div class=\"card\" style=\"border-left: 3px solid var(--danger);\">\
             <h2>Something went wrong</h2>\
             <p>{}</p>\
             <p class=\"back-link\"><a href=\"/dashboard\">&larr; Back to dashboard</a></p></div>",
            html_escape(message)
        ),
        true,
    ))
}

fn no_db_html() -> String {
    "<div class=\"card\" style=\"border-left: 3px solid var(--warning);\">\
     <h2>Dashboard Unavailable</h2>\
     <p>The database connection is temporarily unavailable. Please check back shortly.</p></div>"
        .to_string()
}

fn status_badge(status: &str) -> &'static str {
    match status {
        "active" => "<span class=\"badge badge-active\">Active</span>",
        "past_due" => "<span class=\"badge badge-warn\">Past Due</span>",
        "canceled" => "<span class=\"badge badge-cancel\">Canceled</span>",
        "incomplete" => "<span class=\"badge\">Incomplete</span>",
        _ => "<span class=\"badge\">Unknown</span>",
    }
}

fn mask_key(raw: &str) -> String {
    if raw.len() <= 14 {
        return "••••••••••••".to_string();
    }
    let prefix = &raw[..10];
    let suffix = &raw[raw.len() - 4..];
    format!("{prefix}••••••••{suffix}")
}

fn dashboard_css() -> &'static str {
    r"<style>
    .dash-head {
      display: flex; justify-content: space-between; align-items: flex-start;
      gap: 16px; flex-wrap: wrap; margin-bottom: 24px;
    }
    .dash-head h1 { font-size: 1.9rem; font-weight: 800; letter-spacing: -0.03em; margin-bottom: 4px; color: var(--text-strong); }
    .dash-user { color: var(--text-secondary); font-size: 0.9rem; display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
    .dash-user strong { color: var(--text-strong); }
    .dash-actions { display: flex; gap: 8px; flex-wrap: wrap; }

    .kpi-row { display: grid; grid-template-columns: repeat(4, 1fr); gap: 12px; margin-bottom: 28px; }
    .kpi {
      background: var(--bg-surface); border: 1px solid var(--border);
      border-radius: var(--radius-md); padding: 16px 18px;
      box-shadow: var(--shadow-card);
    }
    .kpi-label {
      font-size: 0.7rem; font-weight: 700; text-transform: uppercase;
      letter-spacing: 0.08em; color: var(--text-muted); margin-bottom: 6px;
    }
    .kpi-value {
      font-size: 1.35rem; font-weight: 700; color: var(--text-strong);
      font-variant-numeric: tabular-nums; display: flex; align-items: center; gap: 8px; flex-wrap: wrap;
    }
    .kpi-endpoint { font-family: var(--font-mono); font-size: 0.86rem; color: var(--cyan); word-break: break-all; }

    .sec-label {
      font-size: 0.72rem; font-weight: 700; letter-spacing: 0.08em;
      text-transform: uppercase; color: var(--text-muted); margin-bottom: 10px;
    }
    .dash-grid { display: grid; grid-template-columns: 1.25fr 0.75fr; gap: 18px; margin-bottom: 26px; }
    .dash-grid .card { margin-bottom: 0; }

    .card-head {
      display: flex; justify-content: space-between; align-items: center; gap: 10px; flex-wrap: wrap;
      margin-bottom: 14px;
    }
    .card-head h2 { margin: 0; font-size: 1.08rem; font-weight: 700; color: var(--text-strong); display: flex; align-items: center; gap: 8px; }

    .card--accent { border-left: 3px solid var(--accent); }
    .card--empty { border: 1px dashed var(--border-strong); background: var(--bg-subtle); text-align: center; padding: 36px 24px; box-shadow: none; }

    .key-row-actions { display: flex; gap: 8px; flex-wrap: wrap; align-items: center; margin-top: 12px; }
    .key-code-box {
      display: flex; align-items: center; justify-content: space-between; gap: 10px; flex-wrap: wrap;
      padding: 11px 14px; background: var(--bg-input); border: 1px solid var(--border);
      border-radius: var(--radius-md); font-family: var(--font-mono); font-size: 0.84rem;
      color: var(--accent-fg); word-break: break-all;
    }

    .keys-table, .workers-table { width: 100%; border-collapse: collapse; font-size: 0.86rem; }
    .keys-table th, .workers-table th {
      text-align: left; font-size: 0.7rem; font-weight: 700; text-transform: uppercase;
      letter-spacing: 0.07em; color: var(--text-muted);
      padding: 8px 10px 9px 0; border-bottom: 1px solid var(--border);
    }
    .keys-table td, .workers-table td {
      padding: 11px 10px 11px 0; border-bottom: 1px solid var(--border);
      color: var(--text-primary); vertical-align: middle;
    }
    .keys-table tr:last-child td, .workers-table tr:last-child td { border-bottom: none; }
    .keys-date { color: var(--text-muted); white-space: nowrap; font-size: 0.82rem; }
    .key-cell { font-family: var(--font-mono); font-size: 0.8rem; color: var(--accent-fg); }
    .key-cell-btns { display: inline-flex; gap: 6px; align-items: center; flex-wrap: wrap; justify-content: flex-end; }

    .model-pill {
      display: inline-block; padding: 2px 8px; margin: 2px 4px 2px 0; border-radius: 5px;
      background: var(--accent-soft); border: 1px solid var(--accent-border);
      font-family: var(--font-mono); font-size: 0.76rem; color: var(--accent-fg);
    }
    .load-wrap { display: inline-flex; align-items: center; gap: 8px; font-variant-numeric: tabular-nums; font-size: 0.82rem; color: var(--text-secondary); }
    .load-track { width: 58px; height: 6px; border-radius: 999px; background: var(--bg-elevated); overflow: hidden; }
    .load-fill { height: 100%; background: linear-gradient(90deg, var(--accent), var(--cyan)); border-radius: 999px; }

    .quick-grid { display: grid; grid-template-columns: repeat(4, 1fr); gap: 12px; margin-bottom: 24px; }
    .quick-card {
      display: block; padding: 16px 18px; background: var(--bg-surface);
      border: 1px solid var(--border); border-radius: var(--radius-md);
      text-decoration: none; transition: border-color 0.15s, transform 0.15s;
      box-shadow: var(--shadow-card);
    }
    .quick-card:hover { border-color: var(--accent-border); transform: translateY(-2px); text-decoration: none; }
    .quick-card h3 { font-size: 0.92rem; color: var(--text-strong); margin-bottom: 4px; }
    .quick-card p { font-size: 0.82rem; color: var(--text-secondary); margin: 0; }

    .dash-skel {
      background: linear-gradient(90deg, var(--bg-surface) 25%, var(--bg-elevated) 50%, var(--bg-surface) 75%);
      background-size: 200% 100%; animation: dash-pulse 1.4s ease-in-out infinite;
      border-radius: 6px; height: 14px; margin-bottom: 10px;
    }
    .dash-skel-w70 { width: 70%; }
    .dash-skel-w45 { width: 45%; }
    @keyframes dash-pulse { 0% { background-position: 200% 0; } 100% { background-position: -200% 0; } }

    .billing-link {
      background: none; border: none; color: var(--accent-bright); cursor: pointer;
      font-size: 0.88rem; font-weight: 600; font-family: inherit; padding: 0; margin-top: 12px;
    }
    .billing-link:hover { text-decoration: underline; }
    .empty-msg { color: var(--text-muted); font-size: 0.88rem; }

    @media (max-width: 860px) {
      .kpi-row, .quick-grid { grid-template-columns: repeat(2, 1fr); }
      .dash-grid { grid-template-columns: 1fr; }
      .keys-table, .workers-table { display: block; overflow-x: auto; }
    }
    @media (max-width: 520px) {
      .kpi-row, .quick-grid { grid-template-columns: 1fr; }
    }
    </style>"
}

fn dashboard_shared_js() -> &'static str {
    r"<script>
(function() {
  document.addEventListener('click', function(e) {
    var toggle = e.target.closest('[data-toggle-key]');
    if (!toggle) return;
    var target = document.getElementById(toggle.getAttribute('data-toggle-key'));
    if (!target) return;
    var revealed = target.getAttribute('data-revealed') === 'true';
    target.textContent = revealed ? target.getAttribute('data-masked') : target.getAttribute('data-full');
    target.setAttribute('data-revealed', revealed ? 'false' : 'true');
    toggle.textContent = revealed ? 'Reveal' : 'Hide';
  });
})();
</script>"
}

#[allow(clippy::too_many_lines)]
fn admin_dashboard_html(
    email: &str,
    relay_url: &str,
    keys: &[ApiKeyRow],
    csrf_field: &str,
) -> String {
    let email_escaped = html_escape(email);
    let relay_escaped = html_escape(relay_url);
    let key_count = keys.len();
    let first_key = keys.first().map(|k| k.raw_key.as_str());

    let mut out = String::new();

    let _ = write!(
        out,
        r#"<div class="dash-head">
          <div>
            <h1>Dashboard</h1>
            <div class="dash-user">Signed in as <strong>{email_escaped}</strong> <span class="badge badge-active">Admin</span></div>
          </div>
          <div class="dash-actions">
            <a href="/integrate#pi" class="btn btn-sm">Pi &amp; API Setup</a>
            <a href="/setup" class="btn btn-sm btn-secondary">+ Connect Worker</a>
            <a href="/downloads" class="btn btn-sm btn-secondary">Downloads</a>
          </div>
        </div>

        <div class="kpi-row">
          <div class="kpi">
            <div class="kpi-label">Relay Endpoint</div>
            <div class="kpi-value">
              <span class="kpi-endpoint">{relay_escaped}</span>
              <button type="button" class="copy-btn" data-copy-text="{relay_escaped}">Copy</button>
            </div>
          </div>
          <div class="kpi">
            <div class="kpi-label">Connected Workers</div>
            <div class="kpi-value" id="kpi-workers">&mdash;</div>
          </div>
          <div class="kpi">
            <div class="kpi-label">Queue Depth</div>
            <div class="kpi-value" id="kpi-queue">&mdash;</div>
          </div>
          <div class="kpi">
            <div class="kpi-label">Active API Keys</div>
            <div class="kpi-value">{key_count}</div>
          </div>
        </div>"#
    );

    // ── Section: API Keys ──
    out.push_str(
        r#"<div class="sec-label">API Keys</div>
        <div class="card">
          <div class="card-head">
            <h2>Your Keys</h2>
            <form method="POST" action="/dashboard/keys/generate" style="display:inline;">"#,
    );
    let _ = write!(
        out,
        r#"{csrf_field}<button type="submit" class="btn btn-sm">+ Generate New Key</button></form></div>"#
    );

    if keys.is_empty() {
        out.push_str("<p class=\"empty-msg\">No active API keys yet. Generate one above to connect workers and Pi Coding Agent.</p>");
    } else {
        out.push_str(
            "<table class=\"keys-table\">\
             <thead><tr><th>Name</th><th>Key</th><th>Created</th><th style=\"text-align:right;\">Actions</th></tr></thead><tbody>",
        );
        for (idx, key) in keys.iter().enumerate() {
            let name_escaped = html_escape(&key.name);
            let key_escaped = html_escape(&key.raw_key);
            let masked = html_escape(&mask_key(&key.raw_key));
            let created = key.created_at.format("%b %d, %Y").to_string();
            let auth_snippet = html_escape(&format!(
                "{{\n  \"modelrelay\": {{\n    \"type\": \"api_key\",\n    \"key\": \"{}\"\n  }}\n}}",
                key.raw_key
            ));
            let _ = write!(
                out,
                r#"<tr>
                  <td>{name_escaped}</td>
                  <td><span class="key-cell" id="adm-key-{idx}" data-masked="{masked}" data-full="{key_escaped}" data-revealed="false">{masked}</span></td>
                  <td class="keys-date">{created}</td>
                  <td>
                    <div class="key-cell-btns">
                      <button type="button" class="copy-btn" data-toggle-key="adm-key-{idx}">Reveal</button>
                      <button type="button" class="copy-btn" data-copy-text="{key_escaped}">Copy Key</button>
                      <button type="button" class="copy-btn" data-copy-text="{auth_snippet}">Pi auth.json</button>
                      <form method="POST" action="/dashboard/keys/{}/revoke" style="display:inline">
                        {csrf_field}
                        <button type="submit" class="btn btn-sm btn-danger"
                          onclick="return confirm('Revoke this API key? This cannot be undone.')">
                          Revoke
                        </button>
                      </form>
                    </div>
                  </td>
                </tr>"#,
                key.id,
            );
        }
        out.push_str("</tbody></table>");
    }
    out.push_str("</div>");

    // ── Section: Workers & Live Telemetry ──
    let _ = write!(
        out,
        r#"<div class="sec-label">Fleet &amp; Telemetry</div>
        <div class="dash-grid">
          <div class="card">
            <div class="card-head">
              <h2>Connected Workers</h2>
              <a href="/setup" style="font-size:0.84rem;font-weight:600;">+ Add machine &rarr;</a>
            </div>
            <div id="admin-workers">
              <div class="dash-skel dash-skel-w70"></div>
              <div class="dash-skel dash-skel-w45"></div>
            </div>
            <script>
              (function(){{
                function pollWorkers(){{
                  fetch('/dashboard/workers',{{credentials:'same-origin'}})
                    .then(function(r){{return r.json();}})
                    .then(function(d){{
                      var el=document.getElementById('admin-workers');
                      el.textContent='';
                      var ws=d.workers||[];
                      var kpiW=document.getElementById('kpi-workers');
                      if(kpiW){{kpiW.textContent=String(ws.length);kpiW.style.color=ws.length>0?'var(--success)':'var(--warning)';}}
                      if(ws.length===0){{
                        var wrap=document.createElement('div');
                        wrap.className='card card--empty';
                        wrap.style.marginBottom='0';
                        var p=document.createElement('p');
                        p.textContent='No GPU workers connected right now. Launch the Desktop app or run modelrelay-worker.';
                        p.style.marginBottom='14px';
                        var a=document.createElement('a');
                        a.href='/downloads'; a.className='btn btn-sm'; a.textContent='Download Desktop App \u2192';
                        wrap.appendChild(p); wrap.appendChild(a);
                        el.appendChild(wrap);
                        return;
                      }}
                      var tbl=document.createElement('table');
                      tbl.className='workers-table';
                      var thead=document.createElement('thead');
                      var hr=document.createElement('tr');
                      ['Worker','Models','Load','Status'].forEach(function(col){{var th=document.createElement('th');th.textContent=col;hr.appendChild(th);}});
                      thead.appendChild(hr); tbl.appendChild(thead);
                      var tbody=document.createElement('tbody');
                      ws.forEach(function(w){{
                        var tr=document.createElement('tr');
                        var tdName=document.createElement('td');
                        tdName.textContent=w.worker_name||w.worker_id||'worker';
                        var tdModels=document.createElement('td');
                        (w.models||[]).forEach(function(m){{
                          var sp=document.createElement('span');
                          sp.className='model-pill';
                          sp.textContent=m==='*'?'All models':m;
                          tdModels.appendChild(sp);
                        }});
                        var tdLoad=document.createElement('td');
                        var inflight=w.in_flight_count||0, maxc=w.max_concurrent||0;
                        var pct=maxc?Math.min(100,Math.round((inflight/maxc)*100)):0;
                        var lw=document.createElement('span'); lw.className='load-wrap';
                        var lt=document.createElement('span'); lt.className='load-track';
                        var lf=document.createElement('span'); lf.className='load-fill'; lf.style.width=pct+'%';
                        lt.appendChild(lf); lw.appendChild(lt);
                        lw.appendChild(document.createTextNode(inflight+'/'+maxc));
                        tdLoad.appendChild(lw);
                        var tdSt=document.createElement('td');
                        var b=document.createElement('span');
                        b.className=w.is_draining?'badge badge-warn':'badge badge-active';
                        b.textContent=w.is_draining?'Draining':'Active';
                        tdSt.appendChild(b);
                        tr.appendChild(tdName); tr.appendChild(tdModels); tr.appendChild(tdLoad); tr.appendChild(tdSt);
                        tbody.appendChild(tr);
                      }});
                      tbl.appendChild(tbody);
                      el.appendChild(tbl);
                    }})
                    .catch(function(){{
                      var el=document.getElementById('admin-workers');
                      el.textContent='';
                      var p=document.createElement('p');
                      p.className='empty-msg';
                      p.textContent='Could not load worker status.';
                      el.appendChild(p);
                    }});
                }}
                function pollStats(){{
                  fetch('/dashboard/stats',{{credentials:'same-origin'}})
                    .then(function(r){{return r.json();}})
                    .then(function(d){{
                      var qd=d.queue_depth||{{}};
                      var total=0;
                      Object.keys(qd).forEach(function(k){{total+=Number(qd[k])||0;}});
                      var kpiQ=document.getElementById('kpi-queue');
                      if(kpiQ) kpiQ.textContent=String(total);
                    }}).catch(function(){{}});
                }}
                pollWorkers(); pollStats();
                setInterval(function(){{pollWorkers();pollStats();}}, 5000);
              }})();
            </script>
          </div>

          <div class="card">
            <div class="card-head"><h2>Account</h2></div>
            <table class="info-table" style="margin-top:0;">
              <tr><td>Email</td><td>{email_escaped}</td></tr>
              <tr><td>Role</td><td>Administrator</td></tr>
              <tr><td>Endpoint</td><td><code>{relay_escaped}</code></td></tr>
            </table>
          </div>
        </div>"#
    );

    // ── Quick Connect Strip ──
    if let Some(key) = first_key {
        let curl_cmd = html_escape(&format!(
            "curl {relay_url}/v1/models \\\n  -H \"Authorization: Bearer {key}\""
        ));
        let _ = write!(
            out,
            r#"<div class="sec-label">Quick Verify</div>
            <div class="card">
              <div class="card-head">
                <h2>Test <code>{relay_escaped}</code> with curl</h2>
                <a href="/integrate#pi" style="font-size:0.84rem;font-weight:600;">Set up Pi Coding Agent &rarr;</a>
              </div>
              <div class="code-shell" style="margin:0;">
                <div class="code-shell-header"><span>GET /v1/models</span><button type="button" class="copy-btn" data-copy-target="dash-curl-adm">Copy</button></div>
                <pre><code id="dash-curl-adm">{curl_cmd}</code></pre>
              </div>
            </div>"#
        );
    }

    out
}

#[allow(clippy::too_many_lines)]
fn subscriber_dashboard_html(
    email: &str,
    relay_url: &str,
    sub: Option<&SubscriptionRow>,
    has_stripe_customer: bool,
    api_key: Option<&str>,
    csrf_field: &str,
) -> String {
    let email_escaped = html_escape(email);
    let relay_escaped = html_escape(relay_url);
    let mut out = String::new();

    let sub_pill = sub.map_or("<span class=\"badge\">No Plan</span>", |s| {
        status_badge(&s.status)
    });

    let _ = write!(
        out,
        r#"<div class="dash-head">
          <div>
            <h1>Dashboard</h1>
            <div class="dash-user">Signed in as <strong>{email_escaped}</strong> {sub_pill}</div>
          </div>
          <div class="dash-actions">
            <a href="/integrate#pi" class="btn btn-sm">Pi &amp; API Setup</a>
            <a href="/setup" class="btn btn-sm btn-secondary">+ Connect Worker</a>
            <a href="/downloads" class="btn btn-sm btn-secondary">Downloads</a>
          </div>
        </div>

        <div class="kpi-row">
          <div class="kpi">
            <div class="kpi-label">Relay Endpoint</div>
            <div class="kpi-value">
              <span class="kpi-endpoint">{relay_escaped}</span>
              <button type="button" class="copy-btn" data-copy-text="{relay_escaped}">Copy</button>
            </div>
          </div>
          <div class="kpi">
            <div class="kpi-label">Active Workers</div>
            <div class="kpi-value" id="sub-kpi-workers">&mdash;</div>
          </div>
          <div class="kpi">
            <div class="kpi-label">Queue Depth</div>
            <div class="kpi-value" id="sub-kpi-queue">&mdash;</div>
          </div>
          <div class="kpi">
            <div class="kpi-label">Plan Status</div>
            <div class="kpi-value" style="font-size:1rem;">{sub_pill}</div>
          </div>
        </div>"#
    );

    // ── Section: API Key ──
    out.push_str("<div class=\"sec-label\">Your API Key</div>");

    if let Some(s) = sub {
        if let Some(key) = api_key {
            let key_escaped = html_escape(key);
            let masked = html_escape(&mask_key(key));
            let auth_snippet = html_escape(&format!(
                "{{\n  \"modelrelay\": {{\n    \"type\": \"api_key\",\n    \"key\": \"{key}\"\n  }}\n}}"
            ));
            let _ = write!(
                out,
                r#"<div class="card card--accent">
                  <div class="card-head">
                    <h2>API Key <span class="badge badge-active">Active</span></h2>
                    <span style="font-size:0.8rem;color:var(--text-muted);">Used by workers, curl, and Pi <code>auth.json</code></span>
                  </div>
                  <div class="key-code-box">
                    <span id="sub-api-key" data-masked="{masked}" data-full="{key_escaped}" data-revealed="false">{masked}</span>
                    <div style="display:flex;gap:6px;flex-wrap:wrap;">
                      <button type="button" class="copy-btn" data-toggle-key="sub-api-key">Reveal</button>
                      <button type="button" class="copy-btn" data-copy-text="{key_escaped}">Copy Key</button>
                      <button type="button" class="copy-btn" data-copy-text="{auth_snippet}">Copy Pi auth.json</button>
                    </div>
                  </div>
                  <div class="key-row-actions">
                    <a href="/integrate#pi" class="btn btn-sm">Set up Pi Coding Agent &rarr;</a>
                    <a href="/setup" class="btn btn-sm btn-secondary">Connect a GPU worker</a>
                  </div>
                </div>"#
            );
        } else if s.api_key_id.is_some() {
            out.push_str(
                "<div class=\"card card--accent\">\
                   <div class=\"card-head\"><h2>API Key <span class=\"badge badge-active\">Provisioned</span></h2></div>\
                   <p>Your API key has been provisioned and stored securely.</p>\
                   <div class=\"key-row-actions\"><a href=\"/integrate\" class=\"btn btn-sm\">Pi &amp; API Setup &rarr;</a></div>\
                 </div>",
            );
        } else if s.status == "active" {
            out.push_str(
                "<div class=\"card\">\
                   <div class=\"card-head\"><h2>API Key <span class=\"badge\">Provisioning&hellip;</span></h2></div>\
                   <div class=\"dash-skel dash-skel-w70\"></div>\
                   <p>Your subscription is active. Your API key is being provisioned and will appear here shortly.</p>\
                 </div>",
            );
        } else {
            out.push_str(
                "<div class=\"card card--empty\">\
                   <h2>Get Your API Key</h2>\
                   <p style=\"margin-bottom:18px;\">An active subscription is required for hosted API key access.</p>\
                   <a href=\"/pricing\" class=\"btn\">View Pricing &rarr;</a>\
                 </div>",
            );
        }
    } else {
        out.push_str(
            "<div class=\"card card--empty\">\
               <h2>Start Routing Inference Requests</h2>\
               <p style=\"margin-bottom:18px;\">Subscribe for a hosted relay key, or self-host ModelRelay for free.</p>\
               <div style=\"display:flex;gap:10px;justify-content:center;flex-wrap:wrap;\">\
                 <a href=\"/pricing\" class=\"btn\">View Pricing &rarr;</a>\
                 <a href=\"/docs/self-hosting\" class=\"btn btn-secondary\">Self-Hosting Guide</a>\
               </div>\
             </div>",
        );
    }

    // ── Section: Live Status & Subscription ──
    let sub_details = if let Some(s) = sub {
        format!(
            "<table class=\"info-table\" style=\"margin-top:0\">\
               <tr><td>Status</td><td>{}</td></tr>\
               <tr><td>Updated</td><td>{}</td></tr>\
             </table>",
            html_escape(&s.status),
            s.updated_at.format("%B %d, %Y"),
        )
    } else {
        "<p class=\"empty-msg\">No active subscription. <a href=\"/pricing\">View pricing &rarr;</a></p>".to_string()
    };

    let billing_btn = if has_stripe_customer {
        format!(
            "<form method=\"POST\" action=\"/dashboard/billing-portal\">\
               {csrf_field}\
               <button type=\"submit\" class=\"billing-link\">Manage billing &amp; invoices &rarr;</button>\
             </form>"
        )
    } else {
        String::new()
    };

    let live_models_block = if let Some(key) = api_key {
        let js_url = relay_url.replace('\\', "\\\\").replace('"', "\\\"");
        let js_key = key.replace('\\', "\\\\").replace('"', "\\\"");
        format!(
            r#"<div style="margin-top:16px;padding-top:14px;border-top:1px solid var(--border);">
              <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:8px;">
                <span class="sec-label" style="margin:0;">Your Live Models (<code>/v1/models</code>)</span>
                <button type="button" class="copy-btn" id="refresh-models-btn">Refresh</button>
              </div>
              <div id="live-models-box"><span class="empty-msg">Checking <code>{relay_escaped}/v1/models</code>&hellip;</span></div>
            </div>
            <script>
              (function(){{
                function loadModels(){{
                  var box=document.getElementById('live-models-box');
                  if(!box) return;
                  fetch("{js_url}/v1/models", {{ headers: {{ "Authorization": "Bearer {js_key}" }} }})
                    .then(function(r){{ if(!r.ok) throw new Error('HTTP '+r.status); return r.json(); }})
                    .then(function(d){{
                      var ms=(d.data||[]).map(function(x){{return x.id;}}).filter(Boolean);
                      box.textContent='';
                      if(ms.length===0){{
                        var sp=document.createElement('span');
                        sp.className='empty-msg';
                        sp.textContent='No models advertised yet. Start ModelRelay Desktop or modelrelay-worker on your GPU box.';
                        box.appendChild(sp);
                        return;
                      }}
                      ms.forEach(function(id){{
                        var tag=document.createElement('span');
                        tag.className='model-pill';
                        tag.textContent=id;
                        box.appendChild(tag);
                      }});
                    }})
                    .catch(function(){{
                      box.textContent='';
                      var sp=document.createElement('span');
                      sp.className='empty-msg';
                      sp.textContent='Could not query live models right now.';
                      box.appendChild(sp);
                    }});
                }}
                var btn=document.getElementById('refresh-models-btn');
                if(btn) btn.addEventListener('click', loadModels);
                loadModels();
              }})();
            </script>"#
        )
    } else {
        String::new()
    };

    let _ = write!(
        out,
        r#"<div class="sec-label">Status &amp; Billing</div>
        <div class="dash-grid">
          <div class="card">
            <div class="card-head">
              <h2>Relay Health</h2>
              <span class="badge badge-cyan">Live</span>
            </div>
            <div id="relay-stats">
              <div class="dash-skel dash-skel-w70"></div>
              <div class="dash-skel dash-skel-w45"></div>
            </div>
            {live_models_block}
            <script>
              (function(){{
                fetch('/dashboard/stats',{{credentials:'same-origin'}})
                  .then(function(r){{return r.json();}})
                  .then(function(d){{
                    var el=document.getElementById('relay-stats');
                    el.textContent='';
                    var qd=d.queue_depth||{{}};
                    var total=0;
                    Object.keys(qd).forEach(function(k){{total+=Number(qd[k])||0;}});
                    var aw=Number(d.active_workers)||0;
                    var kw=document.getElementById('sub-kpi-workers');
                    if(kw){{kw.textContent=String(aw);kw.style.color=aw>0?'var(--success)':'var(--warning)';}}
                    var kq=document.getElementById('sub-kpi-queue');
                    if(kq) kq.textContent=String(total);
                    var tbl=document.createElement('table');
                    tbl.className='info-table'; tbl.style.marginTop='0';
                    [['Workers online',String(aw)],['Queued requests',String(total)]].forEach(function(pair){{
                      var tr=document.createElement('tr');
                      var td1=document.createElement('td'); td1.textContent=pair[0];
                      var td2=document.createElement('td'); td2.textContent=pair[1];
                      tr.appendChild(td1); tr.appendChild(td2); tbl.appendChild(tr);
                    }});
                    el.appendChild(tbl);
                  }})
                  .catch(function(){{
                    var el=document.getElementById('relay-stats');
                    el.textContent='';
                    var p=document.createElement('p');
                    p.className='empty-msg'; p.textContent='Could not load relay stats.';
                    el.appendChild(p);
                  }});
              }})();
            </script>
          </div>
          <div class="card">
            <div class="card-head"><h2>Subscription</h2></div>
            {sub_details}
            {billing_btn}
          </div>
        </div>"#
    );

    // ── Quick Links ──
    out.push_str(
        r#"<div class="sec-label">Next Steps</div>
        <div class="quick-grid">
          <a href="/integrate#pi" class="quick-card">
            <h3>Pi Coding Agent &rarr;</h3>
            <p>Register ModelRelay as a provider + add token to <code>auth.json</code></p>
          </a>
          <a href="/downloads" class="quick-card">
            <h3>Desktop App &rarr;</h3>
            <p>Download the native tray worker for macOS, Linux, or Windows</p>
          </a>
          <a href="/setup" class="quick-card">
            <h3>Worker Setup &rarr;</h3>
            <p>CLI worker config and systemd / launchd persistence guide</p>
          </a>
          <a href="/docs" class="quick-card">
            <h3>Documentation &rarr;</h3>
            <p><code>api.modelrelay.io</code> reference, curl examples, and self-hosting</p>
          </a>
        </div>"#,
    );

    out
}

/// Minimal HTML entity escaping for untrusted strings.
fn html_escape(s: &str) -> String {
    modelrelay_web::templates::html_escape(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_ampersand() {
        assert_eq!(html_escape("a&b"), "a&amp;b");
    }

    #[test]
    fn escapes_less_than() {
        assert_eq!(html_escape("<script>"), "&lt;script&gt;");
    }

    #[test]
    fn escapes_greater_than() {
        assert_eq!(html_escape("a>b"), "a&gt;b");
    }

    #[test]
    fn escapes_double_quote() {
        assert_eq!(html_escape(r#"a"b"#), "a&quot;b");
    }

    #[test]
    fn escapes_single_quote() {
        assert_eq!(html_escape("a'b"), "a&#x27;b");
    }

    #[test]
    fn escapes_all_at_once() {
        assert_eq!(
            html_escape(r#"<div class="x" data-val='a&b'>"#),
            "&lt;div class=&quot;x&quot; data-val=&#x27;a&amp;b&#x27;&gt;"
        );
    }

    #[test]
    fn no_escape_needed() {
        assert_eq!(html_escape("hello world 123"), "hello world 123");
    }

    #[test]
    fn empty_string() {
        assert_eq!(html_escape(""), "");
    }

    #[test]
    fn status_badge_renders_correctly() {
        assert!(status_badge("active").contains("badge-active"));
        assert!(status_badge("past_due").contains("badge-warn"));
        assert!(status_badge("canceled").contains("badge-cancel"));
        assert!(status_badge("unknown_status").contains("Unknown"));
    }

    #[test]
    fn mask_key_hides_middle_bytes() {
        let masked = mask_key("mr_live_1234567890abcdef");
        assert!(masked.starts_with("mr_live_12"));
        assert!(masked.ends_with("cdef"));
        assert!(!masked.contains("567890ab"));
    }
}
