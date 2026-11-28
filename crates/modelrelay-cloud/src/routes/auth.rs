use std::net::IpAddr;
use std::sync::Arc;

#[cfg(test)]
use argon2::PasswordHasher;
#[cfg(test)]
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordVerifier};
use axum::Form;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
#[cfg(test)]
use rand_core::OsRng;
use serde::Deserialize;
use tower_sessions::Session;

use crate::state::CloudState;

use super::csrf;

/// Extract the client IP from the `X-Forwarded-For` header (first entry),
/// falling back to 127.0.0.1 if unavailable.
fn client_ip(headers: &HeaderMap) -> IpAddr {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .and_then(|s| s.trim().parse::<IpAddr>().ok())
        .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST))
}

/// Render a 429 Too Many Requests page.
///
/// `path` is the canonical URL path of the request that triggered the rate limit
/// (currently `/login`), used for per-page og:url and canonical.
fn rate_limit_response(path: &str) -> Response {
    let body = "\
<div class=\"card\" style=\"border-left: 3px solid var(--warning);\">\
  <h2 style=\"display:flex;align-items:center;gap:10px;\">\
    <svg width=\"22\" height=\"22\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"var(--warning)\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><circle cx=\"12\" cy=\"12\" r=\"10\"/><polyline points=\"12 6 12 12 16 14\"/></svg>\
    Slow down\
  </h2>\
  <p>You've made too many login attempts. Please wait <strong>15 minutes</strong> before trying again.</p>\
  <p style=\"margin-top:12px;color:var(--text-secondary);font-size:0.9rem;\">This limit protects your account from unauthorized access attempts.</p>\
  <a href=\"/\" class=\"btn\" style=\"margin-top:20px;\">Back to Home</a>\
</div>";
    let html = modelrelay_web::templates::page_shell("Too Many Requests", path, body, false);
    (StatusCode::TOO_MANY_REQUESTS, Html(html)).into_response()
}

#[derive(Deserialize)]
pub struct LoginQuery {
    next: Option<String>,
}

#[derive(Deserialize)]
pub struct LoginForm {
    email: String,
    password: String,
    #[serde(default)]
    next: Option<String>,
    #[allow(dead_code)]
    _csrf: Option<String>,
}

/// Allow only safe same-origin relative redirect targets (e.g. `/oauth/authorize?...`).
fn safe_next_path(raw: Option<&str>) -> Option<&str> {
    let s = raw?.trim();
    if s.is_empty()
        || s.len() > 2048
        || !s.starts_with('/')
        || s.starts_with("//")
        || s.contains('\r')
        || s.contains('\n')
        || s.contains('\\')
    {
        return None;
    }
    Some(s)
}

/// GET /login — render the login form.
pub async fn login_page(session: Session, Query(query): Query<LoginQuery>) -> Response {
    let next = safe_next_path(query.next.as_deref());
    if let Ok(Some(_)) = session.get::<String>("user_id").await {
        return Redirect::to(next.unwrap_or("/dashboard")).into_response();
    }
    let csrf_field = csrf::hidden_field(&session).await;
    Html(modelrelay_web::templates::page_shell(
        "Log In",
        "/login",
        &login_form_html(None, &csrf_field, next),
        false,
    ))
    .into_response()
}

/// POST /login — verify credentials and set session.
pub async fn login_submit(
    headers: HeaderMap,
    session: Session,
    State(state): State<Arc<CloudState>>,
    Form(form): Form<LoginForm>,
) -> Response {
    let ip = client_ip(&headers);
    if state.rate_limiter.is_limited(ip) {
        return rate_limit_response("/login");
    }

    let csrf_field = csrf::hidden_field(&session).await;
    let next = safe_next_path(form.next.as_deref());

    let Some(ref pool) = state.db else {
        return Html(modelrelay_web::templates::page_shell(
            "Log In",
            "/login",
            "<div class=\"card\"><h2>Error</h2><p>Database not available.</p></div>",
            false,
        ))
        .into_response();
    };

    let email = form.email.trim().to_lowercase();

    let row: Option<(uuid::Uuid, Option<String>)> =
        match sqlx::query_as("SELECT id, password_hash FROM users WHERE lower(email) = $1")
            .bind(&email)
            .fetch_optional(pool)
            .await
        {
            Ok(row) => row,
            Err(e) => {
                tracing::error!("login database query failed: {e}");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Html(modelrelay_web::templates::page_shell(
                        "Log In",
                        "/login",
                        &login_form_html(
                            Some("Service temporarily unavailable. Please try again in a moment."),
                            &csrf_field,
                            next,
                        ),
                        false,
                    )),
                )
                    .into_response();
            }
        };

    let Some((user_id, Some(stored_hash))) = row else {
        state.rate_limiter.record_attempt(ip);
        return Html(modelrelay_web::templates::page_shell(
            "Log In",
            "/login",
            &login_form_html(Some("Invalid email or password."), &csrf_field, next),
            false,
        ))
        .into_response();
    };

    if !verify_password(&form.password, &stored_hash) {
        state.rate_limiter.record_attempt(ip);
        return Html(modelrelay_web::templates::page_shell(
            "Log In",
            "/login",
            &login_form_html(Some("Invalid email or password."), &csrf_field, next),
            false,
        ))
        .into_response();
    }

    if let Err(e) = session.cycle_id().await {
        tracing::warn!("session cycle_id error: {e}");
    }
    if let Err(e) = session.insert("user_id", user_id.to_string()).await {
        tracing::error!("session insert error: {e}");
    }

    Redirect::to(next.unwrap_or("/dashboard")).into_response()
}

/// POST /logout — clear session and redirect to home.
pub async fn logout(session: Session) -> Response {
    if let Err(e) = session.flush().await {
        tracing::error!("session flush error: {e}");
    }
    Redirect::to("/").into_response()
}

#[cfg(test)]
fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("argon2 hash error: {e}"))
}

fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

fn login_form_html(error: Option<&str>, csrf_field: &str, next: Option<&str>) -> String {
    let error_html = error
        .map(|e| format!("<div class=\"error-msg\">{e}</div>"))
        .unwrap_or_default();
    let next_field = next
        .map(|n| {
            format!(
                "<input type=\"hidden\" name=\"next\" value=\"{}\">",
                modelrelay_web::templates::html_escape(n)
            )
        })
        .unwrap_or_default();

    format!(
        r#"<div class="auth-split">
  <div class="auth-value">
    <h1 class="auth-value-headline">Welcome back.</h1>
    <p class="auth-value-sub">Log in to copy your API key for Pi <code>auth.json</code>, inspect connected GPU workers, and check live models on <code>api.modelrelay.io</code>.</p>
    <ul class="auth-benefits">
      <li><svg width="18" height="18" viewBox="0 0 18 18" fill="none"><circle cx="9" cy="9" r="9" fill="var(--accent)" opacity="0.18"/><path d="M5.5 9.5l2 2 5-5" stroke="var(--accent-bright)" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg><span>One-click API key &amp; Pi <code>auth.json</code> copy</span></li>
      <li><svg width="18" height="18" viewBox="0 0 18 18" fill="none"><circle cx="9" cy="9" r="9" fill="var(--accent)" opacity="0.18"/><path d="M5.5 9.5l2 2 5-5" stroke="var(--accent-bright)" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg><span>Live worker telemetry, load bars, and model lists</span></li>
      <li><svg width="18" height="18" viewBox="0 0 18 18" fill="none"><circle cx="9" cy="9" r="9" fill="var(--accent)" opacity="0.18"/><path d="M5.5 9.5l2 2 5-5" stroke="var(--accent-bright)" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg><span>Pre-filled <code>curl</code> commands and Pi provider setup</span></li>
    </ul>
  </div>
  <div class="auth-form-panel">
    <div class="auth-form-inner">
      <h2>Log in to ModelRelay</h2>
      {error_html}
      <form method="POST" action="/login" class="auth-form" id="login-form">
        {csrf_field}
        {next_field}
        <div class="form-group">
          <label for="email">Email</label>
          <input type="email" id="email" name="email" required placeholder="you@example.com" autofocus>
        </div>
        <div class="form-group">
          <label for="password">Password</label>
          <div class="password-wrapper">
            <input type="password" id="password" name="password" required placeholder="Your password">
            <button type="button" class="password-toggle" aria-label="Show password" onclick="togglePassword(this)">
              <svg class="eye-open" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="var(--text-muted)" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/></svg>
              <svg class="eye-closed" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="var(--text-muted)" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="display:none"><path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24"/><line x1="1" y1="1" x2="23" y2="23"/></svg>
            </button>
          </div>
        </div>
        <button type="submit" class="btn auth-submit">Log In</button>
      </form>
      <p class="auth-switch">New hosted signups are closed. <a href="/docs/self-hosting">Self-host free (MIT) &rarr;</a></p>
    </div>
  </div>
</div>
<script>
function togglePassword(btn){{var i=btn.parentElement.querySelector('input');var o=btn.querySelector('.eye-open');var c=btn.querySelector('.eye-closed');if(i.type==='password'){{i.type='text';o.style.display='none';c.style.display='';btn.setAttribute('aria-label','Hide password')}}else{{i.type='password';o.style.display='';c.style.display='none';btn.setAttribute('aria-label','Show password')}}}}
document.querySelectorAll('.auth-form').forEach(function(f){{f.addEventListener('submit',function(){{var b=f.querySelector('.auth-submit');if(b){{b.disabled=true;b.classList.add('loading');b.innerHTML='<span class="spinner"></span>'+b.textContent}}}});}});
</script>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify_roundtrip() {
        let password = "correct-horse-battery-staple";
        let hashed = hash_password(password).expect("hash should succeed");
        assert!(verify_password(password, &hashed));
    }

    #[test]
    fn wrong_password_rejected() {
        let hashed = hash_password("real-password").expect("hash should succeed");
        assert!(!verify_password("wrong-password", &hashed));
    }

    #[test]
    fn different_hashes_for_same_password() {
        let h1 = hash_password("same").expect("hash should succeed");
        let h2 = hash_password("same").expect("hash should succeed");
        assert_ne!(h1, h2);
        assert!(verify_password("same", &h1));
        assert!(verify_password("same", &h2));
    }

    #[test]
    fn verify_returns_false_for_garbage_hash() {
        assert!(!verify_password("anything", "not-a-valid-hash"));
    }

    #[test]
    fn empty_password_hashes_and_verifies() {
        let hashed = hash_password("").expect("hash should succeed");
        assert!(verify_password("", &hashed));
        assert!(!verify_password("notempty", &hashed));
    }

    #[test]
    fn client_ip_from_x_forwarded_for() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "1.2.3.4, 5.6.7.8".parse().unwrap());
        assert_eq!(client_ip(&headers), "1.2.3.4".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn client_ip_single_value() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "10.0.0.1".parse().unwrap());
        assert_eq!(client_ip(&headers), "10.0.0.1".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn client_ip_missing_header_returns_localhost() {
        let headers = HeaderMap::new();
        assert_eq!(client_ip(&headers), "127.0.0.1".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn client_ip_invalid_value_returns_localhost() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "not-an-ip".parse().unwrap());
        assert_eq!(client_ip(&headers), "127.0.0.1".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn safe_next_path_rejects_open_redirects() {
        assert_eq!(
            safe_next_path(Some("/oauth/authorize?state=mr.1")),
            Some("/oauth/authorize?state=mr.1")
        );
        assert_eq!(safe_next_path(Some("https://evil.example/")), None);
        assert_eq!(safe_next_path(Some("//evil.example/")), None);
        assert_eq!(safe_next_path(Some("/\\evil.example")), None);
        assert_eq!(safe_next_path(Some("")), None);
        assert_eq!(safe_next_path(None), None);
    }
}
