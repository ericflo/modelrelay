use std::sync::Arc;

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use axum::extract::State;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use serde::Deserialize;
use tower_sessions::Session;

use crate::state::AppState;

#[derive(Deserialize)]
pub struct SignupForm {
    email: String,
    password: String,
}

#[derive(Deserialize)]
pub struct LoginForm {
    email: String,
    password: String,
}

/// GET /signup — render the sign-up form.
pub async fn signup_page(session: Session) -> Response {
    // If already logged in, redirect to dashboard
    if let Ok(Some(_)) = session.get::<String>("user_id").await {
        return Redirect::to("/dashboard").into_response();
    }
    Html(page_shell("Sign Up", &signup_form_html(None))).into_response()
}

/// POST /signup — create a new user account.
pub async fn signup_submit(
    session: Session,
    State(state): State<Arc<AppState>>,
    Form(form): Form<SignupForm>,
) -> Response {
    let Some(ref pool) = state.db else {
        return Html(page_shell("Sign Up", "<div class=\"card\"><h2>Error</h2><p>Database not available.</p></div>")).into_response();
    };

    let email = form.email.trim().to_lowercase();
    let password = form.password.clone();

    // Basic validation
    if email.is_empty() || !email.contains('@') {
        return Html(page_shell(
            "Sign Up",
            &signup_form_html(Some("Please enter a valid email address.")),
        ))
        .into_response();
    }
    if password.len() < 8 {
        return Html(page_shell(
            "Sign Up",
            &signup_form_html(Some("Password must be at least 8 characters.")),
        ))
        .into_response();
    }

    // Check if user already exists with a password
    let existing: Option<(uuid::Uuid, Option<String>)> = sqlx::query_as(
        "SELECT id, password_hash FROM users WHERE email = $1",
    )
    .bind(&email)
    .fetch_optional(pool)
    .await
    .unwrap_or(None);

    if let Some((_, Some(_))) = existing {
        return Html(page_shell(
            "Sign Up",
            &signup_form_html(Some(
                "An account with this email already exists. <a href=\"/login\">Log in instead</a>.",
            )),
        ))
        .into_response();
    }

    // Hash password
    let password_hash = match hash_password(&password) {
        Ok(h) => h,
        Err(e) => {
            tracing::error!("password hash error: {e}");
            return Html(page_shell(
                "Sign Up",
                &signup_form_html(Some("Internal error. Please try again.")),
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
            return Html(page_shell(
                "Sign Up",
                &signup_form_html(Some("Could not create account. Please try again.")),
            ))
            .into_response();
        }
    };

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
    Html(page_shell("Log In", &login_form_html(None))).into_response()
}

/// POST /login — verify credentials and set session.
pub async fn login_submit(
    session: Session,
    State(state): State<Arc<AppState>>,
    Form(form): Form<LoginForm>,
) -> Response {
    let Some(ref pool) = state.db else {
        return Html(page_shell("Log In", "<div class=\"card\"><h2>Error</h2><p>Database not available.</p></div>")).into_response();
    };

    let email = form.email.trim().to_lowercase();

    let row: Option<(uuid::Uuid, Option<String>)> = sqlx::query_as(
        "SELECT id, password_hash FROM users WHERE email = $1",
    )
    .bind(&email)
    .fetch_optional(pool)
    .await
    .unwrap_or(None);

    let Some((user_id, Some(stored_hash))) = row else {
        return Html(page_shell(
            "Log In",
            &login_form_html(Some("Invalid email or password.")),
        ))
        .into_response();
    };

    if !verify_password(&form.password, &stored_hash) {
        return Html(page_shell(
            "Log In",
            &login_form_html(Some("Invalid email or password.")),
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

fn signup_form_html(error: Option<&str>) -> String {
    let error_html = error
        .map(|e| format!("<div class=\"error-msg\">{e}</div>"))
        .unwrap_or_default();

    format!(
        r#"<div class="card">
  <h2>Create an Account</h2>
  {error_html}
  <form method="POST" action="/signup" class="auth-form">
    <div class="form-group">
      <label for="email">Email</label>
      <input type="email" id="email" name="email" required placeholder="you@example.com">
    </div>
    <div class="form-group">
      <label for="password">Password</label>
      <input type="password" id="password" name="password" required minlength="8" placeholder="At least 8 characters">
    </div>
    <button type="submit" class="btn">Sign Up</button>
  </form>
  <p class="auth-switch">Already have an account? <a href="/login">Log in</a></p>
</div>"#
    )
}

fn login_form_html(error: Option<&str>) -> String {
    let error_html = error
        .map(|e| format!("<div class=\"error-msg\">{e}</div>"))
        .unwrap_or_default();

    format!(
        r#"<div class="card">
  <h2>Log In</h2>
  {error_html}
  <form method="POST" action="/login" class="auth-form">
    <div class="form-group">
      <label for="email">Email</label>
      <input type="email" id="email" name="email" required placeholder="you@example.com">
    </div>
    <div class="form-group">
      <label for="password">Password</label>
      <input type="password" id="password" name="password" required placeholder="Your password">
    </div>
    <button type="submit" class="btn">Log In</button>
  </form>
  <p class="auth-switch">Don't have an account? <a href="/signup">Sign up</a></p>
</div>"#
    )
}

fn page_shell(title: &str, body_content: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{title} — ModelRelay</title>
  <style>
    *, *::before, *::after {{ box-sizing: border-box; margin: 0; padding: 0; }}
    body {{
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
      background: #0d1117; color: #e6edf3; line-height: 1.6;
    }}
    a {{ color: #7c3aed; text-decoration: none; }}
    a:hover {{ text-decoration: underline; }}
    .container {{ max-width: 900px; margin: 0 auto; padding: 0 24px; }}

    nav {{ padding: 20px 0; border-bottom: 1px solid #21262d; }}
    nav .container {{ display: flex; justify-content: space-between; align-items: center; }}
    .logo {{ font-size: 1.25rem; font-weight: 700; color: #e6edf3; }}
    .logo span {{ color: #7c3aed; }}
    .nav-links a {{ color: #8b949e; font-size: 0.9rem; margin-left: 16px; }}
    .nav-links a:hover {{ color: #e6edf3; }}

    .content {{ padding: 60px 0; }}
    .content h1 {{ font-size: 2rem; margin-bottom: 24px; }}

    .card {{
      background: #161b22; border: 1px solid #21262d; border-radius: 12px;
      padding: 32px; margin-bottom: 24px; max-width: 480px; margin-left: auto; margin-right: auto;
    }}
    .card h2 {{ font-size: 1.2rem; margin-bottom: 16px; color: #e6edf3; }}

    .auth-form .form-group {{ margin-bottom: 16px; }}
    .auth-form label {{ display: block; font-size: 0.9rem; color: #8b949e; margin-bottom: 4px; }}
    .auth-form input {{
      width: 100%; padding: 10px 12px; background: #0d1117; border: 1px solid #30363d;
      border-radius: 8px; color: #e6edf3; font-size: 0.95rem;
    }}
    .auth-form input:focus {{ outline: none; border-color: #7c3aed; }}

    .btn {{
      display: inline-block; padding: 10px 20px; background: #7c3aed; color: #fff;
      border: none; border-radius: 8px; font-size: 0.9rem; font-weight: 600;
      cursor: pointer; text-decoration: none; width: 100%;
    }}
    .btn:hover {{ background: #6d28d9; text-decoration: none; }}

    .auth-switch {{ margin-top: 16px; text-align: center; color: #8b949e; font-size: 0.9rem; }}

    .error-msg {{
      background: #3b1219; border: 1px solid #7f1d1d; border-radius: 8px;
      padding: 10px 14px; margin-bottom: 16px; color: #f87171; font-size: 0.9rem;
    }}

    footer {{ padding: 40px 0; border-top: 1px solid #21262d; text-align: center; color: #484f58; font-size: 0.85rem; }}
    footer a {{ color: #8b949e; }}
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
      <h1>{title}</h1>
      {body_content}
    </div>
  </section>

  <footer>
    <div class="container">
      &copy; 2026 ModelRelay &middot; <a href="https://github.com/ericflo/modelrelay">GitHub</a>
    </div>
  </footer>
</body>
</html>"#
    )
}
