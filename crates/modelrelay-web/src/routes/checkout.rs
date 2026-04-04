use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::Deserialize;
use tower_sessions::Session;

use crate::state::AppState;

static CANCEL_HTML: &str = include_str!("../../templates/checkout_cancel.html");

/// POST /checkout — create a Stripe Checkout Session and redirect to Stripe.
///
/// If the user is logged in, pre-fills their email in the checkout session.
pub async fn create(session: Session, State(state): State<Arc<AppState>>) -> Response {
    let Some(ref key) = state.stripe_key else {
        return Html(
            "<h1>Billing not configured yet</h1>\
             <p>Stripe is not set up on this instance. Please check back soon.</p>\
             <p><a href=\"/\">&larr; Back to home</a></p>",
        )
        .into_response();
    };

    let price_id = std::env::var("STRIPE_PRICE_ID").unwrap_or_default();
    if price_id.is_empty() {
        return Html(
            "<h1>Billing not configured yet</h1>\
             <p>No pricing plan is configured. Please check back soon.</p>\
             <p><a href=\"/\">&larr; Back to home</a></p>",
        )
        .into_response();
    }

    // Look up authenticated user's email to pre-fill checkout
    let user_email = if let Ok(Some(uid)) = session.get::<String>("user_id").await {
        if let (Ok(uid), Some(ref pool)) = (uid.parse::<uuid::Uuid>(), &state.db) {
            sqlx::query_scalar::<_, String>("SELECT email FROM users WHERE id = $1")
                .bind(uid)
                .fetch_optional(pool)
                .await
                .ok()
                .flatten()
        } else {
            None
        }
    } else {
        None
    };

    let client = reqwest::Client::new();
    let mut params = vec![
        ("mode".to_string(), "subscription".to_string()),
        (
            "success_url".to_string(),
            "https://modelrelay.io/checkout/success?session_id={CHECKOUT_SESSION_ID}".to_string(),
        ),
        ("cancel_url".to_string(), "https://modelrelay.io/checkout/cancel".to_string()),
        ("line_items[0][price]".to_string(), price_id),
        ("line_items[0][quantity]".to_string(), "1".to_string()),
    ];

    if let Some(ref email) = user_email {
        params.push(("customer_email".to_string(), email.clone()));
    }

    let resp = client
        .post("https://api.stripe.com/v1/checkout/sessions")
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
                    Html("<h1>Error</h1><p>Stripe did not return a checkout URL.</p>")
                        .into_response()
                }
            }
            Err(e) => {
                tracing::error!("stripe response parse error: {e}");
                Html("<h1>Error</h1><p>Could not process Stripe response.</p>").into_response()
            }
        },
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            tracing::error!("stripe API error: {status} — {body}");
            Html(
                "<h1>Checkout Error</h1>\
                 <p>Could not create checkout session. Please try again later.</p>\
                 <p><a href=\"/pricing\">&larr; Back to pricing</a></p>",
            )
            .into_response()
        }
        Err(e) => {
            tracing::error!("stripe request error: {e}");
            Html(
                "<h1>Checkout Error</h1>\
                 <p>Could not reach payment provider. Please try again later.</p>\
                 <p><a href=\"/pricing\">&larr; Back to pricing</a></p>",
            )
            .into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct SuccessQuery {
    session_id: Option<String>,
}

/// GET /checkout/success — look up the Stripe session, find the user, store in session.
pub async fn success(
    session: Session,
    Query(query): Query<SuccessQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    // Try to identify the user from the Stripe session and set a cookie session
    if let (Some(session_id), Some(key), Some(pool)) =
        (&query.session_id, &state.stripe_key, &state.db)
        && let Ok(user_id) = resolve_user_from_stripe(session_id, key, pool).await
        && let Err(e) = session.insert("user_id", user_id.to_string()).await
    {
        tracing::error!("failed to store user_id in session: {e}");
    }

    Html(success_html()).into_response()
}

/// Retrieve the Stripe checkout session, extract the customer email, and find the user.
async fn resolve_user_from_stripe(
    session_id: &str,
    stripe_key: &str,
    pool: &sqlx::PgPool,
) -> Result<uuid::Uuid, String> {
    let client = reqwest::Client::new();
    let url = format!("https://api.stripe.com/v1/checkout/sessions/{session_id}");
    let resp = client
        .get(&url)
        .bearer_auth(stripe_key)
        .send()
        .await
        .map_err(|e| format!("stripe request error: {e}"))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("stripe API error: {body}"));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("stripe parse error: {e}"))?;

    let email = body["customer_email"]
        .as_str()
        .or_else(|| body["customer_details"]["email"].as_str())
        .ok_or("no email in checkout session")?;

    let user_id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
        .bind(email)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("user query error: {e}"))?
        .ok_or_else(|| format!("no user found for email {email}"))?;

    tracing::info!("checkout success: resolved user {user_id} from session {session_id}");
    Ok(user_id)
}

/// GET /checkout/cancel
pub async fn cancel() -> Html<&'static str> {
    Html(CANCEL_HTML)
}

fn success_html() -> String {
    r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Checkout Success — ModelRelay</title>
  <style>
    *, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }
    body {
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
      background: #0d1117; color: #e6edf3; line-height: 1.6;
    }
    a { color: #7c3aed; text-decoration: none; }
    a:hover { text-decoration: underline; }
    .container { max-width: 900px; margin: 0 auto; padding: 0 24px; }
    nav { padding: 20px 0; border-bottom: 1px solid #21262d; }
    nav .container { display: flex; justify-content: space-between; align-items: center; }
    .logo { font-size: 1.25rem; font-weight: 700; color: #e6edf3; }
    .logo span { color: #7c3aed; }
    .nav-links a { color: #8b949e; font-size: 0.9rem; margin-left: 16px; }
    .nav-links a:hover { color: #e6edf3; }
    .content { padding: 60px 0; text-align: center; }
    .content h1 { font-size: 2rem; margin-bottom: 16px; }
    .content p { color: #8b949e; margin-bottom: 24px; }
    .btn {
      display: inline-block; padding: 12px 24px; background: #7c3aed; color: #fff;
      border-radius: 8px; font-weight: 600; text-decoration: none;
    }
    .btn:hover { background: #6d28d9; text-decoration: none; }
    footer { padding: 40px 0; border-top: 1px solid #21262d; text-align: center; color: #484f58; font-size: 0.85rem; }
    footer a { color: #8b949e; }
  </style>
</head>
<body>
  <nav>
    <div class="container">
      <a href="/" class="logo">Model<span>Relay</span></a>
      <div class="nav-links">
        <a href="/dashboard">Dashboard</a>
        <a href="/pricing">Pricing</a>
      </div>
    </div>
  </nav>
  <section class="content">
    <div class="container">
      <h1>&#10003; Subscription Activated!</h1>
      <p>Thank you for subscribing to ModelRelay. Your account is being set up.</p>
      <a href="/dashboard" class="btn">Go to Dashboard &rarr;</a>
    </div>
  </section>
  <footer>
    <div class="container">
      &copy; 2026 ModelRelay &middot; <a href="https://github.com/ericflo/modelrelay">GitHub</a>
    </div>
  </footer>
</body>
</html>"#
        .to_string()
}
