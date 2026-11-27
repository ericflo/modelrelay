use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use sqlx::PgPool;

use crate::state::CloudState;

type HmacSha256 = Hmac<Sha256>;

/// Maximum age/skew tolerated for Stripe webhook timestamps (5 minutes, per
/// Stripe's recommended default replay protection).
const STRIPE_TIMESTAMP_TOLERANCE_SECS: i64 = 300;

/// POST /webhook/stripe — handle Stripe webhook events.
///
/// Verifies the `Stripe-Signature` header using HMAC-SHA256 + timestamp replay
/// window, then dispatches on the event type.
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

/// Verify the Stripe webhook signature (v1 scheme) against current wall clock.
fn verify_signature(sig_header: &str, payload: &[u8], secret: &str) -> Result<(), String> {
    let now_secs = chrono::Utc::now().timestamp();
    verify_signature_at(sig_header, payload, secret, now_secs)
}

/// Verify the Stripe webhook signature (v1 scheme) and replay window relative to `now_unix_secs`.
fn verify_signature_at(
    sig_header: &str,
    payload: &[u8],
    secret: &str,
    now_unix_secs: i64,
) -> Result<(), String> {
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

    let ts_secs: i64 = timestamp
        .parse()
        .map_err(|_| "invalid timestamp in signature header".to_string())?;
    if (now_unix_secs - ts_secs).abs() > STRIPE_TIMESTAMP_TOLERANCE_SECS {
        return Err("webhook timestamp outside replay tolerance window".into());
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
pub async fn provision_api_key(
    client: &reqwest::Client,
    admin_url: &str,
    admin_token: &str,
    name: &str,
) -> Result<(String, String), String> {
    use modelrelay_protocol::admin_api::{CreateKeyRequest, CreateKeyResponse};

    let url = format!("{}/admin/keys", admin_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .bearer_auth(admin_token)
        .json(&CreateKeyRequest {
            name: name.to_string(),
        })
        .send()
        .await
        .map_err(|e| format!("admin API request error: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("admin API returned {status}: {body}"));
    }

    let response: CreateKeyResponse = resp
        .json()
        .await
        .map_err(|e| format!("admin API response parse error: {e}"))?;

    Ok((response.metadata.id, response.secret))
}

/// Call `DELETE {admin_url}/admin/keys/{key_id}` to revoke an API key.
/// Treats HTTP 404 as idempotently revoked (`Ok(())`), while returning `Err`
/// on network failures or non-404 HTTP errors so callers can retry safely.
pub async fn revoke_api_key(
    client: &reqwest::Client,
    admin_url: &str,
    admin_token: &str,
    key_id: &str,
) -> Result<(), String> {
    let url = format!("{}/admin/keys/{}", admin_url.trim_end_matches('/'), key_id);
    let resp = client
        .delete(&url)
        .bearer_auth(admin_token)
        .send()
        .await
        .map_err(|e| format!("admin API revoke request error: {e}"))?;

    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(());
    }

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("admin API revoke returned {status}: {body}"));
    }

    Ok(())
}

/// `checkout.session.completed` — create or find user by email, upsert subscription,
/// and idempotently provision an API key via the admin API.
async fn handle_checkout_completed(
    state: &CloudState,
    pool: &PgPool,
    payload: &serde_json::Value,
) -> Result<(), String> {
    let obj = &payload["data"]["object"];
    let raw_email = obj["customer_email"]
        .as_str()
        .or_else(|| obj["customer_details"]["email"].as_str())
        .ok_or("checkout.session.completed: no customer email found")?;
    let email = raw_email.trim().to_lowercase();
    let stripe_customer_id = obj["customer"].as_str();
    let stripe_subscription_id = obj["subscription"]
        .as_str()
        .ok_or("checkout.session.completed: no subscription ID found")?;

    // Upsert user by normalized lowercase email
    let user_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, stripe_customer_id) \
         VALUES ($1, $2) \
         ON CONFLICT (email) DO UPDATE SET stripe_customer_id = COALESCE(EXCLUDED.stripe_customer_id, users.stripe_customer_id) \
         RETURNING id",
    )
    .bind(&email)
    .bind(stripe_customer_id)
    .fetch_one(pool)
    .await
    .map_err(|e| format!("user upsert error: {e}"))?;

    // Upsert subscription and return whether an API key is already provisioned
    let existing_api_key_id: Option<String> = sqlx::query_scalar(
        "INSERT INTO subscriptions (user_id, stripe_subscription_id, status, updated_at) \
         VALUES ($1, $2, 'active', now()) \
         ON CONFLICT (stripe_subscription_id) DO UPDATE SET status = 'active', updated_at = now() \
         RETURNING api_key_id",
    )
    .bind(user_id)
    .bind(stripe_subscription_id)
    .fetch_one(pool)
    .await
    .map_err(|e| format!("subscription upsert error: {e}"))?;

    // Idempotent key provisioning: skip if this subscription already has a key
    if existing_api_key_id.is_some() {
        tracing::info!(
            "checkout.session.completed replay for subscription={stripe_subscription_id}; key already provisioned"
        );
        return Ok(());
    }

    if let (Some(admin_url), Some(admin_token)) = (&state.admin_url, &state.admin_token) {
        let key_name = format!("user-{email}");
        // Propagate errors so Stripe receives HTTP 500 and retries provisioning.
        let (key_id, raw_key) =
            provision_api_key(&state.http_client, admin_url, admin_token, &key_name).await?;

        let db_write: Result<(), String> = async {
            sqlx::query(
                "INSERT INTO api_keys (user_id, key_id, raw_key, name) VALUES ($1, $2, $3, $4)",
            )
            .bind(user_id)
            .bind(&key_id)
            .bind(&raw_key)
            .bind(&key_name)
            .execute(pool)
            .await
            .map_err(|e| format!("failed to store api_key: {e}"))?;

            sqlx::query(
                "UPDATE subscriptions SET api_key_id = $1, updated_at = now() \
                 WHERE stripe_subscription_id = $2",
            )
            .bind(&key_id)
            .bind(stripe_subscription_id)
            .execute(pool)
            .await
            .map_err(|e| format!("failed to store api_key_id: {e}"))?;
            Ok(())
        }
        .await;

        if let Err(err) = db_write {
            // Compensating revoke so retries don't leave orphaned active keys on the relay.
            if let Err(revoke_err) =
                revoke_api_key(&state.http_client, admin_url, admin_token, &key_id).await
            {
                tracing::error!(
                    key_id = %key_id,
                    error = %revoke_err,
                    "compensating revoke failed after DB write error"
                );
            }
            return Err(err);
        }

        tracing::info!(
            "provisioned API key {key_id} for user={email} subscription={stripe_subscription_id}"
        );
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
            // Propagate non-404 errors so Stripe retries revocation instead of leaving a
            // canceled customer's key live on modelrelay-server.
            revoke_api_key(&state.http_client, admin_url, admin_token, key_id).await?;
            tracing::info!("revoked API key {key_id} for subscription {sub_id}");
        } else {
            tracing::warn!("admin API not configured — cannot revoke API key {key_id}");
        }

        sqlx::query("UPDATE api_keys SET revoked_at = now() WHERE key_id = $1")
            .bind(key_id)
            .execute(pool)
            .await
            .map_err(|e| format!("api_keys revoke error: {e}"))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use hmac::KeyInit;

    fn compute_sig(secret: &str, timestamp: &str, payload: &[u8]) -> String {
        let signed = format!("{timestamp}.{}", String::from_utf8_lossy(payload));
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(signed.as_bytes());
        hex::encode(mac.finalize().into_bytes())
    }

    #[test]
    fn valid_signature_accepted() {
        let secret = "whsec_test_secret";
        let payload = b"{}";
        let ts = "1700000000";
        let sig = compute_sig(secret, ts, payload);
        let header = format!("t={ts},v1={sig}");
        assert!(verify_signature_at(&header, payload, secret, 1_700_000_060).is_ok());
    }

    #[test]
    fn stale_timestamp_replay_rejected() {
        let secret = "whsec_test_secret";
        let payload = b"{}";
        let ts = "1700000000";
        let sig = compute_sig(secret, ts, payload);
        let header = format!("t={ts},v1={sig}");
        let err = verify_signature_at(&header, payload, secret, 1_700_000_600).unwrap_err();
        assert!(err.contains("replay tolerance"), "got: {err}");
    }

    #[test]
    fn wrong_signature_rejected() {
        let secret = "whsec_test_secret";
        let payload = b"{}";
        let ts = "1700000000";
        let header =
            format!("t={ts},v1=badbadbadbadbadbadbadbadbadbadbadbadbadbadbadbadbadbadbadbadbadbad");
        let err = verify_signature_at(&header, payload, secret, 1_700_000_010).unwrap_err();
        assert!(err.contains("signature mismatch"), "got: {err}");
    }

    #[test]
    fn missing_timestamp_rejected() {
        let header = "v1=abc123";
        let err = verify_signature_at(header, b"{}", "secret", 1_700_000_010).unwrap_err();
        assert!(err.contains("missing timestamp"), "got: {err}");
    }

    #[test]
    fn missing_v1_rejected() {
        let header = "t=1700000000";
        let err = verify_signature_at(header, b"{}", "secret", 1_700_000_010).unwrap_err();
        assert!(err.contains("no v1 signature"), "got: {err}");
    }

    #[test]
    fn valid_signature_with_extra_fields() {
        let secret = "whsec_test_secret";
        let payload = b"{\"type\":\"checkout.session.completed\"}";
        let ts = "1700000000";
        let sig = compute_sig(secret, ts, payload);
        let header = format!("t={ts},v0=ignored,v1={sig}");
        assert!(verify_signature_at(&header, payload, secret, 1_700_000_010).is_ok());
    }

    #[test]
    fn different_payload_different_sig() {
        let secret = "whsec_test_secret";
        let ts = "1700000000";
        let sig = compute_sig(secret, ts, b"original");
        let header = format!("t={ts},v1={sig}");
        let err = verify_signature_at(&header, b"tampered", secret, 1_700_000_010).unwrap_err();
        assert!(err.contains("signature mismatch"), "got: {err}");
    }

    #[test]
    fn constant_time_eq_works() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "ab"));
        assert!(!constant_time_eq("", "a"));
        assert!(constant_time_eq("", ""));
    }
}
