use std::sync::Arc;

use axum::extract::State;
use axum::response::{Html, IntoResponse, Redirect, Response};
use tower_sessions::Session;

use crate::state::AppState;

/// GET /dashboard — show subscription status and API key info.
///
/// Reads `user_id` from the session (set during checkout success).
/// If no session or user: prompts to subscribe.
/// If user found: shows subscription status, API key, and billing portal link.
pub async fn page(session: Session, State(state): State<Arc<AppState>>) -> Response {
    let Some(ref pool) = state.db else {
        return Html(page_shell("Dashboard", &no_db_html())).into_response();
    };

    let user_id: Option<String> = session.get("user_id").await.unwrap_or(None);

    let Some(user_id) = user_id else {
        return Redirect::to("/login").into_response();
    };

    let user_id: uuid::Uuid = match user_id.parse() {
        Ok(id) => id,
        Err(_) => return Redirect::to("/login").into_response(),
    };

    // Query user info (including api_key for display)
    let user = sqlx::query_as::<_, UserRow>(
        "SELECT id, email, stripe_customer_id, api_key FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await;

    let user = match user {
        Ok(Some(u)) => u,
        Ok(None) => return Redirect::to("/login").into_response(),
        Err(e) => {
            tracing::error!("dashboard user query error: {e}");
            return Html(page_shell("Dashboard", "<div class=\"card\"><h2>Error</h2><p>Could not load your account. Please try again later.</p></div>")).into_response();
        }
    };

    // Query subscription(s) for this user
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

    let has_stripe_customer = user.stripe_customer_id.is_some();
    let html = dashboard_html(&user.email, sub.as_ref(), has_stripe_customer, user.api_key.as_deref());
    Html(page_shell("Dashboard", &html)).into_response()
}

/// POST /dashboard/billing-portal — create a Stripe billing portal session and redirect.
pub async fn billing_portal(session: Session, State(state): State<Arc<AppState>>) -> Response {
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

    let user_id: Option<String> = session.get("user_id").await.unwrap_or(None);
    let Some(user_id) = user_id else {
        return Redirect::to("/dashboard").into_response();
    };

    let user_id: uuid::Uuid = match user_id.parse() {
        Ok(id) => id,
        Err(_) => return Redirect::to("/dashboard").into_response(),
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
        return Html(page_shell(
            "Error",
            "<div class=\"card\"><h2>No billing account</h2>\
             <p>No Stripe customer found for your account.</p>\
             <p><a href=\"/dashboard\">&larr; Back to dashboard</a></p></div>",
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

#[derive(sqlx::FromRow)]
struct UserRow {
    #[allow(dead_code)]
    id: uuid::Uuid,
    email: String,
    stripe_customer_id: Option<String>,
    api_key: Option<String>,
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

fn status_badge(status: &str) -> &'static str {
    match status {
        "active" => "<span class=\"badge badge-active\">Active</span>",
        "past_due" => "<span class=\"badge badge-warn\">Past Due</span>",
        "canceled" => "<span class=\"badge badge-cancel\">Canceled</span>",
        "incomplete" => "<span class=\"badge\">Incomplete</span>",
        _ => "<span class=\"badge\">Unknown</span>",
    }
}

fn no_db_html() -> String {
    "<div class=\"card\"><h2>Dashboard Unavailable</h2>\
     <p>The database is not connected. Please try again later.</p></div>"
        .to_string()
}

fn dashboard_html(email: &str, sub: Option<&SubscriptionRow>, has_stripe_customer: bool, api_key: Option<&str>) -> String {
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

fn page_shell(title: &str, body_content: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{title} — ModelRelay</title>
  <style>
    *, *::before, *::after {{ box-sizing: border-box; margin: 0; padding: 0; }}
    body {{
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
      background: #0d1117; color: #e6edf3; line-height: 1.6;
    }}
    a {{ color: #7c3aed; text-decoration: none; }}
    a:hover {{ text-decoration: underline; }}
    .container {{ max-width: 900px; margin: 0 auto; padding: 0 24px; }}

    nav {{ padding: 20px 0; border-bottom: 1px solid #21262d; }}
    nav .container {{ display: flex; justify-content: space-between; align-items: center; }}
    .logo {{ font-size: 1.25rem; font-weight: 700; color: #e6edf3; }}
    .logo span {{ color: #7c3aed; }}
    .nav-links a {{ color: #8b949e; font-size: 0.9rem; margin-left: 16px; }}
    .nav-links a:hover {{ color: #e6edf3; }}
    .nav-links form {{ display: inline; }}
    .nav-links button {{ background: none; border: none; color: #8b949e; font-size: 0.9rem; cursor: pointer; margin-left: 16px; font-family: inherit; }}
    .nav-links button:hover {{ color: #e6edf3; }}

    .content {{ padding: 60px 0; }}
    .content h1 {{ font-size: 2rem; margin-bottom: 24px; }}

    .card {{
      background: #161b22; border: 1px solid #21262d; border-radius: 12px;
      padding: 32px; margin-bottom: 24px;
    }}
    .card h2 {{ font-size: 1.2rem; margin-bottom: 12px; color: #e6edf3; }}
    .card p {{ color: #8b949e; }}

    .badge {{
      display: inline-block; padding: 4px 12px; border-radius: 20px;
      font-size: 0.8rem; font-weight: 600; background: #1f2937; color: #8b949e;
    }}
    .badge-active {{ background: #064e3b; color: #34d399; }}
    .badge-warn {{ background: #78350f; color: #fbbf24; }}
    .badge-cancel {{ background: #7f1d1d; color: #f87171; }}

    .info-table {{ margin-top: 16px; width: 100%; border-collapse: collapse; }}
    .info-table td {{ padding: 8px 0; border-bottom: 1px solid #21262d; color: #8b949e; }}
    .info-table td:first-child {{ font-weight: 600; color: #e6edf3; width: 140px; }}

    .key-display {{
      margin-top: 12px; padding: 12px 16px; background: #0d1117;
      border: 1px solid #21262d; border-radius: 8px; font-family: monospace;
      color: #7c3aed; word-break: break-all;
    }}

    .btn {{
      display: inline-block; padding: 10px 20px; background: #7c3aed; color: #fff;
      border: none; border-radius: 8px; font-size: 0.9rem; font-weight: 600;
      cursor: pointer; text-decoration: none;
    }}
    .btn:hover {{ background: #6d28d9; text-decoration: none; }}

    footer {{ padding: 40px 0; border-top: 1px solid #21262d; text-align: center; color: #484f58; font-size: 0.85rem; }}
    footer a {{ color: #8b949e; }}
  </style>
</head>
<body>
  <nav>
    <div class="container">
      <a href="/" class="logo">Model<span>Relay</span></a>
      <div class="nav-links">
        <a href="/dashboard">Dashboard</a>
        <a href="/pricing">Pricing</a>
        <form method="POST" action="/logout"><button type="submit">Log out</button></form>
      </div>
    </div>
  </nav>

  <section class="content">
    <div class="container">
      <h1>{title}</h1>
      {body_content}
    </div>
  </section>

  <footer>
    <div class="container">
      &copy; 2026 ModelRelay &middot; <a href="https://github.com/ericflo/modelrelay">GitHub</a>
    </div>
  </footer>
</body>
</html>"#
    )
}
