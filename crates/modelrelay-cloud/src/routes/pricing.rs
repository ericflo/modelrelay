use axum::response::{Html, IntoResponse, Response};
use tower_sessions::Session;

use super::csrf;

static PRICING_HTML: &str = include_str!("../../templates/pricing.html");

/// Placeholder replaced with a CSRF hidden field at render time.
const CSRF_PLACEHOLDER: &str = "<!-- CSRF_TOKEN -->";

pub async fn page(session: Session) -> Response {
    let csrf_field = csrf::hidden_field(&session).await;
    let html = PRICING_HTML.replace(CSRF_PLACEHOLDER, &csrf_field);
    Html(html).into_response()
}
