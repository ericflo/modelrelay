//! OAuth 2.0 PKCE authorization and key exchange provider.
//!
//! Enables browser apps (such as `chat.neuralcolumn.com`) and CLI clients to
//! connect a user's `ModelRelay` account without copying API keys by hand,
//! following `OpenRouter`'s PKCE key-exchange ergonomics alongside standard
//! RFC 7636 / OAuth 2.0 parameter names:
//!
//! 1. Client redirects user to:
//!    `GET /oauth/authorize?callback_url=...&code_challenge=...&code_challenge_method=S256&state=...&key_label=...`
//!    (standard OAuth aliases `redirect_uri`, `client_name`, `response_type=code` are also accepted).
//! 2. User logs in (if needed) and approves on the consent page (`POST /oauth/authorize`).
//! 3. `ModelRelay` redirects to `callback_url?code=...&state=...` with a single-use,
//!    5-minute authorization code (only its SHA-256 hash is stored in `PostgreSQL`).
//! 4. Client exchanges the code + PKCE `code_verifier` directly from the browser
//!    (CORS-enabled) via `POST /api/v1/auth/keys` (JSON or form) or `POST /oauth/token`,
//!    receiving `{ "key", "access_token", "token_type", "api_base_url", "label" }`.

use std::fmt::Write as _;
use std::net::IpAddr;
use std::sync::Arc;

use argon2::password_hash::rand_core::{OsRng, RngCore};
use axum::Form;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, PRAGMA};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use reqwest::Url;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tower_sessions::Session;

use crate::state::CloudState;

use super::{csrf, webhook};

/// Authorization codes expire after 5 minutes (RFC 6749 §4.1.2 recommends <= 10 min).
const AUTH_CODE_TTL_SECS: i64 = 300;
const MAX_CALLBACK_URL_LEN: usize = 2048;
const MAX_STATE_LEN: usize = 512;
const MAX_LABEL_LEN: usize = 64;

/// Query parameters for `GET /oauth/authorize`.
#[derive(Debug, Deserialize)]
pub struct AuthorizeQuery {
    pub callback_url: Option<String>,
    pub redirect_uri: Option<String>,
    pub code_challenge: Option<String>,
    pub code_challenge_method: Option<String>,
    pub state: Option<String>,
    pub key_label: Option<String>,
    pub client_name: Option<String>,
    pub response_type: Option<String>,
}

/// Form parameters for `POST /oauth/authorize`.
#[derive(Debug, Deserialize)]
pub struct AuthorizeDecisionForm {
    pub decision: String,
    pub callback_url: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub key_label: String,
    #[serde(default)]
    pub selected_key_id: String,
    #[allow(dead_code)]
    _csrf: Option<String>,
}

/// Payload for `POST /api/v1/auth/keys` and `POST /oauth/token`
/// (supports both JSON and `application/x-www-form-urlencoded`).
#[derive(Debug, Deserialize)]
struct TokenExchangeRequest {
    #[serde(default)]
    grant_type: Option<String>,
    code: String,
    code_verifier: String,
    #[serde(default)]
    code_challenge_method: Option<String>,
    #[serde(default)]
    redirect_uri: Option<String>,
    #[serde(default)]
    callback_url: Option<String>,
}

/// Validated OAuth PKCE authorization request.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ValidatedAuthorizeRequest {
    callback_url: Url,
    code_challenge: String,
    code_challenge_method: String,
    state: Option<String>,
    key_label: String,
}

#[derive(sqlx::FromRow)]
struct UserAccount {
    email: String,
    is_admin: bool,
}

#[derive(sqlx::FromRow)]
struct ActiveKeyRow {
    id: uuid::Uuid,
    name: String,
    raw_key: String,
}

#[derive(sqlx::FromRow)]
struct ConsumedCodeRow {
    api_key_row_id: uuid::Uuid,
    callback_url: String,
    code_challenge: String,
    code_challenge_method: String,
    key_label: String,
}

// ─── PKCE & validation helpers ──────────────────────────────────────────────

const BASE64URL_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// RFC 4648 URL-safe Base64 encoding without `=` padding.
#[must_use]
pub fn base64url_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = if chunk.len() > 1 {
            u32::from(chunk[1])
        } else {
            0
        };
        let b2 = if chunk.len() > 2 {
            u32::from(chunk[2])
        } else {
            0
        };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(BASE64URL_ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(BASE64URL_ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(BASE64URL_ALPHABET[((n >> 6) & 0x3f) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(BASE64URL_ALPHABET[(n & 0x3f) as usize] as char);
        }
    }
    out
}

/// Compute RFC 7636 PKCE `S256` challenge: `BASE64URL-ENCODE(SHA256(ASCII(verifier)))`.
#[must_use]
pub fn compute_s256_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64url_encode(&digest)
}

/// Constant-time string equality check.
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0_u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Validate a PKCE `code_verifier` against an expected `S256` `code_challenge`.
#[must_use]
pub fn verify_pkce_s256(verifier: &str, expected_challenge: &str) -> bool {
    if !(43..=128).contains(&verifier.len()) {
        return false;
    }
    if !verifier
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
    {
        return false;
    }
    let computed = compute_s256_challenge(verifier);
    constant_time_eq(&computed, expected_challenge)
}

/// Validate a PKCE `S256` `code_challenge` parameter.
fn is_valid_code_challenge(challenge: &str) -> bool {
    (43..=128).contains(&challenge.len())
        && challenge
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

/// Generate a high-entropy single-use authorization code (`mrc_<43 base64url chars>`).
fn generate_auth_code() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    format!("mrc_{}", base64url_encode(&bytes))
}

/// Compute hex SHA-256 digest of an authorization code for database storage.
fn hash_code(code: &str) -> String {
    hex::encode(Sha256::digest(code.as_bytes()))
}

/// Validate a callback URL for OAuth redirects.
///
/// Permits `https://` URLs anywhere, and `http://` strictly on loopback
/// (`localhost`, `127.0.0.1`, `[::1]`) for local CLI / development workflows.
/// Rejects fragments, embedded credentials, and non-HTTP schemes.
fn validate_callback_url(raw: &str) -> Result<Url, &'static str> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_CALLBACK_URL_LEN {
        return Err("callback_url is missing or too long.");
    }
    let parsed = Url::parse(trimmed).map_err(|_| "callback_url must be a valid absolute URL.")?;
    if parsed.fragment().is_some() {
        return Err("callback_url must not contain a URL fragment (#).");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("callback_url must not contain embedded credentials.");
    }
    let Some(host) = parsed.host_str() else {
        return Err("callback_url must include a host.");
    };
    match parsed.scheme() {
        "https" => Ok(parsed),
        "http" if matches!(host, "localhost" | "127.0.0.1" | "[::1]") => Ok(parsed),
        "http" => Err("http:// callback_url is only allowed for localhost or 127.0.0.1."),
        _ => Err("callback_url must use https:// (or http://localhost for local apps)."),
    }
}

fn sanitize_label(raw: Option<&str>, fallback_host: &str) -> String {
    let cleaned: String = raw
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(MAX_LABEL_LEN)
        .collect();
    if cleaned.is_empty() {
        fallback_host.chars().take(MAX_LABEL_LEN).collect()
    } else {
        cleaned
    }
}

fn validate_authorize_params(
    callback_raw: Option<&str>,
    challenge_raw: Option<&str>,
    method_raw: Option<&str>,
    state_raw: Option<&str>,
    label_raw: Option<&str>,
    response_type: Option<&str>,
) -> Result<ValidatedAuthorizeRequest, &'static str> {
    if let Some(rt) = response_type
        && rt.trim() != "code"
    {
        return Err("Unsupported response_type; only \"code\" is supported.");
    }

    let callback_url = validate_callback_url(callback_raw.unwrap_or(""))?;

    let method = method_raw.unwrap_or("").trim();
    if method != "S256" {
        return Err("PKCE code_challenge_method must be \"S256\".");
    }

    let challenge = challenge_raw.unwrap_or("").trim();
    if !is_valid_code_challenge(challenge) {
        return Err("Invalid or missing PKCE code_challenge.");
    }

    let state = match state_raw.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) if s.len() > MAX_STATE_LEN || s.chars().any(char::is_control) => {
            return Err("Invalid state parameter.");
        }
        Some(s) => Some(s.to_owned()),
        None => None,
    };

    let host = callback_url.host_str().unwrap_or("app");
    let key_label = sanitize_label(label_raw, host);

    Ok(ValidatedAuthorizeRequest {
        callback_url,
        code_challenge: challenge.to_owned(),
        code_challenge_method: "S256".to_owned(),
        state,
        key_label,
    })
}

fn build_callback_redirect(base: &Url, pairs: &[(&str, &str)], state: Option<&str>) -> String {
    let mut target = base.clone();
    {
        let mut query = target.query_pairs_mut();
        for (k, v) in pairs {
            query.append_pair(k, v);
        }
        if let Some(s) = state
            && !s.is_empty()
        {
            query.append_pair("state", s);
        }
    }
    target.into()
}

fn build_login_redirect(req: &ValidatedAuthorizeRequest) -> String {
    let mut authorize_url = Url::parse("https://modelrelay.io/oauth/authorize").expect("valid URL");
    {
        let mut q = authorize_url.query_pairs_mut();
        q.append_pair("callback_url", req.callback_url.as_str());
        q.append_pair("code_challenge", &req.code_challenge);
        q.append_pair("code_challenge_method", &req.code_challenge_method);
        if let Some(ref s) = req.state {
            q.append_pair("state", s);
        }
        q.append_pair("key_label", &req.key_label);
    }
    let next_path = match authorize_url.query() {
        Some(q) => format!("/oauth/authorize?{q}"),
        None => "/oauth/authorize".to_owned(),
    };
    let mut login_url = Url::parse("https://modelrelay.io/login").expect("valid URL");
    login_url.query_pairs_mut().append_pair("next", &next_path);
    match login_url.query() {
        Some(q) => format!("/login?{q}"),
        None => "/login".to_owned(),
    }
}

fn client_ip(headers: &HeaderMap) -> IpAddr {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .and_then(|s| s.trim().parse::<IpAddr>().ok())
        .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST))
}

fn mask_key(raw: &str) -> String {
    if raw.len() <= 14 {
        return "••••••••••••".to_owned();
    }
    let prefix = &raw[..10];
    let suffix = &raw[raw.len() - 4..];
    format!("{prefix}••••{suffix}")
}

fn slugify_label(label: &str) -> String {
    let slug: String = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        "app".to_owned()
    } else {
        trimmed.chars().take(32).collect()
    }
}

fn oauth_error_page(status: StatusCode, title: &str, message: &str, logged_in: bool) -> Response {
    let title_esc = modelrelay_web::templates::html_escape(title);
    let msg_esc = modelrelay_web::templates::html_escape(message);
    let body = format!(
        r#"<div class="card" style="max-width:560px;margin:28px auto;border-left:3px solid var(--danger);">
  <h2>{title_esc}</h2>
  <p style="margin-bottom:18px;">{msg_esc}</p>
  <div style="display:flex;gap:10px;flex-wrap:wrap;">
    <a href="/" class="btn btn-sm">Back to ModelRelay</a>
    <a href="/docs/api" class="btn btn-sm btn-secondary">OAuth &amp; API Docs</a>
  </div>
</div>"#
    );
    (
        status,
        Html(modelrelay_web::templates::page_shell(
            title,
            "/oauth/authorize",
            &body,
            logged_in,
        )),
    )
        .into_response()
}

fn with_cors_headers(mut resp: Response) -> Response {
    let headers = resp.headers_mut();
    headers.insert("access-control-allow-origin", HeaderValue::from_static("*"));
    headers.insert(
        "access-control-allow-methods",
        HeaderValue::from_static("POST, OPTIONS"),
    );
    headers.insert(
        "access-control-allow-headers",
        HeaderValue::from_static("content-type, authorization"),
    );
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(PRAGMA, HeaderValue::from_static("no-cache"));
    resp
}

fn json_error(status: StatusCode, error: &str, description: &str) -> Response {
    let body = serde_json::json!({
        "error": error,
        "error_description": description,
        "message": description,
    });
    with_cors_headers((status, axum::Json(body)).into_response())
}

// ─── GET /oauth/authorize ───────────────────────────────────────────────────

/// Render the OAuth PKCE consent page (or redirect to `/login?next=...` if signed out).
#[allow(clippy::too_many_lines)]
pub async fn authorize_get(
    session: Session,
    State(state): State<Arc<CloudState>>,
    Query(query): Query<AuthorizeQuery>,
) -> Response {
    let callback_raw = query
        .callback_url
        .as_deref()
        .or(query.redirect_uri.as_deref());
    let label_raw = query.key_label.as_deref().or(query.client_name.as_deref());

    let validated = match validate_authorize_params(
        callback_raw,
        query.code_challenge.as_deref(),
        query.code_challenge_method.as_deref(),
        query.state.as_deref(),
        label_raw,
        query.response_type.as_deref(),
    ) {
        Ok(v) => v,
        Err(msg) => {
            return oauth_error_page(StatusCode::BAD_REQUEST, "Invalid OAuth Request", msg, false);
        }
    };

    let Some(user_id_str) = session.get::<String>("user_id").await.ok().flatten() else {
        return Redirect::to(&build_login_redirect(&validated)).into_response();
    };
    let Ok(user_id) = user_id_str.parse::<uuid::Uuid>() else {
        return Redirect::to(&build_login_redirect(&validated)).into_response();
    };

    let Some(ref pool) = state.db else {
        return oauth_error_page(
            StatusCode::SERVICE_UNAVAILABLE,
            "Database Unavailable",
            "ModelRelay cannot process authorization requests while the database is unavailable.",
            true,
        );
    };

    let user =
        match sqlx::query_as::<_, UserAccount>("SELECT email, is_admin FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await
        {
            Ok(Some(u)) => u,
            Ok(None) => return Redirect::to(&build_login_redirect(&validated)).into_response(),
            Err(e) => {
                tracing::error!("oauth authorize user lookup error: {e}");
                return oauth_error_page(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Account Lookup Failed",
                    "Could not load your account. Please try again in a moment.",
                    true,
                );
            }
        };

    let active_keys = sqlx::query_as::<_, ActiveKeyRow>(
        "SELECT id, name, raw_key FROM api_keys \
         WHERE user_id = $1 AND revoked_at IS NULL ORDER BY created_at DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let has_active_sub: bool = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM subscriptions WHERE user_id = $1 AND status = 'active')",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .unwrap_or(false);

    let can_provision = (user.is_admin || has_active_sub)
        && state.admin_url.is_some()
        && state.admin_token.is_some();
    let eligible = !active_keys.is_empty() || can_provision;
    let csrf_field = csrf::hidden_field(&session).await;

    let body = render_consent_html(
        &validated,
        &user,
        &active_keys,
        can_provision,
        eligible,
        &state.public_relay_url,
        &csrf_field,
    );

    Html(modelrelay_web::templates::page_shell_custom(
        "Authorize Application",
        "/oauth/authorize",
        &body,
        true,
        CONSENT_CSS,
        "",
    ))
    .into_response()
}

// ─── POST /oauth/authorize ──────────────────────────────────────────────────

/// Process the user's consent decision and redirect back to `callback_url`.
#[allow(clippy::too_many_lines)]
pub async fn authorize_post(
    session: Session,
    State(state): State<Arc<CloudState>>,
    Form(form): Form<AuthorizeDecisionForm>,
) -> Response {
    let validated = match validate_authorize_params(
        Some(&form.callback_url),
        Some(&form.code_challenge),
        Some(&form.code_challenge_method),
        Some(&form.state),
        Some(&form.key_label),
        None,
    ) {
        Ok(v) => v,
        Err(msg) => {
            return oauth_error_page(StatusCode::BAD_REQUEST, "Invalid OAuth Request", msg, true);
        }
    };

    if form.decision != "approve" {
        let target = build_callback_redirect(
            &validated.callback_url,
            &[("error", "access_denied")],
            validated.state.as_deref(),
        );
        return Redirect::to(&target).into_response();
    }

    let Some(user_id_str) = session.get::<String>("user_id").await.ok().flatten() else {
        return Redirect::to(&build_login_redirect(&validated)).into_response();
    };
    let Ok(user_id) = user_id_str.parse::<uuid::Uuid>() else {
        return Redirect::to(&build_login_redirect(&validated)).into_response();
    };

    let Some(ref pool) = state.db else {
        return oauth_error_page(
            StatusCode::SERVICE_UNAVAILABLE,
            "Database Unavailable",
            "Database is temporarily unavailable.",
            true,
        );
    };

    let user =
        match sqlx::query_as::<_, UserAccount>("SELECT email, is_admin FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await
        {
            Ok(Some(u)) => u,
            Ok(None) => return Redirect::to(&build_login_redirect(&validated)).into_response(),
            Err(e) => {
                tracing::error!("oauth approve user lookup error: {e}");
                return oauth_error_page(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Authorization Failed",
                    "Could not load your account. Please try again.",
                    true,
                );
            }
        };

    let selected = form.selected_key_id.trim();
    let api_key_row_id: uuid::Uuid = if !selected.is_empty() && selected != "new" {
        let Ok(parsed_id) = selected.parse::<uuid::Uuid>() else {
            return oauth_error_page(
                StatusCode::BAD_REQUEST,
                "Invalid Key Selection",
                "The selected API key is not valid.",
                true,
            );
        };
        let found: Option<uuid::Uuid> = sqlx::query_scalar(
            "SELECT id FROM api_keys WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
        )
        .bind(parsed_id)
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
        let Some(row_id) = found else {
            return oauth_error_page(
                StatusCode::BAD_REQUEST,
                "Key Unavailable",
                "That API key no longer exists or was revoked. Return to the app and try again.",
                true,
            );
        };
        row_id
    } else {
        let has_active_sub: bool = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM subscriptions WHERE user_id = $1 AND status = 'active')",
        )
        .bind(user_id)
        .fetch_one(pool)
        .await
        .unwrap_or(false);

        if !user.is_admin && !has_active_sub {
            return oauth_error_page(
                StatusCode::FORBIDDEN,
                "Subscription Required",
                "An active ModelRelay subscription or API key is required to authorize apps.",
                true,
            );
        }

        let (Some(admin_url), Some(admin_token)) = (&state.admin_url, &state.admin_token) else {
            return oauth_error_page(
                StatusCode::SERVICE_UNAVAILABLE,
                "Key Provisioning Unavailable",
                "Could not create a new API key right now. Please select an existing key or try later.",
                true,
            );
        };

        let ts = chrono::Utc::now().format("%Y%m%d%H%M%S");
        let slug = slugify_label(&validated.key_label);
        let key_name = format!("oauth-{slug}-{ts}");
        let (key_id, raw_key) =
            match webhook::provision_api_key(&state.http_client, admin_url, admin_token, &key_name)
                .await
            {
                Ok(pair) => pair,
                Err(e) => {
                    tracing::error!("oauth key provision error: {e}");
                    return oauth_error_page(
                        StatusCode::BAD_GATEWAY,
                        "Key Provisioning Failed",
                        "Could not reach the relay server to create an API key. Please try again.",
                        true,
                    );
                }
            };

        match sqlx::query_scalar::<_, uuid::Uuid>(
            "INSERT INTO api_keys (user_id, key_id, raw_key, name) VALUES ($1, $2, $3, $4) RETURNING id",
        )
        .bind(user_id)
        .bind(&key_id)
        .bind(&raw_key)
        .bind(&key_name)
        .fetch_one(pool)
        .await
        {
            Ok(row_id) => row_id,
            Err(e) => {
                tracing::error!("oauth api_keys insert error: {e}; attempting compensating revoke");
                let _ =
                    webhook::revoke_api_key(&state.http_client, admin_url, admin_token, &key_id)
                        .await;
                return oauth_error_page(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Authorization Failed",
                    "Could not save the new API key. Please try again.",
                    true,
                );
            }
        }
    };

    // Prune expired authorization codes opportunistically.
    let _ = sqlx::query("DELETE FROM oauth_authorization_codes WHERE expires_at < now()")
        .execute(pool)
        .await;

    let raw_code = generate_auth_code();
    let code_hash = hash_code(&raw_code);
    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(AUTH_CODE_TTL_SECS);

    if let Err(e) = sqlx::query(
        "INSERT INTO oauth_authorization_codes \
         (code_hash, user_id, api_key_row_id, callback_url, code_challenge, code_challenge_method, key_label, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(&code_hash)
    .bind(user_id)
    .bind(api_key_row_id)
    .bind(validated.callback_url.as_str())
    .bind(&validated.code_challenge)
    .bind(&validated.code_challenge_method)
    .bind(&validated.key_label)
    .bind(expires_at)
    .execute(pool)
    .await
    {
        tracing::error!("oauth code insert error: {e}");
        return oauth_error_page(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Authorization Failed",
            "Could not issue an authorization code. Please try again.",
            true,
        );
    }

    tracing::info!(
        email = %user.email,
        callback_host = %validated.callback_url.host_str().unwrap_or(""),
        label = %validated.key_label,
        "issued OAuth PKCE authorization code"
    );

    let target = build_callback_redirect(
        &validated.callback_url,
        &[("code", &raw_code)],
        validated.state.as_deref(),
    );

    Redirect::to(&target).into_response()
}

// ─── OPTIONS & POST /api/v1/auth/keys and /oauth/token ──────────────────────

/// CORS preflight for browser PKCE code exchange.
pub async fn token_preflight() -> Response {
    let mut resp = StatusCode::NO_CONTENT.into_response();
    let headers = resp.headers_mut();
    headers.insert("access-control-allow-origin", HeaderValue::from_static("*"));
    headers.insert(
        "access-control-allow-methods",
        HeaderValue::from_static("POST, OPTIONS"),
    );
    headers.insert(
        "access-control-allow-headers",
        HeaderValue::from_static("content-type, authorization"),
    );
    headers.insert("access-control-max-age", HeaderValue::from_static("86400"));
    resp
}

/// Exchange a single-use PKCE authorization code for the authorized `ModelRelay` API key.
///
/// Accepts both `OpenRouter`-style JSON (`POST /api/v1/auth/keys`) and standard OAuth 2.0
/// form-urlencoded (`POST /oauth/token`).
#[allow(clippy::too_many_lines)]
pub async fn exchange_code(
    State(state): State<Arc<CloudState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let ip = client_ip(&headers);
    if state.rate_limiter.is_limited(ip) {
        return json_error(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "Too many failed exchange attempts. Please wait and try again.",
        );
    }

    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();

    let payload: TokenExchangeRequest = if content_type == "application/x-www-form-urlencoded" {
        match parse_form_exchange(&body) {
            Ok(p) => p,
            Err(msg) => return json_error(StatusCode::BAD_REQUEST, "invalid_request", msg),
        }
    } else {
        match serde_json::from_slice(&body) {
            Ok(p) => p,
            Err(_) => {
                return json_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "Expected JSON body with `code` and `code_verifier`.",
                );
            }
        }
    };

    if let Some(ref gt) = payload.grant_type
        && gt.trim() != "authorization_code"
    {
        return json_error(
            StatusCode::BAD_REQUEST,
            "unsupported_grant_type",
            "Only `authorization_code` grant_type is supported.",
        );
    }

    if let Some(ref method) = payload.code_challenge_method
        && method.trim() != "S256"
    {
        return json_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Only PKCE `S256` code_challenge_method is supported.",
        );
    }

    let code = payload.code.trim();
    let verifier = payload.code_verifier.trim();
    if code.is_empty() || verifier.is_empty() {
        return json_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Both `code` and `code_verifier` are required.",
        );
    }

    let Some(ref pool) = state.db else {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "temporarily_unavailable",
            "Database is temporarily unavailable.",
        );
    };

    let code_hash = hash_code(code);
    let consumed = match sqlx::query_as::<_, ConsumedCodeRow>(
        "UPDATE oauth_authorization_codes \
         SET consumed_at = now() \
         WHERE code_hash = $1 AND consumed_at IS NULL AND expires_at > now() \
         RETURNING api_key_row_id, callback_url, code_challenge, code_challenge_method, key_label",
    )
    .bind(&code_hash)
    .fetch_optional(pool)
    .await
    {
        Ok(row) => row,
        Err(e) => {
            tracing::error!("oauth exchange DB error: {e}");
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "Could not verify authorization code.",
            );
        }
    };

    let Some(row) = consumed else {
        state.rate_limiter.record_attempt(ip);
        return json_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "Authorization code is invalid, expired, or already used.",
        );
    };

    if row.code_challenge_method != "S256" || !verify_pkce_s256(verifier, &row.code_challenge) {
        state.rate_limiter.record_attempt(ip);
        return json_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "PKCE code_verifier does not match code_challenge.",
        );
    }

    if let Some(supplied_cb) = payload
        .redirect_uri
        .as_deref()
        .or(payload.callback_url.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && supplied_cb != row.callback_url
    {
        state.rate_limiter.record_attempt(ip);
        return json_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "redirect_uri does not match the authorization request.",
        );
    }

    let raw_key: Option<String> =
        sqlx::query_scalar("SELECT raw_key FROM api_keys WHERE id = $1 AND revoked_at IS NULL")
            .bind(row.api_key_row_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();

    let Some(raw_key) = raw_key else {
        return json_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "The authorized API key was revoked before the code was exchanged.",
        );
    };

    let api_base_url = format!("{}/v1", state.public_relay_url.trim_end_matches('/'));
    let resp = axum::Json(serde_json::json!({
        "key": raw_key,
        "access_token": raw_key,
        "token_type": "Bearer",
        "api_base_url": api_base_url,
        "label": row.key_label,
    }))
    .into_response();

    with_cors_headers(resp)
}

fn parse_form_exchange(body: &[u8]) -> Result<TokenExchangeRequest, &'static str> {
    let body_str = std::str::from_utf8(body).map_err(|_| "Form body must be valid UTF-8.")?;
    let mut url = Url::parse("https://modelrelay.io/oauth/token").expect("valid URL");
    url.set_query(Some(body_str));

    let mut grant_type = None;
    let mut code = None;
    let mut code_verifier = None;
    let mut code_challenge_method = None;
    let mut redirect_uri = None;
    let mut callback_url = None;

    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "grant_type" => grant_type = Some(v.into_owned()),
            "code" => code = Some(v.into_owned()),
            "code_verifier" => code_verifier = Some(v.into_owned()),
            "code_challenge_method" => code_challenge_method = Some(v.into_owned()),
            "redirect_uri" => redirect_uri = Some(v.into_owned()),
            "callback_url" => callback_url = Some(v.into_owned()),
            _ => {}
        }
    }

    Ok(TokenExchangeRequest {
        grant_type,
        code: code.ok_or("Missing `code` parameter.")?,
        code_verifier: code_verifier.ok_or("Missing `code_verifier` parameter.")?,
        code_challenge_method,
        redirect_uri,
        callback_url,
    })
}

// ─── Consent screen HTML ────────────────────────────────────────────────────

const CONSENT_CSS: &str = r"<style>
  .oauth-wrap { max-width: 560px; margin: 12px auto 36px; }
  .oauth-card {
    background: var(--bg-surface); border: 1px solid var(--border);
    border-radius: var(--radius-xl); padding: 32px; box-shadow: var(--shadow-card);
    position: relative; overflow: hidden;
  }
  .oauth-card::before {
    content: ''; position: absolute; top: 0; left: 0; right: 0; height: 3px;
    background: linear-gradient(90deg, var(--accent), var(--cyan));
  }
  .oauth-app-badge {
    display: inline-flex; align-items: center; gap: 8px;
    padding: 5px 12px; border-radius: 999px; margin-bottom: 14px;
    background: var(--accent-soft); border: 1px solid var(--accent-border);
    font-size: 0.78rem; font-weight: 600; color: var(--accent-fg);
  }
  .oauth-card h1 { font-size: 1.65rem; font-weight: 800; letter-spacing: -0.025em; margin-bottom: 8px; color: var(--text-strong); }
  .oauth-origin {
    font-family: var(--font-mono); font-size: 0.84rem; color: var(--cyan-fg);
    background: var(--cyan-soft); border: 1px solid rgba(2, 132, 199, 0.28);
    padding: 3px 9px; border-radius: 6px; display: inline-block; margin-bottom: 16px;
    word-break: break-all;
  }
  .oauth-sub { color: var(--text-secondary); font-size: 0.94rem; margin-bottom: 20px; line-height: 1.6; overflow-wrap: break-word; }
  .oauth-sub code, .oauth-perms code { overflow-wrap: anywhere; word-break: break-word; }
  .oauth-perms {
    list-style: none; display: flex; flex-direction: column; gap: 10px;
    padding: 16px 18px; background: var(--bg-subtle); border: 1px solid var(--border);
    border-radius: var(--radius-md); margin-bottom: 22px; font-size: 0.88rem; color: var(--text-primary);
  }
  .oauth-perms li {
    position: relative; padding-left: 22px; line-height: 1.55;
    overflow-wrap: break-word; min-width: 0;
  }
  .oauth-perms li::before {
    content: '\2713'; position: absolute; left: 0; top: 0;
    color: var(--success); font-weight: 700;
  }
  .oauth-keys-title {
    font-size: 0.74rem; font-weight: 700; text-transform: uppercase;
    letter-spacing: 0.08em; color: var(--text-muted); margin-bottom: 10px;
  }
  .oauth-key-list { display: flex; flex-direction: column; gap: 8px; margin-bottom: 22px; }
  .oauth-key-opt {
    display: flex; align-items: flex-start; gap: 10px; padding: 12px 14px;
    background: var(--bg-input); border: 1px solid var(--border-strong);
    border-radius: var(--radius-md); cursor: pointer; transition: border-color 0.15s, background 0.15s;
  }
  .oauth-key-opt:hover { border-color: var(--accent-border); }
  .oauth-key-opt input { margin-top: 3px; accent-color: var(--accent); flex-shrink: 0; }
  .oauth-key-opt > span { min-width: 0; flex: 1; }
  .oauth-key-name {
    font-size: 0.9rem; font-weight: 600; color: var(--text-strong);
    display: block; overflow-wrap: anywhere;
  }
  .oauth-key-meta {
    font-family: var(--font-mono); font-size: 0.78rem; color: var(--text-muted);
    display: block; margin-top: 2px; line-height: 1.45; overflow-wrap: break-word;
  }
  .oauth-actions { display: flex; gap: 10px; flex-wrap: wrap; }
  .oauth-actions .btn { flex: 1; min-width: 140px; }
  .oauth-foot {
    margin-top: 18px; padding-top: 14px; border-top: 1px solid var(--border);
    display: flex; justify-content: space-between; align-items: center; gap: 12px; flex-wrap: wrap;
    font-size: 0.82rem; color: var(--text-muted);
  }
  @media (max-width: 560px) {
    .oauth-wrap { margin: 4px auto 24px; }
    .oauth-card { padding: 22px 18px; border-radius: var(--radius-lg); }
    .oauth-card h1 { font-size: 1.4rem; }
    .oauth-perms { padding: 14px; }
    .oauth-actions { flex-direction: column; }
    .oauth-actions .btn { width: 100%; min-width: 0; }
  }
</style>";

#[allow(clippy::too_many_lines)]
fn render_consent_html(
    req: &ValidatedAuthorizeRequest,
    user: &UserAccount,
    active_keys: &[ActiveKeyRow],
    can_provision: bool,
    eligible: bool,
    public_relay_url: &str,
    csrf_field: &str,
) -> String {
    let esc = modelrelay_web::templates::html_escape;
    let app_label = esc(&req.key_label);
    let origin = req.callback_url.origin().ascii_serialization();
    let origin_esc = esc(&origin);
    let callback_esc = esc(req.callback_url.as_str());
    let challenge_esc = esc(&req.code_challenge);
    let method_esc = esc(&req.code_challenge_method);
    let state_esc = esc(req.state.as_deref().unwrap_or(""));
    let email_esc = esc(&user.email);
    let relay_esc = esc(public_relay_url);

    let mut key_picker = String::new();
    if eligible {
        if active_keys.is_empty() {
            key_picker.push_str(
                r#"<input type="hidden" name="selected_key_id" value="new">
                <p style="font-size:0.86rem;color:var(--text-secondary);margin-bottom:18px;">Approving will provision a new API key on your account and return it securely via PKCE.</p>"#,
            );
        } else {
            key_picker.push_str(
                "<div class=\"oauth-keys-title\">API Key to Share</div><div class=\"oauth-key-list\">",
            );
            for (idx, k) in active_keys.iter().enumerate() {
                let checked = if idx == 0 { " checked" } else { "" };
                let name_esc = esc(&k.name);
                let masked = esc(&mask_key(&k.raw_key));
                let note = if idx == 0 {
                    "Recommended — routes to GPU workers connected with this key"
                } else {
                    "Active key"
                };
                let _ = write!(
                    key_picker,
                    r#"<label class="oauth-key-opt">
                      <input type="radio" name="selected_key_id" value="{}"{checked}>
                      <span>
                        <span class="oauth-key-name">{name_esc}</span>
                        <span class="oauth-key-meta">{masked} &middot; {note}</span>
                      </span>
                    </label>"#,
                    k.id
                );
            }
            if user.is_admin && can_provision {
                key_picker.push_str(
                    r#"<label class="oauth-key-opt">
                      <input type="radio" name="selected_key_id" value="new">
                      <span>
                        <span class="oauth-key-name">Create a new dedicated API key</span>
                        <span class="oauth-key-meta">Revocable separately; routes to shared workers or workers using this key</span>
                      </span>
                    </label>"#,
                );
            }
            key_picker.push_str("</div>");
        }
    } else {
        key_picker.push_str(
            r#"<div class="error-msg">Your account does not have an active API key or subscription yet. Visit your <a href="/dashboard">Dashboard</a> to set up access first.</div>"#,
        );
    }

    let approve_btn = if eligible {
        r#"<button type="submit" name="decision" value="approve" class="btn">Approve &amp; Connect</button>"#
    } else {
        ""
    };

    format!(
        r#"<div class="oauth-wrap">
  <div class="oauth-card">
    <div class="oauth-app-badge">OAuth PKCE Connection</div>
    <h1>Connect {app_label}</h1>
    <div class="oauth-origin">{origin_esc}</div>
    <p class="oauth-sub"><strong>{app_label}</strong> wants an API key for your ModelRelay account so it can list live models and run chat completions through <code>{relay_esc}</code>.</p>

    <ul class="oauth-perms">
      <li>List live models advertised by your connected GPU workers (<code>GET /v1/models</code>)</li>
      <li>Send inference requests to <code>{relay_esc}</code> (Chat, Responses, Messages)</li>
      <li>Never receives your ModelRelay password; you can revoke keys anytime in your Dashboard</li>
    </ul>

    <form method="POST" action="/oauth/authorize">
      {csrf_field}
      <input type="hidden" name="callback_url" value="{callback_esc}">
      <input type="hidden" name="code_challenge" value="{challenge_esc}">
      <input type="hidden" name="code_challenge_method" value="{method_esc}">
      <input type="hidden" name="state" value="{state_esc}">
      <input type="hidden" name="key_label" value="{app_label}">
      {key_picker}
      <div class="oauth-actions">
        {approve_btn}
        <button type="submit" name="decision" value="deny" class="btn btn-secondary">Cancel</button>
      </div>
    </form>

    <div class="oauth-foot">
      <span>Signed in as <strong>{email_esc}</strong></span>
      <a href="/dashboard">Manage keys &rarr;</a>
    </div>
  </div>
</div>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_matches_rfc7636_appendix_b_vector() {
        // RFC 7636 Appendix B test vector:
        // code_verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
        // code_challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let expected = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert_eq!(compute_s256_challenge(verifier), expected);
        assert!(verify_pkce_s256(verifier, expected));
        assert!(!verify_pkce_s256(
            "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjX0",
            expected
        ));
    }

    #[test]
    fn verify_pkce_rejects_short_or_invalid_verifiers() {
        let short = "too-short";
        let ch = compute_s256_challenge(short);
        assert!(!verify_pkce_s256(short, &ch));
        let bad = "a".repeat(42) + "!";
        assert!(!verify_pkce_s256(&bad, &compute_s256_challenge(&bad)));
    }

    #[test]
    fn callback_url_validation_enforces_https_or_loopback() {
        assert!(validate_callback_url("https://chat.neuralcolumn.com/").is_ok());
        assert!(validate_callback_url("http://localhost:3000/callback").is_ok());
        assert!(validate_callback_url("http://127.0.0.1:8787/").is_ok());
        assert!(validate_callback_url("http://[::1]:8787/").is_ok());

        assert!(validate_callback_url("http://chat.neuralcolumn.com/").is_err());
        assert!(validate_callback_url("javascript:alert(1)").is_err());
        assert!(validate_callback_url("https://chat.neuralcolumn.com/#frag").is_err());
        assert!(validate_callback_url("https://user:pass@chat.neuralcolumn.com/").is_err());
        assert!(validate_callback_url("/relative/path").is_err());
    }

    #[test]
    fn authorize_params_require_s256_and_valid_challenge() {
        let challenge = compute_s256_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
        let ok = validate_authorize_params(
            Some("https://chat.neuralcolumn.com/"),
            Some(&challenge),
            Some("S256"),
            Some("mr.abc123"),
            Some("NeuralColumn Chat"),
            None,
        )
        .expect("valid params");
        assert_eq!(ok.key_label, "NeuralColumn Chat");
        assert_eq!(ok.state.as_deref(), Some("mr.abc123"));

        assert!(
            validate_authorize_params(
                Some("https://chat.neuralcolumn.com/"),
                Some(&challenge),
                Some("plain"),
                None,
                None,
                None,
            )
            .is_err()
        );
        assert!(
            validate_authorize_params(
                Some("https://chat.neuralcolumn.com/"),
                Some("short"),
                Some("S256"),
                None,
                None,
                None,
            )
            .is_err()
        );
    }

    #[test]
    fn callback_redirect_preserves_existing_query_and_encodes_state() {
        let base = Url::parse("https://chat.neuralcolumn.com/?tab=models").unwrap();
        let out = build_callback_redirect(&base, &[("code", "mrc_abc")], Some("mr.xyz+1"));
        assert_eq!(
            out,
            "https://chat.neuralcolumn.com/?tab=models&code=mrc_abc&state=mr.xyz%2B1"
        );
    }

    #[test]
    fn form_exchange_parser_handles_standard_oauth_payload() {
        let parsed = parse_form_exchange(
            b"grant_type=authorization_code&code=mrc_123&code_verifier=dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk&redirect_uri=https%3A%2F%2Fchat.neuralcolumn.com%2F",
        )
        .expect("should parse");
        assert_eq!(parsed.grant_type.as_deref(), Some("authorization_code"));
        assert_eq!(parsed.code, "mrc_123");
        assert_eq!(
            parsed.redirect_uri.as_deref(),
            Some("https://chat.neuralcolumn.com/")
        );
    }
}
