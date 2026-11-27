//! Session-based CSRF token generation and validation.
//!
//! Tokens are stored in the user's session under the key `_csrf_token` and
//! remain stable across concurrent tabs within a session (rotated when a new
//! session is created on login). ALL non-exempt POST requests require a valid
//! CSRF token either in the `X-CSRF-Token` header or in an
//! `application/x-www-form-urlencoded` `_csrf` form field, closing
//! `text/plain` and `multipart/form-data` Content-Type bypasses.

use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tower_sessions::Session;
use uuid::Uuid;

const CSRF_SESSION_KEY: &str = "_csrf_token";
const CSRF_HEADER: &str = "x-csrf-token";

/// Generate a fresh CSRF token, store it in the session, and return it.
/// If a token already exists in the session, return that instead.
pub async fn get_or_create_token(session: &Session) -> String {
    if let Ok(Some(existing)) = session.get::<String>(CSRF_SESSION_KEY).await {
        return existing;
    }
    let token = Uuid::new_v4().to_string();
    let _ = session.insert(CSRF_SESSION_KEY, &token).await;
    token
}

/// Return an HTML hidden input element containing the current CSRF token.
pub async fn hidden_field(session: &Session) -> String {
    let token = get_or_create_token(session).await;
    format!(r#"<input type="hidden" name="_csrf" value="{token}">"#)
}

/// Constant-time comparison to avoid timing side-channels on CSRF tokens.
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0_u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Validate a submitted CSRF token against the session value.
/// Returns `true` if valid. Keeps the token stable across concurrent tabs in
/// the same session so opening multiple dashboard pages does not break forms.
pub async fn validate(session: &Session, form_token: &str) -> bool {
    let Ok(Some(expected)) = session.get::<String>(CSRF_SESSION_KEY).await else {
        return false;
    };
    constant_time_eq(&expected, form_token)
}

/// Axum middleware that rejects ALL non-exempt POST requests with invalid or
/// missing CSRF tokens regardless of `Content-Type`. Skips paths that use their
/// own authentication (e.g. `/webhook/stripe`) or logout.
pub async fn csrf_middleware(request: Request, next: Next) -> Response {
    if request.method() != axum::http::Method::POST {
        return next.run(request).await;
    }

    let path = request.uri().path().to_owned();
    if path.starts_with("/webhook/")
        || path == "/logout"
        || path == "/api/v1/auth/keys"
        || path == "/oauth/token"
    {
        return next.run(request).await;
    }

    // Extract the session — if there's no session layer, let the request through
    // (the handler will fail on its own session extractor anyway).
    let Some(session) = request.extensions().get::<Session>().cloned() else {
        return next.run(request).await;
    };

    // 1. Accept X-CSRF-Token header for any Content-Type (JSON, fetch, etc.).
    if let Some(header_token) = request
        .headers()
        .get(CSRF_HEADER)
        .and_then(|v| v.to_str().ok())
    {
        if validate(&session, header_token.trim()).await {
            return next.run(request).await;
        }
        return (StatusCode::FORBIDDEN, "CSRF token invalid").into_response();
    }

    // 2. Otherwise require `application/x-www-form-urlencoded` with `_csrf` field.
    // Rejects `text/plain`, `multipart/form-data`, and missing `Content-Type`
    // browser cross-origin form POSTs that lack `X-CSRF-Token`.
    let content_type = request
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();

    if media_type != "application/x-www-form-urlencoded" {
        return (StatusCode::FORBIDDEN, "CSRF token missing").into_response();
    }

    let (parts, body) = request.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, 1_048_576).await else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    let body_str = String::from_utf8_lossy(&bytes);
    let csrf_value = body_str
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == "_csrf")
        .map(|(_, v)| v.to_owned());

    let Some(csrf_token) = csrf_value else {
        return (StatusCode::FORBIDDEN, "CSRF token missing").into_response();
    };

    if !validate(&session, &csrf_token).await {
        return (StatusCode::FORBIDDEN, "CSRF token invalid").into_response();
    }

    let request = Request::from_parts(parts, Body::from(bytes));
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csrf_session_key_is_stable() {
        assert_eq!(CSRF_SESSION_KEY, "_csrf_token");
    }

    #[test]
    fn constant_time_eq_compares_correctly() {
        assert!(constant_time_eq("abc-123", "abc-123"));
        assert!(!constant_time_eq("abc-123", "abc-124"));
        assert!(!constant_time_eq("abc", "abcd"));
    }
}
