//! Session-based CSRF token generation and validation.
//!
//! Tokens are stored in the user's session under the key `_csrf_token`.
//! POST form handlers validate the `_csrf` form field against the session value.

use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tower_sessions::Session;
use uuid::Uuid;

const CSRF_SESSION_KEY: &str = "_csrf_token";

/// Generate a fresh CSRF token, store it in the session, and return it.
/// If a token already exists in the session, return that instead.
pub async fn get_or_create_token(session: &Session) -> String {
    if let Ok(Some(existing)) = session.get::<String>(CSRF_SESSION_KEY).await {
        return existing;
    }
    let token = Uuid::new_v4().to_string();
    // Best-effort insert — if session is broken we still return a token
    let _ = session.insert(CSRF_SESSION_KEY, &token).await;
    token
}

/// Return an HTML hidden input element containing the current CSRF token.
pub async fn hidden_field(session: &Session) -> String {
    let token = get_or_create_token(session).await;
    format!(r#"<input type="hidden" name="_csrf" value="{token}">"#)
}

/// Validate a submitted CSRF token against the session value.
/// Returns `true` if valid. Consumes and rotates the token on success.
pub async fn validate(session: &Session, form_token: &str) -> bool {
    let Ok(Some(expected)) = session.get::<String>(CSRF_SESSION_KEY).await else {
        return false;
    };
    if expected != form_token {
        return false;
    }
    // Rotate the token after successful validation
    let new_token = Uuid::new_v4().to_string();
    let _ = session.insert(CSRF_SESSION_KEY, &new_token).await;
    true
}

/// Axum middleware that rejects POST requests with invalid or missing CSRF
/// tokens. Skips paths that use their own auth (e.g. `/webhook/stripe`).
pub async fn csrf_middleware(request: Request, next: Next) -> Response {
    // Only validate POST requests
    if request.method() != axum::http::Method::POST {
        return next.run(request).await;
    }

    // Skip webhook endpoint — it uses HMAC signature verification
    // Skip logout — low-risk (only clears session) and form is in shared template
    let path = request.uri().path().to_owned();
    if path.starts_with("/webhook/") || path == "/logout" {
        return next.run(request).await;
    }

    // Extract the session — if there's no session layer, let the request through
    // (the handler will fail on its own session extractor anyway).
    let Some(session) = request.extensions().get::<Session>().cloned() else {
        return next.run(request).await;
    };

    // Read the content-type to determine if this is a form submission
    let content_type = request
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !content_type.starts_with("application/x-www-form-urlencoded") {
        // Non-form POSTs (JSON API, etc.) are not subject to CSRF here
        return next.run(request).await;
    }

    // Read the body to extract the _csrf field
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

    // Reconstruct the request with the consumed body
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
}
