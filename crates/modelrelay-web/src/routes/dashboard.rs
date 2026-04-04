use axum::response::Html;

static DASHBOARD_HTML: &str = include_str!("../../templates/dashboard.html");

pub async fn page() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}
