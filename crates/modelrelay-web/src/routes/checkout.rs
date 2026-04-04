use axum::extract::State;
use axum::response::{Html, IntoResponse, Redirect, Response};
use std::sync::Arc;

use crate::state::AppState;

static SUCCESS_HTML: &str = include_str!("../../templates/checkout_success.html");
static CANCEL_HTML: &str = include_str!("../../templates/checkout_cancel.html");

/// POST /checkout — create a Stripe Checkout Session and redirect to Stripe.
pub async fn create(State(state): State<Arc<AppState>>) -> Response {
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

    let client = reqwest::Client::new();
    let params = [
        ("mode", "subscription"),
        (
            "success_url",
            "https://modelrelay.io/checkout/success?session_id={CHECKOUT_SESSION_ID}",
        ),
        ("cancel_url", "https://modelrelay.io/checkout/cancel"),
        ("line_items[0][price]", &*price_id),
        ("line_items[0][quantity]", "1"),
    ];

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

/// GET /checkout/success
pub async fn success() -> Html<&'static str> {
    Html(SUCCESS_HTML)
}

/// GET /checkout/cancel
pub async fn cancel() -> Html<&'static str> {
    Html(CANCEL_HTML)
}
