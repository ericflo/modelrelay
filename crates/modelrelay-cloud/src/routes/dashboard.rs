use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use tower_sessions::Session;

use crate::state::CloudState;

// ─── Helpers shared across handlers ─────────────────────────────────────────

/// Load user_id from session, returning a redirect to /login if absent.
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

        let html = admin_dashboard_html(&user.email, &keys);
        Html(modelrelay_web::templates::page_shell("Dashboard", &html, true)).into_response()
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
        );
        Html(modelrelay_web::templates::page_shell("Dashboard", &html, true)).into_response()
    }
}

// ─── POST /dashboard/billing-portal ─────────────────────────────────────────

/// POST /dashboard/billing-portal — create a Stripe billing portal session and redirect.
pub async fn billing_portal(session: Session, State(state): State<Arc<CloudState>>) -> Response {
    let Some(ref key) = state.stripe_key else {
        return Html(
            "<h1>Billing not configured</h1><p><a href=\"/dashboard\">&larr; Back</a></p>",
        )
        .into_response();
    };

    let Some(ref pool) = state.db else {
        return Html(
            "<h1>Database not available</h1><p><a href=\"/dashboard\">&larr; Back</a></p>",
        )
        .into_response();
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
                    Html("<h1>Error</h1><p>Stripe did not return a portal URL.</p>").into_response()
                }
            }
            Err(e) => {
                tracing::error!("stripe portal response parse error: {e}");
                Html("<h1>Error</h1><p>Could not process billing portal response.</p>")
                    .into_response()
            }
        },
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            tracing::error!("stripe billing portal API error: {status} — {body}");
            Html(
                "<h1>Error</h1><p>Could not open billing portal. Please try again later.</p>\
                 <p><a href=\"/dashboard\">&larr; Back to dashboard</a></p>",
            )
            .into_response()
        }
        Err(e) => {
            tracing::error!("stripe billing portal request error: {e}");
            Html(
                "<h1>Error</h1><p>Could not reach payment provider. Please try again later.</p>\
                 <p><a href=\"/dashboard\">&larr; Back to dashboard</a></p>",
            )
            .into_response()
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

fn admin_dashboard_html(email: &str, keys: &[ApiKeyRow]) -> String {
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
            keys_html.push_str(&format!(
                "<tr>\
                   <td>{name_escaped}</td>\
                   <td><code style=\"font-size:0.85em;word-break:break-all;\">{key_escaped}</code></td>\
                   <td>{created}</td>\
                   <td>\
                     <form method=\"POST\" action=\"/dashboard/keys/{}/revoke\" style=\"display:inline;\">\
                       <button type=\"submit\" class=\"btn\" style=\"background:#d73a49;padding:4px 12px;font-size:0.85em;\" \
                         onclick=\"return confirm('Revoke this API key? This cannot be undone.')\">\
                         Revoke\
                       </button>\
                     </form>\
                   </td>\
                 </tr>",
                key.id,
            ));
        }
        keys_html.push_str("</table>");
    }

    keys_html.push_str(
        "<form method=\"POST\" action=\"/dashboard/keys/generate\" style=\"margin-top:16px;\">\
           <button type=\"submit\" class=\"btn\">Generate New API Key</button>\
         </form>\
       </div>",
    );

    let usage_card = "<div class=\"card\">\
       <h2>Usage</h2>\
       <p style=\"margin-top:8px;\"><span class=\"badge\">Coming Soon</span></p>\
       <p style=\"margin-top:12px;color:#8b949e;\">Request counts, connected workers, and usage statistics will be available here once the admin API is connected.</p>\
       <p style=\"margin-top:12px;\"><a href=\"/admin/dashboard\">Open Admin Dashboard &rarr;</a></p>\
     </div>";

    format!("{header}\n{keys_html}\n{usage_card}")
}

fn subscriber_dashboard_html(
    email: &str,
    sub: Option<&SubscriptionRow>,
    has_stripe_customer: bool,
    api_key: Option<&str>,
) -> String {
    let email_escaped = html_escape(email);

    let sub_card = if let Some(s) = sub {
        let badge = status_badge(&s.status);
        let updated = s.updated_at.format("%B %d, %Y").to_string();
        let billing_btn = if has_stripe_customer {
            "<form method=\"POST\" action=\"/dashboard/billing-portal\" style=\"margin-top:16px;\">\
               <button type=\"submit\" class=\"btn\">Manage Billing &rarr;</button>\
             </form>"
        } else {
            ""
        };
        format!(
            "<div class=\"card\">\
               <h2>Subscription</h2>\
               <p style=\"margin-top:8px;\">{badge}</p>\
               <table class=\"info-table\">\
                 <tr><td>Email</td><td>{email_escaped}</td></tr>\
                 <tr><td>Status</td><td>{}</td></tr>\
                 <tr><td>Last Updated</td><td>{updated}</td></tr>\
               </table>\
               {billing_btn}\
             </div>",
            html_escape(&s.status),
        )
    } else {
        let billing_btn = if has_stripe_customer {
            "<form method=\"POST\" action=\"/dashboard/billing-portal\" style=\"margin-top:16px;\">\
               <button type=\"submit\" class=\"btn\">Manage Billing &rarr;</button>\
             </form>"
        } else {
            ""
        };
        format!(
            "<div class=\"card\">\
               <h2>Subscription</h2>\
               <p style=\"margin-top:8px;\"><span class=\"badge\">No Active Subscription</span></p>\
               <p style=\"margin-top:12px;\">You don't have an active subscription. \
                  <a href=\"/pricing\">View pricing</a> to get started.</p>\
               {billing_btn}\
             </div>"
        )
    };

    let api_key_card = if let Some(s) = sub {
        if let Some(key) = api_key {
            format!(
                "<div class=\"card\">\
                   <h2>API Key</h2>\
                   <p style=\"margin-top:8px;\"><span class=\"badge badge-active\">Provisioned</span></p>\
                   <div class=\"key-display\">\
                     <code>{}</code>\
                   </div>\
                   <p style=\"margin-top:8px;color:#8b949e;\">Use this key in your <code>modelrelay-server</code> configuration.</p>\
                 </div>",
                html_escape(key),
            )
        } else if s.api_key_id.is_some() {
            "<div class=\"card\">\
               <h2>API Key</h2>\
               <p style=\"margin-top:8px;\"><span class=\"badge badge-active\">Provisioned</span></p>\
               <p style=\"margin-top:12px;color:#8b949e;\">Your API key has been provisioned. \
                  The raw key was shown once at creation and is stored securely.</p>\
             </div>"
                .to_string()
        } else if s.status == "active" {
            "<div class=\"card\">\
               <h2>API Key</h2>\
               <p style=\"margin-top:8px;\"><span class=\"badge\">Pending Provisioning</span></p>\
               <p style=\"margin-top:12px;color:#8b949e;\">Your subscription is active. \
                  Your API key is being provisioned and will appear here shortly.</p>\
             </div>"
                .to_string()
        } else {
            "<div class=\"card\">\
               <h2>API Key</h2>\
               <p style=\"margin-top:8px;\"><span class=\"badge\">Unavailable</span></p>\
               <p style=\"margin-top:12px;color:#8b949e;\">An active subscription is required for API key access.</p>\
             </div>"
                .to_string()
        }
    } else {
        "<div class=\"card\">\
           <h2>API Key</h2>\
           <p style=\"margin-top:8px;\"><span class=\"badge\">Unavailable</span></p>\
           <p style=\"margin-top:12px;color:#8b949e;\">Subscribe to receive your relay API key.</p>\
         </div>"
            .to_string()
    };

    let usage_card = "<div class=\"card\">\
       <h2>Usage</h2>\
       <p style=\"margin-top:8px;\"><span class=\"badge\">Coming Soon</span></p>\
       <p style=\"margin-top:12px;color:#8b949e;\">Request counts, connected workers, and usage statistics will be available here once the admin API is connected.</p>\
       <p style=\"margin-top:12px;\"><a href=\"/admin/dashboard\">Open Admin Dashboard &rarr;</a></p>\
     </div>";

    format!("{sub_card}\n{api_key_card}\n{usage_card}")
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
