use std::sync::Arc;

use argon2::password_hash::SaltString;
use argon2::password_hash::rand_core::OsRng;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use axum::Form;
use axum::extract::State;
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::Deserialize;
use tower_sessions::Session;

use crate::state::CloudState;

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
    Html(modelrelay_web::templates::page_shell(
        "Sign Up",
        &signup_form_html(None),
        false,
    ))
    .into_response()
}

/// POST /signup — create a new user account.
pub async fn signup_submit(
    session: Session,
    State(state): State<Arc<CloudState>>,
    Form(form): Form<SignupForm>,
) -> Response {
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
            &signup_form_html(Some("Please enter a valid email address.")),
            false,
        ))
        .into_response();
    }
    if password.len() < 8 {
        return Html(modelrelay_web::templates::page_shell(
            "Sign Up",
            &signup_form_html(Some("Password must be at least 8 characters.")),
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
        return Html(modelrelay_web::templates::page_shell(
            "Sign Up",
            &signup_form_html(Some(
                "An account with this email already exists. <a href=\"/login\">Log in instead</a>.",
            )),
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
                &signup_form_html(Some("Internal error. Please try again.")),
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
                &signup_form_html(Some("Could not create account. Please try again.")),
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
    Html(modelrelay_web::templates::page_shell(
        "Log In",
        &login_form_html(None),
        false,
    ))
    .into_response()
}

/// POST /login — verify credentials and set session.
pub async fn login_submit(
    session: Session,
    State(state): State<Arc<CloudState>>,
    Form(form): Form<LoginForm>,
) -> Response {
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
        return Html(modelrelay_web::templates::page_shell(
            "Log In",
            &login_form_html(Some("Invalid email or password.")),
            false,
        ))
        .into_response();
    };

    if !verify_password(&form.password, &stored_hash) {
        return Html(modelrelay_web::templates::page_shell(
            "Log In",
            &login_form_html(Some("Invalid email or password.")),
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
}
