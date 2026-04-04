use axum::response::Html;

static PRICING_HTML: &str = include_str!("../../templates/pricing.html");

pub async fn page() -> Html<&'static str> {
    Html(PRICING_HTML)
}
