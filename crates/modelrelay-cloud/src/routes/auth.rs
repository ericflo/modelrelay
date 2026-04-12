use std::net::IpAddr;
use std::sync::Arc;

use argon2::password_hash::SaltString;
use argon2::password_hash::rand_core::OsRng;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use axum::Form;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
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
fn rate_limit_response() -> Response {
    let body = "\
<div class=\"card\" style=\"border-left: 3px solid #fbbf24;\">\
  <h2 style=\"display:flex;align-items:center;gap:10px;\">\
    <svg width=\"22\" height=\"22\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"#fbbf24\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><circle cx=\"12\" cy=\"12\" r=\"10\"/><polyline points=\"12 6 12 12 16 14\"/></svg>\
    Slow down\
  </h2>\
  <p>You've made too many login or sign-up attempts. Please wait <strong>15 minutes</strong> before trying again.</p>\
  <p style=\"margin-top:12px;color:#8b949e;font-size:0.9rem;\">This limit protects your account from unauthorized access attempts.</p>\
  <a href=\"/\" class=\"btn\" style=\"margin-top:20px;\">Back to Home</a>\
</div>";
    let html = modelrelay_web::templates::page_shell("Too Many Requests", body, false);
    (StatusCode::TOO_MANY_REQUESTS, Html(html)).into_response()
}

#[derive(Deserialize)]
pub struct SignupForm {
    email: String,
    password: String,
    #[allow(dead_code)]
    _csrf: Option<String>,
}

#[derive(Deserialize)]
pub struct LoginForm {
    email: String,
    password: String,
    #[allow(dead_code)]
    _csrf: Option<String>,
}

/// GET /signup — render the sign-up form.
pub async fn signup_page(session: Session) -> Response {
    // If already logged in, redirect to dashboard
    if let Ok(Some(_)) = session.get::<String>("user_id").await {
        return Redirect::to("/dashboard").into_response();
    }
    let csrf_field = csrf::hidden_field(&session).await;
    Html(modelrelay_web::templates::page_shell(
        "Sign Up",
        &signup_form_html(None, &csrf_field),
        false,
    ))
    .into_response()
}

/// POST /signup — create a new user account.
#[allow(clippy::too_many_lines)]
pub async fn signup_submit(
    headers: HeaderMap,
    session: Session,
    State(state): State<Arc<CloudState>>,
    Form(form): Form<SignupForm>,
) -> Response {
    let ip = client_ip(&headers);
    if state.rate_limiter.is_limited(ip) {
        return rate_limit_response();
    }

    let csrf_field = csrf::hidden_field(&session).await;

    let Some(ref pool) = state.db else {
        return Html(modelrelay_web::templates::page_shell(
            "Sign Up",
            "<div class=\"card\"><h2>Error</h2><p>Database not available.</p></div>",
            false,
        ))
        .into_response();
    };

    let email = form.email.trim().to_lowercase();
    let password = form.password.clone();

    // Basic validation
    if email.is_empty() || !email.contains('@') {
        return Html(modelrelay_web::templates::page_shell(
            "Sign Up",
            &signup_form_html(Some("Please enter a valid email address."), &csrf_field),
            false,
        ))
        .into_response();
    }
    if password.len() < 8 {
        return Html(modelrelay_web::templates::page_shell(
            "Sign Up",
            &signup_form_html(Some("Password must be at least 8 characters."), &csrf_field),
            false,
        ))
        .into_response();
    }

    // Check if user already exists with a password
    let existing: Option<(uuid::Uuid, Option<String>)> =
        sqlx::query_as("SELECT id, password_hash FROM users WHERE email = $1")
            .bind(&email)
            .fetch_optional(pool)
            .await
            .unwrap_or(None);

    if let Some((_, Some(_))) = existing {
        state.rate_limiter.record_attempt(ip);
        return Html(modelrelay_web::templates::page_shell(
            "Sign Up",
            &signup_form_html(
                Some("An account with this email already exists. <a href=\"/login\">Log in instead</a>."),
                &csrf_field,
            ),
            false,
        ))
        .into_response();
    }

    // Hash password
    let password_hash = match hash_password(&password) {
        Ok(h) => h,
        Err(e) => {
            tracing::error!("password hash error: {e}");
            return Html(modelrelay_web::templates::page_shell(
                "Sign Up",
                &signup_form_html(Some("Internal error. Please try again."), &csrf_field),
                false,
            ))
            .into_response();
        }
    };

    // Insert or update user (user may already exist from Stripe checkout without a password)
    let user_id: Result<uuid::Uuid, _> = sqlx::query_scalar(
        "INSERT INTO users (email, password_hash) VALUES ($1, $2) \
         ON CONFLICT (email) DO UPDATE SET password_hash = EXCLUDED.password_hash \
         WHERE users.password_hash IS NULL \
         RETURNING id",
    )
    .bind(&email)
    .bind(&password_hash)
    .fetch_one(pool)
    .await;

    let user_id = match user_id {
        Ok(id) => id,
        Err(e) => {
            tracing::error!("user insert error: {e}");
            return Html(modelrelay_web::templates::page_shell(
                "Sign Up",
                &signup_form_html(
                    Some("Could not create account. Please try again."),
                    &csrf_field,
                ),
                false,
            ))
            .into_response();
        }
    };

    // Auto-promote admin users
    if state.admin_emails.contains(&email) {
        if let Err(e) = sqlx::query("UPDATE users SET is_admin = true WHERE id = $1")
            .bind(user_id)
            .execute(pool)
            .await
        {
            tracing::error!("admin auto-promote error: {e}");
        } else {
            tracing::info!(email = %email, "auto-promoted new signup to admin");
        }
    }

    // Set session
    if let Err(e) = session.insert("user_id", user_id.to_string()).await {
        tracing::error!("session insert error: {e}");
    }

    Redirect::to("/dashboard").into_response()
}

/// GET /login — render the login form.
pub async fn login_page(session: Session) -> Response {
    if let Ok(Some(_)) = session.get::<String>("user_id").await {
        return Redirect::to("/dashboard").into_response();
    }
    let csrf_field = csrf::hidden_field(&session).await;
    Html(modelrelay_web::templates::page_shell(
        "Log In",
        &login_form_html(None, &csrf_field),
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
        return rate_limit_response();
    }

    let csrf_field = csrf::hidden_field(&session).await;

    let Some(ref pool) = state.db else {
        return Html(modelrelay_web::templates::page_shell(
            "Log In",
            "<div class=\"card\"><h2>Error</h2><p>Database not available.</p></div>",
            false,
        ))
        .into_response();
    };

    let email = form.email.trim().to_lowercase();

    let row: Option<(uuid::Uuid, Option<String>)> =
        sqlx::query_as("SELECT id, password_hash FROM users WHERE email = $1")
            .bind(&email)
            .fetch_optional(pool)
            .await
            .unwrap_or(None);

    let Some((user_id, Some(stored_hash))) = row else {
        state.rate_limiter.record_attempt(ip);
        return Html(modelrelay_web::templates::page_shell(
            "Log In",
            &login_form_html(Some("Invalid email or password."), &csrf_field),
            false,
        ))
        .into_response();
    };

    if !verify_password(&form.password, &stored_hash) {
        state.rate_limiter.record_attempt(ip);
        return Html(modelrelay_web::templates::page_shell(
            "Log In",
            &login_form_html(Some("Invalid email or password."), &csrf_field),
            false,
        ))
        .into_response();
    }

    if let Err(e) = session.insert("user_id", user_id.to_string()).await {
        tracing::error!("session insert error: {e}");
    }

    Redirect::to("/dashboard").into_response()
}

/// POST /logout — clear session and redirect to home.
pub async fn logout(session: Session) -> Response {
    if let Err(e) = session.flush().await {
        tracing::error!("session flush error: {e}");
    }
    Redirect::to("/").into_response()
}

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

fn signup_form_html(error: Option<&str>, csrf_field: &str) -> String {
    let error_html = error
        .map(|e| format!("<div class=\"error-msg\">{e}</div>"))
        .unwrap_or_default();

    format!(
        r#"<div class="auth-split">
  <div class="auth-value">
    <h1 class="auth-value-headline">Run any AI model.<br>One unified API.</h1>
    <p class="auth-value-sub">ModelRelay routes your inference requests to the fastest available GPU — your own hardware, cloud, or both.</p>
    <ul class="auth-benefits">
      <li><svg width="18" height="18" viewBox="0 0 18 18" fill="none"><circle cx="9" cy="9" r="9" fill='#7c3aed' opacity="0.15"/><path d="M5.5 9.5l2 2 5-5" stroke='#a78bfa' stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>OpenAI-compatible — drop-in replacement</li>
      <li><svg width="18" height="18" viewBox="0 0 18 18" fill="none"><circle cx="9" cy="9" r="9" fill='#7c3aed' opacity="0.15"/><path d="M5.5 9.5l2 2 5-5" stroke='#a78bfa' stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>Connect your own GPUs or use cloud workers</li>
      <li><svg width="18" height="18" viewBox="0 0 18 18" fill="none"><circle cx="9" cy="9" r="9" fill='#7c3aed' opacity="0.15"/><path d="M5.5 9.5l2 2 5-5" stroke='#a78bfa' stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>Automatic load balancing and failover</li>
    </ul>
    <p class="auth-trust">Trusted by developers running production AI workloads.</p>
  </div>
  <div class="auth-form-panel">
    <div class="auth-form-inner">
      <h2>Create your account</h2>
      <p class="auth-form-hint">Start routing AI requests in minutes.</p>
      {error_html}
      <form method="POST" action="/signup" class="auth-form" id="signup-form">
        {csrf_field}
        <div class="form-group">
          <label for="email">Email</label>
          <input type="email" id="email" name="email" required placeholder="you@example.com" autofocus>
        </div>
        <div class="form-group">
          <label for="password">Password</label>
          <div class="password-wrapper">
            <input type="password" id="password" name="password" required minlength="8" placeholder="At least 8 characters">
            <button type="button" class="password-toggle" aria-label="Show password" onclick="togglePassword(this)">
              <svg class="eye-open" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke='#8b949e' stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/></svg>
              <svg class="eye-closed" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke='#8b949e' stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="display:none"><path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24"/><line x1="1" y1="1" x2="23" y2="23"/></svg>
            </button>
          </div>
        </div>
        <button type="submit" class="btn auth-submit">Create Account</button>
      </form>
      <p class="auth-no-cc">No credit card required</p>
      <p class="auth-switch">Already have an account? <a href="/login">Log in</a></p>
    </div>
  </div>
</div>
<script>
function togglePassword(btn){{var i=btn.parentElement.querySelector('input');var o=btn.querySelector('.eye-open');var c=btn.querySelector('.eye-closed');if(i.type==='password'){{i.type='text';o.style.display='none';c.style.display='';btn.setAttribute('aria-label','Hide password')}}else{{i.type='password';o.style.display='';c.style.display='none';btn.setAttribute('aria-label','Show password')}}}}
document.querySelectorAll('.auth-form').forEach(function(f){{f.addEventListener('submit',function(){{var b=f.querySelector('.auth-submit');if(b){{b.disabled=true;b.classList.add('loading');b.innerHTML='<span class="spinner"></span>'+b.textContent}}}});}});
</script>"#
    )
}

fn login_form_html(error: Option<&str>, csrf_field: &str) -> String {
    let error_html = error
        .map(|e| format!("<div class=\"error-msg\">{e}</div>"))
        .unwrap_or_default();

    format!(
        r#"<div class="auth-split">
  <div class="auth-value">
    <h1 class="auth-value-headline">Welcome back.</h1>
    <p class="auth-value-sub">Your AI infrastructure is ready and waiting. Log in to manage your workers, monitor traffic, and grab your API keys.</p>
    <ul class="auth-benefits">
      <li><svg width="18" height="18" viewBox="0 0 18 18" fill="none"><circle cx="9" cy="9" r="9" fill='#7c3aed' opacity="0.15"/><path d="M5.5 9.5l2 2 5-5" stroke='#a78bfa' stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>Real-time dashboard and usage analytics</li>
      <li><svg width="18" height="18" viewBox="0 0 18 18" fill="none"><circle cx="9" cy="9" r="9" fill='#7c3aed' opacity="0.15"/><path d="M5.5 9.5l2 2 5-5" stroke='#a78bfa' stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>Manage API keys and worker connections</li>
      <li><svg width="18" height="18" viewBox="0 0 18 18" fill="none"><circle cx="9" cy="9" r="9" fill='#7c3aed' opacity="0.15"/><path d="M5.5 9.5l2 2 5-5" stroke='#a78bfa' stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>Scale from prototype to production</li>
    </ul>
  </div>
  <div class="auth-form-panel">
    <div class="auth-form-inner">
      <h2>Log in to ModelRelay</h2>
      {error_html}
      <form method="POST" action="/login" class="auth-form" id="login-form">
        {csrf_field}
        <div class="form-group">
          <label for="email">Email</label>
          <input type="email" id="email" name="email" required placeholder="you@example.com" autofocus>
        </div>
        <div class="form-group">
          <label for="password">Password</label>
          <div class="password-wrapper">
            <input type="password" id="password" name="password" required placeholder="Your password">
            <button type="button" class="password-toggle" aria-label="Show password" onclick="togglePassword(this)">
              <svg class="eye-open" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke='#8b949e' stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/></svg>
              <svg class="eye-closed" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke='#8b949e' stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="display:none"><path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24"/><line x1="1" y1="1" x2="23" y2="23"/></svg>
            </button>
          </div>
        </div>
        <button type="submit" class="btn auth-submit">Log In</button>
      </form>
      <p class="auth-switch">Don't have an account? <a href="/signup">Sign up free</a></p>
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
}
