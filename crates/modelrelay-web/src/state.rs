use sqlx::PgPool;

/// Shared application state available to all route handlers.
#[derive(Clone)]
pub struct AppState {
    /// `PostgreSQL` pool — `None` when `DATABASE_URL` is not set.
    pub db: Option<PgPool>,
    /// Stripe secret key — `None` when `STRIPE_SECRET_KEY` is not set.
    pub stripe_key: Option<String>,
    /// Stripe webhook signing secret — `None` when `STRIPE_WEBHOOK_SECRET` is not set.
    pub webhook_secret: Option<String>,
    /// Base URL of the modelrelay-server admin API (e.g. `http://modelrelay-server:8080`).
    pub admin_url: Option<String>,
    /// Bearer token for authenticating with the modelrelay-server admin API.
    pub admin_token: Option<String>,
}
