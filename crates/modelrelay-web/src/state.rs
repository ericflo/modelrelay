use sqlx::PgPool;

/// Shared application state available to all route handlers.
#[derive(Clone)]
pub struct AppState {
    /// PostgreSQL pool — `None` when `DATABASE_URL` is not set.
    pub db: Option<PgPool>,
    /// Stripe secret key — `None` when `STRIPE_SECRET_KEY` is not set.
    pub stripe_key: Option<String>,
}
