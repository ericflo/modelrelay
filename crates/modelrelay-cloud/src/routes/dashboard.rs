use std::fmt::Write;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use tower_sessions::Session;

use crate::state::CloudState;

use super::csrf;

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

/// GET /dashboard — show subscription status and API key info.
pub async fn page(session: Session, State(state): State<Arc<CloudState>>) -> Response {
    let Some(ref pool) = state.db else {
        return Html(modelrelay_web::templates::page_shell(
            "Dashboard",
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

    // Query user info
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
                "<div class=\"card\"><h2>Error</h2><p>Could not load your account. Please try again later.</p></div>",
                true,
            ))
            .into_response();
        }
    };

    if user.is_admin {
        // ── Admin dashboard ──
        let keys = sqlx::query_as::<_, ApiKeyRow>(
            "SELECT id, key_id, raw_key, name, created_at FROM api_keys \
             WHERE user_id = $1 AND revoked_at IS NULL ORDER BY created_at DESC",
        )
        .bind(user_id)
        .fetch_all(pool)
        .await
        .unwrap_or_default();

        let html = admin_dashboard_html(&user.email, &keys, &csrf_field);
        Html(modelrelay_web::templates::page_shell(
            "Dashboard",
            &html,
            true,
        ))
        .into_response()
    } else {
        // ── Regular user dashboard ──
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

        // Load API keys for the user (from the new table)
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
            sub.as_ref(),
            has_stripe_customer,
            api_key_display.as_deref(),
            &csrf_field,
        );
        Html(modelrelay_web::templates::page_shell(
            "Dashboard",
            &html,
            true,
        ))
        .into_response()
    }
}

// ─── POST /dashboard/billing-portal ─────────────────────────────────────────

/// POST /dashboard/billing-portal — create a Stripe billing portal session and redirect.
pub async fn billing_portal(session: Session, State(state): State<Arc<CloudState>>) -> Response {
    let Some(ref key) = state.stripe_key else {
        return error_page("Billing not configured").into_response();
    };

    let Some(ref pool) = state.db else {
        return error_page("Database not available").into_response();
    };

    let user_id = match require_user(&session).await {
        Ok(id) => id,
        Err(r) => return r,
    };

    // Get customer ID
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
            "<div class=\"card\"><h2>No billing account</h2>\
             <p>No Stripe customer found for your account.</p>\
             <p><a href=\"/dashboard\">&larr; Back to dashboard</a></p></div>",
            true,
        ))
        .into_response();
    };

    let client = reqwest::Client::new();
    let params = [
        ("customer", customer_id.as_str()),
        ("return_url", "https://modelrelay.io/dashboard"),
    ];

    let resp = client
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
                    error_page("Stripe did not return a portal URL.").into_response()
                }
            }
            Err(e) => {
                tracing::error!("stripe portal response parse error: {e}");
                error_page("Could not process billing portal response.").into_response()
            }
        },
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            tracing::error!("stripe billing portal API error: {status} — {body}");
            error_page("Could not open billing portal. Please try again later.").into_response()
        }
        Err(e) => {
            tracing::error!("stripe billing portal request error: {e}");
            error_page("Could not reach payment provider. Please try again later.").into_response()
        }
    }
}

// ─── POST /dashboard/keys/generate ──────────────────────────────────────────

/// POST /dashboard/keys/generate — admin-only: provision a new API key.
pub async fn keys_generate(session: Session, State(state): State<Arc<CloudState>>) -> Response {
    let Some(ref pool) = state.db else {
        return error_page("Database not available").into_response();
    };

    let user_id = match require_user(&session).await {
        Ok(id) => id,
        Err(r) => return r,
    };

    // Verify admin
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
        return error_page("Admin API not configured. Cannot generate API keys at this time.")
            .into_response();
    };
    let Some(ref admin_token) = state.admin_token else {
        return error_page("Admin API not configured. Cannot generate API keys at this time.")
            .into_response();
    };

    // Get user email for key name
    let email: String = match sqlx::query_scalar("SELECT email FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(pool)
        .await
    {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("keys_generate user lookup error: {e}");
            return error_page("Could not look up your account.").into_response();
        }
    };

    let ts = chrono::Utc::now().format("%Y%m%d%H%M%S");
    let key_name = format!("admin-{email}-{ts}");

    match super::webhook::provision_api_key(admin_url, admin_token, &key_name).await {
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
                tracing::error!("keys_generate db insert error: {e}");
                return error_page(
                    "Key was provisioned on the server but could not be saved. Contact support.",
                )
                .into_response();
            }
            tracing::info!(key_id = %key_id, email = %email, "admin generated new API key");
            Redirect::to("/dashboard").into_response()
        }
        Err(e) => {
            tracing::error!("keys_generate provision error: {e}");
            error_page(
                "Could not generate API key. The relay server may be unreachable. Please try again later.",
            )
            .into_response()
        }
    }
}

// ─── POST /dashboard/keys/:id/revoke ────────────────────────────────────────

/// POST /dashboard/keys/:id/revoke — admin-only: revoke an API key.
pub async fn keys_revoke(
    session: Session,
    State(state): State<Arc<CloudState>>,
    Path(key_uuid): Path<uuid::Uuid>,
) -> Response {
    let Some(ref pool) = state.db else {
        return error_page("Database not available").into_response();
    };

    let user_id = match require_user(&session).await {
        Ok(id) => id,
        Err(r) => return r,
    };

    // Verify admin
    let is_admin: Option<bool> = sqlx::query_scalar("SELECT is_admin FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();

    if is_admin != Some(true) {
        return (StatusCode::FORBIDDEN, "Forbidden").into_response();
    }

    // Look up the key — must belong to this user and not already revoked
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
        return error_page("API key not found or already revoked.").into_response();
    };

    // Attempt to revoke on the server
    if let (Some(admin_url), Some(admin_token)) = (&state.admin_url, &state.admin_token) {
        if let Err(e) = super::webhook::revoke_api_key(admin_url, admin_token, &key_id).await {
            tracing::warn!("keys_revoke server call failed (best-effort revoke): {e}");
            // Best effort — still mark as revoked in DB
        }
    } else {
        tracing::warn!("admin API not configured — revoking key locally only");
    }

    // Mark as revoked in DB regardless of server result
    if let Err(e) = sqlx::query("UPDATE api_keys SET revoked_at = now() WHERE id = $1")
        .bind(key_uuid)
        .execute(pool)
        .await
    {
        tracing::error!("keys_revoke db update error: {e}");
        return error_page("Could not revoke key. Please try again.").into_response();
    }

    tracing::info!(key_id = %key_id, "admin revoked API key");
    Redirect::to("/dashboard").into_response()
}

// ─── GET /dashboard/workers ─────────────────────────────────────────────────
//
// Proxy endpoint: authenticated cloud users can poll worker status via the
// admin API without needing the raw admin token.

/// GET /dashboard/workers — proxy to admin API `/admin/workers` for authenticated users.
pub async fn workers(session: Session, State(state): State<Arc<CloudState>>) -> Response {
    let _user_id = match require_user(&session).await {
        Ok(id) => id,
        Err(r) => return r,
    };

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

    let client = reqwest::Client::new();
    let url = format!("{}/admin/workers", admin_url.trim_end_matches('/'));
    match client.get(&url).bearer_auth(admin_token).send().await {
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
//
// Proxy endpoint: authenticated cloud users can poll relay stats via the
// admin API without needing the raw admin token.

/// GET /dashboard/stats — proxy to admin API `/admin/stats` for authenticated users.
pub async fn stats(session: Session, State(state): State<Arc<CloudState>>) -> Response {
    let _user_id = match require_user(&session).await {
        Ok(id) => id,
        Err(r) => return r,
    };

    let empty_stats = || serde_json::json!({"queue_depth": {}, "active_workers": 0});

    let Some(ref admin_url) = state.admin_url else {
        return (StatusCode::SERVICE_UNAVAILABLE, axum::Json(empty_stats())).into_response();
    };
    let Some(ref admin_token) = state.admin_token else {
        return (StatusCode::SERVICE_UNAVAILABLE, axum::Json(empty_stats())).into_response();
    };

    let client = reqwest::Client::new();
    let url = format!("{}/admin/stats", admin_url.trim_end_matches('/'));
    match client.get(&url).bearer_auth(admin_token).send().await {
        Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
            Ok(body) => axum::Json(body).into_response(),
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

// ─── GET /setup ─────────────────────────────────────────────────────────────

/// GET /setup — serve the setup wizard with cloud context pre-filled for
/// authenticated users (server URL, API key, proxy poll endpoint).
pub async fn setup(session: Session, State(state): State<Arc<CloudState>>) -> Response {
    // If the user is logged in, inject cloud config; otherwise serve the plain wizard.
    let cloud_config = if let Ok(user_id) = require_user(&session).await {
        let pool = state.db.as_ref();
        let api_key: Option<String> = if let Some(pool) = pool {
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
            server_url: state
                .admin_url
                .as_deref()
                .unwrap_or("https://api.modelrelay.io")
                .to_string(),
            worker_secret: api_key.clone(),
            api_key,
            workers_poll_url: "/dashboard/workers".to_string(),
        })
    } else {
        None
    };

    Html(modelrelay_web::templates::setup_wizard_page_with_config(
        cloud_config.as_ref(),
    ))
    .into_response()
}

// ─── GET /integrate ──────────────────────────────────────────────────────────

/// GET /integrate — show integration snippets for tools, agents, and SDKs.
pub async fn integrate(session: Session, State(state): State<Arc<CloudState>>) -> Response {
    // If the user is logged in, inject cloud config; otherwise serve the plain page.
    let cloud_config = if let Ok(user_id) = require_user(&session).await {
        let pool = state.db.as_ref();
        let api_key: Option<String> = if let Some(pool) = pool {
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
            server_url: state
                .admin_url
                .as_deref()
                .unwrap_or("https://api.modelrelay.io")
                .to_string(),
            worker_secret: api_key.clone(),
            api_key,
            workers_poll_url: "/dashboard/workers".to_string(),
        })
    } else {
        None
    };

    Html(modelrelay_web::templates::integrate_page_with_config(
        cloud_config.as_ref(),
    ))
    .into_response()
}

// ─── HTML rendering ─────────────────────────────────────────────────────────

fn error_page(message: &str) -> Html<String> {
    Html(modelrelay_web::templates::page_shell(
        "Error",
        &format!(
            "<div class=\"card\"><h2>Error</h2><p>{}</p>\
             <p style=\"margin-top:12px;\"><a href=\"/dashboard\">&larr; Back to dashboard</a></p></div>",
            html_escape(message)
        ),
        true,
    ))
}

fn no_db_html() -> String {
    "<div class=\"card\"><h2>Dashboard Unavailable</h2>\
     <p>The database is not connected. Please try again later.</p></div>"
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

#[allow(clippy::too_many_lines)]
fn admin_dashboard_html(email: &str, keys: &[ApiKeyRow], csrf_field: &str) -> String {
    let email_escaped = html_escape(email);

    let header = format!(
        "<div class=\"card\">\
           <h2>Dashboard <span class=\"badge badge-active\">Admin</span></h2>\
           <table class=\"info-table\">\
             <tr><td>Email</td><td>{email_escaped}</td></tr>\
             <tr><td>Role</td><td>Administrator</td></tr>\
           </table>\
         </div>"
    );

    let mut keys_html = String::from(
        "<div class=\"card\">\
           <h2>API Keys</h2>",
    );

    if keys.is_empty() {
        keys_html.push_str(
            "<p style=\"margin-top:8px;color:#8b949e;\">No active API keys. Generate one below.</p>",
        );
    } else {
        keys_html.push_str(
            "<table class=\"info-table\" style=\"margin-top:12px;\">\
            <tr><th>Name</th><th>Key</th><th>Created</th><th></th></tr>",
        );
        for key in keys {
            let name_escaped = html_escape(&key.name);
            let key_escaped = html_escape(&key.raw_key);
            let created = key.created_at.format("%Y-%m-%d %H:%M UTC").to_string();
            let _ = write!(
                keys_html,
                "<tr>\
                   <td>{name_escaped}</td>\
                   <td><code style=\"font-size:0.85em;word-break:break-all;\">{key_escaped}</code></td>\
                   <td>{created}</td>\
                   <td>\
                     <form method=\"POST\" action=\"/dashboard/keys/{}/revoke\" style=\"display:inline;\">\
                       {csrf_field}\
                       <button type=\"submit\" class=\"btn\" style=\"background:#d73a49;padding:4px 12px;font-size:0.85em;\" \
                         onclick=\"return confirm('Revoke this API key? This cannot be undone.')\">\
                         Revoke\
                       </button>\
                     </form>\
                   </td>\
                 </tr>",
                key.id,
            );
        }
        keys_html.push_str("</table>");
    }

    let _ = write!(
        keys_html,
        "<form method=\"POST\" action=\"/dashboard/keys/generate\" style=\"margin-top:16px;\">\
           {csrf_field}\
           <button type=\"submit\" class=\"btn\">Generate New API Key</button>\
         </form>\
       </div>",
    );

    // Live worker status card (polls /dashboard/workers via JS)
    let workers_card = "\
        <div class=\"card\">\
          <h2>Workers</h2>\
          <div id=\"admin-workers\">\
            <p style=\"margin-top:8px;color:#8b949e;\">Loading worker status&hellip;</p>\
          </div>\
          <script>\
            (function(){\
              function poll(){\
                fetch('/dashboard/workers',{credentials:'same-origin'})\
                  .then(function(r){return r.json();})\
                  .then(function(d){\
                    var el=document.getElementById('admin-workers');\
                    var ws=d.workers||[];\
                    if(ws.length===0){\
                      el.innerHTML='<p style=\"margin-top:8px;color:#8b949e;\">No workers connected. <a href=\"/setup\">Set up your first worker &rarr;</a></p>';\
                      return;\
                    }\
                    var h='<table class=\"info-table\" style=\"margin-top:8px;\"><tr><th style=\"font-weight:600;color:#8b949e;border-bottom:1px solid #21262d;padding:8px 12px 8px 0;\">Name</th><th style=\"font-weight:600;color:#8b949e;border-bottom:1px solid #21262d;padding:8px 0;\">Models</th><th style=\"font-weight:600;color:#8b949e;border-bottom:1px solid #21262d;padding:8px 0;\">Load</th></tr>';\
                    for(var i=0;i<ws.length;i++){\
                      var w=ws[i];\
                      var name=w.worker_name||w.worker_id||'worker';\
                      var models=(w.models||[]).join(', ');\
                      var load=w.in_flight_count+'/'+w.max_concurrent;\
                      h+='<tr><td style=\"padding:8px 12px 8px 0;\">'+name+'</td><td style=\"padding:8px 0;font-family:monospace;font-size:0.85em;\">'+models+'</td><td style=\"padding:8px 0;\">'+load+'</td></tr>';\
                    }\
                    h+='</table>';\
                    h+='<p style=\"margin-top:12px;\"><a href=\"/setup\" style=\"color:#7c3aed;font-size:0.9em;font-weight:600;\">+ Connect another machine</a></p>';\
                    el.innerHTML=h;\
                  })\
                  .catch(function(){\
                    document.getElementById('admin-workers').innerHTML=\
                      '<p style=\"margin-top:8px;color:#8b949e;\">Could not load worker status.</p>';\
                  });\
              }\
              poll();\
              setInterval(poll,5000);\
            })();\
          </script>\
        </div>";

    let onboarding_card = if keys.is_empty() {
        "<div class=\"card\" style=\"border-color:#7c3aed;\">\
           <h2>&#x1F680; Get Started</h2>\
           <p style=\"margin-top:8px;\">Generate an API key above, then connect your first worker machine.</p>\
           <p style=\"margin-top:16px;\"><a href=\"/setup\" class=\"btn\">Set Up a Worker &rarr;</a></p>\
         </div>"
            .to_string()
    } else {
        String::new() // Workers card above handles the "add machine" link
    };

    format!("{header}\n{keys_html}\n{workers_card}\n{onboarding_card}")
}

#[allow(clippy::too_many_lines)]
fn subscriber_dashboard_html(
    email: &str,
    sub: Option<&SubscriptionRow>,
    has_stripe_customer: bool,
    api_key: Option<&str>,
    csrf_field: &str,
) -> String {
    let email_escaped = html_escape(email);

    // ── Dashboard-specific styles ──
    let dashboard_css = "\
<style>\
  .dash-container { max-width: 1100px; margin: 0 auto; }\
  .dash-header { margin-bottom: 32px; }\
  .dash-header p { color: #8b949e; font-size: 0.95rem; }\
  .dash-header strong { color: #e6edf3; }\
  .dash-section-label { font-size: 0.75rem; font-weight: 700; letter-spacing: 0.05em; text-transform: uppercase; color: #484f58; margin-bottom: 12px; }\
\
  /* API key card — hero prominence */\
  .card-api-key { border-color: #7c3aed; }\
  .card-api-key h2 { display: flex; align-items: center; gap: 8px; }\
  .key-display { position: relative; margin-top: 12px; }\
  .key-display code { display: block; padding-right: 80px; }\
  .copy-btn { position: absolute; top: 8px; right: 8px; padding: 6px 14px; font-size: 0.8rem; background: #30363d; color: #e6edf3; border: 1px solid #3d444d; border-radius: 6px; cursor: pointer; transition: background 0.15s, border-color 0.15s; font-family: inherit; font-weight: 600; }\
  .copy-btn:hover { background: #3d444d; border-color: #484f58; }\
  .copy-btn.copied { background: #064e3b; border-color: #34d399; color: #34d399; }\
  .key-actions { margin-top: 14px; display: flex; gap: 16px; flex-wrap: wrap; }\
  .key-actions a { color: #7c3aed; font-size: 0.9rem; font-weight: 600; }\
\
  /* Status grid — 2 columns on desktop, 1 on mobile */\
  .dash-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 16px; margin-bottom: 24px; }\
  .dash-grid .card { margin-bottom: 0; }\
\
  /* Quick start links — 3 columns on desktop, stack on mobile */\
  .dash-quick-links { display: grid; grid-template-columns: repeat(3, 1fr); gap: 16px; margin-bottom: 24px; }\
  .dash-quick-link { margin-bottom: 0; text-decoration: none; transition: border-color 0.2s; }\
  .dash-quick-link:hover { border-color: #30363d; text-decoration: none; }\
  .dash-quick-link h2 { font-size: 1rem; }\
  .dash-quick-link p { color: #8b949e; font-size: 0.85rem; margin-top: 4px; }\
\
  /* Empty state CTA */\
  .card-empty-cta { border: 1px dashed #30363d; background: #0d1117; text-align: center; padding: 40px 32px; }\
  .card-empty-cta h2 { font-size: 1.1rem; margin-bottom: 8px; }\
  .card-empty-cta p { color: #8b949e; margin-bottom: 20px; }\
  .card-empty-cta .btn { padding: 14px 36px; font-size: 1.1rem; }\
\
  /* Loading skeleton animation */\
  .skel { background: linear-gradient(90deg, #161b22 25%, #1c2129 50%, #161b22 75%); background-size: 200% 100%; animation: skel-pulse 1.5s ease-in-out infinite; border-radius: 6px; }\
  @keyframes skel-pulse { 0% { background-position: 200% 0; } 100% { background-position: -200% 0; } }\
  .skel-line { height: 14px; margin-bottom: 10px; }\
  .skel-line:last-child { width: 60%; margin-bottom: 0; }\
\
  /* Billing link */\
  .billing-link { background: none; border: none; color: #7c3aed; cursor: pointer; font-size: 0.9rem; font-weight: 600; font-family: inherit; padding: 0; }\
  .billing-link:hover { text-decoration: underline; }\
\
  /* Responsive */\
  @media (max-width: 768px) {\
    .dash-grid { grid-template-columns: 1fr; }\
    .dash-quick-links { grid-template-columns: 1fr; }\
    .key-display code { padding-right: 16px; font-size: 0.8rem; }\
    .copy-btn { position: static; display: block; margin-top: 10px; width: 100%; text-align: center; }\
    .card-empty-cta { padding: 32px 20px; }\
    .card-empty-cta .btn { display: block; width: 100%; }\
  }\
</style>";

    // ── Welcome header ──
    let welcome = format!(
        "{dashboard_css}\
         <div class=\"dash-container\">\
         <div class=\"dash-header\">\
           <p>Signed in as <strong>{email_escaped}</strong></p>\
         </div>"
    );

    // ── Section: API Key (hero card) ──
    let api_key_section = {
        let label = "<div class=\"dash-section-label\">Your API Key</div>";
        let card = if let Some(s) = sub {
            if let Some(key) = api_key {
                format!(
                    "<div class=\"card card-api-key\">\
                       <h2>API Key <span class=\"badge badge-active\">Active</span></h2>\
                       <div class=\"key-display\">\
                         <code id=\"api-key-value\">{}</code>\
                         <button class=\"copy-btn\" id=\"copy-btn\" type=\"button\">Copy</button>\
                       </div>\
                       <div class=\"key-actions\">\
                         <a href=\"/integrate\">Integration snippets &rarr;</a>\
                         <a href=\"/setup\">Connect a worker &rarr;</a>\
                       </div>\
                     </div>\
                     <script>\
                       document.getElementById('copy-btn').addEventListener('click',function(){{\
                         var btn=this;\
                         navigator.clipboard.writeText(document.getElementById('api-key-value').textContent).then(function(){{\
                           btn.textContent='Copied!';\
                           btn.classList.add('copied');\
                           setTimeout(function(){{ btn.textContent='Copy'; btn.classList.remove('copied'); }},2000);\
                         }});\
                       }});\
                     </script>",
                    html_escape(key),
                )
            } else if s.api_key_id.is_some() {
                "<div class=\"card card-api-key\">\
                   <h2>API Key <span class=\"badge badge-active\">Provisioned</span></h2>\
                   <p style=\"margin-top:12px;color:#8b949e;\">Your API key has been provisioned. \
                      The raw key was shown once at creation and is stored securely.</p>\
                   <div class=\"key-actions\"><a href=\"/integrate\">Integration snippets &rarr;</a></div>\
                 </div>"
                    .to_string()
            } else if s.status == "active" {
                "<div class=\"card\">\
                   <h2>API Key <span class=\"badge\">Provisioning&hellip;</span></h2>\
                   <div style=\"margin-top:12px;\">\
                     <div class=\"skel skel-line\" style=\"width:80%;\"></div>\
                     <div class=\"skel skel-line\" style=\"width:50%;\"></div>\
                   </div>\
                   <p style=\"margin-top:12px;color:#8b949e;\">Your subscription is active. \
                      Your API key is being provisioned and will appear here shortly.</p>\
                 </div>"
                    .to_string()
            } else {
                "<div class=\"card-empty-cta card\">\
                   <h2>Get Your API Key</h2>\
                   <p>An active subscription is required for API key access.</p>\
                   <a href=\"/pricing\" class=\"btn\">View Pricing &rarr;</a>\
                 </div>"
                    .to_string()
            }
        } else {
            "<div class=\"card-empty-cta card\">\
               <h2>Start Routing Inference Requests</h2>\
               <p>Subscribe to get your relay API key and connect your own GPU workers.</p>\
               <a href=\"/pricing\" class=\"btn\">View Pricing &rarr;</a>\
             </div>"
                .to_string()
        };
        format!("{label}{card}")
    };

    // ── Section: Status grid ──
    let sub_badge = if let Some(s) = sub {
        format!(
            "{}<table class=\"info-table\" style=\"margin-top:8px;\">\
               <tr><td>Status</td><td>{}</td></tr>\
               <tr><td>Updated</td><td>{}</td></tr>\
             </table>",
            status_badge(&s.status),
            html_escape(&s.status),
            s.updated_at.format("%B %d, %Y"),
        )
    } else {
        "<span class=\"badge\">No Active Subscription</span>\
         <p style=\"margin-top:8px;\"><a href=\"/pricing\">View pricing &rarr;</a></p>"
            .to_string()
    };

    let billing_btn = if has_stripe_customer {
        format!(
            "<form method=\"POST\" action=\"/dashboard/billing-portal\" style=\"margin-top:12px;\">\
               {csrf_field}\
               <button type=\"submit\" class=\"billing-link\">Manage billing &rarr;</button>\
             </form>"
        )
    } else {
        String::new()
    };

    let status_section = format!(
        "<div class=\"dash-section-label\">Status</div>\
         <div class=\"dash-grid\">\
           <div class=\"card\">\
             <h2>Relay Status</h2>\
             <div id=\"relay-stats\">\
               <div style=\"margin-top:12px;\">\
                 <div class=\"skel skel-line\" style=\"width:70%;\"></div>\
                 <div class=\"skel skel-line\" style=\"width:45%;\"></div>\
               </div>\
             </div>\
             <script>\
               (function(){{\
                 fetch('/dashboard/stats',{{credentials:'same-origin'}})\
                   .then(function(r){{return r.json();}})\
                   .then(function(d){{\
                     var el=document.getElementById('relay-stats');\
                     var qd=d.queue_depth||{{}};\
                     var total=0;\
                     for(var k in qd){{if(qd.hasOwnProperty(k))total+=qd[k];}}\
                     var aw=d.active_workers||0;\
                     var h='<table class=\"info-table\" style=\"margin-top:8px;\">'\
                       +'<tr><td>Workers</td><td>'+aw+'</td></tr>'\
                       +'<tr><td>Queue Depth</td><td>'+total+'</td></tr>'\
                       +'</table>';\
                     if(aw===0){{\
                       h+='<div class=\"card-empty-cta\" style=\"margin-top:16px;padding:24px 20px;border-radius:8px;\">'\
                         +'<h2 style=\"font-size:1rem;\">Connect Your First Worker</h2>'\
                         +'<p style=\"margin-bottom:14px;\">No GPU workers are connected yet. Set one up in minutes.</p>'\
                         +'<a href=\"/setup\" class=\"btn\" style=\"padding:10px 24px;font-size:0.95rem;\">Set Up a Worker &rarr;</a>'\
                         +'</div>';\
                     }}\
                     if(Object.keys(qd).length>1){{\
                       var extra='<p style=\"margin-top:8px;color:#8b949e;font-size:0.85em;\">Per-model: ';\
                       for(var m in qd){{if(qd.hasOwnProperty(m))extra+=m+':&nbsp;'+qd[m]+'&ensp;';}}\
                       extra+='</p>';\
                       h+=extra;\
                     }}\
                     el.innerHTML=h;\
                   }})\
                   .catch(function(){{\
                     document.getElementById('relay-stats').innerHTML=\
                       '<p style=\"margin-top:8px;color:#8b949e;\">Could not load relay status.</p>';\
                   }});\
               }})();\
             </script>\
           </div>\
           <div class=\"card\">\
             <h2>Subscription</h2>\
             <div style=\"margin-top:8px;\">{sub_badge}</div>\
             {billing_btn}\
           </div>\
         </div>"
    );

    // ── Quick start links (only when they have a key) ──
    let quick_start = if api_key.is_some() {
        "<div class=\"dash-section-label\">Quick Start</div>\
         <div class=\"dash-quick-links\">\
           <a href=\"/setup\" class=\"card dash-quick-link\">\
             <h2>&#x2699;&#xFE0F; Setup</h2>\
             <p>Connect a GPU worker machine</p>\
           </a>\
           <a href=\"/integrate\" class=\"card dash-quick-link\">\
             <h2>&#x1F4CB; Integrate</h2>\
             <p>Code snippets for your favorite tools</p>\
           </a>\
           <a href=\"https://ericflo.github.io/modelrelay/\" target=\"_blank\" class=\"card dash-quick-link\">\
             <h2>&#x1F4D6; Docs</h2>\
             <p>API reference and examples</p>\
           </a>\
         </div>"
            .to_string()
    } else {
        String::new()
    };

    let close = "</div>"; // close .dash-container

    format!("{welcome}\n{api_key_section}\n{status_section}\n{quick_start}\n{close}")
}

/// Minimal HTML entity escaping for untrusted strings.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
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
}
