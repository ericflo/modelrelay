use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use sqlx::PgPool;

use crate::state::CloudState;

type HmacSha256 = Hmac<Sha256>;

/// POST /webhook/stripe — handle Stripe webhook events.
///
/// Verifies the `Stripe-Signature` header using HMAC-SHA256, then dispatches
/// on the event type.
pub async fn handle(
    State(state): State<Arc<CloudState>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let Some(ref secret) = state.webhook_secret else {
        tracing::error!("webhook received but STRIPE_WEBHOOK_SECRET is not set");
        return StatusCode::INTERNAL_SERVER_ERROR;
    };

    let Some(sig_header) = headers
        .get("Stripe-Signature")
        .and_then(|v| v.to_str().ok())
    else {
        tracing::warn!("webhook missing Stripe-Signature header");
        return StatusCode::BAD_REQUEST;
    };

    if let Err(e) = verify_signature(sig_header, &body, secret) {
        tracing::warn!("webhook signature verification failed: {e}");
        return StatusCode::BAD_REQUEST;
    }

    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("webhook JSON parse error: {e}");
            return StatusCode::BAD_REQUEST;
        }
    };

    let event_type = payload["type"].as_str().unwrap_or("");
    tracing::info!("stripe webhook event: {event_type}");

    let Some(ref pool) = state.db else {
        tracing::error!("webhook received but database is not connected");
        return StatusCode::INTERNAL_SERVER_ERROR;
    };

    let result = match event_type {
        "checkout.session.completed" => handle_checkout_completed(&state, pool, &payload).await,
        "customer.subscription.updated" => handle_subscription_updated(pool, &payload).await,
        "customer.subscription.deleted" => {
            handle_subscription_deleted(&state, pool, &payload).await
        }
        "invoice.payment_failed" => handle_payment_failed(pool, &payload).await,
        _ => {
            tracing::debug!("ignoring unhandled webhook event: {event_type}");
            Ok(())
        }
    };

    match result {
        Ok(()) => StatusCode::OK,
        Err(e) => {
            tracing::error!("webhook handler error for {event_type}: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

/// Verify the Stripe webhook signature (v1 scheme).
fn verify_signature(sig_header: &str, payload: &[u8], secret: &str) -> Result<(), String> {
    let mut timestamp = None;
    let mut signatures = Vec::new();

    for part in sig_header.split(',') {
        let part = part.trim();
        if let Some(t) = part.strip_prefix("t=") {
            timestamp = Some(t);
        } else if let Some(sig) = part.strip_prefix("v1=") {
            signatures.push(sig);
        }
    }

    let timestamp = timestamp.ok_or("missing timestamp in signature header")?;
    if signatures.is_empty() {
        return Err("no v1 signature found in header".into());
    }

    let signed_payload = format!("{timestamp}.{}", String::from_utf8_lossy(payload));
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).map_err(|e| format!("HMAC error: {e}"))?;
    mac.update(signed_payload.as_bytes());
    let expected = hex::encode(mac.finalize().into_bytes());

    if signatures.iter().any(|s| constant_time_eq(s, &expected)) {
        Ok(())
    } else {
        Err("signature mismatch".into())
    }
}

/// Constant-time string comparison to prevent timing attacks.
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Call `POST {admin_url}/admin/keys` to provision a new API key.
async fn provision_api_key(
    admin_url: &str,
    admin_token: &str,
    name: &str,
) -> Result<(String, String), String> {
    let client = reqwest::Client::new();
    let url = format!("{}/admin/keys", admin_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .bearer_auth(admin_token)
        .json(&serde_json::json!({ "name": name }))
        .send()
        .await
        .map_err(|e| format!("admin API request error: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("admin API returned {status}: {body}"));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("admin API response parse error: {e}"))?;

    let key_id = body["id"]
        .as_str()
        .ok_or("admin API response missing 'id'")?
        .to_string();
    let raw_key = body["key"]
        .as_str()
        .ok_or("admin API response missing 'key'")?
        .to_string();

    Ok((key_id, raw_key))
}

/// Call `DELETE {admin_url}/admin/keys/{key_id}` to revoke an API key.
async fn revoke_api_key(admin_url: &str, admin_token: &str, key_id: &str) -> Result<(), String> {
    let client = reqwest::Client::new();
    let url = format!("{}/admin/keys/{}", admin_url.trim_end_matches('/'), key_id);
    let resp = client
        .delete(&url)
        .bearer_auth(admin_token)
        .send()
        .await
        .map_err(|e| format!("admin API revoke request error: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("admin API revoke returned {status}: {body}"));
    }

    Ok(())
}

/// `checkout.session.completed` — create or find user by email, upsert subscription,
/// and provision an API key via the admin API.
async fn handle_checkout_completed(
    state: &CloudState,
    pool: &PgPool,
    payload: &serde_json::Value,
) -> Result<(), String> {
    let obj = &payload["data"]["object"];
    let email = obj["customer_email"]
        .as_str()
        .or_else(|| obj["customer_details"]["email"].as_str())
        .ok_or("checkout.session.completed: no customer email found")?;
    let stripe_customer_id = obj["customer"].as_str();
    let stripe_subscription_id = obj["subscription"]
        .as_str()
        .ok_or("checkout.session.completed: no subscription ID found")?;

    // Upsert user by email
    let user_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, stripe_customer_id) \
         VALUES ($1, $2) \
         ON CONFLICT (email) DO UPDATE SET stripe_customer_id = COALESCE(EXCLUDED.stripe_customer_id, users.stripe_customer_id) \
         RETURNING id",
    )
    .bind(email)
    .bind(stripe_customer_id)
    .fetch_one(pool)
    .await
    .map_err(|e| format!("user upsert error: {e}"))?;

    // Upsert subscription
    sqlx::query(
        "INSERT INTO subscriptions (user_id, stripe_subscription_id, status, updated_at) \
         VALUES ($1, $2, 'active', now()) \
         ON CONFLICT (stripe_subscription_id) DO UPDATE SET status = 'active', updated_at = now()",
    )
    .bind(user_id)
    .bind(stripe_subscription_id)
    .execute(pool)
    .await
    .map_err(|e| format!("subscription upsert error: {e}"))?;

    // Provision API key via admin API
    if let (Some(admin_url), Some(admin_token)) = (&state.admin_url, &state.admin_token) {
        let key_name = format!("user-{email}");
        match provision_api_key(admin_url, admin_token, &key_name).await {
            Ok((key_id, raw_key)) => {
                sqlx::query(
                    "UPDATE subscriptions SET api_key_id = $1, updated_at = now() \
                     WHERE stripe_subscription_id = $2",
                )
                .bind(&key_id)
                .bind(stripe_subscription_id)
                .execute(pool)
                .await
                .map_err(|e| format!("failed to store api_key_id: {e}"))?;

                sqlx::query("UPDATE users SET api_key = $1 WHERE id = $2")
                    .bind(&raw_key)
                    .bind(user_id)
                    .execute(pool)
                    .await
                    .map_err(|e| format!("failed to store api_key: {e}"))?;

                tracing::info!(
                    "provisioned API key {key_id} for user={email} subscription={stripe_subscription_id}"
                );
            }
            Err(e) => {
                tracing::error!(
                    "failed to provision API key for user={email}: {e} — subscription is active but key is missing"
                );
            }
        }
    } else {
        tracing::warn!("admin API not configured — skipping API key provisioning for user={email}");
    }

    tracing::info!(
        "checkout completed: user={email} subscription={stripe_subscription_id} status=active"
    );
    Ok(())
}

/// `customer.subscription.updated` — update subscription status.
async fn handle_subscription_updated(
    pool: &PgPool,
    payload: &serde_json::Value,
) -> Result<(), String> {
    let obj = &payload["data"]["object"];
    let sub_id = obj["id"]
        .as_str()
        .ok_or("subscription.updated: no subscription ID")?;
    let status = obj["status"].as_str().unwrap_or("unknown");

    let rows = sqlx::query(
        "UPDATE subscriptions SET status = $1, updated_at = now() WHERE stripe_subscription_id = $2",
    )
    .bind(status)
    .bind(sub_id)
    .execute(pool)
    .await
    .map_err(|e| format!("subscription update error: {e}"))?
    .rows_affected();

    if rows == 0 {
        tracing::warn!("subscription.updated: no matching subscription for {sub_id}");
    } else {
        tracing::info!("subscription updated: {sub_id} -> {status}");
    }
    Ok(())
}

/// `customer.subscription.deleted` — mark subscription as canceled and revoke API key.
async fn handle_subscription_deleted(
    state: &CloudState,
    pool: &PgPool,
    payload: &serde_json::Value,
) -> Result<(), String> {
    let obj = &payload["data"]["object"];
    let sub_id = obj["id"]
        .as_str()
        .ok_or("subscription.deleted: no subscription ID")?;

    let api_key_id: Option<String> = sqlx::query_scalar(
        "SELECT api_key_id FROM subscriptions WHERE stripe_subscription_id = $1",
    )
    .bind(sub_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| format!("subscription lookup error: {e}"))?
    .flatten();

    if let Some(key_id) = &api_key_id {
        if let (Some(admin_url), Some(admin_token)) = (&state.admin_url, &state.admin_token) {
            match revoke_api_key(admin_url, admin_token, key_id).await {
                Ok(()) => {
                    tracing::info!("revoked API key {key_id} for subscription {sub_id}");
                }
                Err(e) => {
                    tracing::error!(
                        "failed to revoke API key {key_id} for subscription {sub_id}: {e}"
                    );
                }
            }
        } else {
            tracing::warn!("admin API not configured — cannot revoke API key {key_id}");
        }
    }

    let rows = sqlx::query(
        "UPDATE subscriptions SET status = 'canceled', api_key_id = NULL, updated_at = now() \
         WHERE stripe_subscription_id = $1",
    )
    .bind(sub_id)
    .execute(pool)
    .await
    .map_err(|e| format!("subscription delete error: {e}"))?
    .rows_affected();

    sqlx::query(
        "UPDATE users SET api_key = NULL WHERE id = (\
         SELECT user_id FROM subscriptions WHERE stripe_subscription_id = $1)",
    )
    .bind(sub_id)
    .execute(pool)
    .await
    .map_err(|e| format!("user api_key clear error: {e}"))?;

    if rows == 0 {
        tracing::warn!("subscription.deleted: no matching subscription for {sub_id}");
    } else {
        tracing::info!("subscription deleted: {sub_id} -> canceled");
    }
    Ok(())
}

/// `invoice.payment_failed` — mark subscription as `past_due`.
async fn handle_payment_failed(pool: &PgPool, payload: &serde_json::Value) -> Result<(), String> {
    let obj = &payload["data"]["object"];
    let sub_id = obj["subscription"]
        .as_str()
        .ok_or("invoice.payment_failed: no subscription ID")?;

    let rows = sqlx::query(
        "UPDATE subscriptions SET status = 'past_due', updated_at = now() WHERE stripe_subscription_id = $1",
    )
    .bind(sub_id)
    .execute(pool)
    .await
    .map_err(|e| format!("payment failed update error: {e}"))?
    .rows_affected();

    if rows == 0 {
        tracing::warn!("invoice.payment_failed: no matching subscription for {sub_id}");
    } else {
        tracing::info!("payment failed: {sub_id} -> past_due");
    }
    Ok(())
}
