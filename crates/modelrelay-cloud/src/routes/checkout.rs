use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::Deserialize;
use tower_sessions::Session;

use crate::state::CloudState;

static CANCEL_HTML: &str = include_str!("../../templates/checkout_cancel.html");
static SUCCESS_HTML: &str = include_str!("../../templates/checkout_success.html");

fn success_page() -> String {
    modelrelay_web::templates::page_shell("Subscription Active", SUCCESS_HTML, true)
}

fn cancel_page() -> String {
    modelrelay_web::templates::page_shell("Checkout Cancelled", CANCEL_HTML, false)
}

/// POST /checkout — create a Stripe Checkout Session and redirect to Stripe.
///
/// If the user is logged in, pre-fills their email in the checkout session.
pub async fn create(session: Session, State(state): State<Arc<CloudState>>) -> Response {
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
        if let (Ok(uid), Some(pool)) = (uid.parse::<uuid::Uuid>(), &state.db) {
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
        (
            "cancel_url".to_string(),
            "https://modelrelay.io/checkout/cancel".to_string(),
        ),
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
    State(state): State<Arc<CloudState>>,
) -> Response {
    // Try to identify the user from the Stripe session and set a cookie session
    if let (Some(session_id), Some(key), Some(pool)) =
        (&query.session_id, &state.stripe_key, &state.db)
        && let Ok(user_id) = resolve_user_from_stripe(session_id, key, pool).await
        && let Err(e) = session.insert("user_id", user_id.to_string()).await
    {
        tracing::error!("failed to store user_id in session: {e}");
    }

    Html(success_page()).into_response()
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
pub async fn cancel() -> Html<String> {
    Html(cancel_page())
}
